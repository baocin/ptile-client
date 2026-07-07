// WASM golden test (plan Phase 3 step 12): load the built ptiles-wasm pkg,
// run each golden block.bin through its matching decode_* export, deep
// compare against the corresponding golden.json produced by the Python
// reference decoders (test-fixtures/golden/, see plan Phase 1 step 6).
//
// Two known, expected field-naming differences between golden.json (Python
// ref generator's own naming) and the wasm/core contract (which matches the
// OLD seed src/lib.rs field names exactly - the real parity contract):
//   - buildings: golden calls the polygon ring "coordinates"; core/wasm and
//     the old seed both call it "coords". Normalized before comparing.
//   - water: golden includes a "vertex_count" field that isn't part of any
//     Rust struct (decode-time detail only, not surfaced). Dropped before
//     comparing.
// Everything else is compared key-for-key with no normalization.
//
// Usage: node wasm/test/golden.mjs

import { createRequire } from "node:module";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const __dirname = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(__dirname, "..", "..");
const fixturesDir = path.join(repoRoot, "test-fixtures", "golden");

const wasm = require(path.join(repoRoot, "wasm-pkg", "ptiles_wasm.js"));

function loadFixture(layer) {
  const block = readFileSync(path.join(fixturesDir, `${layer}.block.bin`));
  const golden = JSON.parse(readFileSync(path.join(fixturesDir, `${layer}.golden.json`), "utf8"));
  const meta = JSON.parse(readFileSync(path.join(fixturesDir, `${layer}.meta.json`), "utf8"));
  return { block: new Uint8Array(block), golden, meta };
}

function normalizeBuildings(list) {
  return list.map((b) => {
    const { coordinates, ...rest } = b;
    return { ...rest, coords: coordinates };
  });
}

const WATER_GEOM_TYPE = { polygon: 0, linestring: 1, reference: 2 };

function normalizeWater(list) {
  return list.map(({ vertex_count, geom_type, ...rest }) => ({
    ...rest,
    geom_type: WATER_GEOM_TYPE[geom_type] ?? geom_type,
  }));
}

// Deep equality with float tolerance (golden JSON floats round-trip through
// Python; wasm floats round-trip through Rust f64 -> JS number - should be
// bit-identical for these values, but tolerate tiny epsilon just in case).
function deepEqual(a, b, pathStr = "$") {
  // wasm-bindgen serializes Option::None as JS `undefined`; golden.json
  // (parsed from JSON) has `null` for the same slot. Same-shape, JS/JSON
  // impedance mismatch only - not a decoder divergence (the old seed's
  // serde_wasm_bindgen::to_value did the same). Treat as equal.
  const aMissing = a === null || a === undefined;
  const bMissing = b === null || b === undefined;
  if (aMissing && bMissing) return null;
  // wasm-bindgen surfaces large (>2^53) integers as BigInt (exact, straight
  // from Rust i64); golden.json's copy of the same id already lost precision
  // when Node's JSON.parse coerced it to a f64 double (JSON has no bigint -
  // this loss happens on the golden side, not the wasm side). Compare with a
  // tolerance sized to f64's ~15-16 significant-digit precision instead of
  // demanding exact equality above 2^53.
  if (typeof a === "bigint" || typeof b === "bigint") {
    const an = Number(a);
    const bn = Number(b);
    const tolerance = Math.max(1, Math.abs(an) * 1e-15);
    if (Math.abs(an - bn) <= tolerance) return null;
    return `${pathStr}: ${a} !== ${b} (bigint vs f64-precision-loss tolerance exceeded)`;
  }
  if (typeof a === "number" && typeof b === "number") {
    if (Number.isNaN(a) && Number.isNaN(b)) return null;
    if (Math.abs(a - b) > 1e-9) return `${pathStr}: ${a} !== ${b}`;
    return null;
  }
  if (a === b) return null;
  if (a === null || b === null || typeof a !== typeof b) {
    return `${pathStr}: ${JSON.stringify(a)} !== ${JSON.stringify(b)}`;
  }
  if (Array.isArray(a) || Array.isArray(b)) {
    if (!Array.isArray(a) || !Array.isArray(b)) return `${pathStr}: array/non-array mismatch`;
    if (a.length !== b.length) return `${pathStr}: length ${a.length} !== ${b.length}`;
    for (let i = 0; i < a.length; i++) {
      const d = deepEqual(a[i], b[i], `${pathStr}[${i}]`);
      if (d) return d;
    }
    return null;
  }
  if (typeof a === "object") {
    const keysA = Object.keys(a).sort();
    const keysB = Object.keys(b).sort();
    if (JSON.stringify(keysA) !== JSON.stringify(keysB)) {
      return `${pathStr}: key sets differ: [${keysA}] vs [${keysB}]`;
    }
    for (const k of keysA) {
      const d = deepEqual(a[k], b[k], `${pathStr}.${k}`);
      if (d) return d;
    }
    return null;
  }
  return `${pathStr}: ${JSON.stringify(a)} !== ${JSON.stringify(b)}`;
}

const cases = [
  {
    layer: "buildings_v8",
    run: ({ block, meta }) => wasm.decode_buildings(block, meta.cell_center_lat, meta.cell_center_lon),
    expected: (golden) => normalizeBuildings(golden.buildings),
  },
  {
    layer: "business",
    run: ({ block }) => wasm.decode_business(block),
    expected: (golden) => golden.businesses,
  },
  {
    layer: "parks",
    run: ({ block }) => wasm.decode_parks(block),
    expected: (golden) => golden.features,
  },
  {
    layer: "rail",
    run: ({ block }) => wasm.decode_rail(block),
    expected: (golden) => golden.features,
  },
  {
    layer: "roads",
    run: ({ block }) => wasm.decode_roads(block),
    expected: (golden) => golden.roads,
  },
  {
    layer: "water",
    run: ({ block }) => wasm.decode_water(block),
    expected: (golden) => normalizeWater(golden.features),
  },
];

let failed = 0;
for (const c of cases) {
  const fixture = loadFixture(c.layer);
  let actual;
  try {
    actual = c.run(fixture);
  } catch (e) {
    console.log(`FAIL ${c.layer}: threw: ${e}`);
    failed++;
    continue;
  }
  const expected = c.expected(fixture.golden);
  const diff = deepEqual(actual, expected);
  if (diff) {
    console.log(`FAIL ${c.layer}: ${diff}`);
    failed++;
  } else {
    console.log(`PASS ${c.layer}: ${actual.length} records match golden`);
  }
}

// decompress_block smoke test: re-derive a block.bin's compressed bytes isn't
// available from these fixtures (only decompressed blocks are committed), so
// this only checks the function is callable and rejects clearly on garbage
// input rather than round-tripping a real compressed block.
try {
  wasm.decompress_block(new Uint8Array([1, 2, 3]), new Uint8Array());
  console.log("FAIL decompress_block: expected throw on garbage input, got success");
  failed++;
} catch (e) {
  console.log(`PASS decompress_block: rejects garbage input as expected (${String(e).slice(0, 60)}...)`);
}

if (failed > 0) {
  console.log(`\n${failed} case(s) failed`);
  process.exit(1);
} else {
  console.log(`\nall ${cases.length} golden cases + decompress_block smoke test passed`);
}
