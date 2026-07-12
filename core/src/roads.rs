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

use crate::codec::{
    DecodeError, decode_indexed_or_custom, decode_varint, read_i32, read_u8, read_u16, read_u32,
    tables,
};

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

impl Intersection {
    /// `(lon, lat)` in degrees. The stored `*_micro` fields are named "micro"
    /// but are actually at the same `/100_000` scale as road coordinates
    /// (verified against the golden fixture: `-8_679_367` -> `-86.79367`), so
    /// this is the one place that divisor lives.
    pub fn coords(&self) -> [f64; 2] {
        [
            self.lon_micro as f64 / 100_000.0,
            self.lat_micro as f64 / 100_000.0,
        ]
    }
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

    /// zigzag-encode an i32 delta the way `decode_coordinates` expects to read it.
    fn zigzag_encode_i32(v: i32) -> u64 {
        ((v << 1) ^ (v >> 31)) as u32 as u64
    }

    /// Encode a full road record body given absolute-micro first vertex and
    /// (dlon, dlat) micro deltas for the remaining vertices.
    fn build_road_record(
        osm_delta: u64,
        first_lon: i32,
        first_lat: i32,
        deltas: &[(i32, i32)],
        flags: u8,
        road_class_idx: u8,
        tail: &[u8],
    ) -> Vec<u8> {
        let mut rec = Vec::new();
        encode_varint(osm_delta, &mut rec);
        let vertex_count = (1 + deltas.len()) as u16;
        rec.extend_from_slice(&vertex_count.to_le_bytes());
        rec.extend_from_slice(&first_lon.to_le_bytes());
        rec.extend_from_slice(&first_lat.to_le_bytes());
        for (dlon, dlat) in deltas {
            encode_varint(zigzag_encode_i32(*dlon), &mut rec);
            encode_varint(zigzag_encode_i32(*dlat), &mut rec);
        }
        rec.push(flags);
        rec.push(road_class_idx);
        rec.extend_from_slice(tail);
        rec
    }

    /// Wrap one-or-more record bodies into a block with u32 length prefixes.
    fn frame_records(recs: &[Vec<u8>]) -> Vec<u8> {
        let mut block = Vec::new();
        for rec in recs {
            block.extend_from_slice(&(rec.len() as u32).to_le_bytes());
            block.extend_from_slice(rec);
        }
        block
    }

    #[test]
    fn multi_vertex_polyline_preserves_point_order_and_deltas() {
        // 3 vertices; deltas chosen so the reconstructed polyline is a known
        // ordered sequence -- the routing layer relies on point order.
        let rec = build_road_record(
            7,
            -8_678_000,
            3_616_000,
            &[(100, 50), (-200, 30)],
            0x00,
            0, // motorway
            &[],
        );
        let block = frame_records(&[rec]);
        let roads = decode_roads(&block).unwrap();
        assert_eq!(roads.len(), 1);
        let r = &roads[0];
        assert_eq!(r.road_class, "motorway");
        // Absolute micro sequence: 3616000 lat etc, /1e5 into degrees.
        let expect = vec![[-86.78, 36.16], [-86.779, 36.1605], [-86.781, 36.1608]];
        assert_eq!(r.coords.len(), 3);
        for (got, want) in r.coords.iter().zip(expect.iter()) {
            assert!((got[0] - want[0]).abs() < 1e-9, "lon {got:?} vs {want:?}");
            assert!((got[1] - want[1]).abs() < 1e-9, "lat {got:?} vs {want:?}");
        }
    }

    #[test]
    fn osm_id_is_delta_accumulated_across_records() {
        // Two records; second osm_id = first + its delta (delta varint, not zigzag).
        let r1 = build_road_record(1000, -8_678_000, 3_616_000, &[(1, 1)], 0, 8, &[]);
        let r2 = build_road_record(25, -8_678_000, 3_616_000, &[(1, 1)], 0, 8, &[]);
        let block = frame_records(&[r1, r2]);
        let roads = decode_roads(&block).unwrap();
        assert_eq!(roads.len(), 2);
        assert_eq!(roads[0].osm_id, 1000);
        assert_eq!(roads[1].osm_id, 1025);
    }

    #[test]
    fn decodes_all_optional_fields() {
        // flags: name(0x01) ref(0x02) oneway(0x04) speed(0x08) lanes(0x10)
        //        surface(0x20) bridge_tunnel(0x40) = 0x7f
        let mut tail = Vec::new();
        // name (u16-len)
        let name = "Broadway";
        tail.extend_from_slice(&(name.len() as u16).to_le_bytes());
        tail.extend_from_slice(name.as_bytes());
        // ref_tag (u8-len)
        let reft = "US-70";
        tail.push(reft.len() as u8);
        tail.extend_from_slice(reft.as_bytes());
        // oneway = 1 -> forward
        tail.push(1);
        // speed_limit
        tail.push(50);
        // lanes
        tail.push(4);
        // surface idx 1 -> asphalt
        tail.push(1);
        // bridge_tunnel 2 -> tunnel
        tail.push(2);

        let rec = build_road_record(42, -8_678_000, 3_616_000, &[(10, 10)], 0x7f, 6, &tail);
        let block = frame_records(&[rec]);
        let roads = decode_roads(&block).unwrap();
        assert_eq!(roads.len(), 1);
        let r = &roads[0];
        assert_eq!(r.road_class, "secondary");
        assert_eq!(r.name.as_deref(), Some("Broadway"));
        assert_eq!(r.ref_tag.as_deref(), Some("US-70"));
        assert_eq!(r.oneway.as_deref(), Some("forward"));
        assert_eq!(r.speed_limit_kmh, Some(50));
        assert_eq!(r.lanes, Some(4));
        assert_eq!(r.surface.as_deref(), Some("asphalt"));
        assert_eq!(r.bridge_tunnel.as_deref(), Some("tunnel"));
    }

    #[test]
    fn custom_road_class_via_escape_index() {
        // road_class idx 255 -> u8-len-prefixed custom string.
        let custom = "raceway";
        let mut tail = Vec::new();
        tail.push(custom.len() as u8);
        tail.extend_from_slice(custom.as_bytes());
        // Build manually: reuse build helper with road_class_idx=255 then append custom in tail.
        let rec = build_road_record(1, -8_678_000, 3_616_000, &[(1, 1)], 0x00, 255, &tail);
        let block = frame_records(&[rec]);
        let roads = decode_roads(&block).unwrap();
        assert_eq!(roads[0].road_class, "raceway");
    }

    #[test]
    fn bad_record_is_skipped_not_fatal() {
        // First record is well-formed; second claims 20 vertices but supplies
        // no delta bytes -> its decode fails and is skipped, but framing keeps
        // the scan aligned so the block still yields the good record.
        let good = build_road_record(5, -8_678_000, 3_616_000, &[(1, 1)], 0, 8, &[]);
        // Bad body: osm delta + vertex_count=20 + first coord, then nothing.
        let mut bad = Vec::new();
        encode_varint(9, &mut bad);
        bad.extend_from_slice(&20u16.to_le_bytes());
        bad.extend_from_slice(&(-8_678_000_i32).to_le_bytes());
        bad.extend_from_slice(&(3_616_000_i32).to_le_bytes());
        let block = frame_records(&[good, bad]);
        let roads = decode_roads(&block).unwrap();
        assert_eq!(roads.len(), 1, "malformed record skipped, good one kept");
        assert_eq!(roads[0].osm_id, 5);
    }

    #[test]
    fn zero_length_record_terminates_scan() {
        let good = build_road_record(5, -8_678_000, 3_616_000, &[(1, 1)], 0, 8, &[]);
        let mut block = frame_records(&[good]);
        block.extend_from_slice(&0u32.to_le_bytes()); // terminator
        block.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef]); // garbage after terminator ignored
        let roads = decode_roads(&block).unwrap();
        assert_eq!(roads.len(), 1);
    }

    #[test]
    fn record_len_overrun_is_error() {
        // Length prefix claims more bytes than the block holds.
        let block = [200u8, 0, 0, 0, 1, 2, 3];
        let err = decode_roads(&block).unwrap_err();
        assert!(matches!(err, DecodeError::RecordOverrun { .. }));
    }

    #[test]
    fn empty_block_is_empty_not_error() {
        assert!(decode_roads(&[]).unwrap().is_empty());
        // Fewer than 4 bytes: no record can start, no panic.
        assert!(decode_roads(&[1, 2, 3]).unwrap().is_empty());
    }

    #[test]
    fn decode_road_block_reads_intersection_table_v2() {
        let good = build_road_record(5, -8_678_000, 3_616_000, &[(1, 1)], 0, 8, &[]);
        let mut block = frame_records(&[good]);
        block.extend_from_slice(&0u32.to_le_bytes()); // record terminator
        // intersection table: count=2, then two 9-byte entries.
        block.extend_from_slice(&2u16.to_le_bytes());
        for (lon, lat, ty) in [
            (-8_679_367_i32, 3_616_076_i32, 1u8),
            (-8_677_437, 3_616_225, 3),
        ] {
            block.extend_from_slice(&lon.to_le_bytes());
            block.extend_from_slice(&lat.to_le_bytes());
            block.push(ty);
        }
        let (roads, ints) = decode_road_block(&block, 2).unwrap();
        assert_eq!(roads.len(), 1);
        assert_eq!(ints.len(), 2);
        assert_eq!(
            ints[0],
            Intersection {
                lon_micro: -8_679_367,
                lat_micro: 3_616_076,
                intersection_type: 1
            }
        );
        assert_eq!(ints[1].intersection_type, 3);
    }

    #[test]
    fn decode_road_block_v1_ignores_intersection_table() {
        let good = build_road_record(5, -8_678_000, 3_616_000, &[(1, 1)], 0, 8, &[]);
        let mut block = frame_records(&[good]);
        block.extend_from_slice(&0u32.to_le_bytes());
        block.extend_from_slice(&1u16.to_le_bytes());
        block.extend_from_slice(&(-8_679_367_i32).to_le_bytes());
        block.extend_from_slice(&(3_616_076_i32).to_le_bytes());
        block.push(1);
        let (roads, ints) = decode_road_block(&block, 1).unwrap();
        assert_eq!(roads.len(), 1);
        assert!(ints.is_empty(), "v1 must not decode an intersection table");
    }

    #[test]
    fn truncated_intersection_entry_does_not_panic() {
        let good = build_road_record(5, -8_678_000, 3_616_000, &[(1, 1)], 0, 8, &[]);
        let mut block = frame_records(&[good]);
        block.extend_from_slice(&0u32.to_le_bytes());
        block.extend_from_slice(&5u16.to_le_bytes()); // claims 5 intersections
        block.extend_from_slice(&(-8_679_367_i32).to_le_bytes()); // only a partial first entry
        // decode must stop cleanly at the truncation, not panic.
        let (_roads, ints) = decode_road_block(&block, 2).unwrap();
        assert!(ints.is_empty(), "partial intersection entry is dropped");
    }

    #[test]
    fn truncated_block_does_not_panic() {
        let block = [5u8, 0, 0, 0, 1, 2]; // claims 5-byte record, only 2 present
        assert!(decode_roads(&block).is_err());
    }

    /// Decode the real Nashville roads block fixture and cross-check a couple
    /// of routing-relevant invariants against the golden JSON. The full
    /// field-by-field comparison lives in `core/tests/golden.rs`; this is a
    /// std-only smoke test that the in-crate decoder handles a real,
    /// multi-thousand-record block without error and preserves point order.
    #[cfg(feature = "std")]
    #[test]
    fn golden_roads_block_smoke() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("test-fixtures")
            .join("golden")
            .join("roads.block.bin");
        let data = std::fs::read(&path).expect("read roads.block.bin");
        let (roads, ints) = decode_road_block(&data, 2).expect("decode real roads block");
        // meta.json: feature_count_in_index = 3552 roads, 129 intersections.
        assert_eq!(roads.len(), 3552);
        assert_eq!(ints.len(), 129);
        // First golden road: osm_id 19443101, motorway_link, bridge, forward,
        // 2 coords starting at (-86.79397, 36.16412).
        let first = &roads[0];
        assert_eq!(first.osm_id, 19_443_101);
        assert_eq!(first.road_class, "motorway_link");
        assert_eq!(first.bridge_tunnel.as_deref(), Some("bridge"));
        assert_eq!(first.oneway.as_deref(), Some("forward"));
        assert_eq!(first.coords.len(), 2);
        assert!((first.coords[0][0] - -86.79397).abs() < 1e-5);
        assert!((first.coords[0][1] - 36.16412).abs() < 1e-5);
        // Every road has at least two points (a routable polyline).
        assert!(roads.iter().all(|r| r.coords.len() >= 2));
    }
}
