//! Admin layer decoder (`PTILESA`, `US.admin.ptiles`).
//!
//! Unlike every other layer this is NOT block-per-cell — it's a lookup-grid
//! layer. Ported from the reference decoder `ptiles/ptiles/admin.py` (the real
//! contract; SPEC.md's polygon-record prose is wrong). The 256-byte header's
//! section pointers are repurposed:
//! - `dict_offset/dict_length` → zstd-compressed **string tables** (5 of them:
//!   country, state, county, zip, tz)
//! - `index_offset/index_length` → zstd-compressed **boundary polygon table**
//! - `aux_offset/aux_length` → uncompressed **H3 res-7 lookup grid** (sorted by
//!   cell for binary search)
//!
//! Note admin shares its on-disk 7-byte magic `PTILESA` with the address layer
//! (`PTILESA2` truncates to `PTILESA` via the reference `write_header`'s
//! `magic[:7]`). The two are distinguished by structure — admin has
//! `block_count == 0` and `aux_length > 0` — and by filename.

use alloc::string::String;
use alloc::vec::Vec;

use crate::codec::{
    DecodeError, decode_string_u16, read_i32, read_u8, read_u16, read_u32, read_u64,
};
use crate::file::{FileError, zstd_decompress};
use crate::header::{HEADER_SIZE, Header};
use crate::source::PtilesSource;

/// Bytes per lookup-grid entry: `h3_cell(8) + country(1) + state(1) +
/// county(2) + zip(2) + tz(1) + flags(1)`.
pub const GRID_ENTRY_SIZE: usize = 16;

/// One H3-cell → jurisdiction-indices entry from the lookup grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdminGridEntry {
    pub h3_cell: u64,
    pub country_idx: u8,
    pub state_idx: u8,
    pub county_idx: u16,
    pub zip_idx: u16,
    pub tz_idx: u8,
    /// bit 0x01 state / 0x02 county / 0x04 zip / 0x08 tz boundary straddle.
    pub boundary_flags: u8,
}

/// Resolved jurisdiction for a point — the answer to "where am I?".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AdminInfo {
    pub country: String,
    pub state: String,
    pub county: String,
    pub zip: String,
    pub timezone: String,
    pub boundary_flags: u8,
}

/// The 5 string tables, in the on-disk order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AdminStringTables {
    pub country: Vec<String>,
    pub state: Vec<String>,
    pub county: Vec<String>,
    pub zip: Vec<String>,
    pub tz: Vec<String>,
}

/// A boundary polygon (for rendering; not needed for point lookup).
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AdminPolygon {
    pub name: String,
    /// OSM admin level, `None` when the file does not say.
    ///
    /// The writer stores no level field. [`AdminFile::polygons`] resolves it
    /// exactly by testing the ring name against the state string table;
    /// [`decode_polygons`] on its own has no table to test against and falls
    /// back to the `" County"` suffix, so it reports a parish, borough or
    /// independent city as unknown rather than misfiling it as top-level.
    pub admin_level: Option<u8>,
    /// Index into [`AdminStringTables::state`]. County names repeat across
    /// states (two "Davidson County"), so this is the only disambiguator.
    pub state_idx: u8,
    /// `(lon, lat)` degrees.
    pub coords: Vec<[f64; 2]>,
}

/// Decode one `u32`-counted, `u16`-strlen string table at `pos`; returns the
/// strings and bytes consumed.
fn decode_string_table(data: &[u8], pos: usize) -> Result<(Vec<String>, usize), DecodeError> {
    let start = pos;
    let count = read_u32(data, pos)? as usize;
    let mut p = pos + 4;
    // Cap the pre-allocation: `count` is untrusted, but each string is ≥2
    // bytes, so a real table can't have more entries than remaining/2.
    let mut strings = Vec::with_capacity(count.min(1 << 16));
    for _ in 0..count {
        let (s, consumed) = decode_string_u16(data, p)?;
        strings.push(s);
        p += consumed;
    }
    Ok((strings, p - start))
}

/// Decode all 5 string tables (country, state, county, zip, tz) from the
/// decompressed `dict` section.
pub fn decode_string_tables(data: &[u8]) -> Result<AdminStringTables, DecodeError> {
    let mut p = 0usize;
    let (country, c) = decode_string_table(data, p)?;
    p += c;
    let (state, c) = decode_string_table(data, p)?;
    p += c;
    let (county, c) = decode_string_table(data, p)?;
    p += c;
    let (zip, c) = decode_string_table(data, p)?;
    p += c;
    let (tz, _c) = decode_string_table(data, p)?;
    Ok(AdminStringTables {
        country,
        state,
        county,
        zip,
        tz,
    })
}

fn decode_grid_entry(data: &[u8], pos: usize) -> Result<AdminGridEntry, DecodeError> {
    Ok(AdminGridEntry {
        h3_cell: read_u64(data, pos)?,
        country_idx: read_u8(data, pos + 8)?,
        state_idx: read_u8(data, pos + 9)?,
        county_idx: read_u16(data, pos + 10)?,
        zip_idx: read_u16(data, pos + 12)?,
        tz_idx: read_u8(data, pos + 14)?,
        boundary_flags: read_u8(data, pos + 15)?,
    })
}

/// Decode the uncompressed `aux` section: `u32 count` + `count × 16-byte`
/// entries, sorted by `h3_cell`. Allocation is guarded against a corrupt count.
pub fn decode_grid(data: &[u8]) -> Result<Vec<AdminGridEntry>, DecodeError> {
    let count = read_u32(data, 0)? as usize;
    let needed = count
        .checked_mul(GRID_ENTRY_SIZE)
        .and_then(|n| n.checked_add(4))
        .ok_or(DecodeError::UnexpectedEof {
            offset: 0,
            needed: usize::MAX,
        })?;
    if data.len() < needed {
        return Err(DecodeError::UnexpectedEof { offset: 0, needed });
    }
    let mut grid = Vec::with_capacity(count);
    let mut p = 4usize;
    for _ in 0..count {
        grid.push(decode_grid_entry(data, p)?);
        p += GRID_ENTRY_SIZE;
    }
    Ok(grid)
}

/// Binary-search a grid (sorted by `h3_cell`) for `cell`.
pub fn binary_search_grid(grid: &[AdminGridEntry], cell: u64) -> Option<&AdminGridEntry> {
    grid.binary_search_by(|e| e.h3_cell.cmp(&cell))
        .ok()
        .map(|i| &grid[i])
}

/// Decode the decompressed `index` section into boundary polygons
/// (`build_admin.py:357-365`: `u32 count`, per-poly `u8 state_idx`,
/// `u16 name`, `u32 vertex_count`, then absolute `i32 lon`/`i32 lat` pairs).
pub fn decode_polygons(data: &[u8]) -> Result<Vec<AdminPolygon>, DecodeError> {
    let count = read_u32(data, 0)? as usize;
    let mut p = 4usize;
    let mut polygons = Vec::with_capacity(count.min(1 << 16));
    for _ in 0..count {
        let state_idx = read_u8(data, p)?;
        p += 1;
        let (name, consumed) = decode_string_u16(data, p)?;
        p += consumed;
        let vertex_count = read_u32(data, p)? as usize;
        p += 4;
        let mut coords = Vec::with_capacity(vertex_count.min(1 << 16));
        for _ in 0..vertex_count {
            let lon = read_i32(data, p)?;
            let lat = read_i32(data, p + 4)?;
            p += 8;
            coords.push([lon as f64 / 100_000.0, lat as f64 / 100_000.0]);
        }
        let admin_level = name.ends_with(" County").then_some(6);
        polygons.push(AdminPolygon {
            name,
            admin_level,
            state_idx,
            coords,
        });
    }
    Ok(polygons)
}

/// Grid + string tables — everything needed to answer point → jurisdiction.
#[derive(Clone, Debug)]
pub struct AdminLookup {
    pub grid: Vec<AdminGridEntry>,
    pub tables: AdminStringTables,
}

impl AdminLookup {
    /// Resolve a grid entry's indices into strings.
    pub fn resolve(&self, e: &AdminGridEntry) -> AdminInfo {
        let get = |v: &[String], i: usize| v.get(i).cloned().unwrap_or_default();
        AdminInfo {
            country: get(&self.tables.country, e.country_idx as usize),
            state: get(&self.tables.state, e.state_idx as usize),
            county: get(&self.tables.county, e.county_idx as usize),
            zip: get(&self.tables.zip, e.zip_idx as usize),
            timezone: get(&self.tables.tz, e.tz_idx as usize),
            boundary_flags: e.boundary_flags,
        }
    }

    /// Jurisdiction for an H3 res-7 cell, or `None` if the grid has no entry.
    pub fn lookup_cell(&self, cell: u64) -> Option<AdminInfo> {
        binary_search_grid(&self.grid, cell).map(|e| self.resolve(e))
    }

    /// Jurisdiction for a coordinate (resolves the cell via
    /// [`crate::cell_for_coord`]).
    pub fn lookup_coord(&self, lat: f64, lon: f64) -> Option<AdminInfo> {
        self.lookup_cell(crate::query::cell_for_coord(lat, lon))
    }
}

/// An opened `.admin.ptiles` file over any [`PtilesSource`]. Reads and decodes
/// the string tables + lookup grid eagerly (they answer point queries) and
/// keeps the compressed polygon section for lazy [`AdminFile::polygons`].
pub struct AdminFile {
    lookup: AdminLookup,
    polygons_compressed: Vec<u8>,
}

impl AdminFile {
    /// Open and validate an admin file. Fails closed on a non-`PTILESA` magic,
    /// an unsupported version, or a structure that isn't a lookup-grid layer
    /// (guards against an address `PTILESA2` file, which truncates to the same
    /// 7-byte magic but has `block_count > 0` / `aux_length == 0`).
    pub fn open<S: PtilesSource>(source: S) -> Result<AdminFile, FileError> {
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
        if header.block_count != 0 || header.aux_length == 0 {
            // Not a lookup-grid admin file (likely an address `PTILESA2`).
            return Err(FileError::BadMagic {
                found: header.magic,
            });
        }

        let read_section = |offset: u64, len: u32| -> Result<Vec<u8>, FileError> {
            let mut buf = alloc::vec![0u8; len as usize];
            source.read_exact_at(offset, &mut buf)?;
            Ok(buf)
        };

        let dict_raw = read_section(header.dict_offset, header.dict_length)?;
        let tables = decode_string_tables(&zstd_decompress(&dict_raw)?)?;

        let grid_raw = read_section(header.aux_offset, header.aux_length as u32)?;
        let grid = decode_grid(&grid_raw)?;

        let polygons_compressed = read_section(header.index_offset, header.index_length)?;

        Ok(AdminFile {
            lookup: AdminLookup { grid, tables },
            polygons_compressed,
        })
    }

    /// The point-lookup surface.
    pub fn lookup(&self) -> &AdminLookup {
        &self.lookup
    }

    /// Jurisdiction for a coordinate.
    pub fn admin_at(&self, lat: f64, lon: f64) -> Option<AdminInfo> {
        self.lookup.lookup_coord(lat, lon)
    }

    /// Decode the boundary polygons (decompressed on demand).
    ///
    /// The file carries state and sub-state rings in one table with no level
    /// field. With the string tables in hand the split is exact rather than a
    /// name-suffix guess: a ring named after a state is level 4, anything else
    /// is a county-equivalent (parish, borough, census area, independent city).
    pub fn polygons(&self) -> Result<Vec<AdminPolygon>, DecodeError> {
        let mut polys = decode_polygons(&zstd_decompress(&self.polygons_compressed)?)?;
        for p in &mut polys {
            p.admin_level = Some(if self.lookup.tables.state.contains(&p.name) {
                4
            } else {
                6
            });
        }
        Ok(polys)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Build a synthetic string table: u32 count + u16-len strings.
    fn string_table(strings: &[&str]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(strings.len() as u32).to_le_bytes());
        for s in strings {
            out.extend_from_slice(&(s.len() as u16).to_le_bytes());
            out.extend_from_slice(s.as_bytes());
        }
        out
    }

    fn grid_entry(cell: u64, c: u8, s: u8, co: u16, z: u16, tz: u8, flags: u8) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&cell.to_le_bytes());
        out.push(c);
        out.push(s);
        out.extend_from_slice(&co.to_le_bytes());
        out.extend_from_slice(&z.to_le_bytes());
        out.push(tz);
        out.push(flags);
        out
    }

    fn grid_blob(entries: &[Vec<u8>]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        for e in entries {
            out.extend_from_slice(e);
        }
        out
    }

    fn tables() -> AdminStringTables {
        let mut blob = Vec::new();
        blob.extend(string_table(&["United States"])); // country
        blob.extend(string_table(&["", "Tennessee"])); // state (idx 1)
        blob.extend(string_table(&["", "", "Davidson"])); // county (idx 2)
        blob.extend(string_table(&["37201", "37203"])); // zip
        blob.extend(string_table(&["America/Chicago"])); // tz
        decode_string_tables(&blob).unwrap()
    }

    #[test]
    fn string_tables_decode_in_order() {
        let t = tables();
        assert_eq!(t.country, ["United States"]);
        assert_eq!(t.state[1], "Tennessee");
        assert_eq!(t.county[2], "Davidson");
        assert_eq!(t.zip, ["37201", "37203"]);
        assert_eq!(t.tz, ["America/Chicago"]);
    }

    #[test]
    fn grid_decodes_and_binary_searches() {
        // Entries must be sorted by cell for binary search.
        let blob = grid_blob(&[
            grid_entry(100, 0, 1, 2, 1, 0, 0x01),
            grid_entry(200, 0, 1, 2, 0, 0, 0x00),
            grid_entry(300, 0, 1, 2, 1, 0, 0x04),
        ]);
        let grid = decode_grid(&blob).unwrap();
        assert_eq!(grid.len(), 3);
        assert_eq!(binary_search_grid(&grid, 200).unwrap().zip_idx, 0);
        assert_eq!(binary_search_grid(&grid, 300).unwrap().boundary_flags, 0x04);
        assert!(binary_search_grid(&grid, 250).is_none());
    }

    #[test]
    fn lookup_resolves_full_jurisdiction() {
        let grid = decode_grid(&grid_blob(&[grid_entry(42, 0, 1, 2, 1, 0, 0x02)])).unwrap();
        let lk = AdminLookup {
            grid,
            tables: tables(),
        };
        let info = lk.lookup_cell(42).unwrap();
        assert_eq!(info.country, "United States");
        assert_eq!(info.state, "Tennessee");
        assert_eq!(info.county, "Davidson");
        assert_eq!(info.zip, "37203");
        assert_eq!(info.timezone, "America/Chicago");
        assert_eq!(info.boundary_flags, 0x02);
        assert!(lk.lookup_cell(999).is_none());
    }

    #[test]
    fn out_of_range_index_resolves_to_empty_string_not_panic() {
        // county_idx 99 is past the table; resolve must yield "" not panic.
        let grid = decode_grid(&grid_blob(&[grid_entry(1, 0, 1, 99, 0, 0, 0)])).unwrap();
        let lk = AdminLookup {
            grid,
            tables: tables(),
        };
        assert_eq!(lk.lookup_cell(1).unwrap().county, "");
    }

    #[test]
    fn decode_grid_rejects_corrupt_count_without_overalloc() {
        // count = u32::MAX but only a few bytes: must Err, not try to allocate.
        let mut blob = u32::MAX.to_le_bytes().to_vec();
        blob.extend_from_slice(&[0u8; 16]);
        assert!(decode_grid(&blob).is_err());
    }

    #[test]
    fn truncated_grid_and_tables_error_not_panic() {
        assert!(decode_grid(&[]).is_err());
        assert!(decode_grid(&[1, 0, 0, 0]).is_err()); // count 1, no entry bytes
        assert!(decode_string_tables(&[]).is_err());
        assert!(decode_string_tables(&[2, 0, 0, 0]).is_err()); // count 2, no strings
    }

    #[test]
    fn polygons_decode_absolute_microdegree_coords() {
        let mut blob = 1u32.to_le_bytes().to_vec(); // 1 polygon
        blob.push(1); // state_idx
        blob.extend_from_slice(&4u16.to_le_bytes());
        blob.extend_from_slice(b"Zone");
        blob.extend_from_slice(&2u32.to_le_bytes()); // 2 vertices
        blob.extend_from_slice(&(-8_679_367i32).to_le_bytes());
        blob.extend_from_slice(&3_616_076i32.to_le_bytes());
        blob.extend_from_slice(&(-8_679_000i32).to_le_bytes());
        blob.extend_from_slice(&3_616_500i32.to_le_bytes());
        let polys = decode_polygons(&blob).unwrap();
        assert_eq!(polys.len(), 1);
        assert_eq!(polys[0].name, "Zone");
        assert_eq!(polys[0].admin_level, None);
        assert_eq!(polys[0].coords[0], [-86.79367, 36.16076]);
    }

    #[test]
    fn only_the_county_suffix_yields_a_level() {
        let ring = |name: &str| {
            let mut blob = 1u32.to_le_bytes().to_vec();
            blob.push(1);
            blob.extend_from_slice(&(name.len() as u16).to_le_bytes());
            blob.extend_from_slice(name.as_bytes());
            blob.extend_from_slice(&1u32.to_le_bytes());
            blob.extend_from_slice(&0i32.to_le_bytes());
            blob.extend_from_slice(&0i32.to_le_bytes());
            decode_polygons(&blob).unwrap().remove(0).admin_level
        };
        assert_eq!(ring("Davidson County"), Some(6));
        assert_eq!(ring("Tennessee"), None);
        assert_eq!(ring("Orleans Parish"), None);
    }
}

