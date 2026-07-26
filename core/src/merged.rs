//! Merged blocks: the block layout that accompanies a 38-byte (v2) index.
//!
//! Layers with a v2 index pack several H3 cells into one compressed block, so
//! a decompressed block is *not* a record stream — it opens with a table of
//! which cells it holds and where each one's records begin:
//!
//! ```text
//! 0..4    center_lon   i32 LE (microdegrees)
//! 4..8    center_lat   i32 LE
//! 8..12   cell_count   u32 LE
//! 12..    cell_count x (u64 cell_id, u32 rel_offset)
//! then    record data; cell i spans rel_offset[i]..rel_offset[i+1]
//! ```
//!
//! Handing a whole merged block to a record decoder decodes the header as if
//! it were records, which yields a handful of plausible-looking garbage
//! entries before the stream resynchronises — coordinates in the thousands of
//! degrees, for instance. That failure is quiet enough to survive review,
//! which is why [`cell_slice`] exists and why `PtilesFile::read_cell` uses it
//! for every v2 layer rather than leaving it to each decoder.
//!
//! Written against `scripts/shared.py::encode_merged_block`. Note that
//! `shared.py::decode_merged_block` disagrees with its own encoder — it reads a
//! `<I` length prefix per record that the encoder never writes — so the
//! encoder is the authority here, confirmed against real bytes.

use crate::codec::{DecodeError, read_u32, read_u64};

/// Byte range of `cell_id`'s records inside a decompressed merged block.
/// `Ok(None)` if the block does not contain that cell.
pub fn cell_slice(block: &[u8], cell_id: u64) -> Result<Option<&[u8]>, DecodeError> {
    let cell_count = read_u32(block, 8)? as usize;

    let eof = |offset: usize| DecodeError::UnexpectedEof {
        offset,
        needed: usize::MAX,
    };
    let table_start = 12usize;
    let data_start = cell_count
        .checked_mul(12)
        .and_then(|t| table_start.checked_add(t))
        .ok_or_else(|| eof(8))?;
    if block.len() < data_start {
        return Err(DecodeError::UnexpectedEof {
            offset: 12,
            needed: data_start,
        });
    }

    let mut found = None;
    for i in 0..cell_count {
        if read_u64(block, table_start + i * 12)? == cell_id {
            found = Some(i);
            break;
        }
    }
    let Some(i) = found else { return Ok(None) };

    let start = read_u32(block, table_start + i * 12 + 8)? as usize;
    let end = if i + 1 < cell_count {
        read_u32(block, table_start + (i + 1) * 12 + 8)? as usize
    } else {
        block.len() - data_start
    };

    // Offsets come from the file and are not trusted: a corrupt or truncated
    // block must produce an error, never a panicking slice or a range that
    // silently reads a neighbouring cell's records.
    if end < start {
        return Err(DecodeError::UnexpectedEof {
            offset: data_start + start,
            needed: 0,
        });
    }
    let abs_start = data_start.checked_add(start).ok_or_else(|| eof(start))?;
    let abs_end = data_start.checked_add(end).ok_or_else(|| eof(end))?;
    if abs_end > block.len() {
        return Err(DecodeError::UnexpectedEof {
            offset: abs_start,
            needed: abs_end - block.len(),
        });
    }
    Ok(Some(&block[abs_start..abs_end]))
}

/// Every cell id a merged block carries, in stored order.
pub fn cell_ids(block: &[u8]) -> Result<alloc::vec::Vec<u64>, DecodeError> {
    let cell_count = read_u32(block, 8)? as usize;
    let mut out = alloc::vec::Vec::with_capacity(cell_count.min(4096));
    for i in 0..cell_count {
        out.push(read_u64(block, 12 + i * 12)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    fn build(cells: &[(u64, &[u8])]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&0i32.to_le_bytes());
        b.extend_from_slice(&0i32.to_le_bytes());
        b.extend_from_slice(&(cells.len() as u32).to_le_bytes());
        let mut rel = 0u32;
        for (id, recs) in cells {
            b.extend_from_slice(&id.to_le_bytes());
            b.extend_from_slice(&rel.to_le_bytes());
            rel += recs.len() as u32;
        }
        for (_, recs) in cells {
            b.extend_from_slice(recs);
        }
        b
    }

    #[test]
    fn slices_each_cell_exactly() {
        let blk = build(&[(10, b"aaa"), (20, b"bbbb"), (30, b"c")]);
        assert_eq!(cell_slice(&blk, 10).unwrap().unwrap(), b"aaa");
        assert_eq!(cell_slice(&blk, 20).unwrap().unwrap(), b"bbbb");
        assert_eq!(cell_slice(&blk, 30).unwrap().unwrap(), b"c");
    }

    #[test]
    fn absent_cell_is_none_not_an_error() {
        let blk = build(&[(10, b"aaa")]);
        assert!(cell_slice(&blk, 99).unwrap().is_none());
    }

    #[test]
    fn single_cell_block_runs_to_the_end() {
        let blk = build(&[(10, b"only")]);
        assert_eq!(cell_slice(&blk, 10).unwrap().unwrap(), b"only");
    }

    #[test]
    fn lists_cell_ids() {
        let blk = build(&[(10, b"a"), (20, b"b")]);
        assert_eq!(cell_ids(&blk).unwrap(), alloc::vec![10, 20]);
    }

    #[test]
    fn truncation_at_every_length_errors_but_never_panics() {
        let blk = build(&[(10, b"aaa"), (20, b"bbbb")]);
        for cut in 0..blk.len() {
            let _ = cell_slice(&blk[..cut], 10);
            let _ = cell_ids(&blk[..cut]);
        }
    }

    #[test]
    fn absurd_cell_count_does_not_overflow_or_allocate() {
        let mut b = Vec::new();
        b.extend_from_slice(&0i32.to_le_bytes());
        b.extend_from_slice(&0i32.to_le_bytes());
        b.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(cell_slice(&b, 1).is_err());
        assert!(cell_ids(&b).is_err());
    }

    #[test]
    fn descending_offsets_error_rather_than_wrap() {
        // Hand-build a block whose second cell claims an earlier offset than
        // the first, which would make `end < start`.
        let mut b = Vec::new();
        b.extend_from_slice(&0i32.to_le_bytes());
        b.extend_from_slice(&0i32.to_le_bytes());
        b.extend_from_slice(&2u32.to_le_bytes());
        b.extend_from_slice(&10u64.to_le_bytes());
        b.extend_from_slice(&8u32.to_le_bytes()); // cell 10 starts at 8
        b.extend_from_slice(&20u64.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes()); // cell 20 starts at 0
        b.extend_from_slice(b"0123456789");
        assert!(cell_slice(&b, 10).is_err());
    }
}
