// Map context for a trace: which state's layer files to open, which H3 cells a
// trace touches, and what the roads layer says at a point.
//
// The constants and `stateAt` are copied from web-demo/index.html (the snapshot
// base, the layer filename table, the CONUS bboxes). Everything that reads
// bytes is reached through js/ptiles.js, which is symlinked from web-demo and
// carries the range-request cache, the block cache and the prefetch coalescing
// -- reimplementing any of that here would be a second copy to keep in sync.

/**
 * Dated snapshot, not the flat /maps/ root: that root is the legacy set with
 * mixed vintages. The date is also written into every emitted rook:context, so
 * a consumer can tell that a 2012 trace was annotated with a 2026 map.
 */
export const PTILES_BASE = "https://maps.mydatatimeline.com/maps/2026-08-07/";
export const SNAPSHOT = "2026-08-07";

/**
 * Layer key -> filename stem in that snapshot.
 *
 * Every layer this page can open belongs here. A missing key falls through to
 * the bare name, which produces a 404 that looks exactly like "that state has no
 * such layer" -- which is how a half-copied table cost an afternoon: water and
 * parks were absent here, so the vector basemap reported them unavailable while
 * `NC.water_v1.ptiles` sat on the host perfectly happily.
 */
const LAYER_FILES = {
  roads: "roads_v2",
  water: "water_v1",
  parks: "parks_v1",
  trails: "trails_v1",
  rail: "rail_v1",
  buildings: "buildings_v9",
  business: "business_v4",
  address: "address_v2",
  admin: "admin",
};

export function stateUrl(state, layer) {
  return PTILES_BASE + state + "." + (LAYER_FILES[layer] || layer) + ".ptiles";
}

// Rough CONUS bboxes, copied from web-demo/index.html. State is resolved from
// coordinates this way rather than from US.admin: picking a *filename* must not
// require a 28 MB grid download, and the admin layer is opt-in below purely for
// the jurisdiction fields of rook:context.
const STATE_BBOX = {
  AL:[30.1,-88.5,35.1,-84.8], AK:[51,-180,72,-129], AZ:[31.3,-114.9,37.1,-109], AR:[33,-94.7,36.6,-89.6],
  CA:[32.5,-124.5,42.1,-114.1], CO:[36.9,-109.1,41.1,-102], CT:[40.9,-73.8,42.1,-71.7], DC:[38.7,-77.2,39,-76.9],
  DE:[38.4,-75.8,39.9,-75], FL:[24.4,-87.7,31.1,-79.9], GA:[30.3,-85.7,35.1,-80.7], HI:[18.8,-160.3,22.3,-154.7],
  ID:[41.9,-117.3,49.1,-111], IL:[36.9,-91.6,42.6,-87.4], IN:[37.7,-88.1,41.8,-84.7], IA:[40.3,-96.7,43.6,-90.1],
  KS:[36.9,-102.1,40.1,-94.5], KY:[36.4,-89.6,39.2,-81.9], LA:[28.8,-94.1,33.1,-88.7], ME:[43,-71.2,47.5,-66.8],
  MD:[37.8,-79.5,39.8,-75], MA:[41.2,-73.6,42.9,-69.8], MI:[41.6,-90.5,48.4,-82.1], MN:[43.4,-97.3,49.4,-89.4],
  MS:[30,-91.7,35.1,-88], MO:[35.9,-95.8,40.7,-89], MT:[44.3,-116.1,49.1,-104], NE:[39.9,-104.1,43.1,-95.3],
  NV:[35,-120.1,42.1,-114], NH:[42.6,-72.6,45.4,-70.5], NJ:[38.8,-75.6,41.4,-73.8], NM:[31.3,-109.1,37.1,-103],
  NY:[40.4,-79.8,45.1,-71.7], NC:[33.7,-84.4,36.7,-75.3], ND:[45.9,-104.1,49.1,-96.5], OH:[38.3,-84.9,42,-80.4],
  OK:[33.5,-103.1,37.1,-94.3], OR:[41.9,-124.7,46.3,-116.4], PA:[39.6,-80.6,42.3,-74.6], RI:[41.1,-71.9,42.1,-71.1],
  SC:[32,-83.4,35.3,-78.4], SD:[42.4,-104.1,46,-96.4], TN:[34.9,-90.4,36.7,-81.6], TX:[25.7,-106.7,36.6,-93.4],
  UT:[36.9,-114.1,42.1,-109], VT:[42.7,-73.5,45.1,-71.4], VA:[36.5,-83.7,39.5,-75.1], WA:[45.5,-124.9,49.1,-116.9],
  WV:[37.1,-82.7,40.7,-77.6], WI:[42.4,-92.9,47.1,-86.2], WY:[40.9,-111.1,45.1,-104],
};
const STATE_CENTERS = {
  AL:[32.8,-86.8], AK:[64.2,-153.4], AZ:[34.3,-111.1], AR:[34.9,-92.4], CA:[36.8,-119.5], CO:[38.9,-105.5],
  CT:[41.6,-72.7], DC:[38.9,-77.0], DE:[39.0,-75.5], FL:[27.7,-81.5], GA:[32.7,-83.5], HI:[20.8,-156.3],
  ID:[44.4,-114.5], IL:[40.0,-89.2], IN:[39.9,-86.3], IA:[41.9,-93.1], KS:[38.5,-98.4], KY:[37.7,-85.3],
  LA:[31.2,-91.8], ME:[45.3,-69.2], MD:[39.1,-76.8], MA:[42.3,-71.8], MI:[44.3,-85.4], MN:[46.3,-94.2],
  MS:[32.6,-89.7], MO:[38.4,-92.5], MT:[46.9,-110.5], NE:[41.5,-99.9], NV:[39.3,-116.6], NH:[43.7,-71.6],
  NJ:[40.2,-74.7], NM:[34.5,-106.0], NY:[42.9,-75.5], NC:[35.6,-79.4], ND:[47.5,-100.4], OH:[40.4,-82.8],
  OK:[35.6,-96.9], OR:[43.9,-120.6], PA:[40.9,-77.8], RI:[41.7,-71.5], SC:[33.9,-80.9], SD:[44.4,-100.3],
  TN:[35.96,-86.52], TX:[31.2,-99.3], UT:[39.3,-111.1], VT:[44.0,-72.7], VA:[37.5,-78.8], WA:[47.4,-120.5],
  WV:[38.7,-80.7], WI:[44.2,-89.8], WY:[43.0,-107.6],
};

/** Two-letter state for a coordinate, nearest centre among bbox hits. */
export function stateAt(lat, lon) {
  const hits = [];
  for (const s in STATE_BBOX) {
    const b = STATE_BBOX[s];
    if (lat >= b[0] && lat <= b[2] && lon >= b[1] && lon <= b[3]) hits.push(s);
  }
  if (!hits.length) return null;
  let best = null;
  let bestD = Infinity;
  for (const s of hits) {
    const c = STATE_CENTERS[s];
    const d = (lat - c[0]) ** 2 + (lon - c[1]) ** 2;
    if (d < bestD) {
      bestD = d;
      best = s;
    }
  }
  return best;
}

/**
 * Per-state roads layers, opened once and reused.
 *
 * A layer that fails to open -- a state with no file in this snapshot, or a host
 * that cannot be reached -- is recorded as an explicit failure rather than
 * retried per point, and the caller reports it. Silently producing context-free
 * segments is the failure mode the whole client exists to eliminate.
 *
 * As of the 2026-08-07 snapshot every state the fixture traces touch (TN and NC)
 * has roads, water, parks and buildings published, so this path is a guard
 * rather than a routine occurrence.
 */
export function createResolver(P, wasm) {
  const layers = new Map(); // state -> Promise<Layer|null>
  const failures = new Map(); // state -> message
  const CELL_MASK = wasm.cell_filler_mask();

  function norm(cell) {
    return BigInt(cell) & CELL_MASK;
  }

  /**
   * Any layer for any state, opened once and reused.
   *
   * Keyed by state *and* layer: a trace needs roads to classify, and buildings /
   * addresses / businesses the moment you click somewhere and ask what is there.
   * A failure is recorded once rather than retried per point, and the caller
   * reports it -- quietly returning no context is the failure mode this whole
   * client exists to eliminate.
   */
  function layerFor(state, name) {
    const key = `${state}/${name}`;
    if (!layers.has(key)) {
      layers.set(
        key,
        P.open(P.httpSource(stateUrl(state, name))).catch((e) => {
          failures.set(key, e.message || String(e));
          return null;
        }),
      );
    }
    return layers.get(key);
  }

  const roadsFor = (state) => layerFor(state, "roads");

  /** `{cells, states}` a trace touches. Pure wasm, no I/O. */
  function survey(points) {
    const cells = new Map(); // state -> Set<bigint>
    for (const p of points) {
      const st = stateAt(p.lat, p.lon);
      if (!st) continue;
      const cell = norm(BigInt("0x" + wasm.cell_for_coord(p.lat, p.lon)));
      if (!cells.has(st)) cells.set(st, new Set());
      cells.get(st).add(cell);
    }
    return cells;
  }

  /**
   * Warm the block cache for every cell a trace touches, one coalesced pass
   * per state. `Layer.prefetch` merges byte ranges within 64 KiB of each other
   * into single requests, and a trace's cells are spatially adjacent, which is
   * also adjacent in the cell-ordered index -- so a 50-cell trace costs a
   * handful of range reads rather than 50.
   */
  async function prefetch(points) {
    const cells = survey(points);
    await Promise.all(
      [...cells].map(async ([state, set]) => {
        const layer = await roadsFor(state);
        if (!layer) return;
        await layer.prefetch([...set]);
      }),
    );
    return cells;
  }

  /**
   * Road and intersection context at one point, or `{}` when nothing resolves.
   *
   * ponytail: `nearest_road` and `nearest_intersection` each decode the whole
   * roads block on every call (wasm/src/lib.rs), and a downtown cell holds tens
   * of thousands of features -- so this is called for a handful of sampled
   * points per segment, never per point. The upgrade path is a wasm `RoadIndex`
   * class that decodes once in its constructor and answers nearest() from a
   * grid; that would make per-point resolution free and delete the sampling
   * scheme in segments.js entirely.
   */
  async function at(lat, lon) {
    const st = stateAt(lat, lon);
    if (!st) return {};
    const layer = await roadsFor(st);
    if (!layer) return {};
    const cell = norm(BigInt("0x" + wasm.cell_for_coord(lat, lon)));
    let bytes = null;
    try {
      bytes = await layer.cellRecords(cell);
    } catch {
      return {};
    }
    if (!bytes) return {};
    let road = null;
    let intersection = null;
    try {
      road = wasm.nearest_road(bytes, lat, lon, 30);
    } catch {
      road = null;
    }
    try {
      intersection = wasm.nearest_intersection(bytes, lat, lon, 30);
    } catch {
      intersection = null;
    }
    return { state: st, road, intersection };
  }

  /**
   * Resolve one segment from a few sampled points and reduce to one answer:
   * the modal road class, with the closest snap of that class, and the closest
   * intersection seen. One noisy sample cannot then decide a whole segment's
   * prior.
   */
  async function forSegment(points, indices) {
    const seen = [];
    for (const i of indices) {
      const p = points[i];
      const got = await at(p.lat, p.lon);
      seen.push({ i, ...got });
    }
    const roads = seen.filter((s) => s.road);
    let road = null;
    if (roads.length) {
      const counts = new Map();
      for (const s of roads) {
        counts.set(s.road.road_class, (counts.get(s.road.road_class) ?? 0) + 1);
      }
      const modal = [...counts.entries()].sort((a, b) => b[1] - a[1])[0][0];
      road = roads
        .filter((s) => s.road.road_class === modal)
        .sort((a, b) => a.road.distance_m - b.road.distance_m)[0].road;
    }
    const intersection = seen
      .filter((s) => s.intersection)
      .sort((a, b) => a.intersection.distance_m - b.intersection.distance_m)[0]?.intersection ?? null;
    const anchor = points[indices[Math.floor(indices.length / 2)]];
    return {
      lat: anchor.lat,
      lon: anchor.lon,
      snapshot: SNAPSHOT,
      resolved: Date.now(),
      road,
      intersection,
      samples: indices.length,
    };
  }

  /**
   * The cell records covering a point in some layer.
   *
   * Returns the index `entry` as well, because the entry's **stored** cell id is
   * the one to hand to a decoder -- not the masked lookup key. Masking clears the
   * res-7 filler bits, which produces an id that is not a real cell, and
   * `cell_center` of it lands somewhere else entirely: v9 buildings decoded
   * against that centre came out ~9,700 km from Nashville, which reads as "no
   * building here" rather than as an error.
   *
   * `{ error }` rather than null when a read or decode fails: a lookup that
   * quietly finds nothing is indistinguishable from a lookup that broke.
   */
  async function recordsAt(name, lat, lon) {
    const st = stateAt(lat, lon);
    if (!st) return { error: "outside the covered states" };
    const layer = await layerFor(st, name);
    if (!layer) return { error: `${st}.${name} unavailable` };
    const cell = norm(BigInt("0x" + wasm.cell_for_coord(lat, lon)));
    const entry = layer.entryFor(cell);
    if (!entry || !entry.block_length) return { bytes: null, entry: null, layer };
    try {
      return { bytes: await layer.cellRecords(cell), entry, layer };
    } catch (e) {
      return { error: e.message || String(e) };
    }
  }

  /**
   * The building at a point: the polygon containing it, else the nearest
   * centroid within 50 m -- the same rule the FFI's `building()` uses, so the
   * browser and the phone answer this question identically.
   */
  async function buildingAt(lat, lon) {
    const got = await recordsAt("buildings", lat, lon);
    if (got.error) throw new Error(`buildings: ${got.error}`);
    if (!got.bytes) return null;
    // The cell-taking decoder derives the centre itself, so there is no way to
    // hand it the wrong one -- which is exactly the bug this used to have.
    let decoded;
    try {
      decoded = wasm.decode_buildings_for_cell(got.bytes, got.entry.h3_cell.toString(16));
    } catch (e) {
      throw new Error(`buildings: ${e?.message ?? e}`);
    }
    let best = null;
    for (const b of decoded) {
      const coords = b.coords || b.coordinates || [];
      const inside = coords.length >= 3 && pointInPolygon(lat, lon, coords);
      const d = wasm.distance_m(lat, lon, b.centroid_lat, b.centroid_lon);
      if (inside) return { ...b, distance_m: d, inside: true };
      if (d <= 50 && (!best || d < best.distance_m)) best = { ...b, distance_m: d, inside: false };
    }
    return best;
  }

  /** Ray casting on `[lon, lat]` rings, which is the order the format stores. */
  function pointInPolygon(lat, lon, coords) {
    let hit = false;
    for (let i = 0, j = coords.length - 1; i < coords.length; j = i++) {
      const [xi, yi] = coords[i];
      const [xj, yj] = coords[j];
      if (yi > lat !== yj > lat && lon < ((xj - xi) * (lat - yi)) / (yj - yi) + xi) hit = !hit;
    }
    return hit;
  }

  /**
   * Addresses near a point, nearest first.
   *
   * Bounded by `radius_m`, not by the cell: a res-7 cell is ~1.2 km across, so
   * "the addresses in this cell" includes ones the better part of a mile away,
   * and listing those under a pin claims something false about where you clicked.
   */
  async function addressesAt(lat, lon, radius_m = 250, limit = 4) {
    const st = stateAt(lat, lon);
    if (!st) return [];
    const layer = await layerFor(st, "address");
    if (!layer) return [];
    const cell = norm(BigInt("0x" + wasm.cell_for_coord(lat, lon)));
    const entry = layer.entryFor(cell);
    if (!entry) return [];
    let recs;
    try {
      // The address layer is merged, so the block holds several cells and must be
      // sliced by the entry's *stored* id -- the masked lookup key silently
      // returns nothing. `address_cell` also needs the *whole block*, not the
      // per-cell slice `cellRecords` gives: v2 positions are offsets from the
      // block centre, and feeding it a slice reads a length field out of
      // coordinate bytes ("needed 4294967295 more bytes").
      const block = await layer.block(entry);
      recs = wasm.address_cell(block, entry.h3_cell.toString(16), layer.header.version);
    } catch (e) {
      throw new Error(`addresses: ${e?.message ?? e}`);
    }
    return (recs ?? [])
      .filter((r) => r.lat != null && r.lon != null)
      .map((r) => ({ ...r, distance_m: wasm.distance_m(lat, lon, r.lat, r.lon) }))
      .filter((r) => r.distance_m <= radius_m)
      .sort((a, b) => a.distance_m - b.distance_m)
      .slice(0, limit);
  }

  /** Businesses within `radius_m` of a point, nearest first. */
  async function businessesNear(lat, lon, radius_m = 150, limit = 5) {
    const got = await recordsAt("business", lat, lon);
    if (got.error) throw new Error(`businesses: ${got.error}`);
    if (!got.bytes) return [];
    let records;
    try {
      records = wasm.decode_business(got.bytes);
    } catch (e) {
      // wasm rejects with a bare string, so wrap it: "businesses: ..." is what a
      // reader needs, where the raw text alone says nothing about which layer.
      throw new Error(`businesses: ${e?.message ?? e}`);
    }
    return records
      .map((b) => ({ ...b, distance_m: wasm.distance_m(lat, lon, b.lat, b.lon) }))
      .filter((b) => b.distance_m <= radius_m)
      .sort((a, b) => a.distance_m - b.distance_m)
      .slice(0, limit);
  }

  /**
   * Jurisdiction for a point. Opt-in: `AdminReader` needs the whole 28 MB H3
   * grid, so nothing calls this until the user asks for admin fields.
   *
   * ponytail: whole-grid load. The grid is sorted at a fixed 16-byte stride, so
   * a range binary search (~21 small GETs) is the upgrade -- it needs a wasm
   * export that resolves one entry against the string tables without the grid.
   */
  let adminPromise = null;
  function admin() {
    if (!adminPromise) {
      adminPromise = (async () => {
        const src = P.httpSource(stateUrl("US", "admin"));
        const headerBytes = await src.readLive(0, 255);
        const h = wasm.parse_header(headerBytes);
        const grid = await src.read(Number(h.aux_offset), Number(h.aux_offset) + h.aux_length - 1);
        const dictRaw = await src.read(
          Number(h.dict_offset),
          Number(h.dict_offset) + h.dict_length - 1,
        );
        // Plain zstd here, not a dictionary for other blocks.
        const tables = wasm.decompress_block(dictRaw, new Uint8Array(0));
        return new wasm.AdminReader(grid, tables);
      })().catch((e) => {
        // A cached rejection would make one failed load permanent.
        adminPromise = null;
        throw e;
      });
    }
    return adminPromise;
  }

  return {
    survey, prefetch, at, forSegment, admin, failures, stateAt,
    layerFor, buildingAt, addressesAt, businessesNear,
  };
}
