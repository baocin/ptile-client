//! Cell index parse + binary search over H3 cell -> block offset/length entries.
//!
//! Two entry widths exist in the wild and both must be read. Which one a file
//! uses is a property of the generator that wrote it, not of the layer:
//!
//! | width | layout | observed in |
//! | --- | --- | --- |
//! | 19 B | `h3_cell:u64`, `block_offset:6LE`, `block_length:3LE`, `feature_count:u16` | roads, water, business, buildings_v8 |
//! | 38 B | v1 fields preceded by a 16-byte bbox and followed by `cell_index:u16` | parks, rail, places, signals, camera |
//!
//! v1 is SPEC.md's "Spatial Index"; v2 is the undocumented "merged block"
//! index (`ptiles/codec.py::decode_index_v2`, `scripts/shared.py::
//! encode_index_entry_v2`). The 38-byte entry is:
//!
//! ```text
//! 0..8    h3_cell        u64 LE
//! 8..24   min_lon, min_lat, max_lon, max_lat   i32 LE x4  (often all zero)
//! 24..30  block_offset   6-byte LE (low bits)
//! 30..32  block_length   2-byte LE (low bits)
//! 32      block_offset   high byte (bits 48..56)
//! 33      block_length   high byte (bits 16..24)
//! 34..36  feature_count  u16 LE
//! 36..38  cell_index     u16 LE  -- position of this cell within its block
//! ```
//!
//! Width is detected, not assumed: see [`detect_entry_size`]. Reading a
//! 38-byte index as 19-byte does not error — it silently yields entries whose
//! `block_offset`/`block_length` come from the zeroed bbox bytes, i.e. zeros,
//! which downstream reads as "no data for this cell". That silent-empty
//! failure is exactly what this module exists to prevent, so detection
//! validates structurally rather than trusting the header.

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

/// SPEC.md v1 entry width.
pub const ENTRY_SIZE_V1: usize = 19;
/// Undocumented merged-block v2 entry width.
pub const ENTRY_SIZE_V2: usize = 38;

const ENTRY_SIZE: usize = ENTRY_SIZE_V1;

/// The widths this reader knows how to parse, in the order detection tries
/// them. Adding a width means adding it here and to `read_entry` -- not
/// touching any layer's decoder.
pub const KNOWN_ENTRY_SIZES: [usize; 2] = [ENTRY_SIZE_V1, ENTRY_SIZE_V2];

fn read_uint_le(data: &[u8], offset: usize, len: usize) -> u64 {
    let mut v: u64 = 0;
    for i in 0..len {
        v |= (data[offset + i] as u64) << (8 * i);
    }
    v
}

/// Decode one entry of the given width at `pos`. Caller guarantees
/// `data[pos..pos + entry_size]` is in bounds.
fn read_entry(data: &[u8], pos: usize, entry_size: usize) -> IndexEntry {
    match entry_size {
        ENTRY_SIZE_V2 => IndexEntry {
            h3_cell: u64::from_le_bytes(data[pos..pos + 8].try_into().unwrap()),
            // Split fields: low bytes at 24/30, high bytes at 32/33.
            block_offset: read_uint_le(data, pos + 24, 6)
                | ((data[pos + 32] as u64) << 48),
            block_length: (read_uint_le(data, pos + 30, 2) as u32)
                | ((data[pos + 33] as u32) << 16),
            feature_count: u16::from_le_bytes(
                data[pos + 34..pos + 36].try_into().unwrap(),
            ),
        },
        // ENTRY_SIZE_V1 and anything else the caller validated.
        _ => IndexEntry {
            h3_cell: u64::from_le_bytes(data[pos..pos + 8].try_into().unwrap()),
            block_offset: read_uint_le(data, pos + 8, 6),
            block_length: read_uint_le(data, pos + 14, 3) as u32,
            feature_count: u16::from_le_bytes(
                data[pos + 17..pos + 19].try_into().unwrap(),
            ),
        },
    }
}

/// Why a given entry width was chosen. Carried on [`ParsedIndex`] so callers
/// (and tests) can assert the decision, not just the outcome -- a regression
/// that picks the right answer for the wrong reason should still fail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum EntrySizeSource {
    /// `(index_length - 4) / count` divided evenly into a known width and the
    /// entries parsed structurally clean. No guessing involved.
    DeclaredLength,
    /// The declared length implied no known width (or implied one that failed
    /// structural validation), so widths were tried in order and this one
    /// produced a structurally valid index.
    Probed,
    /// Caller supplied the width explicitly.
    Forced,
}

/// A parsed index plus how its layout was determined.
#[derive(Clone, Debug)]
pub struct ParsedIndex {
    pub entries: Vec<IndexEntry>,
    pub entry_size: usize,
    pub entry_size_source: EntrySizeSource,
    /// Width implied by the header's `index_length`, when it divided evenly.
    /// `Some(42)` on the published `US.signals`/`US.camera`, whose header was
    /// written at a 42-byte stride while the encoder emitted 38.
    pub declared_stride: Option<usize>,
}

/// Does this index parse cleanly at `entry_size`?
///
/// Structural, not cryptographic. Two checks, both false when the width is
/// wrong and true whenever it is right:
///
/// 1. **Entry 0 names a non-empty block.** Cheap, and it catches a 38-byte
///    index whose bbox really is zeros. It is *not* sufficient on its own:
///    measured against the published 38-byte files, entry 0 read at 19 bytes
///    comes back with a large non-zero length (US.camera 10420322,
///    US.signals 6750310, GA.parks 3342386), so this check passes and check 2
///    is what actually rejects them. An earlier version of this comment
///    claimed check 1 was the deterministic one; the bytes disagree.
/// 2. **Entries are *mostly* non-descending by `h3_cell`.** The format says
///    sorted and `binary_search` depends on it, but published files do not all
///    hold up: `{ST}.buildings_v8.ptiles` is written in build order, so GA has
///    267 descending steps in 14371 entries (1.9%) and CA 284 in 21687 (1.3%).
///    Tennessee happens to have none, which is why it was the only state whose
///    buildings ever rendered — every other state failed detection outright.
///    Bytes read at the *wrong* stride are unrelated to cell ids and descend
///    about half the time, so a tolerance still separates the two cleanly.
///    [`parse_index_sized`] sorts what it returns, so a tolerated file is
///    still safe to binary-search.
///
/// A first cell with genuinely no data would be rejected here, but such an
/// entry should not exist — cells with nothing in them are left out of the
/// index rather than written empty.
fn is_structurally_valid(data: &[u8], count: usize, entry_size: usize) -> bool {
    if count == 0 {
        return true;
    }
    if read_entry(data, 4, entry_size).block_length == 0 {
        return false;
    }
    let mut descents = 0usize;
    let mut prev = 0u64;
    for i in 0..count {
        let cell = read_entry(data, 4 + i * entry_size, entry_size).h3_cell;
        if cell < prev {
            descents += 1;
        }
        prev = cell;
    }
    // The threshold is measured, not guessed. Reading a real 38-byte index at
    // 19 bytes makes every second entry misaligned garbage, which gives
    // *exactly* 50.0% descents — US.camera, US.signals, GA.parks and GA.rail
    // all land on it to the entry. The worst real 19-byte file is MA
    // buildings at 8.75% (371 of 4242). A quarter sits between the two with
    // roughly 3x margin below and 2x above.
    descents * 4 <= count
}

/// Bytes required to hold `count` entries of `entry_size`, or `None` on
/// overflow. `count` is corruption-controlled, so this is checked.
fn required_len(count: usize, entry_size: usize) -> Option<usize> {
    count.checked_mul(entry_size)?.checked_add(4)
}

/// Choose the entry width for an index section.
///
/// Order of preference:
/// 1. The width implied by `index_length`, if it divides evenly, is a width we
///    know, fits the buffer, and validates structurally.
/// 2. Otherwise each known width in turn, first structurally valid one wins.
///
/// Step 1 costs nothing and settles every well-formed file. Step 2 exists
/// because a header can lie: the published `US.signals.ptiles` declares a
/// 42-byte stride over 38-byte entries, so its `index_length` implies a width
/// this reader has never heard of.
pub fn detect_entry_size(
    data: &[u8],
    index_length: Option<usize>,
) -> Result<(usize, EntrySizeSource, Option<usize>), DecodeError> {
    if data.len() < 4 {
        return Err(DecodeError::UnexpectedEof {
            offset: 0,
            needed: 4,
        });
    }
    let count = u32::from_le_bytes(data[0..4].try_into().unwrap()) as usize;

    // An index with no entries has no width to determine, and every width
    // parses it identically. Report it as settled rather than "probed".
    if count == 0 {
        return Ok((ENTRY_SIZE_V1, EntrySizeSource::DeclaredLength, None));
    }

    let declared = index_length.and_then(|il| {
        if count == 0 || il < 4 {
            return None;
        }
        let body = il - 4;
        if body % count == 0 { Some(body / count) } else { None }
    });

    let fits = |sz: usize| {
        required_len(count, sz).is_some_and(|need| data.len() >= need)
    };

    if let Some(sz) = declared {
        if KNOWN_ENTRY_SIZES.contains(&sz) && fits(sz) && is_structurally_valid(data, count, sz) {
            return Ok((sz, EntrySizeSource::DeclaredLength, declared));
        }
    }

    for sz in KNOWN_ENTRY_SIZES {
        if fits(sz) && is_structurally_valid(data, count, sz) {
            return Ok((sz, EntrySizeSource::Probed, declared));
        }
    }

    Err(DecodeError::UnexpectedEof {
        offset: 0,
        needed: required_len(count, ENTRY_SIZE_V1).unwrap_or(usize::MAX),
    })
}

/// Parse an index of a known width.
pub fn parse_index_sized(
    data: &[u8],
    entry_size: usize,
) -> Result<Vec<IndexEntry>, DecodeError> {
    if data.len() < 4 {
        return Err(DecodeError::UnexpectedEof {
            offset: 0,
            needed: 4,
        });
    }
    let count = u32::from_le_bytes(data[0..4].try_into().unwrap()) as usize;

    // `count` is attacker/corruption-controlled: compute the required size
    // with checked arithmetic so a huge count can't overflow `usize` (wrapping
    // to a small `needed` that then passes the length check and reads out of
    // bounds), and validate the length *before* the `with_capacity` below so a
    // bogus count can't trigger a giant allocation.
    let needed = required_len(count, entry_size).ok_or(DecodeError::UnexpectedEof {
        offset: 0,
        needed: usize::MAX,
    })?;
    if data.len() < needed {
        return Err(DecodeError::UnexpectedEof {
            offset: data.len(),
            needed: needed - data.len(),
        });
    }

    let mut entries = Vec::with_capacity(count);
    for i in 0..count {
        entries.push(read_entry(data, 4 + i * entry_size, entry_size));
    }
    // The format says the index is sorted by cell; several published files are
    // not (see `is_structurally_valid`). Sort rather than trust, because
    // `binary_search` — and every caller that reaches for it — is silently
    // wrong on unsorted input rather than loudly wrong. Already-sorted input,
    // which is the common case, costs one pass.
    if entries.windows(2).any(|w| w[0].h3_cell > w[1].h3_cell) {
        entries.sort_unstable_by_key(|e| e.h3_cell);
    }
    Ok(entries)
}

/// Parse a bare run of index entries: no 4-byte count, just entries.
///
/// This is what a *partial* index read returns. A client using the PTCI coarse
/// index fetches only the byte range one bracket names
/// ([`crate::coarse::CoarseBracket::byte_range`]), which lands mid-section with
/// no count in front of it. Without this, such a client has to decode entries
/// itself -- and hand-decoding index entries in the client is the single
/// mistake this crate exists to prevent.
///
/// The width cannot be detected from a run: detection needs the header's
/// declared `index_length` and a whole section to validate against, and a
/// 38-byte run read as 19-byte yields entries whose offsets come from the
/// zeroed bbox bytes, i.e. silently empty. Callers get the width from
/// [`crate::index_layout`] on the full header+index, or know it because the
/// file carries a coarse index (which only the 38-byte builder writes).
///
/// Trailing bytes that do not complete an entry are ignored, so a caller that
/// rounded its range outward gets the entries that are whole rather than an
/// error.
pub fn parse_entry_run(data: &[u8], entry_size: usize) -> Result<Vec<IndexEntry>, DecodeError> {
    if entry_size == 0 {
        return Err(DecodeError::UnexpectedEof {
            offset: 0,
            needed: 1,
        });
    }
    if !KNOWN_ENTRY_SIZES.contains(&entry_size) {
        // Refuse a width this build cannot read rather than producing entries
        // from misaligned bytes.
        return Err(DecodeError::UnsupportedSectionVersion {
            section: "index entry width",
            found: entry_size.min(u8::MAX as usize) as u8,
            supported: ENTRY_SIZE_V2 as u8,
        });
    }
    let count = data.len() / entry_size;
    let mut entries = Vec::with_capacity(count);
    for i in 0..count {
        entries.push(read_entry(data, i * entry_size, entry_size));
    }
    Ok(entries)
}

/// Detect the entry width and parse, reporting how the width was chosen.
///
/// `index_length` is the header's declared value when available; pass `None`
/// to force probing.
pub fn parse_index_detected(
    data: &[u8],
    index_length: Option<usize>,
) -> Result<ParsedIndex, DecodeError> {
    let (entry_size, entry_size_source, declared_stride) =
        detect_entry_size(data, index_length)?;
    Ok(ParsedIndex {
        entries: parse_index_sized(data, entry_size)?,
        entry_size,
        entry_size_source,
        declared_stride,
    })
}

/// Parse the spatial index section as v1 (19-byte entries), sorted by
/// `h3_cell` as stored on disk (the format guarantees the order, we don't
/// re-sort). Bounds-checked; truncated input yields `Err`, never a panic.
///
/// Forces v1. Prefer [`parse_index_detected`] unless you know the width --
/// this function reads a 38-byte index as garbage rather than failing, which
/// is the historical bug.
pub fn parse_index(data: &[u8]) -> Result<Vec<IndexEntry>, DecodeError> {
    parse_index_sized(data, ENTRY_SIZE)
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

    /// A 38-byte index, as a builder really writes one: the 16-byte bbox at
    /// bytes 8..24 is zeros. Read at 19 bytes this is the silent-empty failure
    /// detection exists to catch, so it must still be caught.
    fn encode_entry_v2(h3_cell: u64, block_offset: u64, block_length: u32) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&h3_cell.to_le_bytes());
        buf.extend_from_slice(&[0u8; 16]); // bbox, zeros
        buf.extend_from_slice(&block_offset.to_le_bytes()[0..6]);
        buf.extend_from_slice(&block_length.to_le_bytes()[0..3]);
        buf.extend_from_slice(&0u16.to_le_bytes()); // feature_count
        buf.extend_from_slice(&0u32.to_le_bytes()); // cell_index
        buf
    }

    #[test]
    fn nearly_sorted_index_parses_and_comes_back_sorted() {
        // Published `{ST}.buildings_v8.ptiles` are written in build order, not
        // cell order: GA has 267 descending steps in 14371 entries, CA 284 in
        // 21687. Tennessee has none, which is why it was the only state whose
        // buildings ever rendered. Rejecting these files outright made every
        // other state draw nothing; `binary_search` still needs sorted input,
        // so the fix is to sort rather than to trust.
        let mut data = Vec::new();
        data.extend_from_slice(&4u32.to_le_bytes());
        data.extend_from_slice(&encode_entry(10, 100, 5, 1));
        data.extend_from_slice(&encode_entry(30, 300, 7, 3)); // out of order
        data.extend_from_slice(&encode_entry(20, 200, 6, 2)); // ...
        data.extend_from_slice(&encode_entry(40, 400, 8, 4));

        let index = parse_index(&data).unwrap();
        assert_eq!(index.len(), 4);
        assert!(index.windows(2).all(|w| w[0].h3_cell <= w[1].h3_cell));
        // Sorting is pointless unless lookups actually work afterwards.
        assert_eq!(binary_search(&index, 20).unwrap().block_offset, 200);
        assert_eq!(binary_search(&index, 30).unwrap().block_offset, 300);
        assert!(binary_search(&index, 25).is_none());
    }

    #[test]
    fn nearly_sorted_index_is_accepted_as_a_width_candidate() {
        // The GA/CA shape: one descending step in a long run. Detection ran
        // before parsing, so this is where those files were actually rejected
        // -- with `unexpected end of input`, which named neither the file nor
        // the real problem.
        let mut data = Vec::new();
        let cells: [u64; 20] = [
            10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 95, 110, 120, 130, 140, 150, 160, 170, 180,
            190,
        ];
        data.extend_from_slice(&(cells.len() as u32).to_le_bytes());
        for (i, c) in cells.iter().enumerate() {
            data.extend_from_slice(&encode_entry(*c, 100 * i as u64, 5, 1));
        }
        let index_length = Some(4 + cells.len() * ENTRY_SIZE_V1);
        let (size, _, _) = detect_entry_size(&data, index_length).unwrap();
        assert_eq!(size, ENTRY_SIZE_V1);
    }

    #[test]
    fn wrong_width_is_still_rejected_despite_sort_tolerance() {
        // Tolerating unsorted entries must not blunt the check that matters.
        // Entry 0 read at the wrong width takes its block_length from inside
        // the zeroed bbox, so it reads zero -- deterministically.
        let mut data = Vec::new();
        data.extend_from_slice(&3u32.to_le_bytes());
        for (cell, off, len) in [(10u64, 100u64, 5u32), (20, 200, 6), (30, 300, 7)] {
            data.extend_from_slice(&encode_entry_v2(cell, off, len));
        }
        let index_length = Some(4 + 3 * ENTRY_SIZE_V2);
        let (size, _, _) = detect_entry_size(&data, index_length).unwrap();
        assert_eq!(size, ENTRY_SIZE_V2, "must not be mistaken for a 19-byte index");
    }

    #[test]
    fn shuffled_index_is_rejected_as_a_width_candidate() {
        // Tolerance is for build-order files that are nearly sorted, not for
        // bytes read at the wrong stride, which alternate real cell / garbage
        // and so descend on every second entry — 50.0% exactly, measured on
        // four published 38-byte files.
        let mut data = Vec::new();
        let cells: [u64; 8] = [80, 10, 70, 20, 60, 30, 50, 40];
        data.extend_from_slice(&(cells.len() as u32).to_le_bytes());
        for (i, c) in cells.iter().enumerate() {
            data.extend_from_slice(&encode_entry(*c, 100 * i as u64, 5, 1));
        }
        assert!(!is_structurally_valid(&data, cells.len(), ENTRY_SIZE_V1));
    }

    #[test]
    fn worst_real_file_is_inside_the_tolerance_and_a_wrong_stride_is_not() {
        // The two ends the threshold has to separate, as ratios rather than
        // fetched files: MA buildings at 371/4242, and the exact half of a
        // 38-byte index read at 19.
        fn descending_run(count: usize, descents: usize) -> Vec<u8> {
            let mut data = Vec::new();
            data.extend_from_slice(&(count as u32).to_le_bytes());
            let mut cell = 1000u64;
            for i in 0..count {
                // Put every descent at a distinct position; ascend otherwise.
                if i > 0 && i <= descents {
                    cell -= 1;
                } else {
                    cell += 10;
                }
                data.extend_from_slice(&encode_entry(cell, 100 + i as u64, 5, 1));
            }
            data
        }
        let ma = descending_run(4242, 371);
        assert!(is_structurally_valid(&ma, 4242, ENTRY_SIZE_V1), "MA must be readable");

        let wrong = descending_run(1000, 500);
        assert!(!is_structurally_valid(&wrong, 1000, ENTRY_SIZE_V1), "50% must be rejected");
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
