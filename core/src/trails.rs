//! Trail block decoder (`.trails_v1.ptiles`, schema v1).
//!
//! Framing follows `scripts/build_trails.py::enc`, which is `rail.rs`'s record
//! layout plus two attribute bytes: zigzag-delta osm_id, geom_type byte
//! (1 = point/trailhead, else linestring), indexed trail_type, indexed
//! surface, indexed SAC hiking scale, flags, optional u16-length name.
//!
//! The two extra bytes are why trails cannot be decoded by `decode_rail`:
//! reading a trail record with the rail decoder consumes the surface byte as
//! flags and then misreads the rest of the block.

use alloc::string::String;
use alloc::vec::Vec;

use crate::codec::{
    DecodeError, decode_varint, read_i32, read_u8, read_u16, tables, zigzag_decode,
};

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TrailFeature {
    pub osm_id: i64,
    pub trail_type: String,
    pub geom_type: u8, // 0 = linestring, 1 = point/trailhead
    pub coords: Vec<[f64; 2]>,
    pub surface: String,
    pub sac_scale: String,
    pub name: Option<String>,
}

fn lookup(table: &[(u8, &str)], idx: u8) -> String {
    table
        .iter()
        .find(|(i, _)| *i == idx)
        .map(|(_, s)| String::from(*s))
        .unwrap_or_else(|| alloc::format!("unknown({idx})"))
}

fn decode_trail_record(
    data: &[u8],
    pos: usize,
    prev_osm_id: i64,
) -> Result<(TrailFeature, usize, i64), DecodeError> {
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
        // Zero-vertex linestrings carry no coordinate bytes at all, so the
        // 8-byte first-vertex header must be skipped too (same guard as rail).
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

    let trail_type = lookup(tables::TRAIL_TYPE_REVERSE, read_u8(data, p)?);
    p += 1;
    let surface = lookup(tables::TRAIL_SURFACE_REVERSE, read_u8(data, p)?);
    p += 1;
    let sac_scale = lookup(tables::SAC_SCALE_REVERSE, read_u8(data, p)?);
    p += 1;

    let flags = read_u8(data, p)?;
    p += 1;

    let mut name = None;
    if flags & 0x01 != 0 {
        let (s, consumed) = crate::codec::decode_string_u16(data, p)?;
        name = Some(s);
        p += consumed;
    }

    Ok((
        TrailFeature {
            osm_id,
            trail_type,
            geom_type,
            coords,
            surface,
            sac_scale,
            name,
        },
        p - start,
        osm_id,
    ))
}

/// Whether a trail type is developed infrastructure rather than a natural way.
///
/// `cycleway` and `footway` are built, surfaced routes -- a greenway or a park
/// path with a hard surface. `path`, `track`, `bridleway` and `steps` are the
/// walking/riding kind. A renderer usually wants to draw the two differently,
/// and the split is a property of the layer's type vocabulary, not of any one
/// renderer, so it lives here rather than being re-derived by each caller.
///
/// A trailhead is a point, not a way, and is not developed either way; it
/// answers false so a caller styling lines is never handed a surprise.
pub fn trail_is_developed(trail_type: &str) -> bool {
    matches!(trail_type, "cycleway" | "footway")
}

/// Decode a decompressed trail block into its features. Sequential records,
/// no length prefix — a record that fails to decode stops the scan.
pub fn decode_trails(data: &[u8]) -> Result<Vec<TrailFeature>, DecodeError> {
    let mut features = Vec::new();
    let mut pos = 0usize;
    let mut prev_osm_id = 0i64;

    while pos < data.len() {
        match decode_trail_record(data, pos, prev_osm_id) {
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

    fn synthetic_path() -> std::vec::Vec<u8> {
        let mut d = std::vec::Vec::new();
        d.push(0x02); // osm delta zigzag(1) = 2
        d.push(0); // geom_type = linestring
        d.extend_from_slice(&2u16.to_le_bytes()); // vertex_count
        d.extend_from_slice(&(-11_385_000i32).to_le_bytes()); // first lon micro
        d.extend_from_slice(&4_868_000i32.to_le_bytes()); // first lat micro
        d.push(0x0A); // dlon zigzag(5) = 10
        d.push(0x05); // dlat zigzag(-3) = 5
        d.push(0); // trail_type 0 = path
        d.push(8); // surface 8 = ground
        d.push(2); // sac 2 = mountain_hiking
        d.push(0x01); // flags: name present
        d.extend_from_slice(&9u16.to_le_bytes());
        d.extend_from_slice(b"Highline ");
        d
    }

    #[test]
    fn developed_split_covers_every_type_in_the_table() {
        // Every type the builder can emit must classify without a surprise,
        // so a new type added to the table cannot silently fall through.
        for (_, name) in tables::TRAIL_TYPE_REVERSE {
            let d = trail_is_developed(name);
            let expected = matches!(*name, "cycleway" | "footway");
            assert_eq!(d, expected, "{name} classified wrong");
        }
        assert!(!trail_is_developed("trailhead"));
        assert!(!trail_is_developed("unknown(200)"));
    }

    #[test]
    fn empty_block_decodes_to_empty_vec() {
        assert_eq!(decode_trails(&[]).unwrap(), Vec::new());
    }

    #[test]
    fn linestring_path_decoding_is_exact() {
        let feats = decode_trails(&synthetic_path()).unwrap();
        assert_eq!(feats.len(), 1);
        let f = &feats[0];
        assert_eq!(f.osm_id, 1);
        assert_eq!(f.geom_type, 0);
        assert_eq!(f.trail_type, "path");
        assert_eq!(f.surface, "ground");
        assert_eq!(f.sac_scale, "mountain_hiking");
        assert_eq!(f.coords.len(), 2);
        assert_eq!(f.coords[0], [-113.85, 48.68]);
        assert_eq!(f.coords[1], [-113.84995, 48.67997]);
        assert_eq!(f.name.as_deref(), Some("Highline "));
    }

    #[test]
    fn point_trailhead_decoding_is_exact() {
        let mut d = std::vec::Vec::new();
        d.extend_from_slice(&[0xC8, 0x01]); // zigzag(100)
        d.push(1); // point
        d.extend_from_slice(&(-11_360_000i32).to_le_bytes());
        d.extend_from_slice(&4_850_000i32.to_le_bytes());
        d.push(6); // trailhead
        d.push(0); // surface unset
        d.push(0); // sac unset
        d.push(0x00); // no name
        let feats = decode_trails(&d).unwrap();
        assert_eq!(feats.len(), 1);
        assert_eq!(feats[0].osm_id, 100);
        assert_eq!(feats[0].trail_type, "trailhead");
        assert_eq!(feats[0].surface, "");
        assert_eq!(feats[0].coords, std::vec![[-113.6, 48.5]]);
    }

    #[test]
    fn cross_record_osm_delta_accumulates() {
        let mut d = synthetic_path();
        d.extend(synthetic_path());
        let feats = decode_trails(&d).unwrap();
        assert_eq!(feats.len(), 2);
        assert_eq!(feats[0].osm_id, 1);
        assert_eq!(feats[1].osm_id, 2);
    }

    #[test]
    fn unknown_indices_fall_back_rather_than_panic() {
        let mut d = std::vec::Vec::new();
        d.push(0x02);
        d.push(1); // point
        d.extend_from_slice(&0i32.to_le_bytes());
        d.extend_from_slice(&0i32.to_le_bytes());
        d.push(200); // trail_type out of range
        d.push(201); // surface out of range
        d.push(202); // sac out of range
        d.push(0x00);
        let feats = decode_trails(&d).unwrap();
        assert_eq!(feats[0].trail_type, "unknown(200)");
        assert_eq!(feats[0].surface, "unknown(201)");
        assert_eq!(feats[0].sac_scale, "unknown(202)");
    }

    #[test]
    fn empty_geometry_linestring_yields_no_coords() {
        let mut d = std::vec::Vec::new();
        d.push(0x02);
        d.push(0); // linestring
        d.extend_from_slice(&0u16.to_le_bytes()); // vertex_count 0
        d.push(0); // path
        d.push(0);
        d.push(0);
        d.push(0x00);
        let feats = decode_trails(&d).unwrap();
        assert_eq!(feats.len(), 1);
        assert!(feats[0].coords.is_empty());
        assert_eq!(feats[0].trail_type, "path");
    }

    #[test]
    fn truncation_returns_err_not_panic() {
        let d = [0xC8u8, 0x01, 0x01]; // point claims 8 coord bytes, supplies none
        assert!(decode_trail_record(&d, 0, 0).is_err());
    }

    #[test]
    fn truncated_block_stops_gracefully() {
        let full = synthetic_path();
        for cut in [1usize, 3, 10, full.len() - 1] {
            let feats = decode_trails(&full[..cut]).unwrap();
            assert!(feats.len() <= 1);
        }
    }

    // The reason trails needs its own decoder: the rail decoder reads the
    // surface byte where it expects flags, so it cannot round-trip a trail.
    #[test]
    fn rail_decoder_misreads_a_trail_record() {
        let trail = decode_trails(&synthetic_path()).unwrap();
        let as_rail = crate::rail::decode_rail(&synthetic_path()).unwrap();
        assert_eq!(trail[0].name.as_deref(), Some("Highline "));
        assert!(as_rail.is_empty() || as_rail[0].name.as_deref() != Some("Highline "));
    }
}
