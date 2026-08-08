// A .ptiles reader with no format knowledge in JavaScript.
//
// demo/index.html (now the legacy demo) hand-decodes the header, both index
// entry widths, the offset base, merged-block cell tables, the coarse index,
// and three record layouts. Every format bug this project has had came from
// that second implementation drifting from the Rust one -- a hardcoded 19-byte
// stride against a generator emitting 38, an index_length computed at 42, a
// business decoder that silently rounds every osm_id past 2^53.
//
// So this module knows how to *fetch* bytes and nothing about what is in them.
// Ranges, the ETag-keyed Cache API, in-flight deduplication and the block cache
// stay here, because they are network concerns and wasm has no business owning
// them. Every question of the form "what do these bytes mean" goes to
// ptiles-core through wasm:
//
//   parse_header            the 256-byte header
//   parse_index_layout      entry width, offset base, declared stride
//   index_entries_absolute  entries with offsets already resolved
//   decompress_block        zstd, with the layer dictionary
//   merged_cell_slice       one cell out of a merged block
//   parse_coarse_index      the PTCI sampled index in `aux`
//   coarse_bracket          cell -> byte range of the index run to fetch
//   decode_*                records
//   cell_for_coord etc.     H3
//
// That also removes two vendored JS libraries the legacy demo needs: the zstd
// build under lib/ and h3-js. Both are in ptiles-core already.
//
// The wasm namespace is injected rather than imported so the same module works
// against a `--target web` build in the browser and a `--target nodejs` build
// under `node --test`. That is what lets the corpus tests exercise this file
// rather than a copy of it.

/**
 * @param {object} wasm  the ptiles-wasm module namespace (already initialised
 *   in the browser: `await init()` before calling this).
 */
export function createPtiles(wasm) {
  // Where the wall clock actually goes. Every optimization claim about this
  // page is unfalsifiable without it: the layer either renders or it does not,
  // and "it felt slow" does not say whether the cost is the CDN, zstd or
  // Leaflet. Counting here rather than in the page means it counts the real
  // bytes -- a cache hit adds no request and no bytes, which is exactly the
  // cold/warm distinction perf_check.py measures.
  // `netSumMs` is the sum over requests and `netWallMs` the union of their
  // intervals -- the wall time with at least one request outstanding. They were
  // one number until the renderer started prefetching, at which point the sum
  // ran past the total render time and the leftover-time column went negative.
  // Both are worth keeping: the ratio between them is the concurrency actually
  // achieved, which is the thing a prefetch is trying to move.
  const stats = {
    requests: 0,   // range requests that reached the network
    bytes: 0,      // compressed bytes over the wire
    netSumMs: 0,   // summed time inside those requests
    netWallMs: 0,  // wall time with >=1 request in flight
    blocks: 0,     // blocks handed to zstd
    zstdMs: 0,     // time inside decompress_block
    inflight: 0,
    since: 0,
    enter() { if (this.inflight++ === 0) this.since = performance.now(); },
    leave() { if (--this.inflight === 0) this.netWallMs += performance.now() - this.since; },
    reset() {
      this.requests = this.bytes = this.netSumMs = this.netWallMs = 0;
      this.blocks = this.zstdMs = 0;
      // Deliberately not resetting `inflight`: a request outstanding across a
      // reset still has a `leave()` coming, and zeroing the counter would make
      // that leave go negative and the next enter() never start the clock.
    },
  };

  // H3 res-7 ids carry filler digits in their low 21 bits, so the id a builder
  // stored and the id a caller looks up with are not always bit-identical even
  // when they name the same cell. Every lookup normalises both sides by
  // zeroing those bits; the entry's *stored* id is what merged-block slicing
  // needs, so entries keep theirs untouched.
  //
  // Getting this wrong is silent: a lookup simply misses and the layer renders
  // empty, which is indistinguishable from sparse coverage. It is how the
  // first version of this module drew nothing at all.
  const CELL_MASK = 0xffffffffffe00000n;
  const norm = (cell) => BigInt(cell) & CELL_MASK;

  // --------------------------------------------------------------- sources

  /**
   * A byte source. `read(from, toInclusive)` is the only thing the reader
   * needs, which makes an HTTP range, a local file and an in-memory buffer
   * interchangeable.
   */

  /** Range-request source with an ETag-keyed persistent cache. */
  function httpSource(url, { cacheName = "ptiles-regions-v2" } = {}) {
    let cacheOpen = null;
    const inflight = new Map();
    let etag = null;

    // `caches` is undefined on insecure origins (file://, plain http beyond
    // localhost). A null cache means "just fetch".
    function cache() {
      if (typeof caches === "undefined") return Promise.resolve(null);
      if (!cacheOpen) cacheOpen = caches.open(cacheName).catch(() => null);
      return cacheOpen;
    }

    function key(from, to) {
      return `${location.origin}/__ptiles/${encodeURIComponent(url)}/` +
        `${encodeURIComponent(etag || "no-etag")}/${from}-${to}`;
    }

    // Drop entries for this url under any other ETag, or the cache grows by a
    // full copy of every region each time the layer is rebuilt.
    async function purgeStale() {
      try {
        const c = await cache();
        if (!c) return;
        const prefix = `${location.origin}/__ptiles/${encodeURIComponent(url)}/`;
        const keep = `${prefix}${encodeURIComponent(etag || "no-etag")}/`;
        for (const req of await c.keys()) {
          if (req.url.startsWith(prefix) && !req.url.startsWith(keep)) await c.delete(req);
        }
      } catch { /* best effort */ }
    }

    async function fetchRange(from, to) {
      const t0 = performance.now();
      stats.enter();
      try {
        const resp = await fetch(url, { headers: { Range: `bytes=${from}-${to}` } });
        if (!resp.ok) throw new Error(`HTTP ${resp.status} range ${from}-${to} of ${url}`);
        const seen = resp.headers.get("ETag");
        if (seen && !etag) etag = seen;
        const bytes = new Uint8Array(await resp.arrayBuffer());
        stats.requests++;
        stats.bytes += bytes.length;
        return bytes;
      } finally {
        stats.netSumMs += performance.now() - t0;
        stats.leave();
      }
    }

    return {
      url,
      get etag() { return etag; },

      /** Uncached: used for the header, whose ETag keys everything else. */
      async readLive(from, to) {
        return fetchRange(from, to);
      },

      async read(from, to) {
        const k = key(from, to);
        const pending = inflight.get(k);
        if (pending) return pending;

        const p = (async () => {
          const c = await cache();
          if (c) {
            try {
              const hit = await c.match(k);
              if (hit) return new Uint8Array(await hit.arrayBuffer());
            } catch { /* fall through to network */ }
          }
          const bytes = await fetchRange(from, to);
          if (c) {
            // A quota failure must not break the read that already succeeded.
            try {
              await c.put(k, new Response(bytes));
              purgeStale();
            } catch { /* over quota, or storage denied */ }
          }
          return bytes;
        })();

        inflight.set(k, p);
        try { return await p; } finally { inflight.delete(k); }
      },
    };
  }

  /** In-memory source, for tests and for a file already fully loaded. */
  function bytesSource(bytes) {
    const all = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
    const read = async (from, to) => all.subarray(Number(from), Number(to) + 1);
    return { url: "(memory)", etag: null, read, readLive: read };
  }

  // ----------------------------------------------------------------- layer

  /**
   * A layer opened with its whole index in memory.
   *
   * `open` costs header + dict + index. For the big national point layers that
   * index is megabytes, which is what `openCoarse` exists to avoid.
   */
  class Layer {
    constructor(source, header, entries, layout, dict) {
      this.source = source;
      this.header = header;
      this.entries = entries;
      this.layout = layout;
      this.dict = dict;
      this.blocks = new Map();
      this.byCell = new Map();
      for (const e of entries) this.byCell.set(norm(e.h3_cell), e);
    }

    /** True when blocks pack several cells behind a table and must be sliced. */
    get merged() {
      return this.layout.entry_size === 38;
    }

    /**
     * Whether this file's header contradicts its own index. Readable either
     * way -- core corrects for it -- but it means the generator has a bug.
     */
    get headerIsInconsistent() {
      return this.layout.header_is_inconsistent;
    }

    entryFor(cell) {
      return this.byCell.get(norm(cell)) ?? null;
    }

    /**
     * Business-name search, for a `{ST}.business_name_index.ptiles` layer.
     *
     * That file buckets every business by the first letter of its *name* into
     * 28 blocks and reuses the index's `h3_cell` field to hold the 0-27 key. So
     * a query costs one ~1 MB block instead of scanning the 54 MB business
     * file -- and it is a prefix accelerator, not substring search: "taco"
     * finds Taco Bell, "bell" cannot, because Taco Bell lives in the `t`
     * bucket. Callers wanting real substring matching must scan.
     *
     * Bucket 26 is probed alongside the query's own, mirroring core's
     * `probe_bucket_keys` -- older builders put some accented and non-Latin
     * names there, and looking only in `c` for "Cafe" would silently miss them.
     *
     * Deliberately NOT going through `entryFor`: that normalises the lookup key
     * as an H3 cell, and masking off the low 21 bits collapses every bucket key
     * to zero.
     */
    async nameSearch(query, limit) {
      const key = wasm.key_for_business_name_query(query);
      const keys = key === 26 ? [26] : [key, 26];
      let hits = [];
      for (const k of keys) {
        const entry = this.entries.find((e) => e.h3_cell === BigInt(k));
        if (!entry || entry.block_length === 0) continue;
        hits = hits.concat(wasm.match_business_name_block(await this.block(entry), query, limit));
      }
      // Each bucket ranks on its own, so re-rank across both in core's order:
      // score (2 exact, 1 prefix, 0 substring) then name ascending.
      hits.sort((a, b) => b.score - a.score || (a.name < b.name ? -1 : a.name > b.name ? 1 : 0));
      return hits.slice(0, limit);
    }

    /**
     * Warm the blocks these cells need, in as few requests as possible.
     *
     * A viewport's cells are neighbours, the index is written in cell order,
     * and so their blocks land next to each other in the file. Fetching them
     * one range at a time paid a round trip for each: measured on roads at
     * Nashville z14, 15 block requests inside a 2155 ms window that was almost
     * entirely waiting. Runs separated by less than `maxGap` are pulled as one
     * range and sliced locally, which trades a few unwanted bytes in the gaps
     * for a round trip each.
     *
     * The cache is populated *synchronously*, before the first await, so a
     * draw loop that reaches a cell mid-flight joins the coalesced read rather
     * than starting its own. That ordering is the whole design; without it the
     * prefetch and the loop race and the request count does not fall.
     *
     * `head` splits the first N cells -- which the caller has ordered nearest
     * the map centre first -- into their own, smaller range, so the middle of
     * the screen fills before the edges rather than the whole viewport landing
     * at once. That costs one more request, and only pays for itself on a layer
     * with enough bytes to be worth waiting for, so it is skipped below
     * `splitMinBytes`. Measured cold at Nashville z14: splitting unconditionally
     * took water from 802 to 1103 ms and parks from 597 to 843 ms, both of whose
     * whole render is a single round trip, while roads' first feature went from
     * 1174 to 824 ms for 2% on the total.
     */
    prefetch(cells, { maxGap = 65536, head = 0, splitMinBytes = 262144 } = {}) {
      // Several cells share one block on a merged layer, so dedupe by offset
      // or the same range gets queued once per cell in it. Insertion order is
      // the caller's order, which is what `head` slices on -- so this dedupe
      // must not sort, and the run builder sorts its own copy.
      const byOffset = new Map();
      for (const cell of cells) {
        const e = this.entryFor(cell);
        if (!e || !e.block_length) continue;
        if (this.blocks.has(e.block_offset)) continue;
        byOffset.set(String(e.block_offset), e);
      }
      const wanted = [...byOffset.values()];
      if (!wanted.length) return Promise.resolve();

      const total = wanted.reduce((n, e) => n + e.block_length, 0);
      const split = head > 0 && wanted.length > head && total >= splitMinBytes;
      const batches = split ? [wanted.slice(0, head), wanted.slice(head)] : [wanted];

      const runs = [];
      for (const batch of batches) {
        const sorted = [...batch].sort(
          (a, b) => Number(a.block_offset) - Number(b.block_offset));
        let open = null;
        for (const e of sorted) {
          const start = Number(e.block_offset);
          const end = start + e.block_length - 1;
          if (open && start - open.end - 1 <= maxGap) {
            open.end = Math.max(open.end, end);
            open.entries.push(e);
          } else {
            open = { start, end, entries: [e] };
            runs.push(open);
          }
        }
      }

      // Every read is started here and now, with no queue in front of it: the
      // coalescing is what keeps the count down, and a queue would delay
      // populating the cache past the point where the draw loop can join it.
      return Promise.all(runs.map((run) => {
        const bytes = this.source.read(run.start, run.end);
        for (const e of run.entries) {
          const from = Number(e.block_offset) - run.start;
          const block = bytes.then((buf) => {
            const t0 = performance.now();
            const out = wasm.decompress_block(
              buf.subarray(from, from + e.block_length), this.dict);
            stats.blocks++;
            stats.zstdMs += performance.now() - t0;
            return out;
          });
          block.catch(() => this.blocks.delete(e.block_offset));
          this.blocks.set(e.block_offset, block);
        }
        return bytes;
      }));
    }

    /**
     * Decompressed block bytes for an entry, cached in memory.
     *
     * The cache holds the *promise*, not the bytes. Caching the resolved value
     * meant two callers asking for one block before the first finished both
     * missed the cache and both ran zstd over the same bytes -- invisible
     * while every caller was serial, and exactly what a prefetch pass creates.
     * The range request itself was already deduplicated in httpSource, so the
     * duplicate work was decompression only, which is why it never showed up
     * as extra traffic.
     *
     * A rejected promise is evicted rather than cached, or one failed range
     * would poison that block for the life of the page.
     */
    block(entry) {
      const k = entry.block_offset;
      let got = this.blocks.get(k);
      if (!got) {
        got = (async () => {
          const from = Number(entry.block_offset);
          const raw = await this.source.read(from, from + entry.block_length - 1);
          const t0 = performance.now();
          const out = wasm.decompress_block(raw, this.dict);
          stats.blocks++;
          stats.zstdMs += performance.now() - t0;
          return out;
        })();
        this.blocks.set(k, got);
        got.catch(() => this.blocks.delete(k));
      }
      return got;
    }

    /**
     * The record bytes belonging to one cell -- what every decoder expects.
     *
     * On a 19-byte layer that is the whole decompressed block; on a 38-byte
     * layer the block holds several cells and must be sliced first. Handing a
     * whole merged block to a record decoder does not error, it parses the cell
     * table as records and yields plausible garbage, so this distinction is not
     * optional.
     */
    async cellRecords(cell) {
      const entry = this.entryFor(cell);
      if (!entry || entry.block_length === 0) return null;
      const block = await this.block(entry);
      if (!this.merged) return block;
      // Slice by the id the builder stored, not the normalised lookup key --
      // the merged block's cell table holds stored ids.
      return wasm.merged_cell_slice(block, entry.h3_cell.toString(16)) ?? null;
    }
  }

  /**
   * Open a layer, reading header, dictionary and the whole index.
   *
   * The header is read live because its ETag keys every cached region below it;
   * that one 256-byte request is the entire cost of a warm open.
   */
  async function open(source) {
    const headerBytes = await source.readLive(0, 255);
    const header = wasm.parse_header(headerBytes);

    // Dictionary and index together. Both of their positions come out of the
    // header, so only the header is a dependency -- fetching them one after
    // the other spent a second round trip waiting for nothing. On TN.roads
    // that is a 512 KiB dictionary and a 428 KiB index, and they are most of
    // what a cold open costs.
    const [dict, indexBytes] = await Promise.all([
      header.dict_length > 0
        ? source.read(
            Number(header.dict_offset),
            Number(header.dict_offset) + header.dict_length - 1)
        : Promise.resolve(new Uint8Array(0)),
      source.read(
        Number(header.index_offset),
        Number(header.index_offset) + header.index_length - 1),
    ]);

    // Both of these run the same ptiles-core code PtilesFile::open runs, so a
    // browser and a Rust caller cannot disagree about the same file.
    const layout = wasm.parse_index_layout(headerBytes, indexBytes);
    const entries = wasm.index_entries_absolute(headerBytes, indexBytes);

    return new Layer(source, header, entries, layout, dict);
  }

  // ---------------------------------------------------------------- coarse

  /**
   * A layer opened through its PTCI sampled index, fetching only the run of
   * real index entries a lookup needs.
   *
   * US.signals carries a 4014 KiB index and entries are only locatable by
   * position, so an ordinary open has to fetch all of it. The builder writes
   * every 256th entry to the header's `aux` region (~5 KiB, immediately after
   * the header), which turns a lookup into header+aux in one request and then
   * one ~10 KiB slice.
   *
   * Returns `null` when the file carries no coarse index -- every layer built
   * before PTCI existed -- and the caller should fall back to `open`.
   */
  async function openCoarse(source) {
    const headerBytes = await source.readLive(0, 255);
    const header = wasm.parse_header(headerBytes);
    if (!header.aux_length) return null;

    const aux = await source.read(
      Number(header.aux_offset),
      Number(header.aux_offset) + header.aux_length - 1);

    // Throws if aux announces itself as PTCI and then does not hold up; null
    // if it simply holds something else.
    const coarse = wasm.parse_coarse_index(aux);
    if (!coarse) return null;

    const dict = header.dict_length > 0
      ? await source.read(
          Number(header.dict_offset),
          Number(header.dict_offset) + header.dict_length - 1)
      : new Uint8Array(0);

    return new CoarseLayer(source, header, aux, coarse, dict);
  }

  class CoarseLayer {
    constructor(source, header, aux, coarse, dict) {
      this.source = source;
      this.header = header;
      this.aux = aux;
      this.coarse = coarse;
      this.dict = dict;
      this.runs = new Map();   // bracket start -> entries in that run
      this.blocks = new Map();
      // A coarse index is only written by the current builder, which verifies
      // its own offsets, so entries are absolute and need no correction.
      this.entrySize = 38;
    }

    get merged() { return true; }

    /** Entries in the index run bracketing `cell`, fetched once per run. */
    async entriesNear(cell) {
      const cellBig = BigInt(cell);
      const br = wasm.coarse_bracket(
        this.aux, cellBig.toString(16), this.header.index_offset, this.entrySize);
      if (!br) return null;

      let run = this.runs.get(br.start);
      if (!run) {
        const bytes = await this.source.read(Number(br.byte_from), Number(br.byte_to));
        // A bracketed range lands mid-index, so there is no count prefix in
        // front of it. core decodes the run; offsets in a coarse-indexed file
        // are already absolute, since only the current builder -- which
        // verifies its own offsets on write -- emits a coarse index.
        run = wasm.parse_entry_run(bytes, this.entrySize);
        this.runs.set(br.start, run);
      }
      return run;
    }

    async cellRecords(cell) {
      const want = norm(cell);
      const run = await this.entriesNear(cell);
      if (!run) return null;
      const entry = run.find((e) => norm(e.h3_cell) === want);
      if (!entry || entry.block_length === 0) return null;

      // Promise-keyed for the same reason Layer.block is: concurrent lookups
      // into one merged block must share the decompression, not repeat it.
      const k = entry.block_offset;
      let pending = this.blocks.get(k);
      if (!pending) {
        pending = (async () => {
          const from = Number(entry.block_offset);
          const raw = await this.source.read(from, from + entry.block_length - 1);
          const t0 = performance.now();
          const out = wasm.decompress_block(raw, this.dict);
          stats.blocks++;
          stats.zstdMs += performance.now() - t0;
          return out;
        })();
        this.blocks.set(k, pending);
        pending.catch(() => this.blocks.delete(k));
      }
      const block = await pending;
      return wasm.merged_cell_slice(block, entry.h3_cell.toString(16)) ?? null;
    }
  }

  // ---------------------------------------------------------------- decode
  //
  // Straight pass-through. Listed explicitly rather than re-exported wholesale
  // so that a layer with no decoder is a visible gap rather than a silent
  // `undefined is not a function` at runtime.
  const decode = {
    roads: (b) => wasm.decode_roads(b),
    water: (b) => wasm.decode_water(b),
    parks: (b) => wasm.decode_parks(b),
    rail: (b) => wasm.decode_rail(b),
    trails: (b) => wasm.decode_trails(b),
    buildings: (b, lat, lon) => wasm.decode_buildings(b, lat, lon),
    business: (b) => wasm.decode_business(b),
    signals: (b) => wasm.decode_signals(b),
    cameras: (b) => wasm.decode_cameras(b),
  };

  // Format vocabulary questions answered by ptiles-core, not re-implemented
  // here. A caller that needs to know what a value *means* -- rather than just
  // what bytes it came from -- should reach for these instead of hardcoding the
  // layer's enums in JavaScript.
  const classify = {
    trailIsDeveloped: (trailType) => wasm.trail_is_developed(trailType || ""),
    buildingHeight: (heightM, buildingType) =>
      wasm.resolved_height(heightM == null ? undefined : heightM, buildingType || ""),
  };

  // H3, from ptiles-core rather than h3-js.
  const h3 = {
    cellFor: (lat, lon) => wasm.cell_for_coord(lat, lon),
    center: (cellHex) => wasm.cell_center(cellHex),
    neighbors: (cellHex) => wasm.neighbor_cells(cellHex),
    forBounds: (a, b, c, d) => wasm.cells_for_bounds(a, b, c, d),
  };

  return { httpSource, bytesSource, open, openCoarse, decode, classify, h3, Layer, CoarseLayer, stats };
}
