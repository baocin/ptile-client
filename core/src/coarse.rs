//! PTCI: the sampled ("coarse") index in a file's `aux` region.
//!
//! The full index is 38 bytes per cell -- 4014 KiB for `US.signals` -- and a
//! reader has to fetch all of it before it can find one cell, because entries
//! are only locatable by position. Sampling every 256th entry gives a ~5 KiB
//! map from cell to a bracketing position, so a client fetches header+aux in
//! one request and then only the slice of the real index it needs.
//!
//! Layout, matching `ptiles/scripts/build_points.py::build_coarse_index`:
//!
//! ```text
//! 0..4    "PTCI"
//! 4       format version (1)
//! 5..8    padding
//! 8..12   stride         u32   entries between samples
//! 12..16  sample_count   u32
//! 16..20  entry_count    u32   total entries, so a reader can cross-check
//! 20..    sample_count x (h3_cell u64 LE, entry_index u32 LE)
//! ```
//!
//! It lives in `aux` rather than a new region because `aux_offset`/`aux_length`
//! already exist in the header and are zero for every layer that predates it,
//! so a reader that doesn't know about PTCI sees `aux_length == 0` and carries
//! on. Absence is normal, not an error -- hence [`parse`] returning
//! `Ok(None)`.
//!
//! Ported from the only implementation that existed, the one in
//! `demo/index.html` (`parseCoarseIndex`/`coarseBracket`). Two things are
//! deliberately not carried over from it:
//!
//! - it ignores the version byte, so a future PTCI v2 would be parsed as v1
//!   and silently mis-bracketed. This fails closed instead, the same way
//!   `versions::check_supported` does for file magic.
//! - it returns `null` for both "no coarse index here" and "the bytes are
//!   malformed", which are different situations: the first is every older
//!   layer, the second is a corrupt or truncated file.

use alloc::vec::Vec;

use crate::codec::DecodeError;

/// `"PTCI"` read as a little-endian u32.
pub const COARSE_MAGIC: u32 = 0x4943_5450;

/// The only format version this parser accepts.
pub const COARSE_VERSION: u8 = 1;

/// Fixed-size prefix before the samples.
const HEADER_LEN: usize = 20;

/// Low 21 bits of an H3 id: the unused digits below resolution 7.
const CELL_FILLER_BITS: u64 = 0x1f_ffff;

/// Drop an H3 id's unused low digits so ids that name the same res-7 cell
/// compare equal regardless of whether the caller kept the filler bits.
#[inline]
fn normalize_cell(cell: u64) -> u64 {
    cell & !CELL_FILLER_BITS
}

/// Bytes per sample: `h3_cell` (8) + `entry_index` (4).
const SAMPLE_LEN: usize = 12;

/// A parsed sampled index: cells in ascending order, each with the position of
/// its entry in the real index.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CoarseIndex {
    /// Entries between samples, as the builder wrote it. Informational: the
    /// bracket comes from the recorded positions, not from multiplying by this.
    pub stride: u32,
    /// Total entries in the real index, so a caller can cross-check against
    /// the header before trusting a bracket.
    pub entry_count: u32,
    /// `(h3_cell, entry_index)` pairs, ascending by cell.
    pub samples: Vec<CoarseSample>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CoarseSample {
    pub h3_cell: u64,
    pub entry_index: u32,
}

/// The half-open run of index positions that may contain `cell`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CoarseBracket {
    /// First entry position to read.
    pub start: u32,
    /// Last entry position to read, inclusive.
    pub end: u32,
}

impl CoarseBracket {
    /// Number of entries in the run.
    pub fn len(&self) -> u32 {
        self.end.saturating_sub(self.start).saturating_add(1)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Byte range of this run within the file, given the header's
    /// `index_offset` and the index entry width. Inclusive of both ends, which
    /// is what an HTTP `Range` header wants.
    pub fn byte_range(&self, index_offset: u64, entry_size: usize) -> (u64, u64) {
        let es = entry_size as u64;
        let base = index_offset + 4;
        (
            base + self.start as u64 * es,
            base + (self.end as u64 + 1) * es - 1,
        )
    }
}

/// Parse an `aux` region as a coarse index.
///
/// `Ok(None)` means the region is simply not a coarse index -- empty, too
/// short to be one, or holding something else entirely. That is the normal
/// case for every layer built before PTCI existed, and callers should fall
/// back to reading the full index.
///
/// `Err` means it announced itself as PTCI and then did not hold up: an
/// unsupported version, or a sample count the region cannot contain. Those
/// are worth surfacing rather than silently falling back, because they mean
/// whatever wrote the file has a bug.
pub fn parse(aux: &[u8]) -> Result<Option<CoarseIndex>, DecodeError> {
    if aux.len() < HEADER_LEN {
        return Ok(None);
    }
    if u32::from_le_bytes(aux[0..4].try_into().unwrap()) != COARSE_MAGIC {
        return Ok(None);
    }

    let version = aux[4];
    if version != COARSE_VERSION {
        // Fail closed. A v2 with a different sample layout would otherwise be
        // read as v1 and produce brackets that point at the wrong entries --
        // which reads as "cell not in this file", the same silent-empty result
        // that every index bug in this format has produced.
        return Err(DecodeError::UnsupportedSectionVersion {
            section: "PTCI coarse index",
            found: version,
            supported: COARSE_VERSION,
        });
    }

    let stride = u32::from_le_bytes(aux[8..12].try_into().unwrap());
    let sample_count = u32::from_le_bytes(aux[12..16].try_into().unwrap()) as usize;
    let entry_count = u32::from_le_bytes(aux[16..20].try_into().unwrap());

    let needed = sample_count
        .checked_mul(SAMPLE_LEN)
        .and_then(|n| n.checked_add(HEADER_LEN))
        .ok_or(DecodeError::UnexpectedEof {
            offset: HEADER_LEN,
            needed: usize::MAX,
        })?;
    if needed > aux.len() {
        return Err(DecodeError::UnexpectedEof {
            offset: aux.len(),
            needed: needed - aux.len(),
        });
    }

    let mut samples = Vec::with_capacity(sample_count);
    for i in 0..sample_count {
        let at = HEADER_LEN + i * SAMPLE_LEN;
        samples.push(CoarseSample {
            h3_cell: u64::from_le_bytes(aux[at..at + 8].try_into().unwrap()),
            entry_index: u32::from_le_bytes(aux[at + 8..at + 12].try_into().unwrap()),
        });
    }

    Ok(Some(CoarseIndex {
        stride,
        entry_count,
        samples,
    }))
}

impl CoarseIndex {
    /// Index positions that may contain `cell`: from the last sample at or
    /// below it, through the next sample.
    ///
    /// `None` when `cell` sorts below the first sample, which means the file
    /// does not contain it. A cell above the last sample brackets to the end
    /// of the index, since there is no later sample to stop at.
    pub fn bracket(&self, cell: u64) -> Option<CoarseBracket> {
        if self.samples.is_empty() {
            return None;
        }

        // Compare normalised ids on both sides.
        //
        // A res-7 H3 id carries filler digits in its low 21 bits, and callers
        // do not agree on whether to keep them: an id straight out of
        // `latLngToCell` has them set, while a caller that masked the cell to
        // its res-7 parent has them cleared. The samples store whatever the
        // builder wrote. Comparing the two forms directly makes a masked query
        // sort *below* the sample naming its own cell, so the search lands one
        // sample early and the run it names does not contain the entry --
        // which surfaces as "cell not in this file", not as an error.
        //
        // Zeroing those bits is order-preserving: distinct res-7 cells differ
        // in digits above bit 21, so the comparison is unchanged for every
        // pair that was already unambiguous.
        let want = normalize_cell(cell);

        // Last sample with `h3_cell <= cell`.
        let mut lo = 0usize;
        let mut hi = self.samples.len();
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if normalize_cell(self.samples[mid].h3_cell) <= want {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        if lo == 0 {
            return None;
        }
        let best = lo - 1;

        let start = self.samples[best].entry_index;
        let end = if best + 1 < self.samples.len() {
            self.samples[best + 1].entry_index
        } else {
            self.entry_count.saturating_sub(1)
        };
        Some(CoarseBracket {
            start,
            end: end.max(start),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// H3-shaped ids. The first version of these tests used 100/200/300, which
    /// all sit below the 21 filler bits and therefore normalise to the same
    /// value -- so they silently stopped distinguishing anything the moment
    /// `bracket` started normalising. Real res-7 ids look like this.
    const C1: u64 = 0x8726_4d10_6fff_ffff;
    const C2: u64 = 0x8726_4d30_6fff_ffff;
    const C3: u64 = 0x8726_4d50_6fff_ffff;

    fn build(version: u8, stride: u32, entry_count: u32, samples: &[(u64, u32)]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&COARSE_MAGIC.to_le_bytes());
        b.push(version);
        b.extend_from_slice(&[0, 0, 0]);
        b.extend_from_slice(&stride.to_le_bytes());
        b.extend_from_slice(&(samples.len() as u32).to_le_bytes());
        b.extend_from_slice(&entry_count.to_le_bytes());
        for (cell, idx) in samples {
            b.extend_from_slice(&cell.to_le_bytes());
            b.extend_from_slice(&idx.to_le_bytes());
        }
        b
    }

    #[test]
    fn absent_aux_is_not_an_error() {
        assert_eq!(parse(&[]).unwrap(), None);
        assert_eq!(parse(&[0u8; 8]).unwrap(), None);
        // Something else entirely in aux: not ours, not an error.
        assert_eq!(parse(&[0xffu8; 64]).unwrap(), None);
    }

    #[test]
    fn round_trips_a_built_index() {
        let raw = build(1, 256, 1000, &[(C1, 0), (C2, 256), (C3, 512)]);
        let c = parse(&raw).unwrap().unwrap();
        assert_eq!(c.stride, 256);
        assert_eq!(c.entry_count, 1000);
        assert_eq!(c.samples.len(), 3);
        assert_eq!(c.samples[1].h3_cell, C2);
        assert_eq!(c.samples[1].entry_index, 256);
    }

    #[test]
    fn an_unknown_version_fails_rather_than_being_read_as_v1() {
        let raw = build(2, 256, 1000, &[(C1, 0)]);
        assert!(parse(&raw).is_err());
    }

    #[test]
    fn a_sample_count_the_region_cannot_hold_is_an_error() {
        let mut raw = build(1, 256, 1000, &[(C1, 0), (C2, 256)]);
        // Claim 9999 samples while carrying 2.
        raw[12..16].copy_from_slice(&9999u32.to_le_bytes());
        assert!(parse(&raw).is_err());
    }

    #[test]
    fn brackets_span_the_sample_at_or_below_through_the_next() {
        let c = parse(&build(1, 256, 1000, &[(C1, 0), (C2, 256), (C3, 512)]))
            .unwrap()
            .unwrap();
        // Between the first and second samples.
        assert_eq!(c.bracket(C1 + (1 << 25)), Some(CoarseBracket { start: 0, end: 256 }));
        // Exactly on a sample.
        assert_eq!(c.bracket(C2), Some(CoarseBracket { start: 256, end: 512 }));
        // Above the last sample: runs to the end of the index.
        assert_eq!(c.bracket(u64::MAX), Some(CoarseBracket { start: 512, end: 999 }));
    }

    /// A caller with a masked id and a builder that stored a raw one must
    /// reach the same bracket. They did not, and a partial open reported every
    /// cell missing on the sample boundaries.
    #[test]
    fn a_masked_cell_brackets_the_same_as_the_raw_id() {
        let raw = 0x872_64d1_06ff_ffffu64;
        let masked = raw & !0x1f_ffff;
        assert_ne!(raw, masked, "the test id must actually carry filler bits");

        let c = parse(&build(1, 256, 1000, &[(raw, 256), (raw + (1 << 25), 512)]))
            .unwrap()
            .unwrap();
        assert_eq!(c.bracket(raw), c.bracket(masked));
        assert_eq!(c.bracket(masked).map(|b| b.start), Some(256));
    }

    #[test]
    fn a_cell_below_the_first_sample_is_not_in_the_file() {
        let c = parse(&build(1, 256, 1000, &[(C1, 0), (C2, 256)]))
            .unwrap()
            .unwrap();
        assert_eq!(c.bracket(C1 - (1 << 25)), None);
        // Exactly the first sample is in the file.
        assert!(c.bracket(C1).is_some());
    }

    #[test]
    fn empty_samples_bracket_to_nothing() {
        let c = parse(&build(1, 256, 0, &[])).unwrap().unwrap();
        assert_eq!(c.bracket(123), None);
    }

    #[test]
    fn byte_range_matches_the_entries_it_names() {
        let b = CoarseBracket { start: 10, end: 19 };
        // index_offset 256, 4-byte count, 38-byte entries.
        let (from, to) = b.byte_range(256, 38);
        assert_eq!(from, 256 + 4 + 10 * 38);
        assert_eq!(to, 256 + 4 + 20 * 38 - 1);
        assert_eq!(to - from + 1, 10 * 38);
        assert_eq!(b.len(), 10);
    }

    /// The JS parser this was ported from happily accepts a truncated sample
    /// table by checking only the declared count, so this pins the stricter
    /// behaviour rather than assuming it.
    #[test]
    fn a_truncated_sample_table_errors_not_panics() {
        let full = build(1, 256, 1000, &[(C1, 0), (C2, 256), (C3, 512)]);
        for cut in HEADER_LEN..full.len() {
            let _ = parse(&full[..cut]);
        }
        // One byte short of the last sample must be an error, not a partial read.
        assert!(parse(&full[..full.len() - 1]).is_err());
    }
}
