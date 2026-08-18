//! Parks block decoder (`.parks.ptiles`, schema v1).
//!
//! Framing and fields cross-checked against `ptiles/parks.py::decode_park`:
//! zigzag-delta osm_id, u8 vertex count (0xff escape to u16), delta
//! coordinates, u8-length park_type string, then an optional u16-length name.

use alloc::string::String;
use alloc::vec::Vec;

use crate::codec::{
    DecodeError, decode_string, decode_varint, read_i32, read_u8, read_u16, zigzag_decode,
};

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ParkFeature {
    pub osm_id: i64,
    pub park_type: String,
    pub coords: Vec<[f64; 2]>,
    pub name: Option<String>,
    /// `name:en`, from v2. 4.9% of named parks carry one.
    pub name_en: Option<String>,
    pub brand: Option<String>,
}

fn decode_park_record(
    data: &[u8],
    pos: usize,
    prev_osm_id: i64,
) -> Result<(ParkFeature, usize, i64), DecodeError> {
    let start = pos;
    let mut p = pos;

    let (delta_raw, consumed) = decode_varint(data, p)?;
    p += consumed;
    let osm_id = prev_osm_id.wrapping_add(zigzag_decode(delta_raw));

    let mut vertex_count = read_u8(data, p)? as usize;
    p += 1;
    if vertex_count == 255 {
        vertex_count = read_u16(data, p)? as usize;
        p += 2;
    }

    // Empty-geometry guard (mirrors `water::decode_water_record`): a
    // zero-vertex record carries no coordinate bytes, so don't read the
    // 8-byte first-vertex header — doing so would fabricate a phantom
    // coordinate and desync the (length-prefix-free) record stream.
    let mut coords = Vec::new();
    if vertex_count > 0 {
        let first_lon = read_i32(data, p)?;
        let first_lat = read_i32(data, p + 4)?;
        p += 8;

        let (c, consumed) =
            crate::codec::decode_coordinates(data, p, first_lon, first_lat, vertex_count)?;
        coords = c;
        p += consumed;
    }

    let park_type_len = read_u8(data, p)? as usize;
    p += 1;
    let park_type = decode_string(data, p, park_type_len)?;
    p += park_type_len;

    let flags = read_u8(data, p)?;
    p += 1;

    let mut name = None;
    if flags & 0x01 != 0 {
        let (s, consumed) = crate::codec::decode_string_u16(data, p)?;
        name = Some(s);
        p += consumed;
    }


    // v2: name:en and brand, flag-guarded. A v1 file has the bits clear, so
    // one decoder reads both -- but the *writer* had to bump, because these
    // records field-walk: an appended field a reader does not know about
    // desyncs every record after it in the cell, silently.
    let mut name_en = None;
    let mut brand = None;
    if flags & 0x02 != 0 {{
        let (s, consumed) = crate::codec::decode_string_u16(data, p)?;
        name_en = Some(s);
        p += consumed;
    }}
    if flags & 0x04 != 0 {{
        let (s, consumed) = crate::codec::decode_string_u16(data, p)?;
        brand = Some(s);
        p += consumed;
    }}
    Ok((
        ParkFeature {
            osm_id,
            park_type,
            coords,
            name,
            name_en,
            brand,
        },
        p - start,
        osm_id,
    ))
}

/// Decode a decompressed parks block into its features. Sequential records,
/// no length prefix — a record that fails to decode stops the scan.
pub fn decode_parks(data: &[u8]) -> Result<Vec<ParkFeature>, DecodeError> {
    let mut features = Vec::new();
    let mut pos = 0usize;
    let mut prev_osm_id = 0i64;

    while pos < data.len() {
        match decode_park_record(data, pos, prev_osm_id) {
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
            "/../test-fixtures/golden/parks.block.bin"
        ))
        .unwrap()
    }

    fn golden() -> serde_json::Value {
        let raw = fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../test-fixtures/golden/parks.golden.json"
        ))
        .unwrap();
        serde_json::from_slice(&raw).unwrap()
    }

    #[test]
    fn empty_block_decodes_to_empty_vec() {
        assert_eq!(decode_parks(&[]).unwrap(), Vec::new());
    }

    #[test]
    fn decodes_golden_block_fully() {
        let feats = decode_parks(&block()).unwrap();
        let g = golden();
        let gf = g["features"].as_array().unwrap();
        assert_eq!(feats.len(), gf.len());
        assert_eq!(feats.len(), 23);
        for (d, e) in feats.iter().zip(gf) {
            assert_eq!(d.osm_id, e["osm_id"].as_i64().unwrap());
            assert_eq!(d.park_type, e["park_type"].as_str().unwrap());
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
        let feats = decode_parks(&block()).unwrap();
        let f0 = &feats[0];
        assert_eq!(f0.osm_id, 130905906);
        assert_eq!(f0.park_type, "protected_area");
        assert_eq!(f0.coords.len(), 7); // closed ring
        assert!((f0.coords[0][0] - (-86.77883)).abs() < 1e-9);
        assert!((f0.coords[0][1] - 36.16128).abs() < 1e-9);
        assert_eq!(f0.name.as_deref(), Some("Ryman Auditorium"));
    }

    fn synthetic_park(vertex_count_bytes: &[u8], nverts: u16) -> std::vec::Vec<u8> {
        let mut d = std::vec::Vec::new();
        d.extend_from_slice(&[0xC8, 0x01]); // osm delta zigzag(100)=200
        d.extend_from_slice(vertex_count_bytes);
        d.extend_from_slice(&(-8_677_883i32).to_le_bytes());
        d.extend_from_slice(&3_616_128i32.to_le_bytes());
        // remaining vertices as (+1,+1) deltas: zigzag(1)=2
        for _ in 1..nverts {
            d.push(0x02);
            d.push(0x02);
        }
        d.push(4); // park_type_len
        d.extend_from_slice(b"park");
        d.push(0x00); // flags: no name
        d
    }

    #[test]
    fn coordinate_decoding_is_exact() {
        let feats = decode_parks(&synthetic_park(&[2u8], 2)).unwrap();
        assert_eq!(feats.len(), 1);
        let f = &feats[0];
        assert_eq!(f.osm_id, 100);
        assert_eq!(f.park_type, "park");
        assert_eq!(f.coords.len(), 2);
        assert_eq!(f.coords[0], [-86.77883, 36.16128]);
        assert_eq!(f.coords[1], [-86.77882, 36.16129]);
        assert_eq!(f.name, None);
    }

    #[test]
    fn u16_vertex_count_escape() {
        // vertex_count byte 255 => read u16 count that follows.
        let mut prefix = std::vec::Vec::new();
        prefix.push(255u8);
        prefix.extend_from_slice(&2u16.to_le_bytes());
        let feats = decode_parks(&synthetic_park(&prefix, 2)).unwrap();
        assert_eq!(feats.len(), 1);
        assert_eq!(feats[0].coords.len(), 2);
    }

    #[test]
    fn empty_geometry_yields_no_coords() {
        // vertex_count 0: guard must skip the 8 coordinate bytes.
        let mut d = std::vec::Vec::new();
        d.push(0x02); // osm delta zigzag(1)
        d.push(0); // vertex_count 0
        d.push(4); // park_type_len
        d.extend_from_slice(b"park");
        d.push(0x00); // flags
        let feats = decode_parks(&d).unwrap();
        assert_eq!(feats.len(), 1);
        assert!(feats[0].coords.is_empty());
        assert_eq!(feats[0].park_type, "park");
        assert_eq!(feats[0].osm_id, 1);
    }

    #[test]
    fn record_level_truncation_returns_err_not_panic() {
        // Claims 3 vertices, supplies only the first-vertex header.
        let mut d = std::vec::Vec::new();
        d.extend_from_slice(&[0xC8, 0x01]);
        d.push(3); // vertex_count 3
        d.extend_from_slice(&0i32.to_le_bytes());
        d.extend_from_slice(&0i32.to_le_bytes());
        assert!(decode_park_record(&d, 0, 0).is_err());
    }

    #[test]
    fn truncated_block_stops_gracefully_no_panic() {
        let full = block();
        for cut in [1usize, 4, 15, full.len() - 1] {
            let feats = decode_parks(&full[..cut]).unwrap();
            assert!(feats.len() <= 23);
        }
    }
}
