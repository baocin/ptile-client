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
    DecodeError, decode_string_u16, decode_varint, read_i16, read_i32, read_u16, read_u32,
    read_u64,
    zigzag_decode,
};
use crate::file::FileError;
use crate::header::{HEADER_SIZE, Header};
use crate::source::PtilesSource;

/// Bytes per v2 index entry.
pub const V2_INDEX_ENTRY_SIZE: usize = 38;

/// One decoded address record. `osm_id` is the absolute id (deltas already
/// accumulated).
///
/// v2 records carry their own position as an `i16` offset from the block
/// centre, so `lat`/`lon` are the address itself. v1 records carry none and
/// leave both `None`; the only location available there is the containing
/// cell, which is ~5 km across and useless for "which house is this".
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AddressRecord {
    /// The OSM element id when the record came from OSM, `0` otherwise.
    ///
    /// Not a key, and not resolvable on its own. Nodes and ways have separate
    /// id spaces in OSM and this field records neither which one nor the
    /// element type, so `130905893` names both a node and a way; v3's bulk
    /// records all carry `0`, so millions share the value by design. It exists
    /// for the delta chain and the in-cell sort, not for identity -- build a
    /// link from a record's position instead.
    pub osm_id: i64,
    pub housenumber: String,
    pub street: String,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    /// Where the record came from. v3 stores this per record; v1/v2 files
    /// predate the bulk sources entirely, so their records are all
    /// [`AddressSource::Osm`] by construction rather than by a stored byte.
    pub source: AddressSource,
}

/// Which corpus an address came from.
///
/// OSM address coverage is wildly uneven — 5.4M records in New York against
/// 137k in Tennessee — so a merged layer mixes an authoritative municipal
/// import with a hand-mapped point on the same street. A caller that wants to
/// show, filter or age records differently by origin cannot recover this after
/// the merge, which is why it is stored rather than inferred.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
pub enum AddressSource {
    #[default]
    Osm,
    /// USDOT National Address Database.
    Nad,
    /// OpenAddresses collected run.
    OpenAddresses,
    /// A source byte this build does not know. Kept rather than rejected: a
    /// later builder adding a fourth corpus must not make the whole cell
    /// undecodable for an older client.
    Unknown(u8),
}

impl AddressSource {
    /// Stable lower-case name, for JSON output and FFI records where an enum
    /// would force every consumer to carry its own mapping.
    pub fn name(&self) -> &'static str {
        match self {
            AddressSource::Osm => "osm",
            AddressSource::Nad => "nad",
            AddressSource::OpenAddresses => "openaddresses",
            AddressSource::Unknown(_) => "unknown",
        }
    }

    pub fn from_byte(b: u8) -> AddressSource {
        match b {
            0 => AddressSource::Osm,
            1 => AddressSource::Nad,
            2 => AddressSource::OpenAddresses,
            other => AddressSource::Unknown(other),
        }
    }
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
///
/// `centre_micro` is the block's `(center_lon, center_lat)` in 1e-5 degrees,
/// present on v2 and later files, whose records carry `i16 off_lon, off_lat`
/// between the osm_id and the strings. Pass `None` for a v1 file, which has no
/// such bytes. `with_source` adds the v3 provenance byte after the offsets.
///
/// This used to take the slice alone and read the strings straight after the
/// osm_id. On a v2 record that lands on the coordinate bytes: `off_lon` is
/// consumed as the housenumber's `u16` length, so a westward offset of -73
/// reads as a 65,463-byte string and the decode dies at offset 7. Every
/// published address file is v2, so the layer did not decode at all.
pub fn decode_address_cell(
    slice: &[u8],
    centre_micro: Option<(i32, i32)>,
    with_source: bool,
) -> Result<Vec<AddressRecord>, DecodeError> {
    let mut records = Vec::new();
    let mut p = 0usize;
    let mut prev_osm_id = 0i64;
    while p < slice.len() {
        let (delta, consumed) = decode_varint(slice, p)?;
        p += consumed;
        let osm_id = prev_osm_id.wrapping_add(zigzag_decode(delta));
        prev_osm_id = osm_id;

        let (lat, lon) = match centre_micro {
            Some((c_lon, c_lat)) => {
                let off_lon = read_i16(slice, p)?;
                let off_lat = read_i16(slice, p + 2)?;
                p += 4;
                (
                    Some((c_lat as f64 + off_lat as f64) / 100_000.0),
                    Some((c_lon as f64 + off_lon as f64) / 100_000.0),
                )
            }
            None => (None, None),
        };

        // v3's one added byte, placed after the offsets so that everything a
        // v2 reader knows how to find still sits where it expects it.
        let source = if with_source {
            let b = *slice.get(p).ok_or(DecodeError::UnexpectedEof {
                offset: p,
                needed: 1,
            })?;
            p += 1;
            AddressSource::from_byte(b)
        } else {
            AddressSource::Osm
        };

        let (housenumber, c) = decode_string_u16(slice, p)?;
        p += c;
        let (street, c) = decode_string_u16(slice, p)?;
        p += c;
        records.push(AddressRecord {
            osm_id,
            housenumber,
            street,
            lat,
            lon,
            source,
        });
    }
    Ok(records)
}

/// Extract the record byte-slice for `cell_id` from a decompressed merged
/// block. Block layout: `i32 center_lon, i32 center_lat, u32 cell_count`, then
/// `cell_count × (u64 cell_id, u32 rel_offset)`, then record data; a cell's
/// records span `rel_offset[i]..rel_offset[i+1]` (or record-data end).
/// `version` must come from the file header: v2 and later carry per-record
/// positions, v3 adds a provenance byte, v1 has neither. The block itself does
/// not say, and guessing wrong silently mangles the other format -- so the
/// caller, which read the header, is made to answer.
pub fn merged_block_cell_slice(
    block: &[u8],
    cell_id: u64,
    version: u8,
) -> Result<Option<Vec<AddressRecord>>, DecodeError> {
    let has_coords = version >= 2;
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
    // Offsets are relative to *this cell's* centre, not the block header's.
    //
    // The block header carries a centre too -- the first cell's -- and reading
    // the offsets against that is what this decoder used to do. A block holds
    // eight cells, so seven in every eight decoded kilometres from where they
    // belong: OSM way 130905893 ("919 Broadway", Nashville) sits at
    // 36.15770,-86.78416 and came back as 36.13647,-86.78984, 2.4 km south,
    // with its number and street perfectly intact. The reference builder is
    // explicit that it measures from each cell's own centre ("per-cell keeps
    // the deltas small and the decode independent of how cells were batched").
    //
    // `try_cell_center` rather than `cell_center`: the latter answers null
    // island for an id it cannot parse, which would swap a 2 km error for a
    // 9,700 km one. An unparseable cell id means the block's own table is
    // wrong, so refuse instead.
    let centre = if has_coords {
        let (clat, clon) =
            crate::query::try_cell_center(cell_id).ok_or(DecodeError::UnexpectedEof {
                offset: 0,
                needed: 0,
            })?;
        Some((
            crate::math::round(clon * 100_000.0) as i32,
            crate::math::round(clat * 100_000.0) as i32,
        ))
    } else {
        None
    };
    Ok(Some(decode_address_cell(
        &block[start..stop],
        centre,
        version >= 3,
    )?))
}

/// Distance from a point to a cell's index bounding box, in metres; zero when
/// the point is inside it. Used to visit cells nearest-first without
/// decompressing anything -- the index is the only geometry available up front.
fn bbox_distance_m(entry: &AddressIndexEntry, lat: f64, lon: f64) -> f64 {
    let (min_lat, max_lat) = (entry.min_lat as f64 / 1e5, entry.max_lat as f64 / 1e5);
    let (min_lon, max_lon) = (entry.min_lon as f64 / 1e5, entry.max_lon as f64 / 1e5);
    let clamped_lat = lat.clamp(min_lat.min(max_lat), max_lat.max(min_lat));
    let clamped_lon = lon.clamp(min_lon.min(max_lon), max_lon.max(min_lon));
    crate::proximity::haversine_distance_m(lat, lon, clamped_lat, clamped_lon)
}

/// Distance from a point to a record, or infinity for a v1 record with no
/// position -- which sorts it last rather than pretending it is at null island.
fn record_distance_m(r: &AddressRecord, lat: f64, lon: f64) -> f64 {
    match (r.lat, r.lon) {
        (Some(rlat), Some(rlon)) => crate::proximity::haversine_distance_m(lat, lon, rlat, rlon),
        _ => f64::INFINITY,
    }
}

/// An opened `.address.ptiles` file over any [`PtilesSource`].
pub struct AddressFile<S: PtilesSource> {
    source: S,
    index: Vec<AddressIndexEntry>,
    /// The layer's zstd dictionary, empty when the file ships without one.
    /// Blocks are compressed against it, so a reader that skips it decodes
    /// nothing at all — which is what this reader did until now, on every real
    /// state file (the builder emits an 8 KiB dictionary).
    dict: Vec<u8>,
    /// Header version: v2 added per-record positions, v3 a provenance byte.
    /// Read at open, because the blocks themselves do not say.
    version: u8,
}

impl<S: PtilesSource> AddressFile<S> {
    /// Open + validate. Fails closed on a magic that is neither `PTILESD` (the
    /// address magic) nor `PTILESA` (what address files carried while the
    /// builder truncated `PTILESA2` to the *admin* magic), an unsupported
    /// version, or a structure that isn't an address file (`block_count == 0`
    /// or `aux_length != 0` → likely an actual admin file).
    pub fn open(source: S) -> Result<AddressFile<S>, FileError> {
        let mut header_buf = [0u8; HEADER_SIZE];
        source.read_exact_at(0, &mut header_buf)?;
        let header = Header::parse(&header_buf)?;
        if &header.magic != b"PTILESD" && &header.magic != b"PTILESA" {
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
        let dict = if header.dict_length > 0 {
            let mut buf = alloc::vec![0u8; header.dict_length as usize];
            source.read_exact_at(header.dict_offset, &mut buf)?;
            buf
        } else {
            Vec::new()
        };
        Ok(AddressFile {
            source,
            index,
            dict,
            version: header.version,
        })
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
        let block = crate::file::decompress_with_dict_fallback(&buf, &self.dict)
            .map_err(|message| FileError::Decompress {
                offset: entry.block_offset,
                message,
            })?;
        Ok(merged_block_cell_slice(&block, entry.h3_cell, self.version)?.unwrap_or_default())
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
    /// Forward geocode with no location hint: "919 Broadway" -> where it is.
    ///
    /// [`find_address`](Self::find_address) can only search cells the caller
    /// already knows to name, which makes it a viewport filter rather than a
    /// geocoder -- typing an address while looking at the other end of the
    /// state finds nothing. This walks the whole file instead.
    ///
    /// That costs a full decompress (~31 MB and 4M records for Tennessee's v3
    /// file), because the layer has no name index; `near` is what keeps it off
    /// that worst case in practice. With a hint, cells are visited nearest
    /// first -- by their index bounding boxes, which is the only geometry
    /// available before decompressing anything -- and the walk stops as soon as
    /// `limit` matches are in hand, so a local search usually touches a handful
    /// of blocks. Results are then ordered by true distance from the hint.
    pub fn search_address(
        &self,
        housenumber: &str,
        street: &str,
        near: Option<(f64, f64)>,
        limit: usize,
    ) -> Result<Vec<AddressRecord>, FileError> {
        let hn = crate::business_search::fold_name(housenumber.trim());
        let st = crate::business_search::fold_name(street.trim());
        // With neither part supplied every record matches, which is a file
        // dump rather than a search.
        if hn.is_empty() && st.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }

        let mut order: Vec<usize> = (0..self.index.len()).collect();
        if let Some((lat, lon)) = near {
            order.sort_by(|&a, &b| {
                bbox_distance_m(&self.index[a], lat, lon)
                    .total_cmp(&bbox_distance_m(&self.index[b], lat, lon))
            });
        }

        let mut out: Vec<AddressRecord> = Vec::new();
        for i in order {
            let entry = &self.index[i];
            if entry.block_length == 0 {
                continue;
            }
            // Cells are in bbox-distance order, so once `limit` hits are held
            // and the next cell's nearest possible point is further than the
            // worst of them, nothing left to read can improve the answer.
            // Without this the walk reads the whole state even when the match
            // is under the cursor: 14.7 s for Tennessee against 0.2 s here.
            if let Some((lat, lon)) = near {
                if out.len() >= limit {
                    let worst = out
                        .iter()
                        .map(|r| record_distance_m(r, lat, lon))
                        .fold(0.0_f64, f64::max);
                    if bbox_distance_m(entry, lat, lon) > worst {
                        break;
                    }
                }
            }
            for r in self.records_for_entry(entry)? {
                if !hn.is_empty() && crate::business_search::fold_name(&r.housenumber) != hn {
                    continue;
                }
                if !st.is_empty() && !crate::business_search::fold_name(&r.street).contains(&st) {
                    continue;
                }
                out.push(r);
            }
        }

        if let Some((lat, lon)) = near {
            out.sort_by(|a, b| {
                record_distance_m(a, lat, lon).total_cmp(&record_distance_m(b, lat, lon))
            });
        }
        out.truncate(limit);
        Ok(out)
    }

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

    // A cell lifted straight out of the published TN.address_v2.ptiles, with
    // the values the Python reader produces for the same bytes. Before the
    // coordinate bytes were read, this input errored at offset 7 claiming it
    // needed 65,463 more bytes -- the i16 lon offset -73 read as a u16 string
    // length. Every published address file is v2, so nothing decoded.
    #[test]
    fn decodes_a_real_v2_cell_with_positions() {
        let bytes = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../test-fixtures/golden/address_v2.cell.bin"
        ))
        .unwrap();
        let centre = (-8_452_193i32, 3_536_743i32); // block header, 1e-5 deg
        let recs = decode_address_cell(&bytes, Some(centre), false).unwrap();
        assert_eq!(recs.len(), 2, "python reads 2 records from this cell");

        assert_eq!(recs[0].osm_id, 568_392_734);
        assert_eq!(recs[0].housenumber, "347");
        assert_eq!(recs[0].street, "N Industrial Road");
        assert!((recs[0].lat.unwrap() - 35.36934).abs() < 1e-5, "lat {:?}", recs[0].lat);
        assert!((recs[0].lon.unwrap() - -84.52266).abs() < 1e-5, "lon {:?}", recs[0].lon);

        assert_eq!(recs[1].osm_id, 568_392_735);
        assert_eq!(recs[1].housenumber, "134");
        assert_eq!(recs[1].street, "Waupaca Drive");
        assert!((recs[1].lat.unwrap() - 35.36067).abs() < 1e-5);
        assert!((recs[1].lon.unwrap() - -84.52269).abs() < 1e-5);
    }

    #[test]
    fn v2_bytes_read_as_v1_do_not_silently_succeed() {
        // The old behaviour, pinned: skipping the coordinate bytes on a v2
        // record must fail loudly rather than return a plausible wrong string.
        let bytes = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../test-fixtures/golden/address_v2.cell.bin"
        ))
        .unwrap();
        assert!(decode_address_cell(&bytes, None, false).is_err());
    }

    #[test]
    fn decode_cell_accumulates_delta_osm_ids() {
        let mut slice = Vec::new();
        slice.extend(record(1000, 0, "100", "Broadway"));
        slice.extend(record(1005, 1000, "102", "Broadway"));
        // The `record` helper builds v1-shaped bytes (no coordinates).
        let recs = decode_address_cell(&slice, None, false).unwrap();
        assert_eq!(recs.len(), 2);
        assert_eq!(
            recs[0],
            AddressRecord {
                osm_id: 1000,
                housenumber: "100".into(),
                street: "Broadway".into(),
                lat: None,
                lon: None,
                source: AddressSource::Osm,
            }
        );
        assert_eq!(recs[1].osm_id, 1005);
    }

    #[test]
    fn empty_cell_slice_is_empty() {
        assert!(decode_address_cell(&[], None, false).unwrap().is_empty());
    }

    #[test]
    fn truncated_record_errors_not_panic() {
        // Valid varint then a truncated u16 length.
        let slice = [0x02u8, 0x05];
        assert!(decode_address_cell(&slice, None, false).is_err());
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

        let a = merged_block_cell_slice(&block, 100, 1).unwrap().unwrap();
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].housenumber, "1");
        assert_eq!(a[1].osm_id, 12);
        let b = merged_block_cell_slice(&block, 200, 1).unwrap().unwrap();
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].street, "B Ave");
        // Missing cell -> None, not error.
        assert!(merged_block_cell_slice(&block, 999, 1).unwrap().is_none());
    }
}
