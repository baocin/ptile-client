//! ptiles-wasm: thin wasm-bindgen wrapper over ptiles-core.
//!
//! Replaces the old root src/lib.rs once at API parity (contract:
//! pkg/ptiles_client.d.ts, 6 decode_* exports returning JsValue).
//! No async: JS fetches ranges + zstd-decompresses, passes decompressed
//! block bytes into these exports.
//!
//! `decompress_block` is Phase 3's optional extra export (plan line ~172)
//! so JS can eventually drop `@bokuweb/zstd-wasm`. It duplicates the
//! dict-then-plain fallback in `core::file::decompress_with_dict_fallback`
//! (that helper is private to core) rather than modifying core, per task
//! scope. Keep the two in sync if the fallback logic changes.

use ruzstd::decoding::{BlockDecodingStrategy, Dictionary, FrameDecoder};
use wasm_bindgen::prelude::*;

use ptiles_core::{decode_buildings as core_decode_buildings, decode_business as core_decode_business,
    decode_parks as core_decode_parks, decode_rail as core_decode_rail, decode_roads as core_decode_roads,
    decode_trails as core_decode_trails, decode_water as core_decode_water};
use ptiles_core::{
    decode_road_block as core_decode_road_block,
    nearest_intersection as core_nearest_intersection, nearest_road as core_nearest_road,
    route_roads_with as core_route_roads_with, score_candidates as core_score_candidates,
    Fix, RoadSegment, RoutePrefs,
    ScoringParams, DEFAULT_THRESHOLD_M,
};
use ptiles_motion::{
    classify, AccelStats, DebounceConfig, MotionClassifier, MotionConfig, MovementType,
    RoadContext, TimedFix, TrafficControl, Vote, VoteDebouncer,
};

use ptiles_core::address::merged_block_cell_slice;
use ptiles_core::admin::{decode_grid, decode_string_tables, AdminLookup};
use ptiles_core::cells_for_bounds as core_cells_for_bounds;
use ptiles_core::{
    match_business_name_block as core_match_business_name_block, name_to_key as core_name_to_key,
};
use ptiles_core::{
    cell_center as core_cell_center, cell_for_coord as core_cell_for_coord,
    neighbor_cells as core_neighbor_cells,
};
use ptiles_core::{
    index_binary_search as core_index_binary_search, merged_cell_slice as core_merged_cell_slice,
    parse_index_detected as core_parse_index_detected, decode_cameras as core_decode_cameras,
    decode_signals as core_decode_signals, index_layout as core_index_layout,
    parse_coarse_index as core_parse_coarse_index, parse_entry_run as core_parse_entry_run,
    Header,
};

// `business.rs`'s `osm_id: i64` (unlike every other layer's delta-coded u64,
// see business.rs doc) can exceed 2^53 on real data, which the default
// serde-wasm-bindgen serializer rejects (`"N can't be represented as a
// JavaScript number"` — the old seed's `serde_wasm_bindgen::to_value` would
// hit the same wall on such a record, this isn't a new failure mode). Route
// large ints through BigInt instead of panicking; every other field keeps
// its default (number/string/array) shape, so the parity contract (field
// names + JS-visible shapes) is unaffected except osm_id becoming `bigint`
// instead of `number` for out-of-range ids.
fn to_js<T: serde::Serialize>(value: &T) -> Result<JsValue, JsValue> {
    let serializer = serde_wasm_bindgen::Serializer::new().serialize_large_number_types_as_bigints(true);
    value
        .serialize(&serializer)
        .map_err(|e| JsValue::from_str(&e.to_string()))
}

/// Parse a lowercase (or uppercase) hex H3 cell string into a `u64`, the
/// single validation point shared by every `cell_hex`-taking export. Trims
/// surrounding whitespace and tolerates an optional `0x`/`0X` prefix so a
/// caller that hands us a formatted address (rather than a bare radix-16
/// string) gets a clear error only for genuinely non-hex input. Pure (no
/// `JsValue`) so it's unit-testable on the host target.
fn parse_cell_hex(cell_hex: &str) -> Result<u64, String> {
    let trimmed = cell_hex.trim();
    let digits = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
        .unwrap_or(trimmed);
    if digits.is_empty() {
        return Err(format!("invalid cell hex {cell_hex:?}: empty"));
    }
    u64::from_str_radix(digits, 16).map_err(|e| format!("invalid cell hex {cell_hex:?}: {e}"))
}

/// Parse+validate a `score_candidates` `fix_json` string into a core [`Fix`].
/// Pure (no `JsValue`) so the JSON-shape contract and the finite-coordinate
/// validation are unit-testable on the host target. Rejects malformed JSON,
/// non-finite `lat`/`lon`, a non-finite or negative `horizontal_accuracy_m`,
/// and a non-finite `speed_mps` — a NaN slipping into the scorer would
/// silently poison every emission score (`exp(-d^2/2sigma^2)`), so it's
/// caught at the boundary rather than propagated.
fn parse_fix_input(fix_json: &str) -> Result<Fix, String> {
    let fix_input: FixInput =
        serde_json::from_str(fix_json).map_err(|e| format!("invalid fix_json: {e}"))?;
    if !fix_input.lat.is_finite() || !fix_input.lon.is_finite() {
        return Err(format!(
            "invalid fix_json: lat/lon must be finite (got lat={}, lon={})",
            fix_input.lat, fix_input.lon
        ));
    }
    if !fix_input.horizontal_accuracy_m.is_finite() || fix_input.horizontal_accuracy_m < 0.0 {
        return Err(format!(
            "invalid fix_json: horizontal_accuracy_m must be finite and non-negative (got {})",
            fix_input.horizontal_accuracy_m
        ));
    }
    if fix_input.speed_mps.is_some_and(|s| !s.is_finite()) {
        return Err("invalid fix_json: speed_mps must be finite when present".to_string());
    }
    Ok(Fix {
        lat: fix_input.lat,
        lon: fix_input.lon,
        horizontal_accuracy_m: fix_input.horizontal_accuracy_m,
        speed_mps: fix_input.speed_mps,
    })
}

#[wasm_bindgen]
pub fn decode_buildings(data: &[u8], cell_center_lat: f64, cell_center_lon: f64) -> Result<JsValue, JsValue> {
    let buildings = core_decode_buildings(data, cell_center_lat, cell_center_lon)
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    to_js(&buildings)
}

#[wasm_bindgen]
pub fn decode_business(data: &[u8]) -> Result<JsValue, JsValue> {
    let business = core_decode_business(data).map_err(|e| JsValue::from_str(&e.to_string()))?;
    to_js(&business)
}

#[wasm_bindgen]
pub fn decode_parks(data: &[u8]) -> Result<JsValue, JsValue> {
    let parks = core_decode_parks(data).map_err(|e| JsValue::from_str(&e.to_string()))?;
    to_js(&parks)
}

/// Decode a `signals` cell's records (PTILESS v1). Input is the byte range for
/// one cell, i.e. the output of `merged_cell_slice` -- signals files carry a
/// 38-byte index and therefore merged blocks, so passing a whole decompressed
/// block here decodes its cell table as records.
#[wasm_bindgen]
pub fn decode_signals(data: &[u8]) -> Result<JsValue, JsValue> {
    let signals = core_decode_signals(data).map_err(|e| JsValue::from_str(&e.to_string()))?;
    to_js(&signals)
}

/// Decode a `camera` cell's records (PTILESC v1). Same merged-block caveat as
/// `decode_signals`.
#[wasm_bindgen]
pub fn decode_cameras(data: &[u8]) -> Result<JsValue, JsValue> {
    let cams = core_decode_cameras(data).map_err(|e| JsValue::from_str(&e.to_string()))?;
    to_js(&cams)
}

/// The record bytes for one cell inside a decompressed merged block.
///
/// Layers with a 38-byte index pack several cells per block behind a cell
/// table; a record decoder handed the whole block parses that table as
/// records and yields plausible garbage rather than an error. Returns `null`
/// if the block does not contain the cell.
#[wasm_bindgen]
pub fn merged_cell_slice(block: &[u8], cell_hex: &str) -> Result<Option<Vec<u8>>, JsValue> {
    let cell = parse_cell_hex(cell_hex).map_err(|e| JsValue::from_str(&e))?;
    Ok(core_merged_cell_slice(block, cell)
        .map_err(|e| JsValue::from_str(&e.to_string()))?
        .map(|b| b.to_vec()))
}

#[wasm_bindgen]
pub fn decode_rail(data: &[u8]) -> Result<JsValue, JsValue> {
    let rail = core_decode_rail(data).map_err(|e| JsValue::from_str(&e.to_string()))?;
    to_js(&rail)
}

/// Reverse geocode a point against already-decoded features.
///
/// `roads_js` / `trails_js` / `addresses_js` are the arrays this module's
/// `decode_roads`, `decode_trails` and `address_cell` return, for whatever
/// cells the caller fetched. Returns
/// `{nearest_way, on_way, address}` — see `ptiles_core::locate`.
#[wasm_bindgen]
pub fn locate_point(
    lat: f64,
    lon: f64,
    roads_js: JsValue,
    trails_js: JsValue,
    addresses_js: JsValue,
) -> Result<JsValue, JsValue> {
    let roads: Vec<ptiles_core::RoadSegment> = from_js_or_empty(roads_js, "roads")?;
    let trails: Vec<ptiles_core::TrailFeature> = from_js_or_empty(trails_js, "trails")?;
    let addresses: Vec<ptiles_core::address::AddressRecord> =
        from_js_or_empty(addresses_js, "addresses")?;
    to_js(&ptiles_core::locate(lat, lon, &roads, &trails, &addresses))
}

/// Forward geocode over already-decoded address records: "400 Broadway".
#[wasm_bindgen]
pub fn geocode_addresses(
    query: &str,
    addresses_js: JsValue,
    limit: Option<u32>,
) -> Result<JsValue, JsValue> {
    let addresses: Vec<ptiles_core::address::AddressRecord> =
        from_js_or_empty(addresses_js, "addresses")?;
    let hits = ptiles_core::match_addresses(query, &addresses, limit.unwrap_or(25) as usize);
    to_js(&hits)
}

/// The nearest address to a point, or null. Separate from `locate_point` for
/// callers that hold only the address layer.
#[wasm_bindgen]
pub fn nearest_address_to(
    lat: f64,
    lon: f64,
    addresses_js: JsValue,
    threshold_m: Option<f64>,
) -> Result<JsValue, JsValue> {
    let addresses: Vec<ptiles_core::address::AddressRecord> =
        from_js_or_empty(addresses_js, "addresses")?;
    let t = threshold_m.unwrap_or(ptiles_core::ADDRESS_THRESHOLD_M);
    match ptiles_core::nearest_address(lat, lon, &addresses, t) {
        Some(a) => to_js(&a),
        None => Ok(JsValue::NULL),
    }
}

/// Deserialize a JS array, treating null/undefined as empty rather than an
/// error: a caller with no trails loaded should still get a road answer.
fn from_js_or_empty<T: serde::de::DeserializeOwned>(
    v: JsValue,
    what: &str,
) -> Result<Vec<T>, JsValue> {
    if v.is_null() || v.is_undefined() {
        return Ok(Vec::new());
    }
    serde_wasm_bindgen::from_value(v).map_err(|e| JsValue::from_str(&format!("{what}: {e}")))
}

/// Single-value flavor of [`from_js_or_empty`] (no null-to-empty default —
/// callers check for null themselves where it's meaningful).
fn from_js<T: serde::de::DeserializeOwned>(v: JsValue, what: &str) -> Result<T, JsValue> {
    serde_wasm_bindgen::from_value(v).map_err(|e| JsValue::from_str(&format!("{what}: {e}")))
}

/// Whether a trail type is built infrastructure (cycleway, footway) rather
/// than a natural way. Exposed so a renderer styles the two apart without
/// re-listing the layer's type vocabulary in JavaScript.
/// Great-circle distance in metres.
///
/// The page had 31 sites doing this by hand in JavaScript -- each one its own
/// chance to use the wrong earth radius or drop the cos(lat) term. One
/// implementation, in core, is the point of this client.
#[wasm_bindgen]
pub fn distance_m(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    ptiles_core::haversine_distance_m(lat1, lon1, lat2, lon2)
}

/// Drop an H3 id's unused low digits, so two ids naming the same res-7 cell
/// compare equal. The mask is a property of the id layout, not of any caller.
#[wasm_bindgen]
pub fn normalize_cell(cell: u64) -> u64 {
    ptiles_core::normalize_cell(cell)
}

/// The filler-bit mask itself, for callers that must mask in their own code
/// (a `Map` keyed by cell id, say) rather than call across the boundary per id.
#[wasm_bindgen]
pub fn cell_filler_mask() -> u64 {
    !ptiles_core::CELL_FILLER_BITS
}

#[wasm_bindgen]
pub fn trail_is_developed(trail_type: &str) -> bool {
    ptiles_core::trail_is_developed(trail_type)
}

#[wasm_bindgen]
pub fn decode_trails(data: &[u8]) -> Result<JsValue, JsValue> {
    let trails = core_decode_trails(data).map_err(|e| JsValue::from_str(&e.to_string()))?;
    to_js(&trails)
}

#[wasm_bindgen]
pub fn decode_roads(data: &[u8]) -> Result<JsValue, JsValue> {
    let roads = core_decode_roads(data).map_err(|e| JsValue::from_str(&e.to_string()))?;
    to_js(&roads)
}

#[wasm_bindgen]
pub fn decode_water(data: &[u8]) -> Result<JsValue, JsValue> {
    let water = core_decode_water(data).map_err(|e| JsValue::from_str(&e.to_string()))?;
    to_js(&water)
}

/// Enriched nearest-road response shape (plan addendum item 1): the
/// polyline is already decoded server/JS-side-independent, so it's
/// included directly rather than making the caller re-fetch geometry.
#[derive(serde::Serialize)]
struct NearestRoadResponse {
    osm_id: u64,
    name: Option<String>,
    road_class: String,
    /// `[lat, lon]` snapped point.
    snapped: [f64; 2],
    distance_m: f64,
    /// `[[lat, lon], ...]` — the full road's decoded polyline (converted
    /// from the decoder's internal `[lon, lat]` coordinate order).
    geometry: Vec<[f64; 2]>,
}

/// Decode a roads block and return the single nearest road segment to
/// `(lat, lon)`, per plan addendum item 1: `{osm_id, name, road_class,
/// snapped, distance_m, geometry}`. `threshold_m` is optional; omit (pass
/// `None`/`undefined` from JS) to use the SPEC.md default of 50 m.
///
/// JS supplies `block_bytes` already decompressed (no-fetch contract is
/// unchanged — this does not do any I/O or H3 lookup itself). Returns
/// `null` if no road is within the threshold.
#[wasm_bindgen]
pub fn nearest_road(block_bytes: &[u8], lat: f64, lon: f64, threshold_m: Option<f64>) -> Result<JsValue, JsValue> {
    let roads = core_decode_roads(block_bytes).map_err(|e| JsValue::from_str(&e.to_string()))?;
    let threshold = threshold_m.unwrap_or(DEFAULT_THRESHOLD_M);
    match core_nearest_road(lat, lon, &roads, threshold) {
        Some(found) => {
            let road = &roads[found.road_index];
            let response = NearestRoadResponse {
                osm_id: road.osm_id,
                name: road.name.clone(),
                road_class: road.road_class.clone(),
                snapped: [found.snapped.0, found.snapped.1],
                distance_m: found.distance_m,
                geometry: road.coords.iter().map(|c| [c[1], c[0]]).collect(),
            };
            to_js(&response)
        }
        None => Ok(JsValue::NULL),
    }
}


/// Route on pre-decoded road segments (JS owns fetch + zstd + corridor).
/// `segments_js`: `[{coords:[[lon,lat],...], road_class, oneway?, speed_limit_kmh?}, ...]`
/// `zone_middle`: bool[] same length (true = arterial-only middle); empty/null = all end-cap.
/// Returns `{distance_m, duration_s, path:[[lat,lon],...]}` or null.
#[wasm_bindgen]
pub fn route_from_segments(
    segments_js: JsValue,
    zone_middle: JsValue,
    lat1: f64,
    lon1: f64,
    lat2: f64,
    lon2: f64,
    snap_m: Option<f64>,
    avoid_highways: Option<bool>,
    avoid_intersections: Option<bool>,
) -> Result<JsValue, JsValue> {
    #[derive(serde::Deserialize)]
    struct SegIn {
        coords: Vec<[f64; 2]>,
        road_class: String,
        #[serde(default)]
        oneway: Option<String>,
        #[serde(default)]
        speed_limit_kmh: Option<u8>,
    }
    let segs_in: Vec<SegIn> = serde_wasm_bindgen::from_value(segments_js)
        .map_err(|e| JsValue::from_str(&format!("segments: {e}")))?;
    let roads: Vec<RoadSegment> = segs_in
        .into_iter()
        .map(|s| RoadSegment {
            osm_id: 0,
            road_class: s.road_class,
            coords: s.coords,
            name: None,
            ref_tag: None,
            oneway: s.oneway,
            speed_limit_kmh: s.speed_limit_kmh,
            lanes: None,
            surface: None,
            bridge_tunnel: None,
        })
        .collect();
    let middle: Vec<bool> = if zone_middle.is_null() || zone_middle.is_undefined() {
        Vec::new()
    } else {
        serde_wasm_bindgen::from_value(zone_middle)
            .map_err(|e| JsValue::from_str(&format!("zone_middle: {e}")))?
    };
    let snap = snap_m.unwrap_or(100.0);
    let prefs = RoutePrefs {
        avoid_highways: avoid_highways.unwrap_or(false),
        avoid_intersections: avoid_intersections.unwrap_or(false),
    };
    match core_route_roads_with(&roads, &middle, lat1, lon1, lat2, lon2, snap, prefs) {
        Some(r) => to_js(&r),
        None => Ok(JsValue::NULL),
    }
}

/// Nearest-intersection response shape: `{lat, lon, distance_m,
/// intersection_type}`. `intersection_type`: 1 = traffic_signals, 2 = stop,
/// 3 = give_way, 4 = roundabout (0/other = untyped).
#[derive(serde::Serialize, PartialEq, Debug)]
struct NearestIntersectionResponse {
    lat: f64,
    lon: f64,
    distance_m: f64,
    intersection_type: u8,
}

/// Pure core of [`nearest_intersection`]: decode a roads block's v2
/// intersection table and return the nearest one to `(lat, lon)` within
/// `threshold_m`. No `JsValue`, so it's unit-testable on the host target.
/// Roads blocks always carry schema v2 (the intersection table only exists in
/// v2), so the version is fixed at 2 here.
fn nearest_intersection_in_block(
    block_bytes: &[u8],
    lat: f64,
    lon: f64,
    threshold_m: f64,
) -> Result<Option<NearestIntersectionResponse>, String> {
    let (_roads, intersections) =
        core_decode_road_block(block_bytes, 2).map_err(|e| e.to_string())?;
    Ok(core_nearest_intersection(lat, lon, &intersections, threshold_m).map(|ni| {
        let [ix_lon, ix_lat] = intersections[ni.index].coords();
        NearestIntersectionResponse {
            lat: ix_lat,
            lon: ix_lon,
            distance_m: ni.distance_m,
            intersection_type: ni.intersection_type,
        }
    }))
}

/// Decode a roads block and return the nearest labeled intersection to
/// `(lat, lon)` — the "am I at an intersection?" query. `threshold_m` is
/// optional (omit/`undefined` from JS for the SPEC.md default of 50 m).
/// Returns `null` when nothing is within the threshold. Reports a mapped
/// intersection point + its control type, not junction degree (the format
/// stores no topology). JS supplies `block_bytes` already decompressed
/// (no-I/O contract, same as `nearest_road`).
#[wasm_bindgen]
pub fn nearest_intersection(
    block_bytes: &[u8],
    lat: f64,
    lon: f64,
    threshold_m: Option<f64>,
) -> Result<JsValue, JsValue> {
    let threshold = threshold_m.unwrap_or(DEFAULT_THRESHOLD_M);
    match nearest_intersection_in_block(block_bytes, lat, lon, threshold)
        .map_err(|e| JsValue::from_str(&e))?
    {
        Some(response) => to_js(&response),
        None => Ok(JsValue::NULL),
    }
}

/// H3 res-7 cells covering a viewport bbox -- the wasm boundary for
/// `ptiles_core::cells_for_bounds` (see docs/INTEGRATION.md's "viewport ->
/// cells" step). Returns lowercase hex cell strings (a JS array of
/// `string`), matching how the demo/`h3-js` represents cells everywhere
/// (`h3.latLngToCell(...)` returns a lowercase hex string, and the demo's
/// `cellMap`/`renderPtilesForCells` consume cells in that same string form,
/// see steele.red/ptiles/index.html) -- not `u64`/BigInt, so callers can
/// pass results straight into existing `h3-js`-shaped code without a
/// conversion step.
///
/// Errors (as a JS exception, matching every other export's rejection
/// pattern) if any coordinate is non-finite, `min` is not `<=` `max`, or the
/// box would cover more than `ptiles_core::MAX_BOUNDS_CELLS` cells.
#[wasm_bindgen]
pub fn cells_for_bounds(min_lat: f64, min_lon: f64, max_lat: f64, max_lon: f64) -> Result<Vec<JsValue>, JsValue> {
    let cells = core_cells_for_bounds(min_lat, min_lon, max_lat, max_lon).map_err(|e| JsValue::from_str(&e.to_string()))?;
    Ok(cells.into_iter().map(|c| JsValue::from_str(&format!("{c:x}"))).collect())
}

/// Decode a roads block into its full segment list (geometry + name +
/// every other `RoadSegment` field), identical shape to `decode_roads`.
/// Exists as its own export (plan addendum item 1's "roads" query) so
/// callers reach for a query-shaped name rather than the raw decoder;
/// ring-1 neighbor-cell expansion is NOT done here -- JS owns block
/// fetching (which cell(s) to fetch bytes for), so ring handling stays in
/// JS per the plan's `query.rs` split (`neighbor_cells` in core, calling
/// convention in JS/CLI).
#[wasm_bindgen]
pub fn roads_in_block(block_bytes: &[u8]) -> Result<JsValue, JsValue> {
    let roads = core_decode_roads(block_bytes).map_err(|e| JsValue::from_str(&e.to_string()))?;
    to_js(&roads)
}

/// Minimal JSON shape accepted for `score_candidates`' `fix_json` param:
/// `{lat, lon, horizontal_accuracy_m, speed_mps?}` (CoreLocation-style
/// fields, see `ptiles_core::scoring::Fix`).
#[derive(serde::Deserialize)]
struct FixInput {
    lat: f64,
    lon: f64,
    horizontal_accuracy_m: f64,
    speed_mps: Option<f64>,
}

/// Rank road/building/business candidates for a GPS fix (plan addendum
/// item 2: emission-probability scoring lives in core, this is just the
/// wasm boundary). `buildings_block`/`business_block` are optional --
/// pass an empty slice (`new Uint8Array()`) from JS when a layer isn't
/// available for the current cell; buildings need `cell_center_lat`/
/// `cell_center_lon` to decode (v8 buildings are cell-relative-delta
/// encoded, see `buildings.rs`), which is also why callers must supply
/// them even when `buildings_block` is empty.
#[wasm_bindgen]
pub fn score_candidates(
    fix_json: &str,
    roads_block: &[u8],
    buildings_block: &[u8],
    business_block: &[u8],
    cell_center_lat: f64,
    cell_center_lon: f64,
) -> Result<JsValue, JsValue> {
    let fix = parse_fix_input(fix_json).map_err(|e| JsValue::from_str(&e))?;

    let roads = core_decode_roads(roads_block).map_err(|e| JsValue::from_str(&e.to_string()))?;
    let buildings = if buildings_block.is_empty() {
        Vec::new()
    } else {
        core_decode_buildings(buildings_block, cell_center_lat, cell_center_lon)
            .map_err(|e| JsValue::from_str(&e.to_string()))?
    };
    let businesses = if business_block.is_empty() {
        Vec::new()
    } else {
        core_decode_business(business_block).map_err(|e| JsValue::from_str(&e.to_string()))?
    };

    let candidates = core_score_candidates(&fix, &roads, &buildings, &businesses, &ScoringParams::default());
    to_js(&candidates)
}

/// Business name search, JS-owns-fetch flavor.
///
/// The `{STATE}.business_name_index.ptiles` sidecar (see
/// `ptiles_core::business_search`'s module doc for its first-letter-bucket
/// format) is a normal `.ptiles`-shaped file: header, dict, index, blocks.
/// wasm does no I/O, so it doesn't open this file itself -- the intended JS
/// flow mirrors what the demo already does for spatial layers
/// (docs/INTEGRATION.md's "single whole-file fetch, parse header/index once,
/// cache" notes) plus these two pure calls:
///
///   1. JS fetches (or already has, whole-file) the name-index file's bytes
///      and parses its header/index once, same as any other `.ptiles` file
///      -- the sidecar's index entries key on a 0-27 bucket value stored in
///      the normal `h3_cell` index field, not a real H3 cell.
///   2. `key_for_business_name_query(query)` -- call this wasm export to get
///      that 0-27 key without reimplementing the bucketing rule in JS.
///   3. JS looks up the index entry for that key, slices out its compressed
///      block, and decompresses it with `decompress_block` (dict-less, per
///      the builder -- pass an empty `dict`).
///   4. `match_business_name_block(block_bytes, query, limit)` -- call this
///      to decode the block's records and get back ranked `BusinessHit`s
///      (`{name, category_idx, lat, lon, cell: null, score}`), same scoring
///      (`2`=exact, `1`=prefix, `0`=substring) as the native
///      `search_business_indexed`/`search_business_brute_force` paths.
///
/// No block ever needs re-fetching for a different query against the same
/// state: once JS has cached the file's index it only pays for step 3's one
/// block per distinct first-letter key.
#[wasm_bindgen]
pub fn key_for_business_name_query(query: &str) -> u8 {
    core_name_to_key(query)
}

/// See [`key_for_business_name_query`]'s doc comment for the full JS-side
/// flow this is step 4 of. Pure decode-and-match over an already-fetched,
/// already-decompressed name-index block -- no I/O, no H3 lookup.
#[wasm_bindgen]
pub fn match_business_name_block(block_bytes: &[u8], query: &str, limit: usize) -> Result<JsValue, JsValue> {
    let hits = core_match_business_name_block(block_bytes, query, limit)
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    to_js(&hits)
}

/// Decompress a compressed `.ptiles` block, trying the layer's zstd
/// dictionary first and falling back to plain (dict-less) decompress on
/// failure. Mirrors `ptiles/compression.py`'s `decompress_block` /
/// `decompress_fallback` pair and `ptiles-core::file::PtilesFile::read_block`'s
/// Which buildings are in line of sight from a point on the ground.
///
/// `buildings`: `[{coords:[[lon,lat],...], height_m: number|null,
/// building_type: string}, ...]` -- the shape `decode_buildings` already
/// returns, so a caller can pass its own decoded records straight back in.
/// Filter to a sensible radius first: this is geometry over a few hundred
/// footprints, not over a whole cell's 18k.
///
/// Returns one entry per input, in the same order:
/// `{visible, height_m, estimated, distance_m}`. `estimated` marks a height
/// that came from the building type rather than the file, which matters
/// because most published buildings carry no height at all.
#[wasm_bindgen]
pub fn viewshed(
    buildings: JsValue,
    lat: f64,
    lon: f64,
    eye_m: f64,
    radius_m: f64,
) -> Result<JsValue, JsValue> {
    let input: Vec<ptiles_core::ViewBuilding> = serde_wasm_bindgen::from_value(buildings)
        .map_err(|e| JsValue::from_str(&format!("buildings: {e}")))?;
    to_js(&ptiles_core::viewshed(lat, lon, eye_m, radius_m, &input))
}

/// One building's visibility from a *set* of observer points.
#[derive(serde::Serialize)]
struct UnionVisibility {
    /// Visible from at least one origin.
    visible: bool,
    height_m: f64,
    estimated: bool,
    /// Distance to the nearest origin that can see it; when nothing can, the
    /// nearest origin tested. Never infinite for a building inside the radius.
    distance_m: f64,
    /// Index of the closest origin with a clear line, or -1.
    seen_from: i32,
    /// How many of the origins can see it -- a riverbank sampled at 24 points
    /// and visible from 20 of them is a different answer from one visible at a
    /// single gap between two towers.
    seen_count: u32,
}

fn viewshed_union(
    buildings: &[ptiles_core::ViewBuilding],
    origins: &[[f64; 2]],
    eye_m: f64,
    radius_m: f64,
) -> Vec<UnionVisibility> {
    let mut out: Vec<UnionVisibility> = buildings
        .iter()
        .map(|_| UnionVisibility {
            visible: false,
            height_m: 0.0,
            estimated: false,
            distance_m: f64::INFINITY,
            seen_from: -1,
            seen_count: 0,
        })
        .collect();

    for (oi, o) in origins.iter().enumerate() {
        let vis = ptiles_core::viewshed(o[0], o[1], eye_m, radius_m, buildings);
        for (i, v) in vis.iter().enumerate() {
            let slot = &mut out[i];
            // Height and its estimated flag are properties of the building, not
            // of the observer, so the last write is the same as the first.
            slot.height_m = v.height_m;
            slot.estimated = v.estimated;
            if v.visible {
                slot.seen_count += 1;
                if !slot.visible || v.distance_m < slot.distance_m {
                    slot.seen_from = oi as i32;
                    slot.distance_m = v.distance_m;
                }
                slot.visible = true;
            } else if !slot.visible && v.distance_m < slot.distance_m {
                // Only tracked until something can see it; once one origin can,
                // distance means "how far from the place you can see it".
                slot.distance_m = v.distance_m;
            }
        }
    }
    out
}

/// The reverse of [`viewshed`]: which of these buildings can see *any* of these
/// points. Line of sight is reciprocal, so running the ordinary viewshed from
/// each target point and taking the union answers "find me somewhere with a
/// view of the river" without any new geometry.
///
/// `origins` is `[[lat, lon], ...]` -- one point for a shop, a sampled run
/// along the bank for a river. `buildings` is deserialized once and reused for
/// every origin, which is the whole reason this is not a JS loop over
/// [`viewshed`]: a few hundred footprints crossing the wasm boundary two dozen
/// times costs more than the geometry does.
///
/// Returns one entry per building, in input order.
#[wasm_bindgen]
pub fn viewshed_multi(
    buildings: JsValue,
    origins: JsValue,
    eye_m: f64,
    radius_m: f64,
) -> Result<JsValue, JsValue> {
    let input: Vec<ptiles_core::ViewBuilding> = serde_wasm_bindgen::from_value(buildings)
        .map_err(|e| JsValue::from_str(&format!("buildings: {e}")))?;
    let points: Vec<[f64; 2]> = serde_wasm_bindgen::from_value(origins)
        .map_err(|e| JsValue::from_str(&format!("origins: {e}")))?;
    to_js(&viewshed_union(&input, &points, eye_m, radius_m))
}

/// The height this crate would assume for a building type with no published
/// height. Exposed so a UI can explain a guess rather than just draw it.
#[wasm_bindgen]
pub fn estimated_height_for(building_type: &str) -> f64 {
    ptiles_core::estimate_height(building_type)
}

/// The height to draw a building at: the published one, or this crate's guess
/// when none was published.
///
/// Returns a bare `f64`, not a struct. serde-wasm-bindgen hands objects to the
/// browser as a `Map` in some engines, where `r.height_m` reads `undefined` --
/// which would silently extrude every guessed building to `NaN`. The caller
/// already knows whether it passed a height, so the flag adds nothing here;
/// `height_or_estimate` in core still returns it for Rust callers.
#[wasm_bindgen]
pub fn resolved_height(height_m: Option<f64>, building_type: &str) -> f64 {
    ptiles_core::height_or_estimate(height_m, building_type).0
}

/// internal fallback (see module doc above for why this isn't a direct call
/// into core). Pass an empty `dict` slice for dict-less layers (parks/address).
#[wasm_bindgen]
pub fn decompress_block(compressed: &[u8], dict: &[u8]) -> Result<Vec<u8>, JsValue> {
    if !dict.is_empty() {
        if let Ok(parsed_dict) = Dictionary::decode_dict(dict) {
            let mut decoder = FrameDecoder::new();
            if decoder.add_dict(parsed_dict).is_ok() {
                if let Some(out) = try_decode_all(&mut decoder, compressed) {
                    return Ok(out);
                }
            }
        }
        // fall through to dict-less attempt on any failure above, matching
        // the Python reference's broad except/return-None + separate retry.
    }

    let mut decoder = FrameDecoder::new();
    try_decode_all(&mut decoder, compressed)
        .ok_or_else(|| JsValue::from_str("zstd decompress failed (dict and plain both failed)"))
}

/// Parse a `.ptiles` file's 256-byte header (demo/browser boundary for
/// `ptiles_core::Header::parse`). Lets JS learn `dict_offset`/`dict_length`/
/// `index_offset`/`index_length`/`blocks_offset` from just the first 256
/// bytes of a Range request, without JS re-implementing the fixed-offset
/// layout from SPEC.md itself (`ptiles_core::header` is the single source of
/// truth for that layout parity-checked against `ptiles/codec.py`).
#[wasm_bindgen]
pub fn parse_header(data: &[u8]) -> Result<JsValue, JsValue> {
    let header = Header::parse(data).map_err(|e| JsValue::from_str(&e.to_string()))?;
    to_js(&header)
}

/// Parse a `.ptiles` file's spatial index section (the `index_offset`..
/// `index_offset+index_length` byte range from the header) into its full
/// entry list. Demo/browser boundary for `ptiles_core::parse_index_detected`
/// so JS never has to hand-roll either entry layout.
///
/// Entry width is detected, not assumed. This used to call `parse_index`,
/// which forces the 19-byte v1 layout: on the 38-byte layers (parks, rail,
/// places, signals, camera) that reads `block_offset` and `block_length` out
/// of the zeroed bbox field, so every cell came back with a zero-length block
/// and the caller saw "no data here" rather than an error.
#[wasm_bindgen]
pub fn parse_index_entries(data: &[u8]) -> Result<JsValue, JsValue> {
    let parsed =
        core_parse_index_detected(data, None).map_err(|e| JsValue::from_str(&e.to_string()))?;
    to_js(&parsed.entries)
}

/// What the reader concludes about a file's index layout, from its header and
/// index bytes: entry width, why that width was chosen, offset base, and the
/// stride the header declared.
///
/// This existed nowhere on the JS side of the boundary. `parse_index_entries`
/// takes index bytes alone, so it cannot see `blocks_offset` and cannot tell
/// whether the offsets it returns are absolute, relative to the block region,
/// or absolute-but-overshooting. Callers had to decide that themselves, and
/// `demo/index.html`'s `pickOffsetBase` is what "themselves" meant -- a second
/// implementation of the rule, in the language that got the index stride wrong.
///
/// Prefer [`index_entries_absolute`] when all you want is offsets you can
/// fetch. Use this when you need to *report* the layout, e.g. to warn that a
/// file's header contradicts its own index.
#[wasm_bindgen]
pub fn parse_index_layout(header_bytes: &[u8], index_bytes: &[u8]) -> Result<JsValue, JsValue> {
    let (header, parsed) = parse_header_and_index(header_bytes, index_bytes)?;
    let layout = core_index_layout(&header, &parsed);
    to_js(&LayoutReport {
        entry_size: layout.entry_size as u32,
        entry_size_source: layout.entry_size_source,
        offset_base: layout.offset_base,
        declared_stride: layout.declared_stride.map(|s| s as u32),
        header_is_inconsistent: layout.header_is_inconsistent(),
        entry_count: parsed.entries.len() as u32,
    })
}

/// Every index entry with `block_offset` already resolved to an absolute file
/// offset -- the byte range to Range-request, with no further arithmetic.
///
/// This is the export a client should reach for. Between choosing the entry
/// width, choosing the offset base and applying it, there are three chances to
/// be wrong, and each one fails the same silent way: a plausible-looking offset
/// that reads the wrong bytes, or a zero-length block that renders as "no data
/// here" rather than as an error. All three happen in `ptiles-core` here.
///
/// Entries whose offset arithmetic would wrap (only reachable with a corrupt
/// index) are dropped rather than returned with a bogus value.
#[wasm_bindgen]
pub fn index_entries_absolute(
    header_bytes: &[u8],
    index_bytes: &[u8],
) -> Result<JsValue, JsValue> {
    let (header, parsed) = parse_header_and_index(header_bytes, index_bytes)?;
    let layout = core_index_layout(&header, &parsed);

    let out: Vec<AbsoluteEntry> = parsed
        .entries
        .iter()
        .filter_map(|e| {
            let offset = layout.absolute_block_offset(e.block_offset, header.blocks_offset)?;
            Some(AbsoluteEntry {
                h3_cell: e.h3_cell,
                block_offset: offset,
                block_length: e.block_length,
                feature_count: e.feature_count,
            })
        })
        .collect();
    to_js(&out)
}

/// Parse the PTCI sampled index from a file's `aux` region.
///
/// Returns `null` when `aux` is not a coarse index -- empty, too short, or
/// holding something else. That is the normal case for every layer built
/// before PTCI existed, and a caller should fall back to reading the full
/// index. It *throws* when the region announces itself as PTCI and then does
/// not hold up (unknown version, impossible sample count), because that means
/// whatever wrote the file has a bug and is worth surfacing rather than
/// silently degrading.
///
/// The JS original (`parseCoarseIndex` in demo/index.html) returned null for
/// both, and ignored the version byte entirely.
#[wasm_bindgen]
pub fn parse_coarse_index(aux: &[u8]) -> Result<JsValue, JsValue> {
    match core_parse_coarse_index(aux).map_err(|e| JsValue::from_str(&e.to_string()))? {
        Some(c) => to_js(&c),
        None => Ok(JsValue::NULL),
    }
}

/// The run of index entries that may contain `cell_hex`, as a byte range to
/// Range-request.
///
/// This is the point of the coarse index: `US.signals` carries a 4014 KiB
/// index, and locating one cell in it otherwise means fetching all of it,
/// because entries are only findable by position. With the samples, a lookup
/// is header+aux in one request and then this range -- 256 entries, under
/// 10 KiB.
///
/// Returns `null` if the cell sorts below the first sample, i.e. the file does
/// not contain it.
#[wasm_bindgen]
pub fn coarse_bracket(
    aux: &[u8],
    cell_hex: &str,
    index_offset: u64,
    entry_size: usize,
) -> Result<JsValue, JsValue> {
    let cell = parse_cell_hex(cell_hex).map_err(|e| JsValue::from_str(&e))?;
    let Some(coarse) =
        core_parse_coarse_index(aux).map_err(|e| JsValue::from_str(&e.to_string()))?
    else {
        return Ok(JsValue::NULL);
    };
    let Some(b) = coarse.bracket(cell) else {
        return Ok(JsValue::NULL);
    };
    let (from, to) = b.byte_range(index_offset, entry_size);
    to_js(&BracketRange {
        start: b.start,
        end: b.end,
        entries: b.len(),
        byte_from: from,
        byte_to: to,
    })
}

#[derive(serde::Serialize)]
struct BracketRange {
    /// First index position in the run.
    start: u32,
    /// Last index position in the run, inclusive.
    end: u32,
    entries: u32,
    /// Inclusive byte range, the form an HTTP `Range` header wants.
    byte_from: u64,
    byte_to: u64,
}

/// Decode a bare run of index entries -- no count prefix, just entries -- at a
/// known width, with `block_offset` left exactly as stored.
///
/// This is the shape a PTCI partial read returns: `coarse_bracket` names a byte
/// range that lands mid-index, so there is no count in front of it. Files
/// carrying a coarse index are written by the current builder, which verifies
/// its own offsets, so the stored values are already absolute and need no base
/// applied -- but that is the caller's knowledge, not something derivable from
/// a run, which is why this returns them unmodified.
///
/// Trailing bytes that do not complete an entry are ignored.
#[wasm_bindgen]
pub fn parse_entry_run(entries: &[u8], entry_size: usize) -> Result<JsValue, JsValue> {
    let parsed = core_parse_entry_run(entries, entry_size)
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    let out: Vec<AbsoluteEntry> = parsed
        .iter()
        .map(|e| AbsoluteEntry {
            h3_cell: e.h3_cell,
            block_offset: e.block_offset,
            block_length: e.block_length,
            feature_count: e.feature_count,
        })
        .collect();
    to_js(&out)
}

/// Shared front half of the two layout exports: both need the header parsed
/// and the index detected before they can say anything about layout.
fn parse_header_and_index(
    header_bytes: &[u8],
    index_bytes: &[u8],
) -> Result<(Header, ptiles_core::ParsedIndex), JsValue> {
    let header = Header::parse(header_bytes).map_err(|e| JsValue::from_str(&e.to_string()))?;
    // Pass the header's `index_length` so detection can use the declared
    // stride, which is what distinguishes a probed width from a declared one --
    // and what makes the 42-byte files identifiably broken rather than merely
    // unusual.
    let parsed = core_parse_index_detected(index_bytes, Some(header.index_length as usize))
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    Ok((header, parsed))
}

/// Narrowed to `u32` on purpose. `to_js` serializes 64-bit integers as BigInt
/// (it has to -- business `osm_id` exceeds 2^53), and `usize` is 64-bit, so
/// leaving these as `usize` would hand JS `19n` for an entry width and `42n`
/// for a stride. Offsets and cell ids stay 64-bit because they genuinely need
/// to; a stride does not.
#[derive(serde::Serialize)]
struct LayoutReport {
    entry_size: u32,
    entry_size_source: ptiles_core::EntrySizeSource,
    offset_base: ptiles_core::BlockOffsetBase,
    declared_stride: Option<u32>,
    header_is_inconsistent: bool,
    entry_count: u32,
}

#[derive(serde::Serialize)]
struct AbsoluteEntry {
    h3_cell: u64,
    block_offset: u64,
    block_length: u32,
    feature_count: u16,
}

/// Find the block offset/length covering `cell_hex` (lowercase hex H3 res-7
/// cell, the string form `cells_for_bounds`/`cell_for_coord` return). Returns
/// `null` if the cell has no block in this file (sparse coverage).
///
/// Takes the raw index bytes and re-parses them. The search itself is
/// O(log n), but the parse is O(n) and happens on **every call** -- an earlier
/// doc comment here claimed the call was O(log n) with no network cost, which
/// was only ever true of the search half. Callers doing more than an occasional
/// lookup should use `parse_index_entries` once and search the result, or hold
/// a `PtilesFile`, which parses on open.
///
/// Entry width is detected rather than assumed; see `parse_index_entries`.
#[wasm_bindgen]
pub fn find_block_for_cell(index_bytes: &[u8], cell_hex: &str) -> Result<JsValue, JsValue> {
    let cell = parse_cell_hex(cell_hex).map_err(|e| JsValue::from_str(&e))?;
    let parsed = core_parse_index_detected(index_bytes, None)
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    match core_index_binary_search(&parsed.entries, cell) {
        Some(entry) => to_js(entry),
        None => Ok(JsValue::NULL),
    }
}

/// H3 res-7 cell (lowercase hex string) containing `(lat, lon)`. Demo/browser
/// boundary for `ptiles_core::cell_for_coord` -- replaces `h3-js`'s
/// `latLngToCell` for every caller in this workspace.
#[wasm_bindgen]
pub fn cell_for_coord(lat: f64, lon: f64) -> String {
    format!("{:x}", core_cell_for_coord(lat, lon))
}

/// `[lat, lon]` center of an H3 res-7 cell (hex string). Demo/browser
/// boundary for `ptiles_core::cell_center` -- replaces `h3-js`'s
/// `cellToLatLng`.
#[wasm_bindgen]
pub fn cell_center(cell_hex: &str) -> Result<Vec<f64>, JsValue> {
    let cell = parse_cell_hex(cell_hex).map_err(|e| JsValue::from_str(&e))?;
    let (lat, lon) = core_cell_center(cell);
    Ok(vec![lat, lon])
}

/// Ring-1 (6 cells) H3 neighbors of `cell_hex`, as lowercase hex strings.
/// Demo/browser boundary for `ptiles_core::neighbor_cells` -- replaces
/// `h3-js`'s `gridRing(cell, 1)` (used by the deployed demo's
/// `BusinessReader.query` for nearby-business radius search).
#[wasm_bindgen]
pub fn neighbor_cells(cell_hex: &str) -> Result<Vec<JsValue>, JsValue> {
    let cell = parse_cell_hex(cell_hex).map_err(|e| JsValue::from_str(&e))?;
    Ok(core_neighbor_cells(cell)
        .into_iter()
        .map(|c| JsValue::from_str(&format!("{c:x}")))
        .collect())
}

/// Admin point → jurisdiction lookup, browser flavor. Admin is a lookup-grid
/// layer (`US.admin.ptiles`): the H3 grid lives uncompressed in the file's
/// `aux` section, and the 5 string tables are a zstd blob in the `dict`
/// section. JS fetches those two byte ranges once (decompressing the dict via
/// [`decompress_block`] with an empty dict), constructs one `AdminReader`, and
/// reuses it for many `admin_at` calls — the grid is decoded once, not per
/// query. Kept separate from the per-block decode exports because admin has no
/// per-cell blocks.
/// Pure core of [`AdminReader::new`]: decode the grid + string tables into an
/// [`AdminLookup`]. No `JsValue`, so it's host-testable.
fn admin_lookup_from_bytes(
    grid_bytes: &[u8],
    string_tables_bytes: &[u8],
) -> Result<AdminLookup, String> {
    let grid = decode_grid(grid_bytes).map_err(|e| e.to_string())?;
    let tables = decode_string_tables(string_tables_bytes).map_err(|e| e.to_string())?;
    Ok(AdminLookup { grid, tables })
}

#[wasm_bindgen]
pub struct AdminReader {
    lookup: AdminLookup,
}

#[wasm_bindgen]
impl AdminReader {
    /// `grid_bytes` = the raw (uncompressed) `aux` section; `string_tables_bytes`
    /// = the *decompressed* `dict` section. Throws on malformed input.
    #[wasm_bindgen(constructor)]
    pub fn new(grid_bytes: &[u8], string_tables_bytes: &[u8]) -> Result<AdminReader, JsValue> {
        let lookup = admin_lookup_from_bytes(grid_bytes, string_tables_bytes)
            .map_err(|e| JsValue::from_str(&e))?;
        Ok(AdminReader { lookup })
    }

    /// Jurisdiction covering `(lat, lon)` as `{country, state, county, zip,
    /// timezone, boundary_flags}`, or `null` if the grid has no entry.
    pub fn admin_at(&self, lat: f64, lon: f64) -> Result<JsValue, JsValue> {
        match self.lookup.lookup_coord(lat, lon) {
            Some(info) => to_js(&info),
            None => Ok(JsValue::NULL),
        }
    }
}

/// Decode the addresses for one H3 cell from an already-decompressed merged
/// block (address layer). JS fetches the block bytes (via the v2 index) and
/// decompresses them (`decompress_block`, empty dict), then calls this per
/// cell. Returns a JS array of `{osm_id, housenumber, street, lat, lon}`
/// (empty if the cell isn't in the block). `cell_hex` is a lowercase hex H3
/// cell string.
///
/// `version` is the file header's version. v2 and later put an `i16` position
/// offset on every record; the block does not announce it, so passing the
/// wrong number here reads the coordinate bytes as a string length. Callers
/// already have `parse_header(...)`.
#[wasm_bindgen]
pub fn address_cell(block_bytes: &[u8], cell_hex: &str, version: u8) -> Result<JsValue, JsValue> {
    let cell = parse_cell_hex(cell_hex).map_err(|e| JsValue::from_str(&e))?;
    let records = merged_block_cell_slice(block_bytes, cell, version >= 2)
        .map_err(|e| JsValue::from_str(&e.to_string()))?
        .unwrap_or_default();
    to_js(&records)
}

/// Name for an `intersection_type` byte, from the format's own vocabulary:
/// `traffic_signals` | `stop` | `give_way` | `roundabout` | `junction`.
///
/// `nearest_intersection` returns the raw integer because that is what the block
/// stores; this is how JS names it without keeping a second copy of the mapping
/// that can drift from the Rust one.
#[wasm_bindgen]
pub fn intersection_type_name(intersection_type: u8) -> String {
    ptiles_core::intersection_type_name(intersection_type).to_string()
}

/// Whether an `intersection_type` is a node traffic waits at (signals, stop,
/// give-way) rather than flows through. This is the distinction
/// `MovementTracker` uses to stretch its "still driving" window.
#[wasm_bindgen]
pub fn intersection_holds_traffic(intersection_type: u8) -> bool {
    matches!(intersection_type, 1 | 2 | 3)
}

/// Accelerometer window summary from three same-length `Float32Array`s (raw
/// m/s^2 per axis, no gravity removal needed — magnitude is used). Returns
/// `{variance, mean_magnitude, dominant_frequency, step_count,
/// window_duration_s}`, the shape [`MovementTracker::push`] takes.
#[wasm_bindgen]
pub fn accel_stats(x: &[f32], y: &[f32], z: &[f32], sample_rate_hz: u32) -> Result<JsValue, JsValue> {
    to_js(&AccelStats::calculate(x, y, z, sample_rate_hz))
}

/// Stateful motion classifier: per-fix vote (speed + road tiles + accel) fed
/// through the CHRE-style debouncer, so `movement` only changes when the
/// evidence actually persists.
///
/// The road half is what disambiguates the awkward cases: stopped in a traffic
/// lane vs standing on the sidewalk. Pass the output of [`nearest_road`]
/// straight through as `road` — its `road_class`/`distance_m` are the two
/// fields read, extras are ignored.
#[wasm_bindgen]
pub struct MovementTracker {
    debouncer: VoteDebouncer,
    /// Speed smoother, used only to fill in a missing platform speed.
    speed: MotionClassifier,
    last_vote: Vote,
}

#[wasm_bindgen]
impl MovementTracker {
    /// `config` is optional (`null`/`undefined` = CHRE defaults): any subset of
    /// `{majority_window, rapid_latency_ms, default_latency_ms,
    /// vehicle_sticky_ms, min_continuous}`.
    #[wasm_bindgen(constructor)]
    pub fn new(config: JsValue) -> Result<MovementTracker, JsValue> {
        let cfg: DebounceConfig = if config.is_null() || config.is_undefined() {
            DebounceConfig::default()
        } else {
            from_js(config, "debounce config")?
        };
        Ok(MovementTracker {
            debouncer: VoteDebouncer::new(cfg),
            speed: MotionClassifier::new(MotionConfig::default()),
            last_vote: Vote { movement: MovementType::Unknown, confidence: 0.0 },
        })
    }

    /// Ingest one fix. `t_ms` is a monotonic timestamp; `speed_mps` and
    /// `accuracy_m` are optional (pass `undefined` when the platform omits
    /// them — speed is then derived from consecutive positions); `accel` is an
    /// [`accel_stats`] result or `null`; `road` is a [`nearest_road`] result or
    /// `null`; `intersection` is a [`nearest_intersection`] result or `null` —
    /// at a signal/stop/give-way the "still driving" grace period stretches
    /// from 150 s to 5 min, so a long light stops reading as an arrival.
    ///
    /// Returns `{movement, vote: {movement, confidence}, smoothed_speed_mps,
    /// at_traffic_control}` where `movement` is the debounced state and `vote`
    /// is this fix alone.
    pub fn push(
        &mut self,
        t_ms: f64,
        lat: f64,
        lon: f64,
        speed_mps: Option<f64>,
        accuracy_m: Option<f64>,
        accel: JsValue,
        road: JsValue,
        intersection: JsValue,
    ) -> Result<JsValue, JsValue> {
        // `null` accel means "no accelerometer window for this fix", which is a
        // different fact from a window that measured nothing -- so it stays
        // `None` rather than being flattened into `AccelStats::EMPTY` here.
        // Partial objects are fine too: the two fields the Rookery exporter
        // omits (`mean_magnitude`, `window_duration_s`) deserialize to `None`,
        // not 0. See ANDROID_INTEGRATION.md.
        let accel: Option<AccelStats> = if accel.is_null() || accel.is_undefined() {
            None
        } else {
            Some(from_js(accel, "accel stats")?)
        };
        let road: Option<RoadContext> = if road.is_null() || road.is_undefined() {
            None
        } else {
            Some(from_js(road, "road context")?)
        };
        let control: Option<TrafficControl> = if intersection.is_null() || intersection.is_undefined()
        {
            None
        } else {
            Some(from_js(intersection, "intersection")?)
        };

        let t = t_ms.max(0.0) as u64;
        self.speed.push(TimedFix::new(
            Fix {
                lat,
                lon,
                horizontal_accuracy_m: accuracy_m.unwrap_or(0.0),
                speed_mps,
            },
            t,
        ));
        // Platform speed wins; the position-derived smoothed speed is the
        // fallback when the fix carries none (browser geolocation often does).
        let effective_speed = speed_mps.or_else(|| self.speed.smoothed_speed_mps());

        self.last_vote = classify(effective_speed, accuracy_m, road.as_ref(), accel.as_ref());
        let movement = self.debouncer.tick_at(&self.last_vote, t, control.as_ref());
        to_js(&MovementUpdate {
            movement: movement.as_str(),
            vote: self.last_vote,
            smoothed_speed_mps: self.speed.smoothed_speed_mps(),
            at_traffic_control: control
                .is_some_and(|c| c.holds_traffic(self.debouncer.config().signal_radius_m)),
        })
    }

    /// Current debounced movement type as a lowercase string.
    #[wasm_bindgen(getter)]
    pub fn movement(&self) -> String {
        self.debouncer.current().as_str().to_string()
    }

    /// Smoothed position-derived speed (m/s), or `undefined` before enough fixes.
    #[wasm_bindgen(getter, js_name = smoothedSpeedMps)]
    pub fn smoothed_speed_mps(&self) -> Option<f64> {
        self.speed.smoothed_speed_mps()
    }
}

#[derive(serde::Serialize)]
struct MovementUpdate {
    movement: &'static str,
    vote: Vote,
    smoothed_speed_mps: Option<f64>,
    /// Whether the fix counted as waiting at a mapped traffic control (and so
    /// got the longer sticky window) — the one bit of the decision JS can't
    /// re-derive from the inputs it passed.
    at_traffic_control: bool,
}

fn try_decode_all(decoder: &mut FrameDecoder, compressed: &[u8]) -> Option<Vec<u8>> {
    let mut input: &[u8] = compressed;
    decoder.reset(&mut input).ok()?;
    decoder
        .decode_blocks(&mut input, BlockDecodingStrategy::All)
        .ok()?;
    decoder.collect()
}

// Host-target unit tests for the pure (non-`JsValue`) cores extracted above.
// Gated off wasm32 so `cargo test -p ptiles-wasm` runs them natively; the
// `#[wasm_bindgen]` exports themselves need a JS runtime and are covered by
// wasm/test/golden.mjs.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn parse_cell_hex_accepts_plain_and_prefixed() {
        // Same value three ways: bare, 0x-prefixed, whitespace-padded.
        let expected = 0x8a2a1072b59ffff_u64;
        assert_eq!(parse_cell_hex("8a2a1072b59ffff").unwrap(), expected);
        assert_eq!(parse_cell_hex("0x8a2a1072b59ffff").unwrap(), expected);
        assert_eq!(parse_cell_hex("0X8A2A1072B59FFFF").unwrap(), expected);
        assert_eq!(parse_cell_hex("  8a2a1072b59ffff\n").unwrap(), expected);
    }

    #[test]
    fn parse_cell_hex_rejects_bad_input() {
        assert!(parse_cell_hex("").is_err());
        assert!(parse_cell_hex("   ").is_err());
        assert!(parse_cell_hex("0x").is_err());
        assert!(parse_cell_hex("not-hex").is_err());
        // Overflows u64 -> parse error, not a silent truncation.
        assert!(parse_cell_hex("ffffffffffffffffff").is_err());
    }

    #[test]
    fn parse_fix_input_parses_full_and_minimal_shapes() {
        let full = parse_fix_input(
            r#"{"lat": 36.16, "lon": -86.79, "horizontal_accuracy_m": 10.0, "speed_mps": 8.0}"#,
        )
        .unwrap();
        assert_eq!(full.lat, 36.16);
        assert_eq!(full.lon, -86.79);
        assert_eq!(full.horizontal_accuracy_m, 10.0);
        assert_eq!(full.speed_mps, Some(8.0));

        // speed_mps is optional (omitted => None).
        let minimal =
            parse_fix_input(r#"{"lat": 1.0, "lon": 2.0, "horizontal_accuracy_m": 5.0}"#).unwrap();
        assert_eq!(minimal.speed_mps, None);
    }

    #[test]
    fn parse_fix_input_rejects_malformed_json() {
        assert!(parse_fix_input("").is_err());
        assert!(parse_fix_input("not json").is_err());
        // Missing required field.
        assert!(parse_fix_input(r#"{"lat": 1.0, "lon": 2.0}"#).is_err());
    }

    #[test]
    fn parse_fix_input_rejects_nonfinite_and_negative() {
        // NaN / Infinity aren't valid JSON numbers, so they'd arrive as the
        // strings below or via a computed value; assert the numeric guard by
        // constructing through serde_json's lenient paths is not possible, so
        // verify the guard directly via the finite checks on parsed values.
        assert!(parse_fix_input(
            r#"{"lat": 1.0, "lon": 2.0, "horizontal_accuracy_m": -1.0}"#
        )
        .is_err());
        // Well-formed, in-range value passes.
        assert!(parse_fix_input(
            r#"{"lat": 0.0, "lon": 0.0, "horizontal_accuracy_m": 0.0}"#
        )
        .is_ok());
    }

    #[test]
    fn nearest_intersection_in_block_finds_golden_intersection() {
        // Real decompressed roads block from the golden fixtures; its v2
        // intersection table's first entry is at (-86.79367, 36.16076) type 1.
        let block = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../test-fixtures/golden/roads.block.bin"
        ))
        .unwrap();
        let r = nearest_intersection_in_block(&block, 36.16076, -86.79367, DEFAULT_THRESHOLD_M)
            .unwrap()
            .expect("an intersection at the query point");
        assert!((r.lat - 36.16076).abs() < 1e-5);
        assert!((r.lon - (-86.79367)).abs() < 1e-5);
        assert_eq!(r.intersection_type, 1);
        assert!(r.distance_m < 1.0);
    }

    #[test]
    fn nearest_intersection_in_block_none_when_too_far() {
        let block = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../test-fixtures/golden/roads.block.bin"
        ))
        .unwrap();
        // Antarctica: no intersection within 50 m -> None (Ok, not Err).
        let r =
            nearest_intersection_in_block(&block, -80.0, 0.0, DEFAULT_THRESHOLD_M).unwrap();
        assert!(r.is_none());
    }

    #[test]
    fn nearest_intersection_in_block_rejects_garbage() {
        // Non-decodable road bytes surface as Err, never a panic.
        assert!(nearest_intersection_in_block(&[0xff, 0xff, 0xff], 0.0, 0.0, 50.0).is_err()
            || nearest_intersection_in_block(&[0xff, 0xff, 0xff], 0.0, 0.0, 50.0)
                .unwrap()
                .is_none());
    }

    #[test]
    fn admin_reader_constructs_and_looks_up() {
        // Minimal synthetic grid + string tables (same byte layout as the
        // reference encoder). Cell 0 -> US / Tennessee.
        let mut tables = Vec::new();
        let st = |strings: &[&str]| {
            let mut o = Vec::new();
            o.extend_from_slice(&(strings.len() as u32).to_le_bytes());
            for s in strings {
                o.extend_from_slice(&(s.len() as u16).to_le_bytes());
                o.extend_from_slice(s.as_bytes());
            }
            o
        };
        tables.extend(st(&["United States"]));
        tables.extend(st(&["Tennessee"]));
        tables.extend(st(&["Davidson"]));
        tables.extend(st(&["37201"]));
        tables.extend(st(&["America/Chicago"]));

        let mut grid = 1u32.to_le_bytes().to_vec(); // 1 entry
        grid.extend_from_slice(&0u64.to_le_bytes()); // h3_cell 0
        grid.extend_from_slice(&[0, 0]); // country_idx, state_idx
        grid.extend_from_slice(&0u16.to_le_bytes()); // county_idx
        grid.extend_from_slice(&0u16.to_le_bytes()); // zip_idx
        grid.extend_from_slice(&[0, 0]); // tz_idx, flags

        let lookup = admin_lookup_from_bytes(&grid, &tables).unwrap();
        let info = lookup.lookup_cell(0).unwrap();
        assert_eq!(info.country, "United States");
        assert_eq!(info.state, "Tennessee");
        // Malformed grid bytes must error, not panic.
        assert!(admin_lookup_from_bytes(&[0xff, 0xff], &tables).is_err());
    }

    #[test]
    fn try_decode_all_returns_none_on_garbage() {
        // Matches wasm/test/golden.mjs's decompress_block garbage-input case:
        // non-zstd bytes must fail cleanly (None), never panic.
        let mut decoder = FrameDecoder::new();
        assert!(try_decode_all(&mut decoder, &[1, 2, 3]).is_none());
    }

    #[test]
    fn try_decode_all_returns_none_on_empty() {
        let mut decoder = FrameDecoder::new();
        assert!(try_decode_all(&mut decoder, &[]).is_none());
    }

    #[test]
    fn try_decode_all_decodes_valid_frame() {
        // ruzstd is decode-only, so feed a real zstd frame captured offline
        // (`printf 'hello world' | zstd -c`) rather than round-tripping. This
        // exercises the happy path of the shared decode helper backing
        // `decompress_block`; the frame includes zstd's content checksum
        // trailer, so a mis-wired decoder would surface as a decode error.
        const FRAME: &[u8] = &[
            40, 181, 47, 253, 4, 88, 89, 0, 0, 104, 101, 108, 108, 111, 32, 119, 111, 114, 108,
            100, 104, 105, 30, 178,
        ];
        let mut decoder = FrameDecoder::new();
        let out = try_decode_all(&mut decoder, FRAME).expect("valid zstd frame should decode");
        assert_eq!(out, b"hello world");
    }

    /// A square footprint `size` m across, centred `north`/`east` metres from
    /// (0, 0) -- the equator, so a degree of longitude is a degree of latitude.
    fn square(north: f64, east: f64, size: f64, height: f64) -> ptiles_core::ViewBuilding {
        let d = |m: f64| m / 111_320.0;
        let (n, e, s) = (north, east, size / 2.0);
        ptiles_core::ViewBuilding {
            coords: vec![
                [d(e - s), d(n - s)],
                [d(e + s), d(n - s)],
                [d(e + s), d(n + s)],
                [d(e - s), d(n + s)],
                [d(e - s), d(n - s)],
            ],
            height_m: Some(height),
            building_type: "yes".to_string(),
        }
    }

    /// The union is what makes "find somewhere with a view of the river" work:
    /// a bank is sampled at many points and a building counts if it sees *any*
    /// of them. An `all` instead of an `any` here would silently return almost
    /// nothing, which reads as "no such place" rather than as a bug.
    #[test]
    fn viewshed_union_takes_any_origin_not_all() {
        let d = |m: f64| m / 111_320.0;
        // A 90 m wall sits directly between the first origin and a 60 m tower;
        // the second origin, 200 m to the east, looks past its end.
        let tower = square(200.0, 0.0, 60.0, 60.0);
        let blocker = square(100.0, 0.0, 60.0, 90.0);
        let origins = [[0.0, 0.0], [0.0, d(200.0)]];

        let scene = [tower, blocker];
        let out = viewshed_union(&scene, &origins, 1.7, 800.0);
        assert!(out[0].visible, "the tower is visible from the clear origin");
        assert_eq!(out[0].seen_count, 1, "and from that one only");
        assert_eq!(out[0].seen_from, 1, "namely the second");
        assert!(out[0].distance_m.is_finite());
        // Sanity that the first origin really is blocked, or the test proves
        // nothing about the union.
        assert!(!ptiles_core::viewshed(0.0, 0.0, 1.7, 800.0, &scene)[0].visible);
    }

    #[test]
    fn viewshed_union_with_no_origins_sees_nothing_and_does_not_panic() {
        let out = viewshed_union(&[square(100.0, 0.0, 20.0, 10.0)], &[], 1.7, 500.0);
        assert_eq!(out.len(), 1);
        assert!(!out[0].visible);
        assert_eq!(out[0].seen_from, -1);
        assert_eq!(out[0].seen_count, 0);
    }

    /// Everything visible from one point must still be visible when that point
    /// is one of several -- a union that lost hits as origins were added would
    /// make the radius slider look like it was working while it corrupted.
    #[test]
    fn viewshed_union_of_one_origin_matches_a_plain_viewshed() {
        let scene = vec![
            square(100.0, 0.0, 20.0, 40.0),
            square(200.0, 0.0, 20.0, 10.0),
            square(100.0, 120.0, 20.0, 8.0),
        ];
        let single = ptiles_core::viewshed(0.0, 0.0, 1.7, 500.0, &scene);
        let union = viewshed_union(&scene, &[[0.0, 0.0]], 1.7, 500.0);
        for (i, (a, b)) in single.iter().zip(union.iter()).enumerate() {
            assert_eq!(a.visible, b.visible, "building {i} disagrees");
            assert_eq!(a.height_m, b.height_m);
            assert_eq!(a.estimated, b.estimated);
        }
    }

    // The `MovementTracker` inputs are whole objects handed back from other
    // exports, so the risk in this wrapper isn't the classifier (host-tested in
    // ptiles-motion) but *field-name drift*: rename `distance_m` in one of the
    // road exports and the tracker silently stops seeing road context. These
    // deserialize the exact shapes those exports emit. serde_json stands in for
    // serde_wasm_bindgen — same field names, same derive.

    #[test]
    fn road_context_accepts_a_whole_nearest_road_response() {
        let response = serde_json::json!({
            "osm_id": 12345_u64,
            "name": "Broadway",
            "road_class": "residential",
            "snapped": [36.16, -86.79],
            "distance_m": 4.25,
            "geometry": [[36.16, -86.79], [36.161, -86.789]],
        });
        let ctx: RoadContext = serde_json::from_value(response).expect("nearest_road shape");
        assert_eq!(ctx.road_class, "residential");
        assert_eq!(ctx.distance_m, 4.25);
    }

    #[test]
    fn traffic_control_accepts_a_whole_nearest_intersection_response() {
        let response = serde_json::json!({
            "lat": 36.16,
            "lon": -86.79,
            "distance_m": 11.0,
            "intersection_type": 1,
        });
        let c: TrafficControl =
            serde_json::from_value(response).expect("nearest_intersection shape");
        assert_eq!(c.intersection_type, 1);
        assert!(c.holds_traffic(DebounceConfig::default().signal_radius_m));
    }

    #[test]
    fn missing_road_fields_are_an_error_not_a_default() {
        // A road context with no class would silently disable every road prior;
        // fail loudly instead.
        let no_class = serde_json::json!({"distance_m": 4.0});
        assert!(serde_json::from_value::<RoadContext>(no_class).is_err());
        let no_distance = serde_json::json!({"road_class": "footway"});
        assert!(serde_json::from_value::<RoadContext>(no_distance).is_err());
    }

    #[test]
    fn a_three_field_accel_reading_is_not_mistaken_for_no_sensor() {
        // What the Rookery Android exporter sends: variance, cadence, steps.
        // The two it omits must arrive as None, not 0 -- 0 is what
        // AccelStats::EMPTY carries, i.e. "there was no accelerometer".
        let partial: AccelStats = serde_json::from_value(serde_json::json!({
            "variance": 0.02,
            "dominant_frequency": 1.8,
            "step_count": 7,
        }))
        .expect("three-field accel");
        assert_eq!(partial.variance, 0.02);
        assert_eq!(partial.step_count, 7);
        assert_eq!(partial.mean_magnitude, None, "omitted, not zero");
        assert_eq!(partial.window_duration_s, None, "omitted, not zero");
        assert!(partial.has_signal(), "a real window with cadence and steps");
        assert!(!AccelStats::EMPTY.has_signal());
        // And it still classifies on the fields it does have.
        assert_eq!(
            ptiles_motion::classify_accel_only(&partial).movement,
            MovementType::Walking
        );
        // A zero in those fields is preserved as a reading, not swallowed.
        let zeroed: AccelStats = serde_json::from_value(serde_json::json!({
            "variance": 0.02, "dominant_frequency": 1.8, "step_count": 7,
            "mean_magnitude": 0.0, "window_duration_s": 0.0,
        }))
        .unwrap();
        assert_eq!(zeroed.mean_magnitude, Some(0.0));
        assert_ne!(zeroed, partial, "reported zero differs from not reported");
    }

    #[test]
    fn partial_debounce_config_keeps_the_other_defaults() {
        let cfg: DebounceConfig =
            serde_json::from_value(serde_json::json!({"vehicle_sticky_ms": 90_000_u64}))
                .expect("partial config");
        let d = DebounceConfig::default();
        assert_eq!(cfg.vehicle_sticky_ms, 90_000);
        assert_eq!(cfg.majority_window, d.majority_window);
        assert_eq!(cfg.signal_sticky_ms, d.signal_sticky_ms);
        assert_eq!(cfg.min_continuous, d.min_continuous);
    }

    #[test]
    fn movement_update_serializes_lowercase_names() {
        let json = serde_json::to_value(MovementUpdate {
            movement: MovementType::Driving.as_str(),
            vote: Vote { movement: MovementType::Stationary, confidence: 0.7 },
            smoothed_speed_mps: Some(1.5),
            at_traffic_control: true,
        })
        .expect("serialize");
        assert_eq!(json["movement"], "driving");
        assert_eq!(json["vote"]["movement"], "stationary");
        assert_eq!(json["vote"]["confidence"], 0.7);
        assert_eq!(json["at_traffic_control"], true);
        assert_eq!(json["smoothed_speed_mps"], 1.5);
    }
}
