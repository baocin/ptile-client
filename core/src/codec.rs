//! varint, zigzag, length-prefixed string, and indexed-value decoding shared by
//! all block decoders. Semantics cross-checked against
//! `~/kino/projects/ptiles/ptiles/codec.py` (reference impl) and SPEC.md.
//! Every read here is bounds-checked — this is fuzz-target code, no panics
//! on truncated input.

use alloc::string::String;
use alloc::vec::Vec;

/// Error type for all decode operations in ptiles-core. `no_std`-compatible.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    #[error("unexpected end of input at offset {offset} (needed {needed} more bytes)")]
    UnexpectedEof { offset: usize, needed: usize },
    #[error("varint at offset {offset} did not terminate within input")]
    VarintOverrun { offset: usize },
    #[error("record length {len} at offset {offset} overruns block of length {block_len}")]
    RecordOverrun {
        offset: usize,
        len: usize,
        block_len: usize,
    },
    /// A section announced itself with a known magic but a version this build
    /// does not parse. Fails closed rather than reading it as the version it
    /// happens to know: a coarse index read at the wrong version yields
    /// brackets pointing at the wrong entries, which surfaces as "cell not in
    /// this file" -- the same silent-empty result as every other index bug
    /// this format has had.
    #[error("{section} version {found} is not supported (this build reads {supported})")]
    UnsupportedSectionVersion {
        section: &'static str,
        found: u8,
        supported: u8,
    },
    /// A record decoded to a position that cannot exist on Earth. The bytes
    /// parsed cleanly, so this is not a truncation: it means the slice being
    /// read is not the layer it was taken for. Reported rather than returned
    /// as a record, because a caller cannot tell `lat = 167.9` from real data
    /// once it is inside a `Vec`.
    /// Microdegrees rather than degrees so the enum can stay `Eq` (`f64`
    /// cannot). Divide by 100_000 to read it.
    #[error("coordinate ({lat_micro}, {lon_micro}) microdegrees at offset {offset} is not on Earth")]
    CoordOutOfRange {
        offset: usize,
        lat_micro: i32,
        lon_micro: i32,
    },
}

/// Convert a microdegree lon/lat pair to degrees, rejecting anything off the
/// globe. Point layers (signals, cameras) have no length prefix and no other
/// structural check, so this is the only thing standing between a mis-sliced
/// block and a plausible-looking record.
pub fn coord_from_micro(
    lon_micro: i32,
    lat_micro: i32,
    offset: usize,
) -> Result<(f64, f64), DecodeError> {
    let lon = lon_micro as f64 / 100_000.0;
    let lat = lat_micro as f64 / 100_000.0;
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return Err(DecodeError::CoordOutOfRange {
            offset,
            lat_micro,
            lon_micro,
        });
    }
    Ok((lon, lat))
}

/// Ensure `data` has at least `needed` bytes available starting at `offset`.
#[inline]
fn need(data: &[u8], offset: usize, needed: usize) -> Result<(), DecodeError> {
    match offset.checked_add(needed) {
        Some(end) if end <= data.len() => Ok(()),
        _ => Err(DecodeError::UnexpectedEof { offset, needed }),
    }
}

/// Read a little-endian u8 at `offset`.
pub fn read_u8(data: &[u8], offset: usize) -> Result<u8, DecodeError> {
    need(data, offset, 1)?;
    Ok(data[offset])
}

/// Read a little-endian u16 at `offset`.
pub fn read_u16(data: &[u8], offset: usize) -> Result<u16, DecodeError> {
    need(data, offset, 2)?;
    Ok(u16::from_le_bytes(
        data[offset..offset + 2].try_into().unwrap(),
    ))
}

/// Read a little-endian i16 at `offset`.
pub fn read_i16(data: &[u8], offset: usize) -> Result<i16, DecodeError> {
    need(data, offset, 2)?;
    Ok(i16::from_le_bytes(
        data[offset..offset + 2].try_into().unwrap(),
    ))
}

/// Read a little-endian u32 at `offset`.
pub fn read_u32(data: &[u8], offset: usize) -> Result<u32, DecodeError> {
    need(data, offset, 4)?;
    Ok(u32::from_le_bytes(
        data[offset..offset + 4].try_into().unwrap(),
    ))
}

/// Read a little-endian i32 at `offset`.
pub fn read_i32(data: &[u8], offset: usize) -> Result<i32, DecodeError> {
    need(data, offset, 4)?;
    Ok(i32::from_le_bytes(
        data[offset..offset + 4].try_into().unwrap(),
    ))
}

/// Read a little-endian u64 at `offset`.
pub fn read_u64(data: &[u8], offset: usize) -> Result<u64, DecodeError> {
    need(data, offset, 8)?;
    Ok(u64::from_le_bytes(
        data[offset..offset + 8].try_into().unwrap(),
    ))
}

/// Decode a protobuf-style unsigned varint (LEB128, 7 bits/byte, MSB = continuation).
/// Returns `(value, bytes_consumed)`.
pub fn decode_varint(data: &[u8], pos: usize) -> Result<(u64, usize), DecodeError> {
    let mut result: u64 = 0;
    let mut shift: u32 = 0;
    let mut p = pos;
    loop {
        if p >= data.len() {
            return Err(DecodeError::VarintOverrun { offset: pos });
        }
        let b = data[p];
        p += 1;
        if shift < 64 {
            result |= ((b & 0x7f) as u64) << shift;
        }
        if b & 0x80 == 0 {
            break;
        }
        shift += 7;
        // protobuf varints for u64 need at most 10 bytes; bail out rather than
        // spin forever on adversarial input with the continuation bit always set.
        if shift >= 70 {
            return Err(DecodeError::VarintOverrun { offset: pos });
        }
    }
    Ok((result, p - pos))
}

/// Zigzag-decode a varint value to a signed 64-bit integer.
#[inline]
pub fn zigzag_decode(n: u64) -> i64 {
    ((n >> 1) as i64) ^ -((n & 1) as i64)
}

/// Zigzag-decode a varint value to a signed 32-bit integer (coordinate deltas).
#[inline]
pub fn zigzag_decode_i32(n: u64) -> i32 {
    let n = n as u32;
    ((n >> 1) as i32) ^ -((n & 1) as i32)
}

/// Decode a UTF-8 string of `len` bytes at `offset`. Lossy-replaces invalid
/// sequences rather than failing the whole block, matching the seed's
/// `from_utf8_lossy` behavior (Python ref uses `errors="replace"` for park
/// types and strict `decode("utf-8")` elsewhere; lossy is the safe superset
/// for untrusted/fuzzed input).
pub fn decode_string(data: &[u8], offset: usize, len: usize) -> Result<String, DecodeError> {
    need(data, offset, len)?;
    Ok(String::from_utf8_lossy(&data[offset..offset + len]).into_owned())
}

/// Decode a uint8-length-prefixed string. Returns `(string, bytes_consumed)`.
pub fn decode_string_u8(data: &[u8], pos: usize) -> Result<(String, usize), DecodeError> {
    let len = read_u8(data, pos)? as usize;
    let s = decode_string(data, pos + 1, len)?;
    Ok((s, 1 + len))
}

/// Decode a uint16-length-prefixed string. Returns `(string, bytes_consumed)`.
pub fn decode_string_u16(data: &[u8], pos: usize) -> Result<(String, usize), DecodeError> {
    let len = read_u16(data, pos)? as usize;
    let s = decode_string(data, pos + 2, len)?;
    Ok((s, 2 + len))
}

/// Decode a delta-encoded coordinate sequence: `vertex_count` (lon, lat) pairs
/// in degrees, given the first absolute vertex in microdegrees and the
/// remainder as zigzag-varint deltas. Returns `(coords, bytes_consumed)`.
/// Bounds-checked throughout — truncated input yields `Err`, never a panic.
pub fn decode_coordinates(
    data: &[u8],
    pos: usize,
    first_lon_micro: i32,
    first_lat_micro: i32,
    vertex_count: usize,
) -> Result<(Vec<[f64; 2]>, usize), DecodeError> {
    let mut coords = Vec::with_capacity(vertex_count.min(1 << 16));
    coords.push([
        first_lon_micro as f64 / 100_000.0,
        first_lat_micro as f64 / 100_000.0,
    ]);
    let mut prev_lon = first_lon_micro;
    let mut prev_lat = first_lat_micro;
    let start = pos;
    let mut p = pos;
    for _ in 1..vertex_count {
        let (dlon_raw, c1) = decode_varint(data, p)?;
        p += c1;
        let (dlat_raw, c2) = decode_varint(data, p)?;
        p += c2;
        prev_lon = prev_lon.wrapping_add(zigzag_decode_i32(dlon_raw));
        prev_lat = prev_lat.wrapping_add(zigzag_decode_i32(dlat_raw));
        coords.push([prev_lon as f64 / 100_000.0, prev_lat as f64 / 100_000.0]);
    }
    Ok((coords, p - start))
}

/// Decode a byte that is either a table index (< 255) into `reverse_index`,
/// or `255` followed by a uint8-length-prefixed custom string. Mirrors
/// `ptiles.codec.decode_indexed_or_custom`. Returns `(value, bytes_consumed)`.
pub fn decode_indexed_or_custom(
    data: &[u8],
    pos: usize,
    reverse_index: &[(u8, &str)],
) -> Result<(String, usize), DecodeError> {
    let idx = read_u8(data, pos)?;
    if idx == 255 {
        let (s, consumed) = decode_string_u8(data, pos + 1)?;
        Ok((s, 1 + consumed))
    } else {
        let s = reverse_index
            .iter()
            .find(|(i, _)| *i == idx)
            .map(|(_, s)| String::from(*s))
            .unwrap_or_else(|| String::from("unknown"));
        Ok((s, 1))
    }
}

/// Decode a per-block deduplicated string table (v8 buildings format):
/// a uint8 count followed by that many uint8-length-prefixed strings.
/// Returns `(table, bytes_consumed)`.
pub fn decode_string_table(data: &[u8], pos: usize) -> Result<(Vec<String>, usize), DecodeError> {
    let count = read_u8(data, pos)? as usize;
    let mut p = pos + 1;
    let mut table = Vec::with_capacity(count);
    for _ in 0..count {
        let (s, consumed) = decode_string_u8(data, p)?;
        table.push(s);
        p += consumed;
    }
    Ok((table, p - pos))
}

/// Decode a table-referenced string: a uint8 index into `table`, or `0xff`
/// followed by a uint8-length-prefixed inline string. Returns
/// `(value, bytes_consumed)`. An out-of-range index yields an empty string
/// (matches `ptiles.codec.decode_table_ref`), not an error — corrupt tables
/// should degrade gracefully rather than abort the whole block.
pub fn decode_table_ref(
    data: &[u8],
    pos: usize,
    table: &[String],
) -> Result<(String, usize), DecodeError> {
    let idx = read_u8(data, pos)?;
    if idx == 0xff {
        let (s, consumed) = decode_string_u8(data, pos + 1)?;
        Ok((s, 1 + consumed))
    } else if (idx as usize) < table.len() {
        Ok((table[idx as usize].clone(), 1))
    } else {
        Ok((String::new(), 1))
    }
}

/// Reverse lookup tables shared by decoders (from `ptiles.codec`).
pub mod tables {
    pub const ROAD_CLASS_REVERSE: &[(u8, &str)] = &[
        (0, "motorway"),
        (1, "motorway_link"),
        (2, "trunk"),
        (3, "trunk_link"),
        (4, "primary"),
        (5, "primary_link"),
        (6, "secondary"),
        (7, "tertiary"),
        (8, "residential"),
        (9, "service"),
        (10, "track"),
        (11, "footway"),
        (12, "cycleway"),
        (13, "path"),
        (14, "pedestrian"),
        (15, "tertiary_link"),
    ];

    pub const SURFACE_REVERSE: &[(u8, &str)] = &[
        (0, "paved"),
        (1, "asphalt"),
        (2, "concrete"),
        (3, "unpaved"),
        (4, "gravel"),
        (5, "dirt"),
        (6, "sand"),
        (7, "grass"),
    ];

    pub const WATER_TYPES: &[&str] = &[
        "lake",
        "reservoir",
        "pond",
        "river",
        "stream",
        "creek",
        "canal",
        "drain",
        "bay",
        "ocean",
        "wetland",
        "marsh",
        "swamp",
        "estuary",
    ];

    pub const TRAIL_TYPE_REVERSE: &[(u8, &str)] = &[
        (0, "path"),
        (1, "track"),
        (2, "bridleway"),
        (3, "cycleway"),
        (4, "footway"),
        (5, "steps"),
        (6, "trailhead"),
    ];

    /// Trails carry their own surface table, not `SURFACE_REVERSE`: the trail
    /// builder reserves index 0 for "unset" and lists surfaces that matter
    /// off-road (compacted, boardwalk, ground). Decoding trails against the
    /// road table would shift every value by one and rename the rest.
    pub const TRAIL_SURFACE_REVERSE: &[(u8, &str)] = &[
        (0, ""),
        (1, "paved"),
        (2, "asphalt"),
        (3, "concrete"),
        (4, "gravel"),
        (5, "compacted"),
        (6, "fine_gravel"),
        (7, "dirt"),
        (8, "ground"),
        (9, "grass"),
        (10, "sand"),
        (11, "wood"),
        (12, "boardwalk"),
    ];

    pub const SAC_SCALE_REVERSE: &[(u8, &str)] = &[
        (0, ""),
        (1, "hiking"),
        (2, "mountain_hiking"),
        (3, "demanding_mountain_hiking"),
        (4, "alpine_hiking"),
        (5, "demanding_alpine_hiking"),
        (6, "difficult_alpine_hiking"),
    ];

    pub const RAIL_TYPE_REVERSE: &[(u8, &str)] = &[
        (0, "rail"),
        (1, "subway"),
        (2, "light_rail"),
        (3, "tram"),
        (4, "monorail"),
        (5, "narrow_gauge"),
        (6, "funicular"),
        (7, "station"),
        (8, "halt"),
        (9, "tram_stop"),
        (10, "subway_entrance"),
    ];
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: encode an unsigned varint (mirror of the decoder, for round-trips).
    fn encode_varint(mut v: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let mut byte = (v & 0x7f) as u8;
            v >>= 7;
            if v != 0 {
                byte |= 0x80;
            }
            out.push(byte);
            if v == 0 {
                break;
            }
        }
        out
    }

    fn encode_zigzag_i32(n: i32) -> u64 {
        ((n << 1) ^ (n >> 31)) as u32 as u64
    }

    // ---- fixed-width readers -------------------------------------------------

    #[test]
    fn read_fixed_widths_happy() {
        let data = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08];
        assert_eq!(read_u8(&data, 0).unwrap(), 0x01);
        assert_eq!(read_u16(&data, 0).unwrap(), 0x0201);
        assert_eq!(read_i16(&data, 0).unwrap(), 0x0201);
        assert_eq!(read_u32(&data, 0).unwrap(), 0x04030201);
        assert_eq!(read_i32(&data, 0).unwrap(), 0x04030201);
        assert_eq!(read_u64(&data, 0).unwrap(), 0x0807060504030201);
    }

    #[test]
    fn read_fixed_widths_signed_negative() {
        let data = [0xff, 0xff, 0xff, 0xff];
        assert_eq!(read_i16(&data, 0).unwrap(), -1);
        assert_eq!(read_i32(&data, 0).unwrap(), -1);
    }

    #[test]
    fn read_at_exact_boundary_ok() {
        let data = [0xaa, 0xbb];
        assert_eq!(read_u16(&data, 0).unwrap(), 0xbbaa);
        // reading a u8 at the last valid index
        assert_eq!(read_u8(&data, 1).unwrap(), 0xbb);
    }

    #[test]
    fn read_truncated_and_empty_error_not_panic() {
        let data = [0x01, 0x02, 0x03];
        assert!(read_u8(&data, 3).is_err());
        assert!(read_u16(&data, 2).is_err());
        assert!(read_u32(&data, 0).is_err());
        assert!(read_u64(&data, 0).is_err());
        let empty: [u8; 0] = [];
        assert!(read_u8(&empty, 0).is_err());
        assert!(read_u16(&empty, 0).is_err());
    }

    #[test]
    fn need_offset_overflow_is_error_not_panic() {
        let data = [0u8; 4];
        // offset + needed overflows usize -> checked_add None -> Err, no panic
        assert!(read_u16(&data, usize::MAX).is_err());
        assert!(read_u8(&data, usize::MAX).is_err());
    }

    // ---- varint --------------------------------------------------------------

    #[test]
    fn varint_single_byte() {
        for v in [0u64, 1, 63, 127] {
            let enc = encode_varint(v);
            assert_eq!(enc.len(), 1);
            let (got, c) = decode_varint(&enc, 0).unwrap();
            assert_eq!((got, c), (v, 1));
        }
    }

    #[test]
    fn varint_multi_byte_roundtrip() {
        // 300 = 0xAC 0x02, classic protobuf example
        let data = [0xACu8, 0x02];
        assert_eq!(decode_varint(&data, 0).unwrap(), (300, 2));
        for v in [128u64, 16384, 1 << 20, 1 << 35, u32::MAX as u64] {
            let enc = encode_varint(v);
            assert_eq!(decode_varint(&enc, 0).unwrap(), (v, enc.len()));
        }
    }

    #[test]
    fn varint_max_u64_is_ten_bytes() {
        let enc = encode_varint(u64::MAX);
        assert_eq!(enc.len(), 10);
        let (v, c) = decode_varint(&enc, 0).unwrap();
        assert_eq!(v, u64::MAX);
        assert_eq!(c, 10);
    }

    #[test]
    fn varint_with_offset_consumes_from_pos() {
        let data = [0xff, 0xAC, 0x02]; // junk byte, then 300
        let (v, c) = decode_varint(&data, 1).unwrap();
        assert_eq!((v, c), (300, 2));
    }

    #[test]
    fn varint_truncated_errors_not_panics() {
        let data = [0x80u8, 0x80, 0x80];
        assert!(matches!(
            decode_varint(&data, 0),
            Err(DecodeError::VarintOverrun { offset: 0 })
        ));
    }

    #[test]
    fn varint_empty_input_is_error() {
        let empty: [u8; 0] = [];
        assert!(decode_varint(&empty, 0).is_err());
        // pos past end
        assert!(decode_varint(&[0x01], 5).is_err());
    }

    #[test]
    fn varint_all_continuation_bits_overruns() {
        // 11 bytes all with continuation bit set -> must bail (shift guard),
        // never spin or panic.
        let data = [0x80u8; 11];
        assert!(matches!(
            decode_varint(&data, 0),
            Err(DecodeError::VarintOverrun { .. })
        ));
    }

    // ---- zigzag --------------------------------------------------------------

    #[test]
    fn zigzag_matches_python_semantics() {
        assert_eq!(zigzag_decode(0), 0);
        assert_eq!(zigzag_decode(1), -1);
        assert_eq!(zigzag_decode(2), 1);
        assert_eq!(zigzag_decode(3), -2);
    }

    #[test]
    fn zigzag_i64_extremes_roundtrip() {
        // encode(n) = (n << 1) ^ (n >> 63)
        let enc = |n: i64| ((n << 1) ^ (n >> 63)) as u64;
        for n in [0i64, 1, -1, i64::MAX, i64::MIN, 123456789, -123456789] {
            assert_eq!(zigzag_decode(enc(n)), n);
        }
    }

    #[test]
    fn zigzag_i32_extremes_roundtrip() {
        for n in [0i32, 1, -1, i32::MAX, i32::MIN, 1_000_000, -1_000_000] {
            assert_eq!(zigzag_decode_i32(encode_zigzag_i32(n)), n);
        }
    }

    // ---- strings -------------------------------------------------------------

    #[test]
    fn decode_string_happy_and_empty() {
        let data = b"hello";
        assert_eq!(decode_string(data, 0, 5).unwrap(), "hello");
        assert_eq!(decode_string(data, 0, 0).unwrap(), "");
        assert_eq!(decode_string(data, 2, 3).unwrap(), "llo");
    }

    #[test]
    fn decode_string_invalid_utf8_is_lossy_not_error() {
        let data = [0xff, 0xfe, b'a'];
        let s = decode_string(&data, 0, 3).unwrap();
        assert!(s.contains('a'));
        assert!(s.contains('\u{fffd}')); // replacement char, no error/panic
    }

    #[test]
    fn decode_string_truncated_is_error() {
        let data = b"hi";
        assert!(decode_string(data, 0, 5).is_err());
        assert!(decode_string(data, 1, 2).is_err());
    }

    #[test]
    fn string_u8_roundtrip_and_zero_len() {
        let data = [0x02, b'h', b'i', b'X'];
        let (s, c) = decode_string_u8(&data, 0).unwrap();
        assert_eq!((s.as_str(), c), ("hi", 3));
        let zero = [0x00u8];
        assert_eq!(decode_string_u8(&zero, 0).unwrap(), (String::new(), 1));
    }

    #[test]
    fn string_u8_truncated_is_error() {
        let data = [0x05, b'h', b'i'];
        assert!(decode_string_u8(&data, 0).is_err());
        // length byte itself missing
        let empty: [u8; 0] = [];
        assert!(decode_string_u8(&empty, 0).is_err());
    }

    #[test]
    fn string_u16_roundtrip() {
        let data = [0x03, 0x00, b'a', b'b', b'c'];
        let (s, c) = decode_string_u16(&data, 0).unwrap();
        assert_eq!((s.as_str(), c), ("abc", 5));
    }

    #[test]
    fn string_u16_truncated_is_error() {
        let data = [0x05, 0x00, b'h', b'i'];
        assert!(decode_string_u16(&data, 0).is_err());
        // length prefix itself truncated
        assert!(decode_string_u16(&[0x01], 0).is_err());
    }

    // ---- coordinates ---------------------------------------------------------

    #[test]
    fn coordinates_single_vertex_consumes_nothing() {
        let data: [u8; 0] = [];
        let (coords, c) = decode_coordinates(&data, 0, 100_000, 200_000, 1).unwrap();
        assert_eq!(c, 0);
        assert_eq!(coords, vec![[1.0, 2.0]]);
    }

    #[test]
    fn coordinates_multi_vertex_delta_decoding() {
        // first vertex (1.0, 2.0); one delta of (+2 micro, -3 micro) zigzag
        let mut data = Vec::new();
        data.extend(encode_varint(encode_zigzag_i32(2)));
        data.extend(encode_varint(encode_zigzag_i32(-3)));
        let (coords, c) = decode_coordinates(&data, 0, 100_000, 200_000, 2).unwrap();
        assert_eq!(c, data.len());
        assert_eq!(coords.len(), 2);
        assert_eq!(coords[0], [1.0, 2.0]);
        // 100_002 / 100_000, 199_997 / 100_000
        assert!((coords[1][0] - 1.00002).abs() < 1e-9);
        assert!((coords[1][1] - 1.99997).abs() < 1e-9);
    }

    #[test]
    fn coordinates_truncated_delta_is_error() {
        // claims 3 vertices but no delta bytes present
        let data: [u8; 0] = [];
        assert!(decode_coordinates(&data, 0, 0, 0, 3).is_err());
        // one full delta then truncation on the second vertex's lat
        let mut data = Vec::new();
        data.extend(encode_varint(encode_zigzag_i32(1)));
        data.extend(encode_varint(encode_zigzag_i32(1)));
        data.extend(encode_varint(encode_zigzag_i32(1))); // dlon of vertex 3, dlat missing
        assert!(decode_coordinates(&data, 0, 0, 0, 3).is_err());
    }

    #[test]
    fn coordinates_negative_wraparound_does_not_panic() {
        // large negative delta near i32::MIN uses wrapping_add, must not panic
        let mut data = Vec::new();
        data.extend(encode_varint(encode_zigzag_i32(i32::MIN)));
        data.extend(encode_varint(encode_zigzag_i32(i32::MIN)));
        let res = decode_coordinates(&data, 0, i32::MIN, i32::MIN, 2);
        assert!(res.is_ok());
    }

    // ---- indexed_or_custom ---------------------------------------------------

    #[test]
    fn indexed_or_custom_table_hit() {
        let table: &[(u8, &str)] = &[(0, "motorway"), (4, "primary")];
        let (s, c) = decode_indexed_or_custom(&[4u8], 0, table).unwrap();
        assert_eq!((s.as_str(), c), ("primary", 1));
    }

    #[test]
    fn indexed_or_custom_unknown_index() {
        let table: &[(u8, &str)] = &[(0, "motorway")];
        let (s, c) = decode_indexed_or_custom(&[9u8], 0, table).unwrap();
        assert_eq!((s.as_str(), c), ("unknown", 1));
    }

    #[test]
    fn indexed_or_custom_inline_255() {
        // 255 marker, then u8-len-prefixed "hi"
        let data = [255u8, 0x02, b'h', b'i'];
        let (s, c) = decode_indexed_or_custom(&data, 0, &[]).unwrap();
        assert_eq!((s.as_str(), c), ("hi", 4));
    }

    #[test]
    fn indexed_or_custom_truncated_inline_is_error() {
        let data = [255u8, 0x05, b'h'];
        assert!(decode_indexed_or_custom(&data, 0, &[]).is_err());
        // marker byte missing entirely
        let empty: [u8; 0] = [];
        assert!(decode_indexed_or_custom(&empty, 0, &[]).is_err());
    }

    // ---- string table --------------------------------------------------------

    #[test]
    fn string_table_happy() {
        // count=2, "ab", "c"
        let data = [0x02, 0x02, b'a', b'b', 0x01, b'c'];
        let (table, c) = decode_string_table(&data, 0).unwrap();
        assert_eq!(table, vec![String::from("ab"), String::from("c")]);
        assert_eq!(c, data.len());
    }

    #[test]
    fn string_table_empty_count() {
        let data = [0x00u8, 0xff];
        let (table, c) = decode_string_table(&data, 0).unwrap();
        assert!(table.is_empty());
        assert_eq!(c, 1);
    }

    #[test]
    fn string_table_truncated_is_error() {
        // count=3 but only one entry present
        let data = [0x03, 0x01, b'a'];
        assert!(decode_string_table(&data, 0).is_err());
        let empty: [u8; 0] = [];
        assert!(decode_string_table(&empty, 0).is_err());
    }

    // ---- table_ref -----------------------------------------------------------

    #[test]
    fn table_ref_index_hit() {
        let table = vec![String::from("a"), String::from("b")];
        let (s, c) = decode_table_ref(&[1u8], 0, &table).unwrap();
        assert_eq!((s.as_str(), c), ("b", 1));
    }

    #[test]
    fn table_ref_out_of_range_yields_empty_not_error() {
        let table = vec![String::from("a")];
        let (s, c) = decode_table_ref(&[9u8], 0, &table).unwrap();
        assert_eq!((s.as_str(), c), ("", 1));
        // empty table, any non-0xff index -> empty
        let (s2, _) = decode_table_ref(&[0u8], 0, &[]).unwrap();
        assert_eq!(s2, "");
    }

    #[test]
    fn table_ref_inline_0xff() {
        let data = [0xffu8, 0x02, b'h', b'i'];
        let (s, c) = decode_table_ref(&data, 0, &[]).unwrap();
        assert_eq!((s.as_str(), c), ("hi", 4));
    }

    #[test]
    fn table_ref_truncated_inline_is_error() {
        let data = [0xffu8, 0x09, b'h'];
        assert!(decode_table_ref(&data, 0, &[]).is_err());
        let empty: [u8; 0] = [];
        assert!(decode_table_ref(&empty, 0, &[]).is_err());
    }
}
