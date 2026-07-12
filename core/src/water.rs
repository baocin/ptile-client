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

use crate::codec::{
    DecodeError, decode_varint, read_i32, read_u8, read_u16, read_u32, tables, zigzag_decode,
};

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

fn decode_water_record(
    data: &[u8],
    pos: usize,
    prev_osm_id: i64,
) -> Result<(WaterFeature, usize, i64), DecodeError> {
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
    use std::fs;

    fn block() -> std::vec::Vec<u8> {
        fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../test-fixtures/golden/water.block.bin"
        ))
        .unwrap()
    }

    fn golden() -> serde_json::Value {
        let raw = fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../test-fixtures/golden/water.golden.json"
        ))
        .unwrap();
        serde_json::from_slice(&raw).unwrap()
    }

    #[test]
    fn empty_block_decodes_to_empty_vec() {
        assert_eq!(decode_water(&[]).unwrap(), Vec::new());
    }

    #[test]
    fn decodes_golden_block_fully() {
        let feats = decode_water(&block()).unwrap();
        let g = golden();
        let gf = g["features"].as_array().unwrap();
        assert_eq!(feats.len(), gf.len());
        assert_eq!(feats.len(), 16);
        for (d, e) in feats.iter().zip(gf) {
            assert_eq!(d.osm_id, e["osm_id"].as_i64().unwrap());
            assert_eq!(d.water_type, e["water_type"].as_str().unwrap());
            assert_eq!(d.name.as_deref(), e["name"].as_str());
            let ec = e["coords"].as_array().unwrap();
            assert_eq!(d.coords.len(), ec.len());
            for (dc, xc) in d.coords.iter().zip(ec) {
                let xa = xc.as_array().unwrap();
                assert!((dc[0] - xa[0].as_f64().unwrap()).abs() < 1e-9);
                assert!((dc[1] - xa[1].as_f64().unwrap()).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn first_golden_feature_exact_fields() {
        let feats = decode_water(&block()).unwrap();
        let f0 = &feats[0];
        assert_eq!(f0.osm_id, 40806685);
        assert_eq!(f0.water_type, "river");
        assert_eq!(f0.coords.len(), 307);
        assert!((f0.coords[0][0] - (-86.65989)).abs() < 1e-9);
        assert!((f0.coords[0][1] - 36.29774).abs() < 1e-9);
        assert_eq!(f0.name.as_deref(), Some("Cumberland River"));
        assert_eq!(f0.width, None);
    }

    // Deterministic coordinate/scale decoding check against hand-built bytes.
    fn synthetic_linestring() -> std::vec::Vec<u8> {
        let mut d = std::vec::Vec::new();
        d.extend_from_slice(&[0xC8, 0x01]); // osm delta: zigzag(100) = 200 varint
        d.push(1); // geom_type = linestring
        d.extend_from_slice(&2u16.to_le_bytes()); // vertex_count = 2
        d.extend_from_slice(&(-8_665_989i32).to_le_bytes()); // first lon micro
        d.extend_from_slice(&3_629_774i32.to_le_bytes()); // first lat micro
        d.push(0x0A); // dlon zigzag(5) = 10
        d.push(0x05); // dlat zigzag(-3) = 5
        d.push(0x00); // flags: none
        d.push(0x03); // water_type index 3 = river
        d
    }

    #[test]
    fn coordinate_decoding_is_exact() {
        let feats = decode_water(&synthetic_linestring()).unwrap();
        assert_eq!(feats.len(), 1);
        let f = &feats[0];
        assert_eq!(f.osm_id, 100);
        assert_eq!(f.geom_type, 1);
        assert_eq!(f.water_type, "river");
        assert_eq!(f.coords.len(), 2);
        assert_eq!(f.coords[0], [-86.65989, 36.29774]);
        // v2 lon = -8_665_989 + 5, lat = 3_629_774 - 3
        assert_eq!(f.coords[1], [-86.65984, 36.29771]);
        assert_eq!(f.name, None);
    }

    #[test]
    fn empty_geometry_yields_no_coords() {
        // vertex_count = 0: no coordinate bytes should be consumed.
        let mut d = std::vec::Vec::new();
        d.extend_from_slice(&[0x02]); // osm delta zigzag(1) = 2
        d.push(0); // geom_type polygon
        d.extend_from_slice(&0u16.to_le_bytes()); // vertex_count 0
        d.push(0x00); // flags
        d.push(0x00); // water_type index 0 = lake
        let feats = decode_water(&d).unwrap();
        assert_eq!(feats.len(), 1);
        assert!(feats[0].coords.is_empty());
        assert_eq!(feats[0].water_type, "lake");
        assert_eq!(feats[0].osm_id, 1);
    }

    #[test]
    fn reference_geometry_carries_ref_id() {
        let mut d = std::vec::Vec::new();
        d.push(0x02); // osm delta zigzag(1)
        d.push(2); // geom_type = reference
        d.extend_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
        d.push(0x00); // flags
        d.push(0x00); // water_type lake
        let feats = decode_water(&d).unwrap();
        assert_eq!(feats.len(), 1);
        assert_eq!(feats[0].ref_feature_id, Some(0xDEAD_BEEF));
        assert!(feats[0].coords.is_empty());
    }

    #[test]
    fn optional_name_and_width_flags() {
        let mut d = std::vec::Vec::new();
        d.push(0x02); // osm
        d.push(1); // linestring
        d.extend_from_slice(&1u16.to_le_bytes()); // 1 vertex
        d.extend_from_slice(&0i32.to_le_bytes());
        d.extend_from_slice(&0i32.to_le_bytes());
        d.push(0x03); // flags: name | width
        d.push(0x03); // river
        d.extend_from_slice(&2u16.to_le_bytes()); // name len 2
        d.extend_from_slice(b"Hi");
        d.extend_from_slice(&42u16.to_le_bytes()); // width
        let feats = decode_water(&d).unwrap();
        assert_eq!(feats[0].name.as_deref(), Some("Hi"));
        assert_eq!(feats[0].width, Some(42));
    }

    #[test]
    fn record_level_truncation_returns_err_not_panic() {
        // Claims 2 vertices but supplies no coordinate bytes.
        let mut d = std::vec::Vec::new();
        d.extend_from_slice(&[0xC8, 0x01, 0x01]);
        d.extend_from_slice(&2u16.to_le_bytes());
        assert!(decode_water_record(&d, 0, 0).is_err());
    }

    #[test]
    fn truncated_block_stops_gracefully_no_panic() {
        let full = block();
        // Truncate mid-stream: decoder must return Ok with a (possibly
        // shorter) prefix of features and never panic.
        for cut in [1usize, 5, 20, full.len() - 1] {
            let feats = decode_water(&full[..cut]).unwrap();
            assert!(feats.len() <= 16);
        }
    }

    #[test]
    fn truncated_input_stops_gracefully_no_panic() {
        let data = [0x01u8, 0x00];
        assert!(decode_water(&data).unwrap().is_empty());
    }
}
