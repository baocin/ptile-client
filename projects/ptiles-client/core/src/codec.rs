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
    Ok(u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap()))
}

/// Read a little-endian i16 at `offset`.
pub fn read_i16(data: &[u8], offset: usize) -> Result<i16, DecodeError> {
    need(data, offset, 2)?;
    Ok(i16::from_le_bytes(data[offset..offset + 2].try_into().unwrap()))
}

/// Read a little-endian u32 at `offset`.
pub fn read_u32(data: &[u8], offset: usize) -> Result<u32, DecodeError> {
    need(data, offset, 4)?;
    Ok(u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()))
}

/// Read a little-endian i32 at `offset`.
pub fn read_i32(data: &[u8], offset: usize) -> Result<i32, DecodeError> {
    need(data, offset, 4)?;
    Ok(i32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()))
}

/// Read a little-endian u64 at `offset`.
pub fn read_u64(data: &[u8], offset: usize) -> Result<u64, DecodeError> {
    need(data, offset, 8)?;
    Ok(u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap()))
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
        "lake", "reservoir", "pond", "river", "stream", "creek", "canal", "drain", "bay",
        "ocean", "wetland", "marsh", "swamp", "estuary",
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

    #[test]
    fn varint_roundtrip() {
        // 300 = 0b100101100 -> low7=0101100|0x80=0xAC, next=0b10=0x02
        let data = [0xACu8, 0x02];
        let (v, c) = decode_varint(&data, 0).unwrap();
        assert_eq!(v, 300);
        assert_eq!(c, 2);
    }

    #[test]
    fn varint_truncated_errors_not_panics() {
        let data = [0x80u8, 0x80, 0x80];
        assert!(decode_varint(&data, 0).is_err());
    }

    #[test]
    fn zigzag_matches_python_semantics() {
        assert_eq!(zigzag_decode(0), 0);
        assert_eq!(zigzag_decode(1), -1);
        assert_eq!(zigzag_decode(2), 1);
        assert_eq!(zigzag_decode(3), -2);
    }

    #[test]
    fn string_u16_truncated_is_error() {
        let data = [0x05, 0x00, b'h', b'i'];
        assert!(decode_string_u16(&data, 0).is_err());
    }
}
