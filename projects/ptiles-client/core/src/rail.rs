//! Rail block decoder (`.rail.ptiles`, schema v1).
//!
//! Framing and fields cross-checked against `ptiles/rail.py::decode_rail`:
//! zigzag-delta osm_id, geom_type byte (1 = point/station, else
//! linestring/track), indexed rail_type, optional u16-length name. The
//! seed's version left `rail_type` as a raw byte and never decoded `name`;
//! this port resolves the rail-type table and the name string.

use alloc::string::String;
use alloc::vec::Vec;

use crate::codec::{decode_varint, read_i32, read_u16, read_u8, tables, zigzag_decode, DecodeError};

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RailFeature {
    pub osm_id: i64,
    pub rail_type: String,
    pub geom_type: u8, // 0 = linestring/track, 1 = point/station
    pub coords: Vec<[f64; 2]>,
    pub name: Option<String>,
}

fn decode_rail_record(data: &[u8], pos: usize, prev_osm_id: i64) -> Result<(RailFeature, usize, i64), DecodeError> {
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
        let first_lon = read_i32(data, p)?;
        let first_lat = read_i32(data, p + 4)?;
        p += 8;
        let (coords, consumed) =
            crate::codec::decode_coordinates(data, p, first_lon, first_lat, vertex_count)?;
        p += consumed;
        coords
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

    Ok((
        RailFeature {
            osm_id,
            rail_type,
            geom_type,
            coords,
            name,
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

    #[test]
    fn empty_block_decodes_to_empty_vec() {
        assert_eq!(decode_rail(&[]).unwrap(), Vec::new());
    }
}
