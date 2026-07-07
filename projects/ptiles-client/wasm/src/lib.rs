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
    decode_water as core_decode_water};
use ptiles_core::{
    nearest_road as core_nearest_road, score_candidates as core_score_candidates, Fix, ScoringParams,
    DEFAULT_THRESHOLD_M,
};
use ptiles_core::cells_for_bounds as core_cells_for_bounds;
use ptiles_core::{
    match_business_name_block as core_match_business_name_block, name_to_key as core_name_to_key,
};
use ptiles_core::{
    cell_center as core_cell_center, cell_for_coord as core_cell_for_coord,
    neighbor_cells as core_neighbor_cells,
};
use ptiles_core::{index_binary_search as core_index_binary_search, parse_index as core_parse_index, Header};

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

#[wasm_bindgen]
pub fn decode_rail(data: &[u8]) -> Result<JsValue, JsValue> {
    let rail = core_decode_rail(data).map_err(|e| JsValue::from_str(&e.to_string()))?;
    to_js(&rail)
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
    let fix_input: FixInput =
        serde_json::from_str(fix_json).map_err(|e| JsValue::from_str(&format!("invalid fix_json: {e}")))?;
    let fix = Fix {
        lat: fix_input.lat,
        lon: fix_input.lon,
        horizontal_accuracy_m: fix_input.horizontal_accuracy_m,
        speed_mps: fix_input.speed_mps,
    };

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
/// entry list. Demo/browser boundary for `ptiles_core::parse_index` so JS
/// never has to hand-roll the 19-byte entry layout.
#[wasm_bindgen]
pub fn parse_index_entries(data: &[u8]) -> Result<JsValue, JsValue> {
    let entries = core_parse_index(data).map_err(|e| JsValue::from_str(&e.to_string()))?;
    to_js(&entries)
}

/// Binary-search an already-parsed index (pass the raw index bytes again;
/// re-parses internally -- the demo caches those bytes per open file, so
/// this stays O(log n) per call with no network cost) for the block
/// offset/length covering `cell_hex` (lowercase hex H3 res-7 cell, same
/// string form `cells_for_bounds`/`cell_for_coord` return). Returns `null`
/// if the cell has no block in this file (sparse coverage).
#[wasm_bindgen]
pub fn find_block_for_cell(index_bytes: &[u8], cell_hex: &str) -> Result<JsValue, JsValue> {
    let cell = u64::from_str_radix(cell_hex, 16)
        .map_err(|e| JsValue::from_str(&format!("invalid cell hex {cell_hex:?}: {e}")))?;
    let entries = core_parse_index(index_bytes).map_err(|e| JsValue::from_str(&e.to_string()))?;
    match core_index_binary_search(&entries, cell) {
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
    let cell = u64::from_str_radix(cell_hex, 16)
        .map_err(|e| JsValue::from_str(&format!("invalid cell hex {cell_hex:?}: {e}")))?;
    let (lat, lon) = core_cell_center(cell);
    Ok(vec![lat, lon])
}

/// Ring-1 (6 cells) H3 neighbors of `cell_hex`, as lowercase hex strings.
/// Demo/browser boundary for `ptiles_core::neighbor_cells` -- replaces
/// `h3-js`'s `gridRing(cell, 1)` (used by the deployed demo's
/// `BusinessReader.query` for nearby-business radius search).
#[wasm_bindgen]
pub fn neighbor_cells(cell_hex: &str) -> Result<Vec<JsValue>, JsValue> {
    let cell = u64::from_str_radix(cell_hex, 16)
        .map_err(|e| JsValue::from_str(&format!("invalid cell hex {cell_hex:?}: {e}")))?;
    Ok(core_neighbor_cells(cell)
        .into_iter()
        .map(|c| JsValue::from_str(&format!("{c:x}")))
        .collect())
}

fn try_decode_all(decoder: &mut FrameDecoder, compressed: &[u8]) -> Option<Vec<u8>> {
    let mut input: &[u8] = compressed;
    decoder.reset(&mut input).ok()?;
    decoder
        .decode_blocks(&mut input, BlockDecodingStrategy::All)
        .ok()?;
    decoder.collect()
}
