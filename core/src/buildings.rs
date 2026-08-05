//! v8 buildings block decoder (`.buildings_v8.ptiles`, schema v8).
//!
//! Ported from the seed crate's `decode_buildings`, cross-checked field-by-
//! field against `ptiles/buildings.py::decode_building_v8`/`decode_v8_block`.
//! Adds `name_source` and `poi_osm_id` (flags2 bits 0x04/0x08), which the
//! Python reference decodes but the seed silently dropped.

use alloc::string::String;
use alloc::vec::Vec;

use crate::codec::{
    DecodeError, decode_string_table, decode_table_ref, decode_varint, read_i16, read_u8, read_u32,
    zigzag_decode,
};

/// The buildings block schema versions this decoder understands. v6/v7
/// blocks use an incompatible record layout (raw-delta osm_id, u8 vertex
/// count, i32 absolute first vertex, wall-segment geometry) and must not be
/// fed to the v8/v9 decoder. See `ptiles/buildings.py::BuildingsReader._read_block`,
/// which dispatches on `version >= 8`.
///
/// v9 adds `business_tag` (flags2 0x20) and `opening_hours` (flags2 0x40)
/// which this decoder reads but the current `Building` struct doesn't expose
/// — they're consumed for byte-position tracking and then discarded.
pub const SCHEMA_VERSION: u8 = 8;

/// Error from the version-gated [`decode_buildings_v8`] entry point.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum BuildingsError {
    /// Block's declared schema version is not [`SCHEMA_VERSION`] (v8).
    #[error("unsupported buildings schema version {found} (only v{SCHEMA_VERSION} is supported)")]
    UnsupportedVersion { found: u8 },
    /// The block byte-stream was malformed or truncated.
    #[error(transparent)]
    Decode(#[from] DecodeError),
}

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
    /// Height in metres, when the builder recorded one (`flags2 & 0x10`).
    ///
    /// Stored as a `u8` of half-metre steps, so the value is a multiple of 0.5
    /// and cannot exceed 127.5 m — a genuinely taller building is clamped by
    /// the encoder, not by this decoder. Absent on most published states:
    /// coverage is per-state and ranges from ~100% (NY, CA, FL, PA) to exactly
    /// zero (TX, GA, WA, OH, MI, IL, TN, ...), so callers must treat `None` as
    /// "not published here" rather than "ground level".
    pub height_m: Option<f64>,
}

/// `f64::round` half-away-from-zero, implemented without `std` (no `libm`
/// dependency needed — this is the only float rounding ptiles-core does).
#[inline]
fn round_f64(x: f64) -> f64 {
    let t = x as i64 as f64;
    if x >= 0.0 {
        if x - t >= 0.5 { t + 1.0 } else { t }
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
    // Half-metre steps in a u8, matching the builder
    // (`ptiles/scripts/encode_v8.py`: `data[pos] * 0.5`, SPEC.md "0.5 m steps,
    // 0-127.5 m"). This field was skipped for a long time, which is why nothing
    // downstream could draw a building's height even where one was published.
    let height_m = if flags2 & 0x10 != 0 {
        match read_u8(rec, p) {
            Ok(raw) => {
                p += 1;
                Some(f64::from(raw) * 0.5)
            }
            // A record that announces a height and then ends is malformed, but
            // the ring is already decoded by this point and is the valuable
            // part. `decode_buildings` drops any record that returns `Err`, so
            // propagating with `?` would throw the footprint away to avoid
            // guessing one byte. Keep the building, lose only the height.
            Err(_) => None,
        }
    } else {
        None
    };

    // v9's business_tag (0x20) and opening_hours (0x40) are still unmodelled and
    // sit after this point. Leaving them unread is safe only because a block is
    // a sequence of length-prefixed records, so the next record's start comes
    // from its own prefix rather than from this cursor.
    let _ = p;

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
            height_m,
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
        // `checked_add` guards the (theoretical, 32-bit-usize) overflow of a
        // hostile `record_len` near `usize::MAX`; on 64-bit it can't overflow
        // but the bounds check is still required.
        let end = match p.checked_add(record_len) {
            Some(end) if end <= data.len() => end,
            _ => {
                return Err(DecodeError::RecordOverrun {
                    offset: p,
                    len: record_len,
                    block_len: data.len(),
                });
            }
        };
        let rec = &data[p..end];
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
        p = end;
    }

    Ok(buildings)
}

/// Version-gated wrapper around [`decode_buildings`]. Accepts schema v8
/// or v9 *before* decoding — v9 only adds optional trailing fields that
/// are skipped by the v8 decoder (flags2 0x20 business_tag, 0x40
/// opening_hours). v6/v7 blocks are still rejected.
pub fn decode_buildings_v8(
    data: &[u8],
    version: u8,
    cell_center_lat: f64,
    cell_center_lon: f64,
) -> Result<Vec<Building>, BuildingsError> {
    if !matches!(version, 8 | 9) {
        return Err(BuildingsError::UnsupportedVersion { found: version });
    }
    Ok(decode_buildings(data, cell_center_lat, cell_center_lon)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    // --- v8 block synthesis helpers (mirror the on-disk encoding) ---

    /// LEB128-encode an unsigned varint.
    fn put_uvarint(out: &mut Vec<u8>, mut v: u64) {
        loop {
            let mut b = (v & 0x7f) as u8;
            v >>= 7;
            if v != 0 {
                b |= 0x80;
            }
            out.push(b);
            if v == 0 {
                break;
            }
        }
    }

    /// Zigzag-encode a signed 64-bit value (inverse of `zigzag_decode`).
    fn zigzag_encode(n: i64) -> u64 {
        ((n << 1) ^ (n >> 63)) as u64
    }

    fn put_svarint(out: &mut Vec<u8>, v: i64) {
        put_uvarint(out, zigzag_encode(v));
    }

    /// A single v8 building record: osm_id delta, packed vertex count, cell-
    /// relative first vertex (i16 offsets), then (n-1) zigzag delta pairs,
    /// building-type index, and flags2 (no optional fields set here).
    struct RecordSpec {
        osm_delta: i64,
        offset_lon: i16,
        offset_lat: i16,
        /// zigzag delta pairs (dlon_micro, dlat_micro) for vertices 2..=n.
        deltas: Vec<(i32, i32)>,
        btype_idx: u8,
    }

    fn encode_record(spec: &RecordSpec) -> Vec<u8> {
        let mut body = Vec::new();
        put_svarint(&mut body, spec.osm_delta);
        let vertex_count = spec.deltas.len() + 1;
        assert!(
            (4..=18).contains(&vertex_count),
            "helper only encodes the packed 4..=18 vertex range"
        );
        let vc_packed = (vertex_count - 4) as u8;
        body.push(vc_packed << 4);
        body.extend_from_slice(&spec.offset_lon.to_le_bytes());
        body.extend_from_slice(&spec.offset_lat.to_le_bytes());
        for (dlon, dlat) in &spec.deltas {
            put_svarint(&mut body, *dlon as i64);
            put_svarint(&mut body, *dlat as i64);
        }
        body.push(spec.btype_idx);
        body.push(0x00); // flags2: no name/category/name_source/poi
        body
    }

    /// Assemble a full v8 block: string table + length-prefixed records.
    fn encode_block(string_table: &[&str], records: &[RecordSpec]) -> Vec<u8> {
        let mut out = Vec::new();
        out.push(string_table.len() as u8);
        for s in string_table {
            out.push(s.len() as u8);
            out.extend_from_slice(s.as_bytes());
        }
        for r in records {
            let body = encode_record(r);
            out.extend_from_slice(&(body.len() as u32).to_le_bytes());
            out.extend_from_slice(&body);
        }
        out
    }

    /// A record with arbitrary flags2 and a raw tail, for the optional fields
    /// `encode_record` does not model. `tail` is whatever those flags imply,
    /// already encoded.
    fn encode_record_with_flags2(spec: &RecordSpec, flags2: u8, tail: &[u8]) -> Vec<u8> {
        let mut body = encode_record(spec);
        let last = body.len() - 1;
        assert_eq!(body[last], 0x00, "encode_record should end with a zero flags2");
        body[last] = flags2;
        body.extend_from_slice(tail);
        body
    }

    fn encode_block_raw(string_table: &[&str], records: &[Vec<u8>]) -> Vec<u8> {
        let mut out = Vec::new();
        out.push(string_table.len() as u8);
        for s in string_table {
            out.push(s.len() as u8);
            out.extend_from_slice(s.as_bytes());
        }
        for body in records {
            out.extend_from_slice(&(body.len() as u32).to_le_bytes());
            out.extend_from_slice(body);
        }
        out
    }

    fn one_square() -> RecordSpec {
        RecordSpec {
            osm_delta: 42,
            offset_lon: 50,
            offset_lat: -30,
            deltas: vec![(100, 0), (0, 100), (-100, 0), (0, -100)],
            btype_idx: 0,
        }
    }

    // --- height (flags2 & 0x10) ---

    #[test]
    fn height_is_decoded_in_half_metre_steps() {
        // 31 * 0.5 = 15.5 m. Half-metre granularity is the whole point of the
        // u8 encoding, so check a value that is not a whole number of metres.
        let rec = encode_record_with_flags2(&one_square(), 0x10, &[31]);
        let block = encode_block_raw(&["house"], &[rec]);
        let out = decode_buildings(&block, 1.0, 2.0).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].height_m, Some(15.5));
    }

    #[test]
    fn height_absent_when_flag_clear() {
        // Not zero — absent. A building with no published height is not a
        // building at ground level, and callers key off `None` to say so.
        let block = encode_block(&["house"], &[one_square()]);
        let out = decode_buildings(&block, 1.0, 2.0).unwrap();
        assert_eq!(out[0].height_m, None);
    }

    #[test]
    fn height_spans_the_full_u8_range() {
        for (raw, want) in [(0u8, 0.0), (1, 0.5), (255, 127.5)] {
            let rec = encode_record_with_flags2(&one_square(), 0x10, &[raw]);
            let block = encode_block_raw(&["house"], &[rec]);
            let out = decode_buildings(&block, 1.0, 2.0).unwrap();
            assert_eq!(out[0].height_m, Some(want), "raw byte {raw}");
        }
    }

    #[test]
    fn truncated_height_byte_keeps_the_building_and_its_ring() {
        // flags2 claims a height, then the record ends. The footprint is
        // already decoded at that point, so the building must survive with
        // `height_m == None` -- propagating the error would drop the whole
        // record, trading a whole polygon for one missing byte.
        let rec = encode_record_with_flags2(&one_square(), 0x10, &[]);
        let block = encode_block_raw(&["house"], &[rec]);
        let out = decode_buildings(&block, 1.0, 2.0).unwrap();
        assert_eq!(out.len(), 1, "the building must not be dropped");
        assert_eq!(out[0].height_m, None);
        assert_eq!(out[0].coords.len(), 5, "ring must be intact");
    }

    #[test]
    fn height_is_read_before_unmodelled_v9_fields() {
        // v9 appends business_tag (0x20) and opening_hours (0x40) *after* the
        // height byte, and this decoder still ignores both. Height must come
        // back correctly anyway, and — the part that actually matters — the
        // record after it must decode, proving the unread tail never
        // desynchronises the stream. It cannot, because each record carries its
        // own length prefix, but that is exactly the kind of "cannot" worth
        // pinning down.
        let tail = [
            21u8, // height_raw -> 10.5 m
            1,    // business_tag: table ref -> string_table[1]
            5, b'0', b'9', b'-', b'1', b'8', // opening_hours: u8-len string
        ];
        let first = encode_record_with_flags2(&one_square(), 0x10 | 0x20 | 0x40, &tail);
        let second = encode_record_with_flags2(&one_square(), 0x10, &[8]); // 4.0 m
        let block = encode_block_raw(&["house", "cafe"], &[first, second]);

        let out = decode_buildings(&block, 1.0, 2.0).unwrap();
        assert_eq!(out.len(), 2, "the record after a v9 tail must still decode");
        assert_eq!(out[0].height_m, Some(10.5));
        assert_eq!(out[1].height_m, Some(4.0));
        assert_eq!(out[1].building_type, "house");
    }

    // --- empty / degenerate input ---

    #[test]
    fn empty_block_needs_at_least_string_table_count_byte() {
        // A single 0x00 is a valid (empty) string table; no records follow.
        let data = [0x00u8];
        assert_eq!(decode_buildings(&data, 36.16, -86.78).unwrap(), Vec::new());
    }

    #[test]
    fn empty_string_table_and_zero_terminator_yields_no_buildings() {
        // table count 0, then a zero-length record terminator.
        let data = [0x00u8, 0x00, 0x00, 0x00, 0x00];
        assert_eq!(decode_buildings(&data, 1.0, 2.0).unwrap(), Vec::new());
    }

    #[test]
    fn truly_empty_input_errors_not_panics() {
        assert!(decode_buildings(&[], 0.0, 0.0).is_err());
    }

    // --- delta / coordinate decoding correctness ---

    #[test]
    fn coordinate_delta_decoding_is_exact() {
        // cell center (lon=2.0, lat=1.0) -> micro (200000, 100000).
        // first vertex offset (+50, -30) -> (200050, 99970).
        // deltas walk the ring and close it back to the first vertex.
        let deltas = vec![(100, 0), (0, 100), (-100, 0), (0, -100)];
        let spec = RecordSpec {
            osm_delta: 42,
            offset_lon: 50,
            offset_lat: -30,
            deltas: deltas.clone(),
            btype_idx: 0, // -> string_table[0]
        };
        let block = encode_block(&["house"], &[spec]);
        let out = decode_buildings(&block, 1.0, 2.0).unwrap();
        assert_eq!(out.len(), 1);
        let b = &out[0];

        assert_eq!(b.osm_id, 42);
        assert_eq!(b.building_type, "house");

        // Reconstruct expected coords in microdegrees.
        let mut lon = 200_000i32 + 50;
        let mut lat = 100_000i32 - 30;
        let mut expected = vec![[lon as f64 / 1e5, lat as f64 / 1e5]];
        for (dlon, dlat) in &deltas {
            lon += dlon;
            lat += dlat;
            expected.push([lon as f64 / 1e5, lat as f64 / 1e5]);
        }
        assert_eq!(b.coords.len(), 5);
        for (got, want) in b.coords.iter().zip(expected.iter()) {
            assert!((got[0] - want[0]).abs() < 1e-9);
            assert!((got[1] - want[1]).abs() < 1e-9);
        }
    }

    #[test]
    fn polygon_ring_is_closed_when_deltas_return_to_start() {
        // Deltas summing to zero => last vertex == first vertex (closed ring).
        let spec = RecordSpec {
            osm_delta: 1,
            offset_lon: 0,
            offset_lat: 0,
            deltas: vec![(10, 0), (0, 10), (-10, -10)],
            btype_idx: 0xff, // inline custom type follows... handled below
        };
        // btype_idx 0xff needs an inline string; rebuild body manually since the
        // helper only emits table-indexed types.
        let mut body = Vec::new();
        put_svarint(&mut body, spec.osm_delta);
        body.push(((spec.deltas.len() + 1 - 4) as u8) << 4);
        body.extend_from_slice(&spec.offset_lon.to_le_bytes());
        body.extend_from_slice(&spec.offset_lat.to_le_bytes());
        for (dlon, dlat) in &spec.deltas {
            put_svarint(&mut body, *dlon as i64);
            put_svarint(&mut body, *dlat as i64);
        }
        body.push(0xff);
        body.push(3);
        body.extend_from_slice(b"gym");
        body.push(0x00); // flags2

        let mut block = vec![0x00u8]; // empty string table
        block.extend_from_slice(&(body.len() as u32).to_le_bytes());
        block.extend_from_slice(&body);

        let out = decode_buildings(&block, 0.0, 0.0).unwrap();
        assert_eq!(out.len(), 1);
        let b = &out[0];
        assert_eq!(b.building_type, "gym");
        assert_eq!(b.coords.len(), 4);
        assert_eq!(
            b.coords.first(),
            b.coords.last(),
            "closed ring: first vertex must equal last"
        );
    }

    #[test]
    fn multiple_records_chain_osm_id_deltas_and_decode_independently() {
        // Two buildings; osm_id is a running sum of zigzag deltas.
        let r1 = RecordSpec {
            osm_delta: 1000,
            offset_lon: 10,
            offset_lat: 10,
            deltas: vec![(5, 0), (0, 5), (-5, -5)],
            btype_idx: 0,
        };
        let r2 = RecordSpec {
            osm_delta: -400, // osm_id = 1000 + (-400) = 600
            offset_lon: -20,
            offset_lat: 20,
            deltas: vec![(7, 0), (0, 7), (-3, -3), (-4, -4)],
            btype_idx: 1,
        };
        let block = encode_block(&["a", "b"], &[r1, r2]);
        let out = decode_buildings(&block, 5.0, 5.0).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].osm_id, 1000);
        assert_eq!(out[0].building_type, "a");
        assert_eq!(out[0].coords.len(), 4);
        assert_eq!(out[1].osm_id, 600);
        assert_eq!(out[1].building_type, "b");
        assert_eq!(out[1].coords.len(), 5);
    }

    #[test]
    fn out_of_range_building_type_index_falls_back_to_yes() {
        let spec = RecordSpec {
            osm_delta: 7,
            offset_lon: 0,
            offset_lat: 0,
            deltas: vec![(1, 1), (1, 1), (-2, -2)],
            btype_idx: 200, // no such table entry
        };
        let block = encode_block(&["only-one"], &[spec]);
        let out = decode_buildings(&block, 0.0, 0.0).unwrap();
        assert_eq!(out[0].building_type, "yes");
    }

    // --- truncation: Err, never panic ---

    #[test]
    fn record_length_overrunning_block_is_reported_not_panicked() {
        // Empty string table, then a record claiming 100 bytes with no body.
        let mut data = vec![0x00u8];
        data.extend_from_slice(&100u32.to_le_bytes());
        let err = decode_buildings(&data, 0.0, 0.0).unwrap_err();
        assert!(matches!(err, DecodeError::RecordOverrun { .. }));
    }

    #[test]
    fn truncated_record_body_does_not_panic() {
        // Valid block, then chop the final byte so the last record's declared
        // length overruns the buffer -> RecordOverrun, no panic.
        let spec = RecordSpec {
            osm_delta: 1,
            offset_lon: 0,
            offset_lat: 0,
            deltas: vec![(1, 0), (0, 1), (-1, -1)],
            btype_idx: 0,
        };
        let mut block = encode_block(&["x"], &[spec]);
        block.pop();
        let res = decode_buildings(&block, 0.0, 0.0);
        assert!(matches!(res, Err(DecodeError::RecordOverrun { .. })));
    }

    #[test]
    fn every_prefix_of_a_valid_block_returns_ok_or_err_never_panics() {
        // Fuzz-lite: decoding any truncation of a real synthetic block must
        // never panic.
        let spec = RecordSpec {
            osm_delta: 123,
            offset_lon: 3,
            offset_lat: -4,
            deltas: vec![(2, 2), (2, -2), (-4, 0)],
            btype_idx: 0,
        };
        let block = encode_block(&["shed"], &[spec]);
        for n in 0..=block.len() {
            let _ = decode_buildings(&block[..n], 1.0, 1.0);
        }
    }

    // --- version gating ---

    #[test]
    fn decode_buildings_v8_accepts_version_8() {
        let spec = RecordSpec {
            osm_delta: 5,
            offset_lon: 0,
            offset_lat: 0,
            deltas: vec![(1, 0), (0, 1), (-1, -1)],
            btype_idx: 0,
        };
        let block = encode_block(&["y"], &[spec]);
        let out = decode_buildings_v8(&block, SCHEMA_VERSION, 0.0, 0.0).unwrap();
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn decode_buildings_v8_rejects_non_v8_before_decoding() {
        // Even valid-looking v8 bytes are refused under a v6/v7 version tag.
        // v9 is now accepted (same decoder layout, new fields skipped).
        let block = [0x00u8];
        for bad in [0u8, 1, 6, 7, 255] {
            let err = decode_buildings_v8(&block, bad, 0.0, 0.0).unwrap_err();
            assert_eq!(err, BuildingsError::UnsupportedVersion { found: bad });
        }
        // v9 is accepted (returns empty block, not version error).
        assert!(decode_buildings_v8(&block, 9, 0.0, 0.0).is_ok());
    }

    #[test]
    fn decode_buildings_v8_propagates_decode_errors() {
        let err = decode_buildings_v8(&[], SCHEMA_VERSION, 0.0, 0.0).unwrap_err();
        assert!(matches!(err, BuildingsError::Decode(_)));
    }

    // --- golden fixture (std-only: reads the on-disk block) ---

    #[cfg(feature = "std")]
    #[test]
    fn decodes_golden_buildings_v8_block() {
        // Decompressed v8 block + its cell center, from
        // test-fixtures/golden/buildings_v8.{block.bin,meta.json}.
        let block = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../test-fixtures/golden/buildings_v8.block.bin"
        ))
        .expect("read golden block");
        let cell_center_lat = 36.166500290002354;
        let cell_center_lon = -86.78319797133413;

        let out = decode_buildings(&block, cell_center_lat, cell_center_lon)
            .expect("golden block decodes");

        // Sanity: the fixture holds many buildings.
        assert!(
            out.len() > 100,
            "expected many buildings, got {}",
            out.len()
        );

        // Known first record from buildings_v8.golden.json.
        let first = &out[0];
        assert_eq!(first.osm_id, 107627729);
        assert_eq!(first.building_type, "commercial");
        assert_eq!(first.name.as_deref(), Some("Music City Center"));
        assert!((first.centroid_lat - 36.15689).abs() < 1e-5);
        assert!((first.centroid_lon - (-86.77833875)).abs() < 1e-5);

        // Height, against real published bytes. This block is downtown
        // Nashville, where 149 of 1354 buildings carry one -- coverage is
        // partial everywhere, not per-state all-or-nothing, so a fixture with
        // *some* heights is the honest case to pin.
        //
        // `out.len()` is the desync canary: the height byte is the last field
        // this decoder reads, so getting its width wrong would corrupt nothing
        // visible in the record itself and instead show up as a wrong record
        // count. Assert both together or neither is load-bearing.
        assert_eq!(out.len(), 1354, "record count -- a wrong height width desyncs here");
        let with_height: Vec<f64> = out.iter().filter_map(|b| b.height_m).collect();
        assert_eq!(with_height.len(), 149, "buildings carrying a height");
        for h in &with_height {
            assert!(
                (h * 2.0).fract() == 0.0,
                "height {h} is not a multiple of 0.5 -- wrong scale factor"
            );
            assert!((2.5..=127.5).contains(h), "height {h} out of encodable range");
        }

        // Every decoded footprint is a closed ring (first vertex == last).
        for b in &out {
            assert!(b.coords.len() >= 4, "polygon needs >= 4 vertices");
            assert_eq!(
                b.coords.first(),
                b.coords.last(),
                "building {} ring not closed",
                b.osm_id
            );
        }
    }
}
