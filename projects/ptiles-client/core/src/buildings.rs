//! v8 buildings block decoder (`.buildings_v8.ptiles`, schema v8).
//!
//! Ported from the seed crate's `decode_buildings`, cross-checked field-by-
//! field against `ptiles/buildings.py::decode_building_v8`/`decode_v8_block`.
//! Adds `name_source` and `poi_osm_id` (flags2 bits 0x04/0x08), which the
//! Python reference decodes but the seed silently dropped.

use alloc::string::String;
use alloc::vec::Vec;

use crate::codec::{
    decode_string_table, decode_table_ref, decode_varint, read_i16, read_u32, read_u8,
    zigzag_decode, DecodeError,
};

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Building {
    pub osm_id: i64,
    pub building_type: String,
    pub centroid_lat: f64,
    pub centroid_lon: f64,
    /// (lon, lat) pairs in degrees.
    pub coords: Vec<[f64; 2]>,
    pub name: Option<String>,
    pub category: Option<String>,
    pub name_source: Option<String>,
    pub poi_osm_id: Option<u64>,
}

/// `f64::round` half-away-from-zero, implemented without `std` (no `libm`
/// dependency needed — this is the only float rounding ptiles-core does).
#[inline]
fn round_f64(x: f64) -> f64 {
    let t = x as i64 as f64;
    if x >= 0.0 {
        if x - t >= 0.5 {
            t + 1.0
        } else {
            t
        }
    } else if t - x >= 0.5 {
        t - 1.0
    } else {
        t
    }
}

fn compute_centroid(coords: &[[f64; 2]]) -> (f64, f64) {
    if coords.is_empty() {
        return (0.0, 0.0);
    }
    let mut sum_lon = 0.0;
    let mut sum_lat = 0.0;
    for c in coords {
        sum_lon += c[0];
        sum_lat += c[1];
    }
    let n = coords.len() as f64;
    (sum_lon / n, sum_lat / n)
}

/// Decode one v8 building record body. `cell_center_{lon,lat}_micro` are the
/// H3 res-7 cell's center in microdegrees, used to reconstruct the
/// cell-relative first vertex. Returns `(building, new_prev_osm_id)`.
fn decode_building_record(
    rec: &[u8],
    prev_osm_id: i64,
    cell_center_lon_micro: i32,
    cell_center_lat_micro: i32,
    string_table: &[String],
) -> Result<(Building, i64), DecodeError> {
    let mut p = 0usize;

    let (delta_raw, consumed) = decode_varint(rec, p)?;
    p += consumed;
    let osm_id = prev_osm_id.wrapping_add(zigzag_decode(delta_raw));

    let flags = read_u8(rec, p)?;
    p += 1;
    let vc_packed = (flags >> 4) & 0x0f;
    let vertex_count = if vc_packed == 0x0f {
        let vc = read_u8(rec, p)? as usize;
        p += 1;
        vc
    } else {
        vc_packed as usize + 4
    };

    let mut coords = Vec::with_capacity(vertex_count.min(1 << 16));
    if vertex_count > 0 {
        let offset_lon = read_i16(rec, p)? as i32;
        let offset_lat = read_i16(rec, p + 2)? as i32;
        p += 4;
        let mut prev_lon = cell_center_lon_micro.wrapping_add(offset_lon);
        let mut prev_lat = cell_center_lat_micro.wrapping_add(offset_lat);
        coords.push([prev_lon as f64 / 100_000.0, prev_lat as f64 / 100_000.0]);
        for _ in 1..vertex_count {
            let (dlon_raw, c1) = decode_varint(rec, p)?;
            p += c1;
            let (dlat_raw, c2) = decode_varint(rec, p)?;
            p += c2;
            prev_lon = prev_lon.wrapping_add(crate::codec::zigzag_decode_i32(dlon_raw));
            prev_lat = prev_lat.wrapping_add(crate::codec::zigzag_decode_i32(dlat_raw));
            coords.push([prev_lon as f64 / 100_000.0, prev_lat as f64 / 100_000.0]);
        }
    }

    let btype_idx = read_u8(rec, p)?;
    p += 1;
    let building_type = if btype_idx == 0xff {
        let (s, consumed) = crate::codec::decode_string_u8(rec, p)?;
        p += consumed;
        s
    } else if (btype_idx as usize) < string_table.len() {
        string_table[btype_idx as usize].clone()
    } else {
        String::from("yes")
    };

    let flags2 = read_u8(rec, p)?;
    p += 1;

    let mut name = None;
    let mut category = None;
    let mut name_source = None;
    let mut poi_osm_id = None;

    if flags2 & 0x01 != 0 {
        let (s, consumed) = decode_table_ref(rec, p, string_table)?;
        p += consumed;
        name = if s.is_empty() { None } else { Some(s) };
    }
    if flags2 & 0x02 != 0 {
        let (s, consumed) = decode_table_ref(rec, p, string_table)?;
        p += consumed;
        category = if s.is_empty() { None } else { Some(s) };
    }
    if flags2 & 0x04 != 0 {
        let (s, consumed) = decode_table_ref(rec, p, string_table)?;
        p += consumed;
        name_source = if s.is_empty() { None } else { Some(s) };
    }
    if flags2 & 0x08 != 0 {
        poi_osm_id = Some(crate::codec::read_u64(rec, p)?);
        p += 8;
    }
    let _ = p; // flags2 & 0x10 (has_height_m) is not modeled; trailing bytes ignored.

    let (centroid_lon, centroid_lat) = compute_centroid(&coords);

    Ok((
        Building {
            osm_id,
            building_type,
            centroid_lat,
            centroid_lon,
            coords,
            name,
            category,
            name_source,
            poi_osm_id,
        },
        osm_id,
    ))
}

/// Decode a decompressed v8 buildings block into its buildings.
///
/// Format: per-cell string table (`decode_string_table`), then repeated
/// `{ u32 record_len, record_body }` terminated by a zero-length record or
/// end of input. `cell_center_lat`/`cell_center_lon` are the H3 res-7 cell's
/// center in degrees (looked up by the caller via `query::cell_center`);
/// first-vertex offsets in each record are relative to this point.
pub fn decode_buildings(
    data: &[u8],
    cell_center_lat: f64,
    cell_center_lon: f64,
) -> Result<Vec<Building>, DecodeError> {
    let cell_center_lat_micro = round_f64(cell_center_lat * 100_000.0) as i32;
    let cell_center_lon_micro = round_f64(cell_center_lon * 100_000.0) as i32;

    let (string_table, mut p) = decode_string_table(data, 0)?;

    let mut buildings = Vec::new();
    let mut prev_osm_id = 0i64;

    while p + 4 <= data.len() {
        let record_len = read_u32(data, p)? as usize;
        p += 4;
        if record_len == 0 {
            break;
        }
        if p + record_len > data.len() {
            return Err(DecodeError::RecordOverrun {
                offset: p,
                len: record_len,
                block_len: data.len(),
            });
        }
        let rec = &data[p..p + record_len];
        if let Ok((bldg, new_prev)) = decode_building_record(
            rec,
            prev_osm_id,
            cell_center_lon_micro,
            cell_center_lat_micro,
            &string_table,
        ) {
            prev_osm_id = new_prev;
            buildings.push(bldg);
        }
        p += record_len;
    }

    Ok(buildings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_block_needs_at_least_string_table_count_byte() {
        // A single 0x00 is a valid (empty) string table; no records follow.
        let data = [0x00u8];
        assert_eq!(decode_buildings(&data, 36.16, -86.78).unwrap(), Vec::new());
    }

    #[test]
    fn truly_empty_input_errors_not_panics() {
        assert!(decode_buildings(&[], 0.0, 0.0).is_err());
    }
}
