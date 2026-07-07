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

    let needed = 4 + entry_count * ENTRY_SIZE;
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

    fn encode_entry(h3_cell: u64, block_offset: u64, block_length: u32, feature_count: u16) -> Vec<u8> {
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
}
