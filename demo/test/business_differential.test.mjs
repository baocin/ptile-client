// JS vs wasm on real business blocks.
//
// index.html carries its own PTILESB v3 record decoder and a comment saying
// why:
//
//     // Business: JS v3 decoder (wasm business is wrong framing for live files)
//
// That comment is wrong, and this test is the evidence. Run against the real
// bytes in conformance/corpus/TN.business.ptiles, the two decoders agree on
// the record framing exactly -- same record count in every block, and every
// string, coordinate and category identical. Nothing about the framing
// differs.
//
// What does differ is `osm_id`, on 100% of records, and there it is the JS
// side that is wrong. These uids run to ~6.3e18, past the 2^53 where a double
// stops representing consecutive integers. The JS decoder accumulates them in
// Number space:
//
//     var uid = prevUid + zigzagDecode(dr.value); prevUid = uid;
//
// so the value is rounded on arrival and the error then compounds through the
// delta chain. On this corpus 29/51 records come out as exactly the float64
// rounding of the true value; the remaining 22 drift further, and some flip
// sign entirely when a delta crosses an i64 boundary that Number arithmetic
// cannot wrap. wasm keeps the value in i64 and hands it back as a BigInt
// (wasm/src/lib.rs uses serialize_large_number_types_as_bigints for exactly
// this reason).
//
// So the business layer is a case for using the wasm decoder, not against it.
// Left unfixed, the demo links some businesses to the wrong OSM object.
//
// This is a migration harness as much as a regression test: as index.html's
// hand-rolled decoders are replaced by wasm calls, each one gets a check like
// this first, and a green differential is what "parity" means.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { createRequire } from "node:module";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = join(HERE, "..", "..");
const WASM_PKG = join(ROOT, "wasm-pkg", "ptiles_wasm.js");
const CORPUS = join(ROOT, "conformance", "corpus", "TN.business.ptiles");

// Not a skip. A missing wasm build means this suite proves nothing, and a
// silent pass is the failure mode this whole corpus exists to prevent.
if (!existsSync(WASM_PKG)) {
  throw new Error(
    `wasm-pkg/ not built -- this differential cannot run.\n` +
    `Build it with:\n` +
    `  PATH="$HOME/.cargo/bin:$PATH" wasm-pack build wasm --target nodejs --out-dir ../wasm-pkg --release`,
  );
}

const require = createRequire(import.meta.url);
const wasm = require(WASM_PKG);
const html = readFileSync(join(ROOT, "demo", "index.html"), "utf8");

/** Extract `function NAME(...) {...}` by brace matching. */
function grabFunction(name) {
  const start = html.indexOf(`function ${name}(`);
  if (start < 0) throw new Error(`function ${name} not found in index.html`);
  return braceMatch(start);
}

/** Same, for `X.prototype.y = function (...) {...}`. */
function grabAssign(prefix) {
  const start = html.indexOf(prefix);
  if (start < 0) throw new Error(`${prefix} not found in index.html`);
  return braceMatch(start);
}

function braceMatch(start) {
  let i = html.indexOf("{", start), depth = 0;
  for (; i < html.length; i++) {
    if (html[i] === "{") depth++;
    else if (html[i] === "}") {
      depth--;
      if (depth === 0) return html.slice(start, i + 1);
    }
  }
  throw new Error("unbalanced braces");
}

function loadJsDecoders() {
  const src = [
    html.match(/var ENTRY_SIZE_V1 = [^\n]*\n/)[0],
    html.match(/var KNOWN_ENTRY_SIZES = [^\n]*\n/)[0],
  ];
  for (const f of [
    "u16", "u32", "u64", "f32", "readPacked", "decodeVarint", "zigzagDecode",
    "parsePtilesHeader", "readIndexEntry", "indexIsStructurallyValid",
    "parsePtilesIndex", "pickOffsetBase",
  ]) src.push(grabFunction(f));
  // The record decoder is a prototype method, so it needs a constructor to
  // hang off; _decodeRecords itself touches no instance state.
  src.push("function BusinessReader(){}");
  src.push(grabAssign("BusinessReader.prototype._decodeRecords = function(raw) {") + ";");
  src.push("return { parsePtilesHeader, parsePtilesIndex, pickOffsetBase, BusinessReader };");
  return new Function(src.join("\n"))();
}

const R = loadJsDecoders();

/** Every decodable block in the corpus file, decompressed. */
function corpusBlocks() {
  const fd = readFileSync(CORPUS);
  const all = new Uint8Array(fd.buffer, fd.byteOffset, fd.length);
  const h = R.parsePtilesHeader(all.subarray(0, 256));
  const p = R.parsePtilesIndex(all, h);
  const base = R.pickOffsetBase(p, h);
  const dict = h.dictLength
    ? all.subarray(h.dictOffset, h.dictOffset + h.dictLength)
    : new Uint8Array(0);

  const out = [];
  for (const e of p.entries.filter((e) => e.blockLength > 0)) {
    let off = e.blockOffset;
    if (base.kind === "relative") off += h.blocksOffset;
    else if (base.kind === "corrected") off -= base.overshoot;
    out.push({
      cell: e.h3Cell.toString(16),
      raw: wasm.decompress_block(all.subarray(off, off + e.blockLength), dict),
    });
  }
  return out;
}

const BLOCKS = corpusBlocks();
const JS = new R.BusinessReader();

test("the corpus actually yielded business blocks", () => {
  assert.ok(
    BLOCKS.length >= 8,
    `only ${BLOCKS.length} blocks decompressed -- nothing below proves anything`,
  );
});

test("JS and wasm frame business records identically", () => {
  for (const { cell, raw } of BLOCKS) {
    const js = JS._decodeRecords(raw);
    const rs = wasm.decode_business(raw);
    assert.equal(
      js.length, rs.length,
      `cell ${cell}: JS found ${js.length} records, wasm found ${rs.length}. ` +
      `A count difference is a framing difference -- one of them is walking ` +
      `the record lengths wrong.`,
    );
  }
});

test("every field except osm_id decodes identically", () => {
  let compared = 0;
  for (const { cell, raw } of BLOCKS) {
    const js = JS._decodeRecords(raw);
    const rs = wasm.decode_business(raw);
    for (let i = 0; i < js.length; i++) {
      const a = js[i], b = rs[i];
      const where = `cell ${cell} record ${i}`;
      assert.equal(a.name, b.name, `${where}: name`);
      assert.equal(a.lat.toFixed(5), b.lat.toFixed(5), `${where}: lat`);
      assert.equal(a.lon.toFixed(5), b.lon.toFixed(5), `${where}: lon`);
      assert.equal(a.phone || "", b.phone || "", `${where}: phone`);
      assert.equal(a.website || "", b.website || "", `${where}: website`);
      assert.equal(a.address || "", b.address || "", `${where}: address`);
      assert.equal(a.brand || "", b.brand || "", `${where}: brand`);
      const jsCat = a.category ? Number(a.category.match(/\d+/)?.[0] ?? 0) : 0;
      assert.equal(jsCat, b.category_idx, `${where}: category`);
      compared++;
    }
  }
  assert.ok(compared > 0, "no records were compared");
});

// This asserts a known defect, deliberately. If someone fixes index.html's
// decoder to carry the uid as a BigInt, this test fails -- and it should,
// because the exception it documents would no longer be needed. Delete it
// then, and fold osm_id into the test above.
test("osm_id is the one field JS cannot represent, and wasm is the correct side", () => {
  let differed = 0, total = 0;
  for (const { cell, raw } of BLOCKS) {
    const js = JS._decodeRecords(raw);
    const rs = wasm.decode_business(raw);
    for (let i = 0; i < js.length; i++) {
      total++;
      const a = js[i], b = rs[i];
      if (String(a.uid) === String(b.osm_id)) continue;
      differed++;
      assert.ok(
        !Number.isSafeInteger(a.uid),
        `cell ${cell} record ${i}: JS and wasm disagree on osm_id ` +
        `(${a.uid} vs ${b.osm_id}) but the JS value ${a.uid} IS exactly ` +
        `representable as a double. That is a real decode divergence, not the ` +
        `known precision loss, and needs investigating rather than excusing.`,
      );
      assert.equal(
        typeof b.osm_id, "bigint",
        `cell ${cell} record ${i}: wasm must hand back a BigInt to be exact here`,
      );
    }
  }
  assert.ok(total > 0, "no records were compared");
  assert.ok(
    differed > 0,
    "no osm_id differed -- if index.html's decoder was fixed to use BigInt, " +
    "delete this test and assert osm_id alongside every other field instead",
  );
});
