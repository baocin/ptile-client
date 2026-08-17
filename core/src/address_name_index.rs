//! `{STATE}.address_name_index.ptiles` (`PTILESY`): street name -> its cells.
//!
//! Forward geocoding without this has to visit cells, because a street name
//! exists only inside a block. Measured on Tennessee: 14.7 s to answer
//! "919 Broadway" with no location hint, and even the browser's bounded 25 km
//! sweep read ~3 MB to conclude nothing matched nearby. Both are scans.
//!
//! The sidecar is deliberately the same shape as the business name index: 28
//! buckets keyed by the first letter of the folded street name, on the ordinary
//! v1 19-byte index, so [`crate::PtilesFile`] reads it with no new index code.
//! Each bucket block holds, repeatedly:
//!
//! ```text
//! u16 len | street bytes (folded) | varint cell_count | cell_count x varint delta
//! ```
//!
//! Names are stored folded ([`crate::address::fold_street_for_match`]'s rule),
//! not as they display: the index answers "which cells", and the address
//! records themselves carry the spelling a user reads. Storing display names
//! would also split `Beale St` from `BEALE Street` and answer half the query.

use alloc::string::String;
use alloc::vec::Vec;

use crate::address::fold_street_for_match;
use crate::codec::{DecodeError, decode_string_u16, decode_varint};
use crate::file::{FileError, PtilesFile};
use crate::source::PtilesSource;

/// Buckets: `a`-`z`, then one for names starting with a non-letter, then one
/// for the empty name.
pub const BUCKET_COUNT: u64 = 28;

/// One street and the cells it appears in.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct StreetCells {
    /// Folded name, as stored.
    pub street: String,
    pub cells: Vec<u64>,
}

/// Which bucket a folded street name lives in. Must agree with the builder;
/// a disagreement is silent, because a lookup in the wrong bucket is just a
/// miss.
pub fn bucket_of(folded: &str) -> u64 {
    match folded.as_bytes().first() {
        None => 27,
        Some(c) if c.is_ascii_lowercase() => (c - b'a') as u64,
        Some(_) => 26,
    }
}

/// Decode one bucket block.
pub fn decode_bucket(block: &[u8]) -> Result<Vec<StreetCells>, DecodeError> {
    let mut out = Vec::new();
    let mut p = 0usize;
    while p < block.len() {
        let (street, c) = decode_string_u16(block, p)?;
        p += c;
        let (count, c) = decode_varint(block, p)?;
        p += c;
        let mut cells = Vec::with_capacity(count as usize);
        let mut prev = 0u64;
        for _ in 0..count {
            let (delta, c) = decode_varint(block, p)?;
            p += c;
            prev = prev.wrapping_add(delta);
            cells.push(prev);
        }
        out.push(StreetCells { street, cells });
    }
    Ok(out)
}

/// An opened street-name index.
pub struct AddressNameIndex<S: PtilesSource> {
    file: PtilesFile<S>,
}

impl<S: PtilesSource> AddressNameIndex<S> {
    pub fn open(source: S) -> Result<AddressNameIndex<S>, FileError> {
        let file = PtilesFile::open(source)?;
        if &file.header().magic != b"PTILESY" {
            return Err(FileError::BadMagic {
                found: file.header().magic,
            });
        }
        Ok(AddressNameIndex { file })
    }

    /// Cells containing any street whose folded name contains `query`'s.
    ///
    /// Substring, not equality, so "broadway" finds "west broadway circle" --
    /// the same rule the record-level matcher uses. Only the query's own
    /// bucket is read, which is the entire point: one block instead of every
    /// cell in the state.
    ///
    /// An empty result means "no such street", which is a real answer here and
    /// lets a caller skip the address file entirely.
    pub fn cells_for_street(&self, query: &str) -> Result<Vec<u64>, FileError> {
        let folded = fold_street_for_match(query.trim());
        if folded.is_empty() {
            return Ok(Vec::new());
        }
        let bucket = bucket_of(&folded);
        let Some(block) = self.file.read_block(bucket)? else {
            return Ok(Vec::new());
        };
        let mut cells: Vec<u64> = decode_bucket(&block)?
            .into_iter()
            .filter(|s| s.street.contains(&folded))
            .flat_map(|s| s.cells)
            .collect();
        cells.sort_unstable();
        cells.dedup();
        Ok(cells)
    }

    /// Every street in one bucket, for tooling and tests.
    pub fn bucket(&self, bucket: u64) -> Result<Vec<StreetCells>, FileError> {
        match self.file.read_block(bucket)? {
            Some(block) => Ok(decode_bucket(&block)?),
            None => Ok(Vec::new()),
        }
    }

    pub fn header(&self) -> &crate::header::Header {
        self.file.header()
    }
}
