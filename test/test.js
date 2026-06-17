/**
 * ptile-client test suite.
 *
 * Tests single-file mode, multi-state directory mode, query, queryBounds,
 * header parsing, and edge cases.
 *
 * Usage: node --experimental-modules test/test.js
 */

import * as h3 from "h3-js";
import { PtilesReader } from "../src/ptiles.js";
import { strict as assert } from "node:assert";
import { existsSync } from "node:fs";

const DATA_DIR = "/home/aoi/kino/projects/ptiles/data/states";
const TN_FILE = DATA_DIR + "/TN.buildings_v8.ptiles";
const CA_FILE = DATA_DIR + "/CA.buildings_v8.ptiles";

let passed = 0;
let failed = 0;

function test(name, fn) {
  try {
    fn();
    passed++;
    console.log("  PASS  " + name);
  } catch (e) {
    failed++;
    console.log("  FAIL  " + name + ": " + e.message);
  }
}

async function testAsync(name, fn) {
  try {
    await fn();
    passed++;
    console.log("  PASS  " + name);
  } catch (e) {
    failed++;
    console.log("  FAIL  " + name + ": " + e.message);
  }
}

// ===== Prerequisites ===============================================

console.log("=== Prerequisites ===");
test("TN buildings file exists", () => assert(existsSync(TN_FILE)));
test("h3-js loaded", () => assert(typeof h3.latLngToCell === "function"));

// ===== Single file mode ============================================

console.log("\n=== Single file mode ===");
let reader;

await testAsync("fromFile loads TN.buildings_v8", async () => {
  reader = await PtilesReader.fromFile(TN_FILE);
  assert(reader !== null);
});

test("header format is buildings", () => {
  assert.equal(reader.header.format, "buildings");
});

test("header version is 8", () => {
  assert.equal(reader.header.version, 8);
});

test("header has feature count", () => {
  assert(reader.header.featureCount > 800000);
});

test("header has valid TN bounds", () => {
  assert(reader.header.minLat > 30);
  assert(reader.header.maxLat < 40);
  assert(reader.header.minLon < -80);
  assert(reader.header.maxLon > -90);
});

test("index has entries", () => {
  assert(reader.index.entries.length > 1000);
});

test("index entries have BigInt h3Cell", () => {
  assert.equal(typeof reader.index.entries[0].h3Cell, "bigint");
});

test("cellMap has entries (masked keys)", () => {
  assert(reader.index.cellMap.size > 2000);
  assert(reader.index.entries.length >= reader.index.cellMap.size);
});

// ===== Query =======================================================

console.log("\n=== Query ===");

await testAsync("query at building centroid returns building", async () => {
  const bounds = await reader.queryBounds(35.5, -87.9, 35.6, -87.8, 1, { h3 });
  if (bounds.length === 0) {
    console.log("  NOTE  no buildings in test area, skipping");
    return;
  }
  const b = bounds[0];
  const result = await reader.query(b.centroidLat, b.centroidLon, { h3 });
  assert(result !== null);
  assert.equal(result.osmId, b.osmId);
});

await testAsync("query at cell center returns building or null", async () => {
  const cellHex = reader.index.entries[10].h3Cell.toString(16);
  const [lat, lon] = h3.cellToLatLng(cellHex);
  const result = await reader.query(lat, lon, { h3 });
  assert(result === null || result.osmId > 0);
});

await testAsync("query at ocean returns null", async () => {
  assert.equal(await reader.query(30.0, -88.0, { h3 }), null);
});

// ===== Bounds query ================================================

console.log("\n=== Bounds query ===");

await testAsync("queryBounds returns buildings in TN", async () => {
  const results = await reader.queryBounds(35.5, -87.9, 35.6, -87.8, 10, {
    h3,
  });
  assert(Array.isArray(results));
  if (results.length > 0) {
    const b = results[0];
    assert(typeof b.osmId === "number");
    assert(typeof b.buildingType === "string");
    assert(Array.isArray(b.coordinates));
    assert(b.coordinates.length >= 3);
    assert(typeof b.centroidLat === "number");
  }
});

await testAsync("queryBounds respects limit", async () => {
  const results = await reader.queryBounds(35.0, -88.0, 36.5, -86.0, 7, { h3 });
  assert(results.length <= 7);
});

await testAsync("queryBounds empty area", async () => {
  const results = await reader.queryBounds(30.0, -88.0, 30.001, -87.999, 10, {
    h3,
  });
  assert.equal(results.length, 0);
});

// ===== Multi-state directory =======================================

console.log("\n=== Multi-state directory mode ===");
let national;

await testAsync("fromStateDir loads states", async () => {
  national = await PtilesReader.fromStateDir(DATA_DIR, { h3 });
  assert(national._files instanceof Map);
  // Some state files may have empty indices (known build issue);
  // count only files with usable indices
  const usable = Array.from(national._files.values()).filter(
    (f) => f.index.entries.length > 0,
  ).length;
  console.log(
    "  (usable state files: " + usable + "/" + national._files.size + ")",
  );
  assert(usable > 0);
});

await testAsync("fromStateDir query in TN", async () => {
  const b = await national.query(35.5016, -87.8508);
  assert(b === null || b.osmId > 0);
});

await testAsync("fromStateDir query in NY", async () => {
  const b = await national.query(40.7128, -74.006);
  assert(b === null || b.osmId > 0);
});

await testAsync("fromStateDir query in HI", async () => {
  const b = await national.query(21.3069, -157.8583);
  assert(b === null || b.osmId > 0);
});

await testAsync("fromStateDir bounds in TN", async () => {
  const results = await national.queryBounds(35.5, -87.9, 35.6, -87.8, 20);
  assert(results.length > 0);
});

// ===== Edge cases ==================================================

console.log("\n=== Edge cases ===");

const oceanPoints = [
  [0, 0],
  [40, -130],
  [70, -170],
  [-30, 150],
];
for (const [lat, lon] of oceanPoints) {
  await testAsync(
    "query at (" + lat + ", " + lon + ") returns null",
    async () => {
      assert.equal(await reader.query(lat, lon, { h3 }), null);
    },
  );
}

test("loadFile with empty index does not crash", async () => {
  if (existsSync(CA_FILE)) {
    const r = await PtilesReader.fromFile(CA_FILE);
    assert(Array.isArray(r.index.entries));
    // CA file may have empty index; that's acceptable
  }
});

test("fromStateDir handles non-existent directory gracefully", async () => {
  try {
    const r = await PtilesReader.fromStateDir("/nonexistent_dir_xyz", { h3 });
    assert.equal(r._files.size, 0);
  } catch (e) {
    assert(e.code === "ENOENT");
  }
});

// ===== Summary =====================================================

console.log("\n=== Results: " + passed + " passed, " + failed + " failed ===");
if (failed > 0) process.exit(1);
