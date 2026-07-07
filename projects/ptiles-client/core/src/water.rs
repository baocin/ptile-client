//! Water block decoder (`.water.ptiles`, schema v1).
//!
//! Framing (sequential records, no per-record length prefix) matches the
//! seed crate's `decode_water`. Field semantics cross-checked against
//! `ptiles/water.py::decode_water_record`: the seed only *skipped* the
//! optional name/width/depth bytes without ever populating `name`; this
//! port actually decodes `name` and `width`, and keeps `ref_feature_id`
//! for reference-type geometries.

use alloc::string::String;
use alloc::vec::Vec;

use crate::codec::{decode_varint, read_i32, read_u16, read_u32, read_u8, tables, zigzag_decode, DecodeError};

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct WaterFeature {
    pub osm_id: i64,
    pub geom_type: u8, // 0 = polygon, 1 = linestring, 2 = reference
    pub water_type: String,
    pub coords: Vec<[f64; 2]>,
    pub ref_feature_id: Option<u32>,
    pub name: Option<String>,
    pub width: Option<u16>,
}

fn decode_water_record(data: &[u8], pos: usize, prev_osm_id: i64) -> Result<(WaterFeature, usize, i64), DecodeError> {
    let start = pos;
    let mut p = pos;

    let (delta_raw, consumed) = decode_varint(data, p)?;
    p += consumed;
    let osm_id = prev_osm_id.wrapping_add(zigzag_decode(delta_raw));

    let geom_type = read_u8(data, p)?;
    p += 1;

    let mut coords = Vec::new();
    let mut ref_feature_id = None;

    if geom_type == 2 {
        ref_feature_id = Some(read_u32(data, p)?);
        p += 4;
    } else {
        let vertex_count = read_u16(data, p)? as usize;
        p += 2;
        if vertex_count > 0 {
            let first_lon = read_i32(data, p)?;
            let first_lat = read_i32(data, p + 4)?;
            p += 8;
            let (c, consumed) =
                crate::codec::decode_coordinates(data, p, first_lon, first_lat, vertex_count)?;
            coords = c;
            p += consumed;
        }
    }

    let flags = read_u8(data, p)?;
    p += 1;
    let wt = read_u8(data, p)?;
    p += 1;
    let water_type = tables::WATER_TYPES
        .get(wt as usize)
        .map(|s| String::from(*s))
        .unwrap_or_else(|| alloc::format!("unknown({wt})"));

    let mut name = None;
    let mut width = None;

    if flags & 0x01 != 0 {
        let (s, consumed) = crate::codec::decode_string_u16(data, p)?;
        name = Some(s);
        p += consumed;
    }
    if flags & 0x02 != 0 {
        width = Some(read_u16(data, p)?);
        p += 2;
    }
    if flags & 0x04 != 0 {
        p += 2; // depth, not modeled
    }

    Ok((
        WaterFeature {
            osm_id,
            geom_type,
            water_type,
            coords,
            ref_feature_id,
            name,
            width,
        },
        p - start,
        osm_id,
    ))
}

/// Decode a decompressed water block into its features. Sequential records
/// with no length prefix; a record that fails to decode stops the scan
/// (matches the Python reference's `except: break` behavior — later bytes
/// can't be resynchronized without a length prefix).
pub fn decode_water(data: &[u8]) -> Result<Vec<WaterFeature>, DecodeError> {
    let mut features = Vec::new();
    let mut pos = 0usize;
    let mut prev_osm_id = 0i64;

    while pos < data.len() {
        match decode_water_record(data, pos, prev_osm_id) {
            Ok((feat, consumed, new_prev)) => {
                prev_osm_id = new_prev;
                pos += consumed.max(1);
                features.push(feat);
            }
            Err(_) => break,
        }
    }

    Ok(features)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_block_decodes_to_empty_vec() {
        assert_eq!(decode_water(&[]).unwrap(), Vec::new());
    }

    #[test]
    fn truncated_input_stops_gracefully_no_panic() {
        let data = [0x01u8, 0x00]; // osm delta=0 zigzag, then truncated geom_type read
        let result = decode_water(&data).unwrap();
        assert!(result.is_empty());
    }
}
