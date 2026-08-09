// The trail/park/water/rail lookups exposed to the browser: nearest_trail,
// nearest_trailhead, nearest_rail, nearest_station, park_at, water_at and
// point_in_polygon.
//
// golden.mjs proves the decoders match the Python reference. These prove the
// layer above them -- the "which one am I on/in" answers the demo would
// otherwise re-implement in JavaScript, which is exactly where a wrong earth
// radius or an off-by-one in a polygon wrap-around silently mis-answers.
//
// Feature inputs are the shapes the decode_* exports return, so they are
// built here as plain objects rather than decoded from bytes; the golden
// blocks are used where a real record's field naming matters.
//
// Usage: node --test wasm/test/lookups.test.mjs

import test from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const __dirname = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(__dirname, "..", "..");
const wasm = require(path.join(repoRoot, "wasm-pkg", "ptiles_wasm.js"));
const fixturesDir = path.join(repoRoot, "test-fixtures", "golden");

const LAT = 36.0;
const LON = -86.795;

// A path running east along lat 36.0, and a trailhead ~22 m north of it.
// Coordinates are [lon, lat] pairs, the order every decoder emits.
const trails = [
  {
    osm_id: 1,
    trail_type: "path",
    geom_type: 0,
    coords: [
      [-86.8, 36.0],
      [-86.79, 36.0],
    ],
    surface: "compacted",
    sac_scale: "hiking",
    name: "Greenway",
  },
  {
    osm_id: 2,
    trail_type: "trailhead",
    geom_type: 1,
    coords: [[-86.795, 36.0002]],
    surface: "",
    sac_scale: "",
    name: "North Gate",
  },
];

const rail = [
  {
    osm_id: 3,
    rail_type: "rail",
    geom_type: 0,
    coords: [
      [-86.8, 36.001],
      [-86.79, 36.001],
    ],
    name: "Main Line",
  },
  {
    osm_id: 4,
    rail_type: "station",
    geom_type: 1,
    coords: [[-86.795, 36.0005]],
    name: "Union",
  },
];

// A square roughly 400 m on a side, centred on the query point.
const square = (lat, lon, half) => [
  [lon - half, lat - half],
  [lon + half, lat - half],
  [lon + half, lat + half],
  [lon - half, lat + half],
];

test("nearest_trail answers the way and skips the trailhead point", () => {
  const way = wasm.nearest_trail(LAT, LON, trails);
  assert.equal(way.kind, "trail");
  assert.equal(way.name, "Greenway");
  assert.equal(way.class, "path");
  assert.equal(way.on_it, true);
  assert.ok(way.distance_m < 1, `distance ${way.distance_m}`);
});

test("nearest_trailhead answers the point the way lookup skips", () => {
  const head = wasm.nearest_trailhead(LAT, LON, trails);
  assert.equal(head.kind, "trailhead");
  assert.equal(head.name, "North Gate");
  assert.ok(head.distance_m > 10 && head.distance_m < 50, `distance ${head.distance_m}`);
  // A point answer carries its own position; a way answer carries a snap.
  assert.equal(head.lat, 36.0002);
});

test("a missing or empty layer is null, not an error", () => {
  assert.equal(wasm.nearest_trail(LAT, LON, []), null);
  assert.equal(wasm.nearest_trail(LAT, LON, null), null);
  assert.equal(wasm.nearest_trailhead(LAT, LON, undefined), null);
  assert.equal(wasm.park_at(LAT, LON, []), null);
  assert.equal(wasm.water_at(LAT, LON, null), null);
  assert.equal(wasm.nearest_station(LAT, LON, []), null);
});

test("rail separates the track from the station", () => {
  const track = wasm.nearest_rail(LAT, LON, rail);
  assert.equal(track.kind, "rail");
  assert.equal(track.name, "Main Line");

  const station = wasm.nearest_station(LAT, LON, rail);
  assert.equal(station.kind, "station");
  assert.equal(station.name, "Union");
  assert.ok(station.distance_m < 100, `distance ${station.distance_m}`);
});

test("park_at prefers the park you are standing in over a nearer edge", () => {
  const parks = [
    { osm_id: 1, park_type: "park", coords: square(LAT, -86.792, 0.0005), name: "Small" },
    { osm_id: 2, park_type: "nature_reserve", coords: square(LAT, LON, 0.01), name: "Big" },
  ];
  const inside = wasm.park_at(LAT, LON, parks);
  assert.equal(inside.name, "Big", "containment beats a nearer boundary");
  assert.equal(inside.inside, true);
  assert.equal(inside.distance_m, 0);

  const outside = wasm.park_at(36.05, LON, parks);
  assert.equal(outside.inside, false);
  assert.ok(outside.distance_m > 0);
});

test("water_at never calls a river centreline containment", () => {
  const water = [
    {
      osm_id: 1,
      geom_type: 1,
      water_type: "river",
      coords: [
        [-86.8, 36.0],
        [-86.79, 36.0],
      ],
      ref_feature_id: null,
      name: "Cumberland",
      width: null,
    },
  ];
  const at = wasm.water_at(LAT, LON, water);
  assert.equal(at.kind, "water");
  assert.equal(at.inside, false, "a linestring has no interior");
  assert.ok(at.distance_m < 1);

  // Reference geometry carries no coordinates: reporting it would place it
  // wherever the reader guessed.
  const reference = [
    {
      osm_id: 2,
      geom_type: 2,
      water_type: "lake",
      coords: [],
      ref_feature_id: 7,
      name: null,
      width: null,
    },
  ];
  assert.equal(wasm.water_at(LAT, LON, reference), null);
});

test("point_in_polygon agrees with park_at on the same ring", () => {
  const ring = square(LAT, LON, 0.002);
  const flat = ring.flat();
  assert.equal(wasm.point_in_polygon(LAT, LON, Float64Array.from(flat)), true);
  assert.equal(wasm.point_in_polygon(LAT + 0.5, LON, Float64Array.from(flat)), false);
  // Fewer than three vertices is not a polygon, whatever the point.
  assert.equal(wasm.point_in_polygon(LAT, LON, Float64Array.from([LON, LAT])), false);

  const parks = [{ osm_id: 1, park_type: "park", coords: ring, name: "Square" }];
  assert.equal(wasm.park_at(LAT, LON, parks).inside, true);
});

test("the lookups accept what the decoders actually return", () => {
  // The golden blocks are real bytes from the published TN layers, so this
  // catches a field-naming drift between decode_* output and the lookup
  // inputs that hand-written fixtures above would not.
  const parks = wasm.decode_parks(new Uint8Array(readFileSync(path.join(fixturesDir, "parks.block.bin"))));
  assert.ok(parks.length > 0, "golden parks block decodes to features");
  const meta = JSON.parse(readFileSync(path.join(fixturesDir, "parks.meta.json"), "utf8"));
  const at = wasm.park_at(meta.cell_center_lat, meta.cell_center_lon, parks);
  assert.ok(at, "a park block always has a nearest park");
  assert.equal(typeof at.distance_m, "number");
  assert.ok(Number.isFinite(at.distance_m) && at.distance_m < 800000, `distance ${at.distance_m}`);

  const railFeatures = wasm.decode_rail(new Uint8Array(readFileSync(path.join(fixturesDir, "rail.block.bin"))));
  // Whatever this block holds, neither lookup may throw or answer with the
  // other's shape.
  const track = wasm.nearest_rail(meta.cell_center_lat, meta.cell_center_lon, railFeatures);
  const station = wasm.nearest_station(meta.cell_center_lat, meta.cell_center_lon, railFeatures);
  if (track) assert.equal(track.kind, "rail");
  if (station) assert.equal(station.kind, "station");
});

// "Can a camera see me": a camera ~22 m south of the subject, facing north.
const cameraAt = (lat, lon, camera_type, direction, angle, name) => ({
  osm_id: 1,
  lat,
  lon,
  device_type: "camera",
  placement: "public",
  camera_type,
  direction,
  angle,
  operator: null,
  name,
  ref_tag: null,
});

const SUBJECT_LAT = 36.0002;
const CAM_LAT = 36.0;

test("a camera aimed at you sees you, one aimed away does not", () => {
  const cameras = [
    cameraAt(CAM_LAT, LON, "fixed", 0, 60, "At You"),
    cameraAt(CAM_LAT, LON, "fixed", 180, 60, "Facing Away"),
  ];
  const seen = wasm.cameras_seeing(SUBJECT_LAT, LON, cameras, null, null);
  assert.equal(seen.length, 2, "both are in range, whatever they point at");

  const [atYou, away] = [seen[0], seen[1]];
  assert.equal(atYou.sees, true);
  assert.equal(atYou.aimed_at_you, true);
  assert.equal(atYou.aim_assumed, false, "direction and angle were both tagged");
  assert.ok(Math.abs(atYou.bearing_deg) < 1, `due north, got ${atYou.bearing_deg}`);

  assert.equal(away.sees, false);
  assert.equal(away.aimed_at_you, false);
  assert.equal(away.line_of_sight, true, "still reported, with the reason");
});

test("an untagged aim and a dome both assume they can point at you", () => {
  const untagged = wasm.cameras_seeing(SUBJECT_LAT, LON, [cameraAt(CAM_LAT, LON, "fixed", null, null, "Untagged")], null, null);
  assert.equal(untagged[0].sees, true);
  assert.equal(untagged[0].aim_assumed, true);

  // Pointed away on paper, but a dome sweeps.
  const dome = wasm.cameras_seeing(SUBJECT_LAT, LON, [cameraAt(CAM_LAT, LON, "dome", 180, 60, "Dome")], null, null);
  assert.equal(dome[0].sees, true);
  assert.equal(dome[0].aim_assumed, true);
});

test("range excludes, and a building blocks", () => {
  const cameras = [cameraAt(CAM_LAT, LON, "fixed", null, null, "Near")];
  assert.deepEqual(wasm.cameras_seeing(SUBJECT_LAT, LON, cameras, null, 5), []);

  // A tall block straddling the sight line halfway between.
  const wall = {
    coords: square(36.0001, LON, 0.00003),
    height_m: 20,
    building_type: "office",
  };
  const blocked = wasm.cameras_seeing(SUBJECT_LAT, LON, cameras, [wall], null);
  assert.equal(blocked[0].line_of_sight, false);
  // Indices cross as BigInt: they are usize in Rust.
  assert.equal(blocked[0].blocked_by, 0n);
  assert.equal(blocked[0].sees, false);

  // A 1 m canopy is below the line, which runs 4 m down to 1.7 m.
  const canopy = { coords: square(36.0001, LON, 0.00003), height_m: 1, building_type: "canopy" };
  const clear = wasm.cameras_seeing(SUBJECT_LAT, LON, cameras, [canopy], null);
  assert.equal(clear[0].line_of_sight, true);
  assert.equal(clear[0].sees, true);
});

test("bearing_to is clockwise from north and matches the camera answer", () => {
  assert.ok(Math.abs(wasm.bearing_to(36.0, LON, 36.001, LON)) < 0.5);
  assert.ok(Math.abs(wasm.bearing_to(36.0, LON, 36.0, LON + 0.001) - 90) < 0.5);
  assert.ok(Math.abs(wasm.bearing_to(36.0, LON, 35.999, LON) - 180) < 0.5);

  const seen = wasm.cameras_seeing(SUBJECT_LAT, LON, [cameraAt(CAM_LAT, LON, "fixed", null, null, "N")], null, null);
  assert.ok(Math.abs(seen[0].bearing_deg - wasm.bearing_to(CAM_LAT, LON, SUBJECT_LAT, LON)) < 1e-9);
});
