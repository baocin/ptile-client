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

fn try_decode_all(decoder: &mut FrameDecoder, compressed: &[u8]) -> Option<Vec<u8>> {
    let mut input: &[u8] = compressed;
    decoder.reset(&mut input).ok()?;
    decoder
        .decode_blocks(&mut input, BlockDecodingStrategy::All)
        .ok()?;
    decoder.collect()
}
