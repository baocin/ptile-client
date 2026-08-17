// The address layer through the browser's own reader.
//
// web-demo/js/ptiles.js plus the wasm bindings are what the page runs, and
// until now nothing exercised the address layer through them at all -- the
// only address coverage was Rust-side. That gap is why a stale wasm build
// decoded v3 as v2 and drifted 51,775 bytes into a block while every Rust test
// stayed green, and why the page could ship pinned to a file version its
// decoder no longer matched.
//
// Both golden fixtures are used deliberately: v2 (no source byte) and v3 (one
// byte more per record). A reader that ignores the header version passes one
// and fails the other, which is exactly the bug that shipped.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { createRequire } from "node:module";

import { createPtiles } from "../js/ptiles.js";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = join(HERE, "..", "..");
const WASM_PKG = join(ROOT, "wasm-pkg", "ptiles_wasm.js");
const GOLDEN = join(ROOT, "test-fixtures", "golden");

if (!existsSync(WASM_PKG)) {
  throw new Error(
    `wasm-pkg/ not built -- this test cannot run.\n` +
      `  PATH="$HOME/.cargo/bin:$PATH" wasm-pack build wasm --target nodejs --out-dir ../wasm-pkg --release`,
  );
}

const require = createRequire(import.meta.url);
const wasm = require(WASM_PKG);
const P = createPtiles(wasm);

const openFixture = (name) =>
  P.open(P.bytesSource(new Uint8Array(readFileSync(join(GOLDEN, name)))));

/** Every record in the file, decoded the way index.html decodes them. */
async function allRecords(layer) {
  const out = [];
  for (const entry of layer.entries) {
    if (!entry.block_length) continue;
    const block = await layer.block(entry);
    const recs =
      wasm.address_cell(block, entry.h3_cell.toString(16), layer.header.version) || [];
    for (const r of recs) out.push(r);
  }
  return out;
}

test("v3 decodes through the browser reader, with positions and sources", async () => {
  const layer = await openFixture("address_v3_dict.ptiles");
  assert.equal(layer.header.version, 3);
  const recs = await allRecords(layer);
  assert.equal(recs.length, 4, "fixture holds four addresses");
  for (const r of recs) {
    assert.ok(Number.isFinite(r.lat) && Number.isFinite(r.lon), "record has a position");
    assert.ok(r.housenumber.length > 0 && r.street.length > 0);
  }
  // A reader that skipped the v3 source byte would report every record as osm
  // *and* shift the strings; the fixture mixes sources so it cannot.
  const sources = new Set(recs.map((r) => r.source));
  assert.ok(sources.size > 1, `expected mixed sources, got ${[...sources]}`);
});

test("v2 still decodes, and reports osm for every record", async () => {
  const layer = await openFixture("address_v2_dict.ptiles");
  assert.equal(layer.header.version, 2);
  const recs = await allRecords(layer);
  assert.equal(recs.length, 4);
  assert.ok(recs.every((r) => r.source === "osm"), "v2 predates the other corpora");
  assert.ok(recs.every((r) => Number.isFinite(r.lat)));
});

test("records land inside the cell that indexes them", async () => {
  // The geometric check, in the browser's reader rather than only in Rust: a
  // wrong centre keeps the street name and moves the pin kilometres, and the
  // block header's centre is the first cell's, not each cell's.
  const layer = await openFixture("address_v3_dict.ptiles");
  for (const entry of layer.entries) {
    if (!entry.block_length) continue;
    const block = await layer.block(entry);
    const recs =
      wasm.address_cell(block, entry.h3_cell.toString(16), layer.header.version) || [];
    for (const r of recs) {
      const resolved = BigInt("0x" + wasm.cell_for_coord(r.lat, r.lon));
      assert.equal(
        resolved,
        entry.h3_cell,
        `${r.housenumber} ${r.street} at ${r.lat},${r.lon} is not in ${entry.h3_cell.toString(16)}`,
      );
    }
  }
});

test("the index bytes the whole-file search needs survive open()", async () => {
  // IndexEntry drops the per-cell bbox, so the page cannot order cells without
  // the raw index. Losing this silently turns statewide search back into a
  // viewport scan.
  const layer = await openFixture("address_v3_dict.ptiles");
  assert.ok(layer.indexBytes instanceof Uint8Array);
  assert.ok(layer.indexBytes.length > 0);
});

test("cells come back ordered by how near they could be", async () => {
  const layer = await openFixture("address_v3_dict.ptiles");
  const recs = await allRecords(layer);
  const target = recs[0];

  const order = wasm.address_cells_by_distance(layer.indexBytes, target.lat, target.lon);
  assert.ok(order.length >= 2, "fixture has two cells");
  for (let i = 1; i < order.length; i++) {
    assert.ok(
      order[i].distance_m >= order[i - 1].distance_m,
      "distances must be non-decreasing",
    );
  }
  assert.equal(order[0].distance_m, 0, "the cell containing the point is zero away");

  // The nearest cell must be the one that actually holds the record.
  const block = await layer.block(layer.entryFor(BigInt("0x" + order[0].cell_hex)));
  const inNearest =
    wasm.address_cell(block, order[0].cell_hex, layer.header.version) || [];
  assert.ok(
    inNearest.some((r) => r.housenumber === target.housenumber && r.street === target.street),
    "nearest cell should contain the record its own coordinates named",
  );
});

test("forward search finds a record in a cell the viewport never names", async () => {
  // The whole reason the page stopped scanning the viewport: search from the
  // far cell and still find the record in the near one.
  const layer = await openFixture("address_v3_dict.ptiles");
  const recs = await allRecords(layer);
  const near = recs[0];
  const far = recs.find((r) => Math.abs(r.lat - near.lat) > 0.01);
  assert.ok(far, "fixture should span two cells");

  const order = wasm.address_cells_by_distance(layer.indexBytes, far.lat, far.lon);
  let hits = [];
  for (const ref of order) {
    const entry = layer.entryFor(BigInt("0x" + ref.cell_hex));
    if (!entry || !entry.block_length) continue;
    const block = await layer.block(entry);
    const cellRecs =
      wasm.address_cell(block, ref.cell_hex, layer.header.version) || [];
    hits = hits.concat(
      wasm.geocode_addresses(`${near.housenumber} ${near.street}`, cellRecs, 25) || [],
    );
  }
  assert.ok(
    hits.some((h) => Math.abs(h.lat - near.lat) < 1e-6),
    `searching from ${far.lat},${far.lon} did not reach ${near.housenumber} ${near.street}`,
  );
});

test("street type spelling does not change what forward search returns", async () => {
  // "Beale St" vs "BEALE Street" cost 51 real addresses in Memphis when
  // matching was raw substring.
  const layer = await openFixture("address_v3_dict.ptiles");
  const recs = await allRecords(layer);
  const target = recs.find((r) => /\bSt$/i.test(r.street) || /Street/i.test(r.street));
  assert.ok(target, "fixture should carry a street with a type word");

  const short = target.street.replace(/Street/i, "St");
  const long = target.street.replace(/\bSt\b/i, "Street");
  const hitsFor = (street) =>
    (wasm.geocode_addresses(`${target.housenumber} ${street}`, recs, 25) || []).length;

  assert.equal(
    hitsFor(short),
    hitsFor(long),
    `"${short}" and "${long}" must match the same records`,
  );
  assert.ok(hitsFor(long) > 0, "the long spelling must match something");
});
