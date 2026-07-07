//! Roads block decoder (`.roads.ptiles`, schema v2).
//!
//! The seed crate's `decode_roads` guessed at a simplified, partial framing
//! (raw numeric road_class, no optional fields beyond `name`, wrong flag
//! bits). Per the plan's disagreement rule, SPEC.md/the Python reference
//! (`ptiles/roads.py::decode_road`) is the real encoder contract and wins
//! here — this port follows `decode_road` exactly, which additionally
//! recovers `ref_tag`/`oneway`/`speed_limit_kmh`/`lanes`/`surface`/
//! `bridge_tunnel` that the seed always left as `None`.

use alloc::string::String;
use alloc::vec::Vec;

use crate::codec::{decode_indexed_or_custom, decode_varint, read_i32, read_u16, read_u32, read_u8, tables, DecodeError};

/// An intersection point (v2+ road blocks carry a trailing table of these
/// after the road records). Matches `ptiles/roads.py::Intersection`.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Intersection {
    pub lon_micro: i32,
    pub lat_micro: i32,
    /// 1 = traffic_signals, 2 = stop, 3 = give_way, 4 = roundabout.
    pub intersection_type: u8,
}

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RoadSegment {
    pub osm_id: u64,
    pub road_class: String,
    /// (lon, lat) pairs in degrees.
    pub coords: Vec<[f64; 2]>,
    pub name: Option<String>,
    pub ref_tag: Option<String>,
    pub oneway: Option<String>,
    pub speed_limit_kmh: Option<u8>,
    pub lanes: Option<u8>,
    pub surface: Option<String>,
    pub bridge_tunnel: Option<String>,
}

/// Decode one road record body (everything after the u32 record-length
/// prefix). Returns `(segment, new_prev_osm_id)`.
fn decode_road_record(rec: &[u8], prev_osm_id: u64) -> Result<(RoadSegment, u64), DecodeError> {
    let mut p = 0usize;

    // OSM way ID: delta varint, NOT zigzag.
    let (delta, consumed) = decode_varint(rec, p)?;
    p += consumed;
    let osm_id = prev_osm_id.wrapping_add(delta);

    let vertex_count = read_u16(rec, p)? as usize;
    p += 2;

    let first_lon = read_i32(rec, p)?;
    let first_lat = read_i32(rec, p + 4)?;
    p += 8;

    let (coords, consumed) =
        crate::codec::decode_coordinates(rec, p, first_lon, first_lat, vertex_count)?;
    p += consumed;

    let flags = read_u8(rec, p)?;
    p += 1;

    let (road_class, consumed) = decode_indexed_or_custom(rec, p, tables::ROAD_CLASS_REVERSE)?;
    p += consumed;

    let mut name = None;
    let mut ref_tag = None;
    let mut oneway = None;
    let mut speed_limit_kmh = None;
    let mut lanes = None;
    let mut surface = None;
    let mut bridge_tunnel = None;

    if flags & 0x01 != 0 {
        let (s, consumed) = crate::codec::decode_string_u16(rec, p)?;
        name = Some(s);
        p += consumed;
    }
    if flags & 0x02 != 0 {
        let (s, consumed) = crate::codec::decode_string_u8(rec, p)?;
        ref_tag = Some(s);
        p += consumed;
    }
    if flags & 0x04 != 0 {
        let ow = read_u8(rec, p)?;
        p += 1;
        oneway = Some(String::from(match ow {
            1 => "forward",
            2 => "reverse",
            _ => "no",
        }));
    }
    if flags & 0x08 != 0 {
        speed_limit_kmh = Some(read_u8(rec, p)?);
        p += 1;
    }
    if flags & 0x10 != 0 {
        lanes = Some(read_u8(rec, p)?);
        p += 1;
    }
    if flags & 0x20 != 0 {
        let (s, consumed) = decode_indexed_or_custom(rec, p, tables::SURFACE_REVERSE)?;
        surface = Some(s);
        p += consumed;
    }
    if flags & 0x40 != 0 {
        let bt = read_u8(rec, p)?;
        p += 1;
        bridge_tunnel = match bt {
            1 => Some(String::from("bridge")),
            2 => Some(String::from("tunnel")),
            _ => None,
        };
    }
    let _ = p; // any trailing bytes belong to fields not yet defined; ignored, not an error.

    Ok((
        RoadSegment {
            osm_id,
            road_class,
            coords,
            name,
            ref_tag,
            oneway,
            speed_limit_kmh,
            lanes,
            surface,
            bridge_tunnel,
        },
        osm_id,
    ))
}

/// Decode a decompressed roads block into its road segments.
///
/// Format: repeated `{ u32 record_len, record_body }`, terminated by a
/// zero-length record or end of input. A record that fails to decode is
/// skipped (matches `decode_road_segment`'s try/except-and-continue
/// behavior in the Python reference) rather than aborting the whole block.
pub fn decode_roads(data: &[u8]) -> Result<Vec<RoadSegment>, DecodeError> {
    let (roads, _pos) = decode_road_records(data)?;
    Ok(roads)
}

/// Shared road-record scan used by both `decode_roads` and
/// `decode_road_block`. Returns the decoded segments plus the byte offset
/// immediately after the terminating zero-length record (or end of input),
/// so callers can continue on to a trailing intersection table.
fn decode_road_records(data: &[u8]) -> Result<(Vec<RoadSegment>, usize), DecodeError> {
    let mut roads = Vec::new();
    let mut p = 0usize;
    let mut prev_osm_id = 0u64;

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
        if let Ok((seg, new_prev)) = decode_road_record(rec, prev_osm_id) {
            prev_osm_id = new_prev;
            roads.push(seg);
        }
        p += record_len;
    }

    Ok((roads, p))
}

/// Decode a `.roads.ptiles` block that may carry a trailing intersection
/// table (schema v2+, per `ptiles/roads.py::decode_block`). `version` is
/// the file header's schema version.
pub fn decode_road_block(
    data: &[u8],
    version: u8,
) -> Result<(Vec<RoadSegment>, Vec<Intersection>), DecodeError> {
    let (roads, mut p) = decode_road_records(data)?;

    let mut intersections = Vec::new();
    if version >= 2 && p + 2 <= data.len() {
        let count = read_u16(data, p)? as usize;
        p += 2;
        for _ in 0..count {
            if p + 9 > data.len() {
                break;
            }
            let lon_micro = read_i32(data, p)?;
            let lat_micro = read_i32(data, p + 4)?;
            let intersection_type = read_u8(data, p + 8)?;
            p += 9;
            intersections.push(Intersection {
                lon_micro,
                lat_micro,
                intersection_type,
            });
        }
    }

    Ok((roads, intersections))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn encode_varint(mut v: u64, out: &mut Vec<u8>) {
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(b);
                break;
            }
            out.push(b | 0x80);
        }
    }

    #[test]
    fn decodes_minimal_road_with_name() {
        let mut rec = Vec::new();
        encode_varint(42, &mut rec); // osm_id delta
        rec.extend_from_slice(&1u16.to_le_bytes()); // vertex_count = 1
        rec.extend_from_slice(&(-8_678_000_i32).to_le_bytes()); // lon micro
        rec.extend_from_slice(&(3_616_000_i32).to_le_bytes()); // lat micro
        rec.push(0x01); // flags: has name
        rec.push(8); // road_class idx = residential
        let name = "Main St";
        rec.extend_from_slice(&(name.len() as u16).to_le_bytes());
        rec.extend_from_slice(name.as_bytes());

        let mut block = Vec::new();
        block.extend_from_slice(&(rec.len() as u32).to_le_bytes());
        block.extend_from_slice(&rec);

        let roads = decode_roads(&block).unwrap();
        assert_eq!(roads.len(), 1);
        assert_eq!(roads[0].osm_id, 42);
        assert_eq!(roads[0].road_class, "residential");
        assert_eq!(roads[0].name.as_deref(), Some("Main St"));
        assert_eq!(roads[0].coords, vec![[-86.78, 36.16]]);
    }

    #[test]
    fn truncated_block_does_not_panic() {
        let block = [5u8, 0, 0, 0, 1, 2]; // claims 5-byte record, only 2 present
        assert!(decode_roads(&block).is_err());
    }
}
