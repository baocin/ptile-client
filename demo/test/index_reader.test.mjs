// Tests for demo/index.html's index reader.
//
// index.html is one file with no module boundary, so this harness extracts the
// pure functions by name and evaluates them in isolation. That is uglier than
// importing a module, but it tests the code that actually ships rather than a
// copy that can drift from it -- and the drift is the whole problem here: the
// reader silently rendered five layers blank for as long as nobody diffed its
// index stride against the generator's.
//
// Run: node --test demo/test/
//
// Cases needing real bytes skip when the fixtures are absent, and
// `fixtures_were_actually_exercised` fails if none were found, so an empty
// data directory cannot pass for a green run.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));
const INDEX_HTML = join(HERE, "..", "index.html");

const SEARCH_DIRS = [
  "/home/aoi/kino/data/ptiles",
  "/home/aoi/kino/projects/ptiles/tiles",
  "/mnt/core/kino/ptiles/data/states",
];
let fixturesUsed = 0;

function findFixture(name) {
  for (const d of SEARCH_DIRS) {
    const p = join(d, name);
    if (existsSync(p)) return p;
  }
  return null;
}

/** Pull the named top-level functions and consts out of index.html. */
function loadReader() {
  const html = readFileSync(INDEX_HTML, "utf8");
  const wanted = [
    "u16", "u32", "u64", "f32", "readPacked",
    "parsePtilesHeader", "readIndexEntry", "indexIsStructurallyValid",
    "parsePtilesIndex", "mergedCellSlice", "pickOffsetBase",
  ];
  const src = [];
  src.push(grab(html, /var ENTRY_SIZE_V1 = [^\n]*\n/, "ENTRY_SIZE consts"));
  src.push(grab(html, /var KNOWN_ENTRY_SIZES = [^\n]*\n/, "KNOWN_ENTRY_SIZES"));
  for (const fn of wanted) src.push(grabFunction(html, fn));
  src.push(`return { ${wanted.join(", ")}, ENTRY_SIZE_V1, ENTRY_SIZE_V2, KNOWN_ENTRY_SIZES };`);
  return new Function(src.join("\n"))();
}

function grab(html, re, what) {
  const m = html.match(re);
  if (!m) throw new Error(`could not find ${what} in index.html`);
  return m[0];
}

/** Extract `function NAME(...) { ... }` by brace matching. */
function grabFunction(html, name) {
  const start = html.indexOf(`function ${name}(`);
  if (start < 0) throw new Error(`function ${name} not found in index.html`);
  let i = html.indexOf("{", start), depth = 0;
  for (; i < html.length; i++) {
    if (html[i] === "{") depth++;
    else if (html[i] === "}") {
      depth--;
      if (depth === 0) return html.slice(start, i + 1);
    }
  }
  throw new Error(`unbalanced braces reading ${name}`);
}

const R = loadReader();

// ------------------------------------------------------------ synthetic cases

function entryV1(cell, off, len, fc) {
  const b = Buffer.alloc(19);
  b.writeBigUInt64LE(BigInt(cell), 0);
  b.writeUIntLE(off, 8, 6);
  b.writeUIntLE(len, 14, 3);
  b.writeUInt16LE(fc, 17);
  return b;
}

function entryV2(cell, off, len, fc) {
  const b = Buffer.alloc(38); // bbox bytes 8..24 stay zero, as real builders write them
  b.writeBigUInt64LE(BigInt(cell), 0);
  b.writeUIntLE(off % 281474976710656, 24, 6);
  b.writeUInt16LE(len & 0xffff, 30);
  b.writeUInt8(Math.floor(off / 281474976710656), 32);
  b.writeUInt8((len >> 16) & 0xff, 33);
  b.writeUInt16LE(fc, 34);
  b.writeUInt16LE(0, 36);
  return b;
}

/** A buffer shaped like a whole file, with the index at `indexOffset`. */
function fileWith(entries, { indexOffset = 256, declaredStride = null, blocksOffset = null } = {}) {
  const count = entries.length;
  const size = entries[0].length;
  const index = Buffer.concat([
    (() => { const b = Buffer.alloc(4); b.writeUInt32LE(count, 0); return b; })(),
    ...entries,
  ]);
  const trueLen = 4 + count * size;
  const h = {
    indexOffset,
    indexLength: declaredStride === null ? trueLen : 4 + count * declaredStride,
    blocksOffset: blocksOffset === null ? indexOffset + trueLen : blocksOffset,
  };
  const buf = Buffer.alloc(indexOffset + Math.max(trueLen, h.indexLength));
  index.copy(buf, indexOffset);
  return { buf: new Uint8Array(buf), h };
}

test("19-byte index is detected from the declared length", () => {
  const { buf, h } = fileWith([entryV1(100, 5000, 42, 1), entryV1(200, 5042, 17, 1)]);
  const p = R.parsePtilesIndex(buf, h);
  assert.equal(p.entrySize, 19);
  assert.equal(p.entrySizeSource, "declared");
  assert.equal(p.entries.length, 2);
  assert.equal(p.entries[0].blockOffset, 5000);
  assert.equal(p.entries[0].blockLength, 42);
});

test("38-byte index is detected from the declared length", () => {
  const { buf, h } = fileWith([entryV2(100, 5000, 42, 3), entryV2(200, 5042, 17, 4)]);
  const p = R.parsePtilesIndex(buf, h);
  assert.equal(p.entrySize, 38);
  assert.equal(p.entrySizeSource, "declared");
  assert.equal(p.entries[0].blockOffset, 5000);
  assert.equal(p.entries[0].blockLength, 42);
  assert.equal(p.entries[1].featureCount, 4);
});

test("the historical bug: a 38-byte index read as 19-byte yields empty blocks", () => {
  const e = entryV2(100, 900000, 512, 7);
  // Byte-for-byte, this is what the old hardcoded `off += 19` produced.
  const asV1 = R.readIndexEntry(new Uint8Array(e), new DataView(e.buffer, e.byteOffset, e.length), 0, 19);
  assert.equal(asV1.blockOffset, 0, "offset read from the zeroed bbox");
  assert.equal(asV1.blockLength, 0, "length read from the zeroed bbox -- hence 'no data', silently");
});

test("a 42-byte declared stride is rejected and the width probed", () => {
  const { buf, h } = fileWith(
    [entryV2(100, 9000, 42, 1), entryV2(200, 9042, 17, 1)],
    { declaredStride: 42 },
  );
  const p = R.parsePtilesIndex(buf, h);
  assert.equal(p.declaredStride, 42);
  assert.equal(p.entrySize, 38);
  assert.equal(p.entrySizeSource, "probed", "42 is not a width we know");
});

test("a declared stride that divides evenly but is wrong loses to the bytes", () => {
  // Detection sees the full index; only structural validation can reject 19
  // here, since 4 + 2*19 divides evenly and 19 is a width we support.
  const entries = [entryV2(100, 9000, 42, 1), entryV2(200, 9042, 17, 1)];
  const cnt = Buffer.alloc(4); cnt.writeUInt32LE(2, 0);
  const sl = new Uint8Array(Buffer.concat([cnt, ...entries]));
  const dv = new DataView(sl.buffer, sl.byteOffset, sl.length);
  assert.equal(R.indexIsStructurallyValid(sl, dv, 2, 19), false,
    "at 19 the first entry's length comes from the zeroed bbox");
  assert.equal(R.indexIsStructurallyValid(sl, dv, 2, 38), true);
});

test("an index_length shorter than the entries fails loudly", () => {
  // Unlike the 42-byte over-declaration, which merely reads a few spare bytes
  // and is recoverable, under-declaring truncates the index before detection
  // ever sees it. That must throw rather than silently serve a prefix.
  const { buf, h } = fileWith(
    [entryV2(100, 9000, 42, 1), entryV2(200, 9042, 17, 1)],
    { declaredStride: 19 },
  );
  assert.throws(() => R.parsePtilesIndex(buf, h), /no known entry width/);
});

test("an index with no valid width throws instead of returning nothing", () => {
  const { buf, h } = fileWith([entryV1(100, 0, 0, 0), entryV1(200, 0, 0, 0)]);
  assert.throws(() => R.parsePtilesIndex(buf, h), /no known entry width/);
});

test("descending cell order is rejected at that width", () => {
  const e = Buffer.concat([entryV1(500, 1000, 10, 1), entryV1(100, 1010, 10, 1)]);
  const cnt = Buffer.alloc(4); cnt.writeUInt32LE(2, 0);
  const sl = new Uint8Array(Buffer.concat([cnt, e]));
  const dv = new DataView(sl.buffer, sl.byteOffset, sl.length);
  assert.equal(R.indexIsStructurallyValid(sl, dv, 2, 19), false);
});

test("offset base: absolute, relative, and corrected", () => {
  const abs = fileWith([entryV1(100, 5000, 10, 1)]);
  assert.equal(R.pickOffsetBase({ entries: abs.h ? [{ blockOffset: 5000 }] : [], entrySize: 19 },
    { indexOffset: 256, blocksOffset: 279 }).kind, "absolute");

  assert.equal(R.pickOffsetBase({ entries: [{ blockOffset: 10 }], entrySize: 19 },
    { indexOffset: 256, blocksOffset: 5000 }).kind, "relative");

  // 2 entries x 38 B: index really ends at 256 + 4 + 76 = 336. A header
  // claiming 344 overshoots by 8, exactly the count*4 shape of the published
  // point layers.
  const c = R.pickOffsetBase({ entries: [{ blockOffset: 344 }, { blockOffset: 400 }], entrySize: 38 },
    { indexOffset: 256, blocksOffset: 344 });
  assert.equal(c.kind, "corrected");
  assert.equal(c.adjust, -8);
});

test("mergedCellSlice returns each cell's own records", () => {
  const cells = [[10n, Buffer.from("aaa")], [20n, Buffer.from("bbbb")], [30n, Buffer.from("c")]];
  const head = Buffer.alloc(12);
  head.writeInt32LE(0, 0); head.writeInt32LE(0, 4); head.writeUInt32LE(cells.length, 8);
  const table = Buffer.alloc(cells.length * 12);
  let rel = 0;
  cells.forEach(([id, recs], i) => {
    table.writeBigUInt64LE(id, i * 12);
    table.writeUInt32LE(rel, i * 12 + 8);
    rel += recs.length;
  });
  const block = new Uint8Array(Buffer.concat([head, table, ...cells.map((c) => c[1])]));
  assert.deepEqual(Buffer.from(R.mergedCellSlice(block, 10n)), Buffer.from("aaa"));
  assert.deepEqual(Buffer.from(R.mergedCellSlice(block, 20n)), Buffer.from("bbbb"));
  assert.deepEqual(Buffer.from(R.mergedCellSlice(block, 30n)), Buffer.from("c"));
  assert.equal(R.mergedCellSlice(block, 99n), null, "absent cell is null, not a throw");
});

test("mergedCellSlice rejects a truncated cell table", () => {
  const b = Buffer.alloc(12);
  b.writeUInt32LE(1000, 8); // claims 1000 cells
  assert.throws(() => R.mergedCellSlice(new Uint8Array(b), 1n), /truncated/);
});

// ----------------------------------------------------------------- real files

const REAL = [
  ["TN.roads.ptiles", 19],
  ["TN.water.ptiles", 19],
  ["TN.business.ptiles", 19],
  ["TN.buildings_v8.ptiles", 19],
  ["TN.parks.ptiles", 38],
  ["TN.rail.ptiles", 38],
  ["TN.places.ptiles", 38],
  ["US.signals.ptiles", 38],
  ["US.camera.ptiles", 38],
];

for (const [name, wantSize] of REAL) {
  test(`real file ${name} parses as a ${wantSize}-byte index`, (t) => {
    const path = findFixture(name);
    if (!path) return t.skip(`${name} not present`);

    // Header + index only, the same bytes openPtilesRemote range-fetches.
    const fd = readFileSync(path);
    const head = new Uint8Array(fd.buffer, fd.byteOffset, 256);
    const h = R.parsePtilesHeader(head);
    const buf = new Uint8Array(fd.buffer, fd.byteOffset, h.indexOffset + h.indexLength);

    const p = R.parsePtilesIndex(buf, h);
    assert.equal(p.entrySize, wantSize, `${name} entry width`);
    assert.ok(p.entries.length > 0, `${name} has entries`);
    assert.ok(p.entries[0].blockLength > 0, `${name} entry 0 names a real block`);

    const base = R.pickOffsetBase(p, h);
    assert.ok(["absolute", "relative", "corrected"].includes(base.kind));
    fixturesUsed++;
    console.log(`  ${name}: ${p.entrySize} B entries (${p.entrySizeSource}), ` +
      `${p.entries.length} entries, offsets ${base.kind}`);
  });
}

test("fixtures_were_actually_exercised", () => {
  assert.ok(
    fixturesUsed > 0,
    `no real .ptiles fixture found in ${SEARCH_DIRS.join(", ")} -- the real-file ` +
    `cases all skipped, so this suite proved nothing about actual generator output`,
  );
});
