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
    // `height_m` is dropped before comparing, and cannot be added to the
    // fixture instead: `test-fixtures/extract_golden.py` generates it via the
    // Python reference decoder, and that decoder skips the field outright
    // (`ptiles/buildings.py`: "flags2 & 0x10: has_height_m (skip)"). So the
    // golden JSON physically cannot carry a height without changing a second
    // repo. Height is asserted separately below, and in the Rust unit test
    // against this same block.
    run: ({ block, meta }) =>
      wasm.decode_buildings(block, meta.cell_center_lat, meta.cell_center_lon)
        .map(({ height_m, ...rest }) => rest),
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

// Height crosses the wasm boundary. The golden comparison above has to drop
// the field, so without this nothing would catch `height_m` failing to
// serialize out of Rust — the exact silent-empty shape this repo keeps hitting.
// Counts match core's own golden assertion on the same block.
try {
  const { block, meta } = loadFixture("buildings_v8");
  const bldgs = wasm.decode_buildings(block, meta.cell_center_lat, meta.cell_center_lon);
  const heights = bldgs.map((b) => b.height_m).filter((h) => h != null);
  const halfMetre = heights.every((h) => Number.isInteger(h * 2));
  if (bldgs.length === 1354 && heights.length === 149 && halfMetre) {
    console.log(`PASS buildings_v8 height: ${heights.length}/${bldgs.length} carry a height, all multiples of 0.5 m`);
  } else {
    console.log(`FAIL buildings_v8 height: ${heights.length}/${bldgs.length} with height (want 149/1354), halfMetre=${halfMetre}`);
    failed++;
  }
} catch (e) {
  console.log(`FAIL buildings_v8 height: threw: ${e}`);
  failed++;
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

// roads_in_block / nearest_road (plan addendum item 1): exercise against
// the same roads.block.bin fixture used above, at a known coordinate.
{
  const { block, golden } = loadFixture("roads");

  const allRoads = wasm.roads_in_block(block);
  const diff = deepEqual(allRoads, golden.roads);
  if (diff) {
    console.log(`FAIL roads_in_block: ${diff}`);
    failed++;
  } else {
    console.log(`PASS roads_in_block: ${allRoads.length} records match golden`);
  }

  // Known coordinate: golden.roads[0]'s first vertex (osm_id 19443101,
  // motorway_link, coords[0] = [-86.79397, 36.16412] as [lon, lat]).
  const knownRoad = golden.roads[0];
  const [knownLon, knownLat] = knownRoad.coords[0];

  const nearest = wasm.nearest_road(block, knownLat, knownLon);
  if (!nearest) {
    console.log("FAIL nearest_road: expected a match at the road's own first vertex, got null");
    failed++;
  } else if (Number(nearest.osm_id) !== knownRoad.osm_id) {
    console.log(`FAIL nearest_road: expected osm_id ${knownRoad.osm_id}, got ${nearest.osm_id}`);
    failed++;
  } else if (nearest.distance_m > 1.0) {
    console.log(`FAIL nearest_road: expected ~0m snap distance at the road's own vertex, got ${nearest.distance_m}m`);
    failed++;
  } else if (nearest.road_class !== knownRoad.road_class) {
    console.log(`FAIL nearest_road: expected road_class ${knownRoad.road_class}, got ${nearest.road_class}`);
    failed++;
  } else if (!Array.isArray(nearest.geometry) || nearest.geometry.length !== knownRoad.coords.length) {
    console.log(`FAIL nearest_road: geometry length mismatch (expected ${knownRoad.coords.length}, got ${nearest.geometry?.length})`);
    failed++;
  } else if (nearest.geometry[0][0] !== knownLat || nearest.geometry[0][1] !== knownLon) {
    console.log(`FAIL nearest_road: geometry[0] expected [lat, lon]=[${knownLat}, ${knownLon}], got ${JSON.stringify(nearest.geometry[0])}`);
    failed++;
  } else {
    console.log(`PASS nearest_road: matched osm_id ${knownRoad.osm_id} at distance ${nearest.distance_m}m, [lat,lon] geometry shape confirmed`);
  }

  // Far-away coordinate, tight threshold: expect no match.
  const farAway = wasm.nearest_road(block, 0.0, 0.0, 1.0);
  if (farAway !== null) {
    console.log(`FAIL nearest_road: expected null far from all roads, got ${JSON.stringify(farAway)}`);
    failed++;
  } else {
    console.log("PASS nearest_road: null when no road is within threshold");
  }
}

// score_candidates (plan addendum item 2): synthetic fix near the known
// road coordinate above, moving fast enough that the road candidate should
// rank first (and score/rank fields should be sane).
{
  const { block: roadsBlock, golden } = loadFixture("roads");
  const knownRoad = golden.roads[0];
  const [knownLon, knownLat] = knownRoad.coords[0];

  const fix = JSON.stringify({
    lat: knownLat,
    lon: knownLon,
    horizontal_accuracy_m: 10.0,
    speed_mps: 8.0,
  });

  const emptyBlock = new Uint8Array();
  const candidates = wasm.score_candidates(fix, roadsBlock, emptyBlock, emptyBlock, 0.0, 0.0);

  if (!Array.isArray(candidates) || candidates.length === 0) {
    console.log(`FAIL score_candidates: expected a non-empty ranked list, got ${JSON.stringify(candidates)}`);
    failed++;
  } else {
    const top = candidates[0];
    const sorted = candidates.every((c, i) => i === 0 || candidates[i - 1].score >= c.score);
    if (top.kind !== "Road") {
      console.log(`FAIL score_candidates: expected top candidate kind "Road" (moving fix on its own road vertex), got ${JSON.stringify(top)}`);
      failed++;
    } else if (!sorted) {
      console.log("FAIL score_candidates: candidates not sorted descending by score");
      failed++;
    } else if (Number(top.osm_id) !== knownRoad.osm_id) {
      console.log(`FAIL score_candidates: expected top osm_id ${knownRoad.osm_id}, got ${top.osm_id}`);
      failed++;
    } else {
      console.log(`PASS score_candidates: ${candidates.length} ranked candidates, top=${JSON.stringify(top, (_, v) => (typeof v === "bigint" ? v.toString() : v))}`);
    }
  }
}

if (failed > 0) {
  console.log(`\n${failed} case(s) failed`);
  process.exit(1);
} else {
  console.log(`\nall ${cases.length} golden cases + decompress_block/nearest_road/roads_in_block/score_candidates tests passed`);
}
