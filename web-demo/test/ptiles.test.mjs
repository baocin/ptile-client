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
      const cell = layer.entries[i].h3_cell;
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
    const slice = await layer.cellRecords(e.h3_cell);
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
      const rec = await layer.cellRecords(e.h3_cell);
      if (!rec) continue;
      const out = P.decode[kind](rec);
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
      const cell = full.entries[i].h3_cell;
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

test("H3 comes from core, and round-trips", () => {
  // Nashville. The legacy demo uses h3-js for this; core has it, so the
  // vendored copy is one more thing web-demo does not ship.
  const cell = P.h3.cellFor(36.1627, -86.7816);
  assert.match(cell, /^[0-9a-f]{15}$/);
  const [lat, lon] = P.h3.center(cell);
  assert.ok(Math.abs(lat - 36.1627) < 0.5 && Math.abs(lon + 86.7816) < 0.5);
  assert.equal(P.h3.neighbors(cell).length, 6);
});
