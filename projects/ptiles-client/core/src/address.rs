//! Address layer decoder (`PTILESA2` on disk → truncates to `PTILESA`,
//! `{STATE}.address.ptiles`).
//!
//! A per-cell feature layer on the **v2 merged-block index** (38-byte entries),
//! which the main `file.rs` block reader does not handle — so this module
//! parses the v2 index and merged blocks itself, mirroring `admin.rs`'s
//! self-contained open path. Ported from the reference encoder
//! `ptiles/scripts/build_address.py` + `ptiles/scripts/shared.py`
//! (`encode_index_entry_v2`, `encode_merged_block`, `enc`); there is no
//! reference *decoder*.
//!
//! A record carries only `{osm_id, housenumber, street}` — no coordinates; the
//! location is the H3 res-7 cell it lives in. Records within a cell are
//! sequential with no length prefix (walk the cell's byte slice to its end),
//! and `osm_id` is delta-varint-zigzag from the previous record in that cell
//! (the delta resets to the raw id at each cell boundary).

use alloc::string::String;
use alloc::vec::Vec;

use crate::codec::{
    DecodeError, decode_string_u16, decode_varint, read_i32, read_u16, read_u32, read_u64,
    zigzag_decode,
};
use crate::file::{FileError, zstd_decompress};
use crate::header::{HEADER_SIZE, Header};
use crate::source::PtilesSource;

/// Bytes per v2 index entry.
pub const V2_INDEX_ENTRY_SIZE: usize = 38;

/// One decoded address record. `osm_id` is the absolute id (deltas already
/// accumulated); the location is implied by the containing cell.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AddressRecord {
    pub osm_id: i64,
    pub housenumber: String,
    pub street: String,
}

/// One v2 spatial-index entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AddressIndexEntry {
    pub h3_cell: u64,
    pub min_lon: i32,
    pub min_lat: i32,
    pub max_lon: i32,
    pub max_lat: i32,
    pub block_offset: u64,
    pub block_length: u32,
    pub feature_count: u16,
    pub cell_index: u16,
}

/// Decode one v2 index entry from `data[pos..pos+38]`.
fn decode_v2_entry(data: &[u8], pos: usize) -> Result<AddressIndexEntry, DecodeError> {
    // Bounds: the whole 38-byte entry.
    if pos + V2_INDEX_ENTRY_SIZE > data.len() {
        return Err(DecodeError::UnexpectedEof {
            offset: pos,
            needed: V2_INDEX_ENTRY_SIZE,
        });
    }
    let h3_cell = read_u64(data, pos)?;
    let min_lon = read_i32(data, pos + 8)?;
    let min_lat = read_i32(data, pos + 12)?;
    let max_lon = read_i32(data, pos + 16)?;
    let max_lat = read_i32(data, pos + 20)?;
    // block_offset: 6-byte low part (24..30) | offset_hi u8 (32) << 48.
    let mut off = [0u8; 8];
    off[..6].copy_from_slice(&data[pos + 24..pos + 30]);
    let offset_lo = u64::from_le_bytes(off);
    let offset_hi = data[pos + 32] as u64;
    let block_offset = offset_lo | (offset_hi << 48);
    // block_length: len_lo u16 (30..32) | len_hi u8 (33) << 16.
    let len_lo = read_u16(data, pos + 30)? as u32;
    let len_hi = data[pos + 33] as u32;
    let block_length = len_lo | (len_hi << 16);
    let feature_count = read_u16(data, pos + 34)?;
    let cell_index = read_u16(data, pos + 36)?;
    Ok(AddressIndexEntry {
        h3_cell,
        min_lon,
        min_lat,
        max_lon,
        max_lat,
        block_offset,
        block_length,
        feature_count,
        cell_index,
    })
}

/// Parse the v2 index section: `u32 count` + `count × 38-byte` entries
/// (globally sorted by `h3_cell`).
pub fn parse_v2_index(data: &[u8]) -> Result<Vec<AddressIndexEntry>, DecodeError> {
    let count = read_u32(data, 0)? as usize;
    let needed = count
        .checked_mul(V2_INDEX_ENTRY_SIZE)
        .and_then(|n| n.checked_add(4))
        .ok_or(DecodeError::UnexpectedEof {
            offset: 0,
            needed: usize::MAX,
        })?;
    if data.len() < needed {
        return Err(DecodeError::UnexpectedEof { offset: 0, needed });
    }
    let mut entries = Vec::with_capacity(count);
    let mut p = 4usize;
    for _ in 0..count {
        entries.push(decode_v2_entry(data, p)?);
        p += V2_INDEX_ENTRY_SIZE;
    }
    Ok(entries)
}

/// Binary-search the (h3_cell-sorted) v2 index.
pub fn index_search(index: &[AddressIndexEntry], cell: u64) -> Option<&AddressIndexEntry> {
    index
        .binary_search_by(|e| e.h3_cell.cmp(&cell))
        .ok()
        .map(|i| &index[i])
}

/// Walk one cell's record slice into [`AddressRecord`]s. Sequential records,
/// no length prefix; `osm_id` deltas start from 0 for the first record.
pub fn decode_address_cell(slice: &[u8]) -> Result<Vec<AddressRecord>, DecodeError> {
    let mut records = Vec::new();
    let mut p = 0usize;
    let mut prev_osm_id = 0i64;
    while p < slice.len() {
        let (delta, consumed) = decode_varint(slice, p)?;
        p += consumed;
        let osm_id = prev_osm_id.wrapping_add(zigzag_decode(delta));
        prev_osm_id = osm_id;
        let (housenumber, c) = decode_string_u16(slice, p)?;
        p += c;
        let (street, c) = decode_string_u16(slice, p)?;
        p += c;
        records.push(AddressRecord {
            osm_id,
            housenumber,
            street,
        });
    }
    Ok(records)
}

/// Extract the record byte-slice for `cell_id` from a decompressed merged
/// block. Block layout: `i32 center_lon, i32 center_lat, u32 cell_count`, then
/// `cell_count × (u64 cell_id, u32 rel_offset)`, then record data; a cell's
/// records span `rel_offset[i]..rel_offset[i+1]` (or record-data end).
pub fn merged_block_cell_slice(
    block: &[u8],
    cell_id: u64,
) -> Result<Option<Vec<AddressRecord>>, DecodeError> {
    let cell_count = read_u32(block, 8)? as usize;
    let table_start = 12usize;
    let data_start = table_start
        .checked_add(
            cell_count
                .checked_mul(12)
                .ok_or(DecodeError::UnexpectedEof {
                    offset: 8,
                    needed: usize::MAX,
                })?,
        )
        .ok_or(DecodeError::UnexpectedEof {
            offset: 8,
            needed: usize::MAX,
        })?;
    if block.len() < data_start {
        return Err(DecodeError::UnexpectedEof {
            offset: 12,
            needed: data_start,
        });
    }
    // Find the target cell's table index and its rel_offset; also the next
    // rel_offset to bound the slice.
    let mut found: Option<usize> = None;
    for i in 0..cell_count {
        let ent = table_start + i * 12;
        if read_u64(block, ent)? == cell_id {
            found = Some(i);
            break;
        }
    }
    let Some(i) = found else { return Ok(None) };
    let rel = read_u32(block, table_start + i * 12 + 8)? as usize;
    let end = if i + 1 < cell_count {
        read_u32(block, table_start + (i + 1) * 12 + 8)? as usize
    } else {
        block.len() - data_start
    };
    let start = data_start
        .checked_add(rel)
        .ok_or(DecodeError::UnexpectedEof {
            offset: 0,
            needed: usize::MAX,
        })?;
    let stop = data_start
        .checked_add(end)
        .ok_or(DecodeError::UnexpectedEof {
            offset: 0,
            needed: usize::MAX,
        })?;
    if stop > block.len() || start > stop {
        return Err(DecodeError::UnexpectedEof {
            offset: start,
            needed: stop,
        });
    }
    Ok(Some(decode_address_cell(&block[start..stop])?))
}

/// An opened `.address.ptiles` file over any [`PtilesSource`].
pub struct AddressFile<S: PtilesSource> {
    source: S,
    index: Vec<AddressIndexEntry>,
}

impl<S: PtilesSource> AddressFile<S> {
    /// Open + validate. Fails closed on non-`PTILESA` magic, unsupported
    /// version, or a structure that isn't an address file (`block_count == 0`
    /// or `aux_length != 0` → likely an admin file).
    pub fn open(source: S) -> Result<AddressFile<S>, FileError> {
        let mut header_buf = [0u8; HEADER_SIZE];
        source.read_exact_at(0, &mut header_buf)?;
        let header = Header::parse(&header_buf)?;
        if &header.magic != b"PTILESA" {
            return Err(FileError::BadMagic {
                found: header.magic,
            });
        }
        crate::versions::check_supported(&header.magic, header.version)
            .map_err(FileError::UnsupportedVersion)?;
        if header.block_count == 0 || header.aux_length != 0 {
            // Not an address (merged-block) file — likely admin.
            return Err(FileError::BadMagic {
                found: header.magic,
            });
        }
        let mut index_buf = alloc::vec![0u8; header.index_length as usize];
        source.read_exact_at(header.index_offset, &mut index_buf)?;
        let index = parse_v2_index(&index_buf)?;
        Ok(AddressFile { source, index })
    }

    /// The parsed v2 index.
    pub fn index(&self) -> &[AddressIndexEntry] {
        &self.index
    }

    /// Read + decompress the merged block for an index entry and return the
    /// records for its cell.
    fn records_for_entry(
        &self,
        entry: &AddressIndexEntry,
    ) -> Result<Vec<AddressRecord>, FileError> {
        let mut buf = alloc::vec![0u8; entry.block_length as usize];
        self.source.read_exact_at(entry.block_offset, &mut buf)?;
        let block = zstd_decompress(&buf)?;
        Ok(merged_block_cell_slice(&block, entry.h3_cell)?.unwrap_or_default())
    }

    /// All addresses in a specific H3 cell (empty if the cell isn't indexed).
    pub fn addresses_in_cell(&self, cell: u64) -> Result<Vec<AddressRecord>, FileError> {
        match index_search(&self.index, cell) {
            Some(entry) => self.records_for_entry(entry),
            None => Ok(Vec::new()),
        }
    }

    /// All addresses in the cell containing `(lat, lon)` plus ring-1 neighbors
    /// when `ring >= 1` (reverse lookup).
    pub fn addresses_at(
        &self,
        lat: f64,
        lon: f64,
        ring: u8,
    ) -> Result<Vec<AddressRecord>, FileError> {
        let center = crate::query::cell_for_coord(lat, lon);
        let mut cells = alloc::vec![center];
        if ring >= 1 {
            cells.extend(crate::query::neighbor_cells(center));
        }
        let mut out = Vec::new();
        for cell in cells {
            if let Some(entry) = index_search(&self.index, cell) {
                out.extend(self.records_for_entry(entry)?);
            }
        }
        Ok(out)
    }

    /// Forward lookup: addresses in/near `(lat, lon)` whose housenumber and
    /// street both fold-match the query (accent/case-insensitive via
    /// [`crate::business_search::fold_name`]). `street` matches by substring so
    /// `"broadway"` finds `"W Broadway"`; `housenumber` matches exactly (folded).
    pub fn find_address(
        &self,
        lat: f64,
        lon: f64,
        ring: u8,
        housenumber: &str,
        street: &str,
    ) -> Result<Vec<AddressRecord>, FileError> {
        let hn = crate::business_search::fold_name(housenumber.trim());
        let st = crate::business_search::fold_name(street.trim());
        let all = self.addresses_at(lat, lon, ring)?;
        Ok(all
            .into_iter()
            .filter(|r| {
                crate::business_search::fold_name(&r.housenumber) == hn
                    && crate::business_search::fold_name(&r.street).contains(&st)
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Build one record's bytes (delta osm_id from pid, u16 hn, u16 street).
    fn record(osm_id: i64, pid: i64, hn: &str, st: &str) -> Vec<u8> {
        let mut b = Vec::new();
        let delta = osm_id - pid;
        let zz = ((delta << 1) ^ (delta >> 63)) as u64;
        // minimal varint
        let mut v = zz;
        loop {
            let mut byte = (v & 0x7f) as u8;
            v >>= 7;
            if v != 0 {
                byte |= 0x80;
            }
            b.push(byte);
            if v == 0 {
                break;
            }
        }
        b.extend_from_slice(&(hn.len() as u16).to_le_bytes());
        b.extend_from_slice(hn.as_bytes());
        b.extend_from_slice(&(st.len() as u16).to_le_bytes());
        b.extend_from_slice(st.as_bytes());
        b
    }

    #[test]
    fn decode_cell_accumulates_delta_osm_ids() {
        let mut slice = Vec::new();
        slice.extend(record(1000, 0, "100", "Broadway"));
        slice.extend(record(1005, 1000, "102", "Broadway"));
        let recs = decode_address_cell(&slice).unwrap();
        assert_eq!(recs.len(), 2);
        assert_eq!(
            recs[0],
            AddressRecord {
                osm_id: 1000,
                housenumber: "100".into(),
                street: "Broadway".into()
            }
        );
        assert_eq!(recs[1].osm_id, 1005);
    }

    #[test]
    fn empty_cell_slice_is_empty() {
        assert!(decode_address_cell(&[]).unwrap().is_empty());
    }

    #[test]
    fn truncated_record_errors_not_panic() {
        // Valid varint then a truncated u16 length.
        let slice = [0x02u8, 0x05];
        assert!(decode_address_cell(&slice).is_err());
    }

    #[test]
    fn v2_index_entry_unpacks_packed_offset_and_length() {
        // Hand-pack an entry with a >48-bit offset and >16-bit length.
        let mut e = Vec::new();
        e.extend_from_slice(&42u64.to_le_bytes()); // h3_cell
        e.extend_from_slice(&(-100i32).to_le_bytes()); // min_lon
        e.extend_from_slice(&(-200i32).to_le_bytes()); // min_lat
        e.extend_from_slice(&300i32.to_le_bytes()); // max_lon
        e.extend_from_slice(&400i32.to_le_bytes()); // max_lat
        e.extend_from_slice(&[0x11, 0x22, 0x33, 0x44, 0x55, 0x66]); // offset_lo (6B)
        e.extend_from_slice(&0x0777u16.to_le_bytes()); // len_lo
        e.push(0x01); // offset_hi
        e.push(0x02); // len_hi
        e.extend_from_slice(&7u16.to_le_bytes()); // feature_count
        e.extend_from_slice(&3u16.to_le_bytes()); // cell_index
        let mut blob = 1u32.to_le_bytes().to_vec();
        blob.extend(e);
        let idx = parse_v2_index(&blob).unwrap();
        assert_eq!(idx.len(), 1);
        let x = idx[0];
        assert_eq!(x.h3_cell, 42);
        assert_eq!(x.block_offset, 0x0066_5544_3322_11 | (0x01u64 << 48));
        assert_eq!(x.block_length, 0x0777 | (0x02u32 << 16));
        assert_eq!(x.feature_count, 7);
        assert_eq!(x.cell_index, 3);
    }

    #[test]
    fn parse_v2_index_rejects_corrupt_count() {
        let mut blob = u32::MAX.to_le_bytes().to_vec();
        blob.extend_from_slice(&[0u8; 38]);
        assert!(parse_v2_index(&blob).is_err());
        assert!(parse_v2_index(&[]).is_err());
    }

    #[test]
    fn merged_block_slices_two_cells() {
        // Two cells, records for each; build the block by hand.
        let c0_recs = {
            let mut v = Vec::new();
            v.extend(record(10, 0, "1", "A St"));
            v.extend(record(12, 10, "3", "A St"));
            v
        };
        let c1_recs = record(99, 0, "9", "B Ave");
        let mut block = Vec::new();
        block.extend_from_slice(&0i32.to_le_bytes()); // center_lon
        block.extend_from_slice(&0i32.to_le_bytes()); // center_lat
        block.extend_from_slice(&2u32.to_le_bytes()); // cell_count
        // cell table: (cell_id, rel_offset)
        block.extend_from_slice(&100u64.to_le_bytes());
        block.extend_from_slice(&0u32.to_le_bytes());
        block.extend_from_slice(&200u64.to_le_bytes());
        block.extend_from_slice(&(c0_recs.len() as u32).to_le_bytes());
        block.extend_from_slice(&c0_recs);
        block.extend_from_slice(&c1_recs);

        let a = merged_block_cell_slice(&block, 100).unwrap().unwrap();
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].housenumber, "1");
        assert_eq!(a[1].osm_id, 12);
        let b = merged_block_cell_slice(&block, 200).unwrap().unwrap();
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].street, "B Ave");
        // Missing cell -> None, not error.
        assert!(merged_block_cell_slice(&block, 999).unwrap().is_none());
    }
}
