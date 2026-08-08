// web-demo's reader, against the conformance corpus.
//
// The point of `web-demo/js/ptiles.js` is that it contains no format knowledge:
// every question about what bytes mean goes to ptiles-core through wasm. This
// checks that it actually works that way -- that it opens all eleven corpus
// files, including the two whose headers are wrong, and that the coarse
// (partial-index) path returns exactly what the full path returns.
//
// The module is written to take the wasm namespace as a parameter precisely so
// this file can exercise the same code the browser runs, rather than a copy of
// it. The legacy demo could not be tested this way: its decoders are inlined in
// a 2656-line HTML file and have to be scraped out by regex.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, existsSync, readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { createRequire } from "node:module";

import { createPtiles } from "../js/ptiles.js";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = join(HERE, "..", "..");
const WASM_PKG = join(ROOT, "wasm-pkg", "ptiles_wasm.js");
const CORPUS = join(ROOT, "conformance", "corpus");

if (!existsSync(WASM_PKG)) {
  throw new Error(
    `wasm-pkg/ not built -- this test cannot run.\n` +
    `  PATH="$HOME/.cargo/bin:$PATH" wasm-pack build wasm --target nodejs --out-dir ../wasm-pkg --release`,
  );
}

const require = createRequire(import.meta.url);
const P = createPtiles(require(WASM_PKG));
const manifest = JSON.parse(readFileSync(join(ROOT, "conformance", "manifest.json"), "utf8"));
const FILES = readdirSync(CORPUS).filter((f) => f.endsWith(".ptiles")).sort();

// What a real caller has: an H3 id from latLngToCell, whose low 21 bits are
// masked off. The index stores raw ids with those bits set, so looking up by a
// raw entry id would pass even if normalisation were missing entirely -- which
// is exactly how the first version of the reader shipped drawing nothing.
const CELL_MASK = 0xffffffffffe00000n;
const asCaller = (cell) => cell & CELL_MASK;

function source(name) {
  const fd = readFileSync(join(CORPUS, name));
  return P.bytesSource(new Uint8Array(fd.buffer, fd.byteOffset, fd.length));
}

test("the corpus is present", () => {
  assert.ok(FILES.length >= 8, `only ${FILES.length} corpus files`);
});

test("every corpus file opens, with the layout the manifest records", async () => {
  for (const name of FILES) {
    const layer = await P.open(source(name));
    const want = manifest.files[name];

    assert.equal(layer.layout.entry_size, want.entry_size, `${name}: entry width`);
    assert.equal(layer.entries.length, want.entry_count, `${name}: entry count`);
    assert.equal(layer.merged, want.entry_size === 38, `${name}: merged blocks`);

    // The two stride42 files are readable but their headers contradict their
    // own indexes, and the reader should be able to say so.
    assert.equal(
      layer.headerIsInconsistent, name.includes("stride42"),
      `${name}: header_is_inconsistent`,
    );
  }
});

test("every indexed cell yields record bytes", async () => {
  let cells = 0;
  for (const name of FILES) {
    const layer = await P.open(source(name));
    // A spread rather than all of them: 1598 blocks through zstd is slow, and
    // core/tests/conformance_corpus.rs already decompresses every one.
    for (let i = 0; i < layer.entries.length; i += 11) {
      const cell = asCaller(layer.entries[i].h3_cell);
      const rec = await layer.cellRecords(cell);
      assert.ok(
        rec && rec.length > 0,
        `${name}: cell ${cell.toString(16)} is in the index but yielded no records`,
      );
      cells++;
    }
  }
  assert.ok(cells > 50, `only ${cells} cells read`);
});

// A counting source, so "how many reads did that take" is answerable.
function countingSource(name) {
  const inner = source(name);
  const reads = [];
  return {
    reads,
    src: {
      url: inner.url,
      etag: inner.etag,
      readLive: inner.readLive,
      read: (from, to) => { reads.push([Number(from), Number(to)]); return inner.read(from, to); },
    },
  };
}

test("prefetch coalesces neighbouring blocks, and hands back the same bytes", async () => {
  // The two things a coalescing prefetch can get wrong are both silent. It can
  // fail to coalesce, which costs a round trip per block and looks fine; or it
  // can slice the combined buffer at the wrong offset, which hands a decoder
  // bytes from the neighbouring block. That does not throw -- it decodes as
  // plausible garbage -- so the assertion has to be that the records are
  // byte-identical to the ones the ordinary path returns, not merely present.
  for (const name of FILES) {
    const plain = await P.open(source(name));
    const cells = plain.entries.slice(0, 24)
      .filter((e) => e.block_length > 0)
      .map((e) => asCaller(e.h3_cell));
    if (cells.length < 4) continue;

    const want = [];
    for (const cell of cells) want.push(await plain.cellRecords(cell));

    const counted = countingSource(name);
    const pre = await P.open(counted.src);
    counted.reads.length = 0;              // drop the dict and index reads
    await pre.prefetch(cells);
    const blockReads = counted.reads.length;

    const got = [];
    for (const cell of cells) got.push(await pre.cellRecords(cell));

    assert.deepEqual(got, want, `${name}: prefetched records differ from plain ones`);
    assert.equal(
      counted.reads.length, blockReads,
      `${name}: reading a prefetched cell went back to the source`,
    );

    // Distinct blocks is the honest denominator: several cells share a block on
    // a merged layer, and those collapse before any coalescing happens, so
    // counting cells would call a merged layer coalesced when it is not.
    //
    // Strictly fewer, not "no more than": one read per block is exactly what
    // the code did before, so `<=` would pass on the thing this replaced. Every
    // corpus file in fact collapses to a single read; the assertion is loose
    // because how many runs a file falls into is a property of the file.
    const distinct = new Set(cells.map((c) => String(plain.entryFor(c).block_offset))).size;
    if (distinct >= 4) {
      assert.ok(
        blockReads < distinct,
        `${name}: ${blockReads} reads for ${distinct} distinct blocks -- not coalesced`,
      );
    }
  }
});

test("splitting the centre cells off is opt-in, and still returns the same bytes", async () => {
  // The split exists so the middle of the screen fills first, and it costs a
  // request, so it must not fire on a layer too small to pay for it. Both
  // halves of that are asserted here: forced on, it makes a second read and the
  // records are unchanged; left to its own judgement on a corpus slice, whose
  // blocks are far under the threshold, it does not fire at all.
  const name = "TN.roads.ptiles";
  const plain = await P.open(source(name));
  const cells = plain.entries.slice(0, 24)
    .filter((e) => e.block_length > 0)
    .map((e) => asCaller(e.h3_cell));
  const want = [];
  for (const cell of cells) want.push(await plain.cellRecords(cell));

  const forced = countingSource(name);
  const split = await P.open(forced.src);
  forced.reads.length = 0;
  await split.prefetch(cells, { head: 4, splitMinBytes: 0 });
  assert.equal(forced.reads.length, 2, "head and tail should be one read each");

  const got = [];
  for (const cell of cells) got.push(await split.cellRecords(cell));
  assert.deepEqual(got, want, "splitting changed the bytes");

  const judged = countingSource(name);
  const whole = await P.open(judged.src);
  judged.reads.length = 0;
  await whole.prefetch(cells, { head: 4 });
  assert.equal(
    judged.reads.length, 1,
    "split on a payload far under splitMinBytes -- it is paying a request for nothing",
  );
});

test("merged layers slice the cell out rather than returning the whole block", async () => {
  // Handing a whole merged block to a record decoder does not error -- it
  // parses the cell table as records and yields plausible garbage. So the
  // slice has to be strictly smaller than the block it came from.
  const name = "TN.parks.ptiles";
  const layer = await P.open(source(name));
  assert.ok(layer.merged, `${name} is expected to use merged blocks`);

  let checked = 0;
  for (const e of layer.entries.slice(0, 12)) {
    const block = await layer.block(e);
    const slice = await layer.cellRecords(asCaller(e.h3_cell));
    assert.ok(slice, `${name}: cell ${e.h3_cell.toString(16)} sliced to nothing`);
    assert.ok(
      slice.length < block.length,
      `${name}: cell ${e.h3_cell.toString(16)} "slice" is the whole block ` +
      `(${slice.length} of ${block.length} bytes) -- the cell table would decode as records`,
    );
    checked++;
  }
  assert.ok(checked > 0);
});

test("records decode through wasm on every layer that has a decoder", async () => {
  const cases = [
    ["TN.roads.ptiles", "roads"],
    ["TN.water.ptiles", "water"],
    ["TN.parks.ptiles", "parks"],
    ["TN.rail.ptiles", "rail"],
    ["TN.business.ptiles", "business"],
    ["US.signals.ptiles", "signals"],
    ["US.camera.ptiles", "cameras"],
  ];

  for (const [name, kind] of cases) {
    const layer = await P.open(source(name));
    let decoded = 0;
    for (const e of layer.entries.slice(0, 6)) {
      const rec = await layer.cellRecords(asCaller(e.h3_cell));
      if (!rec) continue;
      // business needs the schema version and the entry's stored cell id; every
      // other layer here is self-describing.
      const out = kind === "business"
        ? P.decode.business(rec, layer.header.version, e.h3_cell.toString(16))
        : P.decode[kind](rec);
      assert.ok(Array.isArray(out), `${name}: ${kind} did not return an array`);
      decoded += out.length;
    }
    assert.ok(decoded > 0, `${name}: ${kind} decoded nothing from the first cells`);
  }
});

// The coarse path is the reason a client can open US.signals without fetching
// a 4014 KiB index. It is only worth having if it returns the same answer.
test("the coarse path returns byte-identical records to the full path", async () => {
  const withAux = FILES.filter((f) => (manifest.files[f].aux_length ?? 0) > 0);
  assert.ok(withAux.length >= 2, `expected 2+ files with a coarse index, got ${withAux.length}`);

  for (const name of withAux) {
    const full = await P.open(source(name));
    const coarse = await P.openCoarse(source(name));
    assert.ok(coarse, `${name}: carries a coarse index but openCoarse returned null`);

    let compared = 0;
    for (let i = 0; i < full.entries.length; i += 29) {
      const cell = asCaller(full.entries[i].h3_cell);
      const a = await full.cellRecords(cell);
      const b = await coarse.cellRecords(cell);
      assert.ok(b, `${name}: coarse path found nothing for cell ${cell.toString(16)}`);
      assert.deepEqual(
        Array.from(b), Array.from(a),
        `${name}: cell ${cell.toString(16)} decodes differently through the coarse path`,
      );
      compared++;
    }
    assert.ok(compared > 5, `${name}: only ${compared} cells compared`);
  }
});

test("a file with no coarse index reports that rather than throwing", async () => {
  // Every layer built before PTCI is in this state; the caller has to be able
  // to tell it apart and fall back to a full open.
  assert.equal(await P.openCoarse(source("TN.roads.ptiles")), null);
});

test("an unindexed cell returns null, not an exception", async () => {
  const layer = await P.open(source("TN.rail.ptiles"));
  assert.equal(await layer.cellRecords(0n), null);
  assert.equal(layer.entryFor(0n), null);
});

// The raw stored id and the masked id a caller supplies must reach the same
// entry. If only one of them works the layer renders empty against real
// input while every test that used stored ids still passes.
test("a cell resolves whether it is normalised or raw", async () => {
  const layer = await P.open(source("TN.roads.ptiles"));
  for (const e of layer.entries.slice(0, 8)) {
    const viaRaw = layer.entryFor(e.h3_cell);
    const viaMasked = layer.entryFor(asCaller(e.h3_cell));
    assert.ok(viaRaw, `raw id ${e.h3_cell.toString(16)} did not resolve`);
    assert.equal(
      viaMasked, viaRaw,
      `masked id ${asCaller(e.h3_cell).toString(16)} resolved differently from ` +
      `the stored id ${e.h3_cell.toString(16)} -- a caller using latLngToCell ` +
      `would miss every cell and the layer would render empty`,
    );
  }
});

test("H3 comes from core, and round-trips", () => {
  // Nashville. The legacy demo uses h3-js for this; core has it, so the
  // vendored copy is one more thing web-demo does not ship.
  const cell = P.h3.cellFor(36.1627, -86.7816);
  assert.match(cell, /^[0-9a-f]{15}$/);
  const [lat, lon] = P.h3.center(cell);
  assert.ok(Math.abs(lat - 36.1627) < 0.5 && Math.abs(lon + 86.7816) < 0.5);
  assert.equal(P.h3.neighbors(cell).length, 6);
});

// The name index is not in the corpus (24 MB for one state), so this runs
// against the local build when there is one and skips otherwise -- the same
// arrangement core/src/business_search.rs's own real-file tests use.
//
// What it guards is the one thing core's tests cannot see: this reader's own
// bucket lookup. `entryFor` normalises its argument as an H3 cell, and masking
// the low 21 bits turns every bucket key 0-27 into zero, so a nameSearch built
// on `entryFor` finds bucket 0 for every query and silently returns nothing
// for 27 of the 28 letters.
const NAME_INDEX = "/home/aoi/kino/data/ptiles/TN.business_name_index.ptiles";

test("name index search finds by prefix, and does not pretend to do substrings",
  { skip: !existsSync(NAME_INDEX) && "no local TN.business_name_index.ptiles" },
  async () => {
    const fd = readFileSync(NAME_INDEX);
    const layer = await P.open(P.bytesSource(new Uint8Array(fd.buffer, fd.byteOffset, fd.length)));

    const taco = await layer.nameSearch("taco bell", 50);
    assert.ok(taco.length > 0, "no Taco Bell in Tennessee");
    assert.ok(
      taco.every((h) => h.name.toLowerCase().includes("taco bell")),
      `nameSearch returned a non-match: ${JSON.stringify(taco.slice(0, 3))}`,
    );
    assert.ok(taco[0].score >= 1, "the best hit for a full prefix should be exact or prefix");
    for (const h of taco) {
      assert.ok(h.lat > 34 && h.lat < 37 && h.lon < -81 && h.lon > -91,
        `hit outside Tennessee: ${h.name} at ${h.lat},${h.lon}`);
    }

    // Ranking must hold across the two probed buckets, not just within one.
    for (let i = 1; i < taco.length; i++) {
      assert.ok(taco[i - 1].score >= taco[i].score,
        "hits are not ranked by score across buckets");
    }

    // The documented limit, asserted rather than described: "bell" cannot reach
    // Taco Bell, which lives in the `t` bucket. If this ever starts passing,
    // the index became a real inverted index and the brute-force fallback in
    // index.html is dead weight.
    const bell = await layer.nameSearch("bell", 50);
    assert.ok(
      !bell.some((h) => h.name.toLowerCase().startsWith("taco")),
      "mid-word query reached another bucket -- the prefix-only caveat is stale",
    );
  });

// --- business v4, the framing this reader got wrong for months --------------
//
// The corpus is all v3, and v3 is forgiving: a `u32 record_len` in front of every
// record resynchronises the stream, so a decoder that stops early still produces
// correct records. v4 dropped the prefix. A decoder that leaves the
// extended-attributes trailer unread therefore starts the next record 30-42 bytes
// early, and because a v4 record has no structural check the result is thousands
// of well-formed garbage records followed by "unexpected end of input at offset
// 42". That is what every business lookup in this demo was doing against the
// published `business_v4` files.
//
// The fixture is the real block for the res-7 cell containing 36.35605,-86.07246,
// captured by test-fixtures/extract_business_v4.py.
const GOLDEN = join(ROOT, "test-fixtures", "golden");

test("business v4 decodes flush, in place, and with its provenance", async (t) => {
  const blockPath = join(GOLDEN, "business_v4.block.bin");
  const metaPath = join(GOLDEN, "business_v4.meta.json");
  if (!existsSync(blockPath) || !existsSync(metaPath)) {
    t.skip("no business_v4 golden fixture");
    return;
  }
  const block = new Uint8Array(readFileSync(blockPath));
  const meta = JSON.parse(readFileSync(metaPath, "utf8"));

  const recs = P.decode.business(block, meta.file_version, meta.cell_id_hex);
  // Exactly the index's count: with no per-record framing this is the only cheap
  // signal that the stream stayed in sync.
  assert.equal(recs.length, meta.feature_count_in_index,
    "record count must match the index's feature_count");
  for (const b of recs) {
    assert.ok(b.name.length > 0, `unnamed record at ${b.lat},${b.lon}`);
    // Inside its own cell. A res-7 cell is ~5 km across; the old decoder answered
    // a few hundred metres from 0,0 with no error at all.
    assert.ok(Math.abs(b.lat - meta.cell_center_lat) < 0.05, `${b.name} lat ${b.lat}`);
    assert.ok(Math.abs(b.lon - meta.cell_center_lon) < 0.05, `${b.name} lon ${b.lon}`);
    // The trailer the decoder used to skip.
    assert.ok([1, 2].includes(b.source_type), `${b.name} source_type ${b.source_type}`);
    assert.ok(b.source_id && b.source_id.length >= 20, `${b.name} source_id ${b.source_id}`);
  }
});

test("a v4 block decoded as v3 is wrong, which is why the version is required", async (t) => {
  const blockPath = join(GOLDEN, "business_v4.block.bin");
  if (!existsSync(blockPath)) {
    t.skip("no business_v4 golden fixture");
    return;
  }
  const block = new Uint8Array(readFileSync(blockPath));
  // Pinning the failure mode, not just the fix: reading v4 with v3 framing must
  // not quietly return plausible records. It either throws or returns a different
  // count -- what it must never do is look like success.
  let v3 = null;
  try {
    v3 = P.decode.business(block, 3, "8744c9a0affffff");
  } catch {
    v3 = null;
  }
  assert.notEqual(v3 && v3.length, 7,
    "a v4 block read as v3 returned exactly the right number of records, " +
    "which would mean this test can no longer tell the two framings apart");
});
