//! Cell index parse + binary search over H3 cell -> block offset/length entries.
//! Format: SPEC.md "Spatial Index" section — `entry_count: u32` followed by
//! that many 19-byte entries (`h3_cell: u64`, `block_offset: 6-byte LE`,
//! `block_length: 3-byte LE`, `feature_count: u16`), sorted by `h3_cell` for
//! binary search. Cross-checked against `ptiles/codec.py::read_index` /
//! `decode_index_entry` / `INDEX_ENTRY_SIZE = 19`.
//!
//! Note: some real-world files (observed in `TN.rail.ptiles`, `TN.parks.ptiles`,
//! `TN.places.ptiles`) use an undocumented v2 38-byte "merged block" index
//! format (`ptiles/codec.py::decode_index_v2`) not covered by SPEC.md. This
//! module implements only the SPEC.md v1 format; v1 is what the large
//! per-cell-feature layers (roads, water, buildings_v8, business) actually
//! use in practice — verified against real `TN.*.ptiles` files during this
//! task. v2 support is out of scope here (flagged for a follow-up task).

use alloc::vec::Vec;

use crate::codec::DecodeError;

/// One entry in the spatial index: an H3 res-7 cell mapped to the byte range
/// of its (still-compressed) data block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct IndexEntry {
    pub h3_cell: u64,
    /// Absolute byte offset of the compressed block in the file.
    pub block_offset: u64,
    /// Compressed block size in bytes.
    pub block_length: u32,
    pub feature_count: u16,
}

const ENTRY_SIZE: usize = 19;

fn read_uint_le(data: &[u8], offset: usize, len: usize) -> u64 {
    let mut v: u64 = 0;
    for i in 0..len {
        v |= (data[offset + i] as u64) << (8 * i);
    }
    v
}

/// Parse the spatial index section (v1, 19-byte entries) into a `Vec<IndexEntry>`
/// sorted by `h3_cell` (as stored on disk — the format guarantees this, we don't
/// re-sort). Bounds-checked; truncated input yields `Err`, never a panic.
pub fn parse_index(data: &[u8]) -> Result<Vec<IndexEntry>, DecodeError> {
    if data.len() < 4 {
        return Err(DecodeError::UnexpectedEof {
            offset: 0,
            needed: 4,
        });
    }
    let entry_count = u32::from_le_bytes(data[0..4].try_into().unwrap()) as usize;

    // `entry_count` is attacker/corruption-controlled: compute the required
    // size with checked arithmetic so a huge count can't overflow `usize`
    // (wrapping to a small `needed` that then passes the length check and
    // reads out of bounds), and validate the length *before* the
    // `with_capacity` below so a bogus count can't trigger a giant allocation.
    let needed = entry_count
        .checked_mul(ENTRY_SIZE)
        .and_then(|body| body.checked_add(4))
        .ok_or(DecodeError::UnexpectedEof {
            offset: 0,
            needed: usize::MAX,
        })?;
    if data.len() < needed {
        return Err(DecodeError::UnexpectedEof {
            offset: data.len(),
            needed: needed - data.len(),
        });
    }

    let mut entries = Vec::with_capacity(entry_count);
    let mut pos = 4;
    for _ in 0..entry_count {
        let h3_cell = u64::from_le_bytes(data[pos..pos + 8].try_into().unwrap());
        let block_offset = read_uint_le(data, pos + 8, 6);
        let block_length = read_uint_le(data, pos + 14, 3) as u32;
        let feature_count = u16::from_le_bytes(data[pos + 17..pos + 19].try_into().unwrap());
        entries.push(IndexEntry {
            h3_cell,
            block_offset,
            block_length,
            feature_count,
        });
        pos += ENTRY_SIZE;
    }

    Ok(entries)
}

/// Binary search a (h3_cell-sorted) index for an exact cell match.
pub fn binary_search(index: &[IndexEntry], cell: u64) -> Option<&IndexEntry> {
    index
        .binary_search_by_key(&cell, |e| e.h3_cell)
        .ok()
        .map(|i| &index[i])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_entry(
        h3_cell: u64,
        block_offset: u64,
        block_length: u32,
        feature_count: u16,
    ) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&h3_cell.to_le_bytes());
        buf.extend_from_slice(&block_offset.to_le_bytes()[0..6]);
        buf.extend_from_slice(&block_length.to_le_bytes()[0..3]);
        buf.extend_from_slice(&feature_count.to_le_bytes());
        buf
    }

    #[test]
    fn parses_and_binary_searches() {
        let mut data = Vec::new();
        data.extend_from_slice(&2u32.to_le_bytes()); // entry_count
        data.extend_from_slice(&encode_entry(100, 1000, 50, 3));
        data.extend_from_slice(&encode_entry(200, 1050, 60, 5));

        let index = parse_index(&data).unwrap();
        assert_eq!(index.len(), 2);
        assert_eq!(binary_search(&index, 200).unwrap().block_offset, 1050);
        assert!(binary_search(&index, 999).is_none());
    }

    #[test]
    fn truncated_index_is_error() {
        let mut data = Vec::new();
        data.extend_from_slice(&5u32.to_le_bytes()); // claims 5 entries
        data.extend_from_slice(&encode_entry(100, 1000, 50, 3)); // only 1 present
        assert!(parse_index(&data).is_err());
    }

    #[test]
    fn empty_index_parses_to_zero_entries() {
        let data = 0u32.to_le_bytes(); // count 0, no entries
        let index = parse_index(&data).unwrap();
        assert!(index.is_empty());
        // Any lookup on an empty index misses, never panics.
        assert!(binary_search(&index, 0).is_none());
        assert!(binary_search(&index, u64::MAX).is_none());
    }

    #[test]
    fn too_short_for_count_header_is_error() {
        // Fewer than the 4 bytes needed even to read entry_count.
        assert!(parse_index(&[]).is_err());
        assert!(parse_index(&[0u8, 0, 0]).is_err());
    }

    #[test]
    fn count_header_present_but_no_entry_bytes_is_error() {
        let data = 1u32.to_le_bytes(); // claims 1 entry, zero entry bytes follow
        assert!(parse_index(&data).is_err());
    }

    #[test]
    fn huge_entry_count_does_not_overflow_or_over_allocate() {
        // entry_count = u32::MAX: `count * 19` overflows 32-bit usize if
        // computed unchecked. Must be a clean Err, never a panic/OOM.
        let data = u32::MAX.to_le_bytes();
        assert!(parse_index(&data).is_err());
    }

    #[test]
    fn all_fields_round_trip_including_max_widths() {
        // Exercise the 6-byte offset and 3-byte length decoders at their
        // full stored widths (values that fill every byte they occupy).
        let off_6b: u64 = (1u64 << 48) - 1; // max 6-byte LE value
        let len_3b: u32 = (1u32 << 24) - 1; // max 3-byte LE value
        let mut data = Vec::new();
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(&encode_entry(
            0xDEAD_BEEF_CAFE_F00D,
            off_6b,
            len_3b,
            u16::MAX,
        ));
        let index = parse_index(&data).unwrap();
        assert_eq!(index.len(), 1);
        let e = index[0];
        assert_eq!(e.h3_cell, 0xDEAD_BEEF_CAFE_F00D);
        assert_eq!(e.block_offset, off_6b);
        assert_eq!(e.block_length, len_3b);
        assert_eq!(e.feature_count, u16::MAX);
    }

    #[test]
    fn binary_search_hits_first_last_and_misses_between_and_outside() {
        let mut data = Vec::new();
        data.extend_from_slice(&3u32.to_le_bytes());
        data.extend_from_slice(&encode_entry(10, 100, 5, 1));
        data.extend_from_slice(&encode_entry(20, 200, 6, 2));
        data.extend_from_slice(&encode_entry(30, 300, 7, 3));
        let index = parse_index(&data).unwrap();

        assert_eq!(binary_search(&index, 10).unwrap().block_offset, 100); // first
        assert_eq!(binary_search(&index, 30).unwrap().block_offset, 300); // last
        assert_eq!(binary_search(&index, 20).unwrap().feature_count, 2); // middle
        assert!(binary_search(&index, 15).is_none()); // gap between
        assert!(binary_search(&index, 5).is_none()); // below range
        assert!(binary_search(&index, 40).is_none()); // above range
    }

    #[test]
    fn trailing_bytes_after_declared_entries_are_ignored() {
        // A file whose index section is followed by more data must parse
        // exactly `entry_count` entries and not choke on the extra bytes.
        let mut data = Vec::new();
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(&encode_entry(42, 500, 9, 4));
        data.extend_from_slice(&[0xFF; 32]); // trailing junk
        let index = parse_index(&data).unwrap();
        assert_eq!(index.len(), 1);
        assert_eq!(binary_search(&index, 42).unwrap().block_offset, 500);
    }
}
