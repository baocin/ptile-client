//! Rail block decoder (`.rail.ptiles`, schema v1).
//!
//! Framing and fields cross-checked against `ptiles/rail.py::decode_rail`:
//! zigzag-delta osm_id, geom_type byte (1 = point/station, else
//! linestring/track), indexed rail_type, optional u16-length name. The
//! seed's version left `rail_type` as a raw byte and never decoded `name`;
//! this port resolves the rail-type table and the name string.

use alloc::string::String;
use alloc::vec::Vec;

use crate::codec::{
    DecodeError, decode_varint, read_i32, read_u8, read_u16, tables, zigzag_decode,
};

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RailFeature {
    pub osm_id: i64,
    pub rail_type: String,
    pub geom_type: u8, // 0 = linestring/track, 1 = point/station
    pub coords: Vec<[f64; 2]>,
    pub name: Option<String>,
    /// `name:en`, from v2. Rail is the layer that carries it most: 94.2% of
    /// named lines measured on Shikoku.
    pub name_en: Option<String>,
    pub brand: Option<String>,
}

fn decode_rail_record(
    data: &[u8],
    pos: usize,
    prev_osm_id: i64,
) -> Result<(RailFeature, usize, i64), DecodeError> {
    let start = pos;
    let mut p = pos;

    let (delta_raw, consumed) = decode_varint(data, p)?;
    p += consumed;
    let osm_id = prev_osm_id.wrapping_add(zigzag_decode(delta_raw));

    let geom_type = read_u8(data, p)?;
    p += 1;

    let coords = if geom_type == 1 {
        let lon = read_i32(data, p)?;
        let lat = read_i32(data, p + 4)?;
        p += 8;
        alloc::vec![[lon as f64 / 100_000.0, lat as f64 / 100_000.0]]
    } else {
        let vertex_count = read_u16(data, p)? as usize;
        p += 2;
        // Empty-geometry guard (mirrors `water::decode_water_record`): a
        // zero-vertex linestring carries no coordinate bytes, so skip the
        // 8-byte first-vertex header rather than reading stray bytes and
        // fabricating a phantom coordinate.
        if vertex_count > 0 {
            let first_lon = read_i32(data, p)?;
            let first_lat = read_i32(data, p + 4)?;
            p += 8;
            let (coords, consumed) =
                crate::codec::decode_coordinates(data, p, first_lon, first_lat, vertex_count)?;
            p += consumed;
            coords
        } else {
            Vec::new()
        }
    };

    let rail_type_idx = read_u8(data, p)?;
    p += 1;
    let rail_type = tables::RAIL_TYPE_REVERSE
        .iter()
        .find(|(i, _)| *i == rail_type_idx)
        .map(|(_, s)| String::from(*s))
        .unwrap_or_else(|| alloc::format!("unknown({rail_type_idx})"));

    let flags = read_u8(data, p)?;
    p += 1;

    let mut name = None;
    if flags & 0x01 != 0 {
        let (s, consumed) = crate::codec::decode_string_u16(data, p)?;
        name = Some(s);
        p += consumed;
    }


    // v2: name:en and brand, flag-guarded, so one decoder reads v1 and v2
    // alike -- a v1 file simply has the bits clear. The writer still had to
    // bump: these records field-walk, and an appended field a reader skips
    // desyncs every record after it in the cell, silently.
    let mut name_en = None;
    let mut brand = None;
    if flags & 0x02 != 0 {
        let (s, consumed) = crate::codec::decode_string_u16(data, p)?;
        name_en = Some(s);
        p += consumed;
    }
    if flags & 0x04 != 0 {
        let (s, consumed) = crate::codec::decode_string_u16(data, p)?;
        brand = Some(s);
        p += consumed;
    }
    Ok((
        RailFeature {
            osm_id,
            rail_type,
            geom_type,
            coords,
            name,
            name_en,
            brand,
        },
        p - start,
        osm_id,
    ))
}

/// Decode a decompressed rail block into its features. Sequential records,
/// no length prefix — a record that fails to decode stops the scan.
pub fn decode_rail(data: &[u8]) -> Result<Vec<RailFeature>, DecodeError> {
    let mut features = Vec::new();
    let mut pos = 0usize;
    let mut prev_osm_id = 0i64;

    while pos < data.len() {
        match decode_rail_record(data, pos, prev_osm_id) {
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
            "/../test-fixtures/golden/rail.block.bin"
        ))
        .unwrap()
    }

    fn golden() -> serde_json::Value {
        let raw = fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../test-fixtures/golden/rail.golden.json"
        ))
        .unwrap();
        serde_json::from_slice(&raw).unwrap()
    }

    #[test]
    fn empty_block_decodes_to_empty_vec() {
        assert_eq!(decode_rail(&[]).unwrap(), Vec::new());
    }

    #[test]
    fn decodes_golden_block_fully() {
        let feats = decode_rail(&block()).unwrap();
        let g = golden();
        let gf = g["features"].as_array().unwrap();
        assert_eq!(feats.len(), gf.len());
        assert_eq!(feats.len(), 2);
        for (d, e) in feats.iter().zip(gf) {
            assert_eq!(d.osm_id, e["osm_id"].as_i64().unwrap());
            assert_eq!(d.rail_type, e["rail_type"].as_str().unwrap());
            assert_eq!(d.geom_type, e["geom_type"].as_u64().unwrap() as u8);
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
        let feats = decode_rail(&block()).unwrap();
        let f0 = &feats[0];
        assert_eq!(f0.osm_id, 1440913532);
        assert_eq!(f0.rail_type, "station");
        assert_eq!(f0.geom_type, 1);
        assert_eq!(f0.coords.len(), 1);
        assert!((f0.coords[0][0] - (-86.77373)).abs() < 1e-9);
        assert!((f0.coords[0][1] - 36.16206).abs() < 1e-9);
        assert_eq!(f0.name.as_deref(), Some("Riverfront"));
    }

    // Hand-built point-station record with the rail-type index resolved and a
    // name string decoded (the two features the seed decoder mishandled).
    fn synthetic_station() -> std::vec::Vec<u8> {
        let mut d = std::vec::Vec::new();
        d.extend_from_slice(&[0xC8, 0x01]); // osm delta zigzag(100) = 200 varint
        d.push(1); // geom_type = point/station
        d.extend_from_slice(&(-8_677_373i32).to_le_bytes()); // lon micro
        d.extend_from_slice(&3_616_206i32.to_le_bytes()); // lat micro
        d.push(7); // rail_type index 7 = station
        d.push(0x01); // flags: name present
        d.extend_from_slice(&2u16.to_le_bytes()); // name len 2
        d.extend_from_slice(b"Hi");
        d
    }

    #[test]
    fn point_station_decoding_is_exact() {
        let feats = decode_rail(&synthetic_station()).unwrap();
        assert_eq!(feats.len(), 1);
        let f = &feats[0];
        assert_eq!(f.osm_id, 100);
        assert_eq!(f.geom_type, 1);
        assert_eq!(f.rail_type, "station");
        assert_eq!(f.coords, std::vec![[-86.77373, 36.16206]]);
        assert_eq!(f.name.as_deref(), Some("Hi"));
    }

    #[test]
    fn linestring_track_decoding_is_exact() {
        let mut d = std::vec::Vec::new();
        d.push(0x02); // osm delta zigzag(1) = 2
        d.push(0); // geom_type = linestring/track
        d.extend_from_slice(&2u16.to_le_bytes()); // vertex_count = 2
        d.extend_from_slice(&(-8_665_989i32).to_le_bytes()); // first lon micro
        d.extend_from_slice(&3_629_774i32.to_le_bytes()); // first lat micro
        d.push(0x0A); // dlon zigzag(5) = 10
        d.push(0x05); // dlat zigzag(-3) = 5
        d.push(0); // rail_type index 0 = rail
        d.push(0x00); // flags: none
        let feats = decode_rail(&d).unwrap();
        assert_eq!(feats.len(), 1);
        let f = &feats[0];
        assert_eq!(f.osm_id, 1);
        assert_eq!(f.geom_type, 0);
        assert_eq!(f.rail_type, "rail");
        assert_eq!(f.coords.len(), 2);
        assert_eq!(f.coords[0], [-86.65989, 36.29774]);
        assert_eq!(f.coords[1], [-86.65984, 36.29771]);
        assert_eq!(f.name, None);
    }

    #[test]
    fn cross_record_osm_delta_accumulates() {
        let mut d = synthetic_station();
        d.extend(synthetic_station()); // second delta of +100 from prev osm_id
        let feats = decode_rail(&d).unwrap();
        assert_eq!(feats.len(), 2);
        assert_eq!(feats[0].osm_id, 100);
        assert_eq!(feats[1].osm_id, 200);
    }

    #[test]
    fn unknown_rail_type_index_falls_back() {
        let mut d = std::vec::Vec::new();
        d.push(0x02); // osm
        d.push(1); // point
        d.extend_from_slice(&0i32.to_le_bytes());
        d.extend_from_slice(&0i32.to_le_bytes());
        d.push(200); // rail_type index far out of table range
        d.push(0x00); // flags
        let feats = decode_rail(&d).unwrap();
        assert_eq!(feats[0].rail_type, "unknown(200)");
    }

    #[test]
    fn empty_geometry_linestring_yields_no_coords() {
        // vertex_count = 0: no coordinate bytes should be consumed.
        let mut d = std::vec::Vec::new();
        d.push(0x02); // osm
        d.push(0); // linestring
        d.extend_from_slice(&0u16.to_le_bytes()); // vertex_count 0
        d.push(0); // rail_type rail
        d.push(0x00); // flags
        let feats = decode_rail(&d).unwrap();
        assert_eq!(feats.len(), 1);
        assert!(feats[0].coords.is_empty());
        assert_eq!(feats[0].rail_type, "rail");
    }

    #[test]
    fn record_level_truncation_returns_err_not_panic() {
        // Point geom claims 8 coord bytes but supplies only osm + geom_type.
        let d = [0xC8u8, 0x01, 0x01];
        assert!(decode_rail_record(&d, 0, 0).is_err());
    }

    #[test]
    fn truncated_block_stops_gracefully_no_panic() {
        let full = block();
        for cut in [1usize, 3, 10, full.len().saturating_sub(1)] {
            let feats = decode_rail(&full[..cut]).unwrap();
            assert!(feats.len() <= 2);
        }
    }

    #[test]
    fn truncated_name_returns_err_not_panic() {
        let mut d = std::vec::Vec::new();
        d.push(0x02); // osm
        d.push(1); // point
        d.extend_from_slice(&0i32.to_le_bytes());
        d.extend_from_slice(&0i32.to_le_bytes());
        d.push(7); // station
        d.push(0x01); // flags: name present
        d.extend_from_slice(&9u16.to_le_bytes()); // claims 9 name bytes...
        d.extend_from_slice(b"xy"); // ...but supplies 2
        // Record decode must error (block scan then stops without panic).
        assert!(decode_rail_record(&d, 0, 0).is_err());
        assert!(decode_rail(&d).unwrap().is_empty());
    }
}
