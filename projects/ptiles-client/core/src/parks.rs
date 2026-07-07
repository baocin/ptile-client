//! Parks block decoder (`.parks.ptiles`, schema v1).
//!
//! Framing and fields cross-checked against `ptiles/parks.py::decode_park`:
//! zigzag-delta osm_id, u8 vertex count (0xff escape to u16), delta
//! coordinates, u8-length park_type string, then an optional u16-length name.

use alloc::string::String;
use alloc::vec::Vec;

use crate::codec::{decode_string, decode_varint, read_i32, read_u16, read_u8, zigzag_decode, DecodeError};

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ParkFeature {
    pub osm_id: i64,
    pub park_type: String,
    pub coords: Vec<[f64; 2]>,
    pub name: Option<String>,
}

fn decode_park_record(data: &[u8], pos: usize, prev_osm_id: i64) -> Result<(ParkFeature, usize, i64), DecodeError> {
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

    let first_lon = read_i32(data, p)?;
    let first_lat = read_i32(data, p + 4)?;
    p += 8;

    let (coords, consumed) =
        crate::codec::decode_coordinates(data, p, first_lon, first_lat, vertex_count)?;
    p += consumed;

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

    Ok((
        ParkFeature {
            osm_id,
            park_type,
            coords,
            name,
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

    #[test]
    fn empty_block_decodes_to_empty_vec() {
        assert_eq!(decode_parks(&[]).unwrap(), Vec::new());
    }
}
