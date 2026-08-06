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
      const resp = await fetch(url, { headers: { Range: `bytes=${from}-${to}` } });
      if (!resp.ok) throw new Error(`HTTP ${resp.status} range ${from}-${to} of ${url}`);
      const seen = resp.headers.get("ETag");
      if (seen && !etag) etag = seen;
      return new Uint8Array(await resp.arrayBuffer());
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

    /** Decompressed block bytes for an entry, cached in memory. */
    async block(entry) {
      const k = entry.block_offset;
      let got = this.blocks.get(k);
      if (!got) {
        const from = Number(entry.block_offset);
        const raw = await this.source.read(from, from + entry.block_length - 1);
        got = wasm.decompress_block(raw, this.dict);
        this.blocks.set(k, got);
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

    const dict = header.dict_length > 0
      ? await source.read(
          Number(header.dict_offset),
          Number(header.dict_offset) + header.dict_length - 1)
      : new Uint8Array(0);

    const indexBytes = await source.read(
      Number(header.index_offset),
      Number(header.index_offset) + header.index_length - 1);

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

      const k = entry.block_offset;
      let block = this.blocks.get(k);
      if (!block) {
        const from = Number(entry.block_offset);
        const raw = await this.source.read(from, from + entry.block_length - 1);
        block = wasm.decompress_block(raw, this.dict);
        this.blocks.set(k, block);
      }
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
    buildings: (b, lat, lon) => wasm.decode_buildings(b, lat, lon),
    business: (b) => wasm.decode_business(b),
    signals: (b) => wasm.decode_signals(b),
    cameras: (b) => wasm.decode_cameras(b),
  };

  // H3, from ptiles-core rather than h3-js.
  const h3 = {
    cellFor: (lat, lon) => wasm.cell_for_coord(lat, lon),
    center: (cellHex) => wasm.cell_center(cellHex),
    neighbors: (cellHex) => wasm.neighbor_cells(cellHex),
    forBounds: (a, b, c, d) => wasm.cells_for_bounds(a, b, c, d),
  };

  return { httpSource, bytesSource, open, openCoarse, decode, h3, Layer, CoarseLayer };
}
