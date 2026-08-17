//! Every business name in a state, folded and flat, for substring search.
//!
//! The name index next to this one buckets records by the **first letter of
//! the name** into 28 zstd blocks, each holding whole records: name, position,
//! uid, category and flags. That makes a prefix query cheap and a substring
//! query incomplete -- `affle` reads the `a` bucket and `Waffle House` lives
//! in `w` -- and makes completeness expensive, because reading all 28 decodes
//! 829,528 records to look at their names. Measured on the published Tennessee
//! sidecar: 53-122 ms for two buckets, 1050-1480 ms for all of them.
//!
//! This section holds nothing but the names, already folded, in one blob, with
//! the positions in a parallel array. A query is then a byte substring match
//! over text that needs no normalising, and the answer is complete by
//! construction: there are no buckets to miss.
//!
//! Measured on the same 829,528 Tennessee names, through this crate rather
//! than through `zstd` and `grep` -- which flattered it by about five times,
//! since `ruzstd` is a pure-Rust decoder and the standard library's substring
//! matcher does not use SIMD:
//!
//! | | size | first use | per query |
//! |---|---|---|---|
//! | bucket index, two blocks | 24 MB | -- | 53-122 ms, incomplete |
//! | bucket index, all 28 | 24 MB | -- | 1050-1480 ms |
//! | this section | 8.5 MB | 135 ms | 35 ms, complete |
//!
//! The 135 ms is decompression, paid once and kept: a search box is asked the
//! same question with one more letter on it, and the second keystroke costs
//! only the scan.
//!
//! Level 12 rather than 19 on purpose: 19 is 0.4 MB smaller and *slower* to
//! decompress, because the larger window costs more to replay than it saves in
//! bytes.
//!
//! Half the 8.5 MB is the position array. Storing a record id instead and
//! resolving positions for the handful of hits actually shown would nearly
//! halve it again; that needs an id the business layer can be looked up by,
//! which it does not have yet.
//!
//! Layout, little-endian, in the file's aux section:
//!
//! ```text
//! magic    4   b"PTNS"
//! version  1   = 1
//! count    4   records, and the length of the position array
//! coords   4   compressed length, then that many bytes:
//!              zstd of count x (i32 lat_micro, i32 lon_micro)
//! names        the rest: zstd of the folded names, '\n' separated,
//!              in the same order as the positions
//! ```

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::DecodeError;

/// Magic of the name-scan section.
pub const NAME_SCAN_MAGIC: &[u8; 4] = b"PTNS";

/// One matched name and where it is.
#[derive(Clone, Debug, PartialEq)]
pub struct NameHit {
    /// The folded name, as stored: lowercased and accent-stripped.
    pub name: String,
    pub lat: f64,
    pub lon: f64,
    /// 2 exact, 1 prefix, 0 substring -- the same scale the bucket index uses.
    pub score: u8,
}

/// The decompressed section, ready to search repeatedly.
///
/// Held rather than re-read per query on purpose: the decompression is the
/// expensive half, and a search box is asked the same question with one more
/// letter on it.
pub struct NameScan {
    /// Validated once, at parse, rather than per line per query: the blob is
    /// checked for UTF-8 here so a search is a plain `str` match, which uses
    /// the standard library's two-way algorithm. Matching over raw bytes with
    /// `windows().any()` instead cost 50 ms a query against 18 MB; this is the
    /// same work with a matcher that skips.
    names: String,
    coords: Vec<(i32, i32)>,
}

impl NameScan {
    /// Parse the section, decompressing both halves.
    pub fn parse(aux: &[u8]) -> Result<Option<NameScan>, DecodeError> {
        if aux.len() < 13 || &aux[..4] != NAME_SCAN_MAGIC {
            return Ok(None);
        }
        if aux[4] != 1 {
            // A newer section is refused rather than misread: a wrong offset
            // here yields names against the wrong positions, which looks like
            // data rather than like an error.
            return Ok(None);
        }
        let count = u32::from_le_bytes([aux[5], aux[6], aux[7], aux[8]]) as usize;
        let coords_len = u32::from_le_bytes([aux[9], aux[10], aux[11], aux[12]]) as usize;
        let at = 13usize;
        let coords_raw = aux
            .get(at..at + coords_len)
            .ok_or(DecodeError::UnexpectedEof { offset: at, needed: coords_len })?;
        let names_raw = aux
            .get(at + coords_len..)
            .ok_or(DecodeError::UnexpectedEof { offset: at + coords_len, needed: 1 })?;

        let coord_bytes = crate::file::decompress_with_dict_fallback(coords_raw, &[])
            .map_err(|_| DecodeError::UnexpectedEof { offset: at, needed: coords_len })?;
        let names = crate::file::decompress_with_dict_fallback(names_raw, &[])
            .map_err(|_| DecodeError::UnexpectedEof { offset: at + coords_len, needed: 1 })?;

        let mut coords = Vec::with_capacity(count);
        for chunk in coord_bytes.chunks_exact(8) {
            let lat = i32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            let lon = i32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
            coords.push((lat, lon));
        }
        // A blob that is not UTF-8 is unusable rather than partly usable: the
        // names are written by one builder and a bad one means the section is
        // not what it claims.
        let names = String::from_utf8(names)
            .map_err(|_| DecodeError::UnexpectedEof { offset: at + coords_len, needed: 1 })?;
        Ok(Some(NameScan { names, coords }))
    }

    /// How many names the section holds.
    pub fn len(&self) -> usize {
        self.coords.len()
    }

    pub fn is_empty(&self) -> bool {
        self.coords.is_empty()
    }

    /// Every name containing `query`, best match first.
    ///
    /// `query` is folded by the caller's own rule before matching, so a search
    /// box passes what the user typed and this compares like against like.
    /// Positions come from the parallel array by line number, which is why the
    /// scan counts lines as it goes rather than searching the blob as one
    /// string and hunting for the line afterwards.
    pub fn search(&self, query_folded: &str, limit: usize) -> Vec<NameHit> {
        if query_folded.is_empty() || limit == 0 {
            return Vec::new();
        }
        let mut hits: Vec<NameHit> = Vec::new();
        for (line, name) in self.names.split('\n').enumerate() {
            if name.len() < query_folded.len() || !name.contains(query_folded) {
                continue;
            }
            let Some(&(lat, lon)) = self.coords.get(line) else {
                // More names than positions means a section built wrong; the
                // names past the end are dropped rather than placed at (0, 0),
                // which is in the Atlantic and looks like a real answer.
                break;
            };
            hits.push(NameHit {
                name: name.to_string(),
                lat: lat as f64 / 100_000.0,
                lon: lon as f64 / 100_000.0,
                score: if name == query_folded {
                    2
                } else if name.starts_with(query_folded) {
                    1
                } else {
                    0
                },
            });
        }
        hits.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.name.cmp(&b.name)));
        hits.truncate(limit);
        hits
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// Build a section the way the builder does, so the test exercises the
    /// format rather than a convenient shape.
    fn section(entries: &[(&str, f64, f64)]) -> Vec<u8> {
        let mut coords = Vec::new();
        let mut names: Vec<u8> = Vec::new();
        for (i, (name, lat, lon)) in entries.iter().enumerate() {
            if i > 0 {
                names.push(b'\n');
            }
            names.extend_from_slice(name.as_bytes());
            coords.extend_from_slice(&((lat * 100_000.0) as i32).to_le_bytes());
            coords.extend_from_slice(&((lon * 100_000.0) as i32).to_le_bytes());
        }
        let coords_z = zstd_compress(&coords);
        let names_z = zstd_compress(&names);

        let mut out = NAME_SCAN_MAGIC.to_vec();
        out.push(1);
        out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        out.extend_from_slice(&(coords_z.len() as u32).to_le_bytes());
        out.extend_from_slice(&coords_z);
        out.extend_from_slice(&names_z);
        out
    }

    /// Minimal zstd frame: a single raw (uncompressed) block, which every
    /// decoder accepts and which keeps the test free of an encoder dependency.
    fn zstd_compress(data: &[u8]) -> Vec<u8> {
        let mut out = vec![0x28, 0xB5, 0x2F, 0xFD, 0x20];
        out.push(data.len() as u8);
        let header = ((data.len() as u32) << 3) | 0b001;
        out.extend_from_slice(&header.to_le_bytes()[..3]);
        out.extend_from_slice(data);
        out
    }

    fn scan() -> NameScan {
        NameScan::parse(&section(&[
            ("waffle house", 35.0, -88.0),
            ("calvary waffle shop", 36.0, -87.0),
            ("dudleys recycling", 35.5, -88.5),
        ]))
        .unwrap()
        .expect("a section")
    }

    /// The whole point: a substring that is not a prefix.
    #[test]
    fn a_substring_in_the_middle_of_a_name_is_found() {
        let hits = scan().search("affle", 10);

        assert_eq!(hits.len(), 2);
        assert!(hits.iter().all(|h| h.score == 0));
        assert!(hits.iter().any(|h| h.name == "waffle house"));
    }

    #[test]
    fn a_prefix_outranks_a_substring_and_an_exact_name_outranks_both() {
        let hits = scan().search("waffle house", 10);
        assert_eq!(hits[0].score, 2);

        let hits = scan().search("waffle", 10);
        assert_eq!(hits[0].name, "waffle house", "prefix first");
        assert_eq!(hits[0].score, 1);
        assert_eq!(hits[1].score, 0);
    }

    #[test]
    fn a_hit_carries_the_position_of_its_own_line() {
        let hits = scan().search("dudleys", 10);

        assert_eq!(hits.len(), 1);
        assert!((hits[0].lat - 35.5).abs() < 1e-6, "{}", hits[0].lat);
        assert!((hits[0].lon + 88.5).abs() < 1e-6, "{}", hits[0].lon);
    }

    #[test]
    fn nothing_matching_is_no_hits_rather_than_an_error() {
        assert!(scan().search("bicycle", 10).is_empty());
        assert!(scan().search("", 10).is_empty());
        assert!(scan().search("waffle", 0).is_empty());
    }

    #[test]
    fn a_file_without_the_section_is_not_an_error() {
        assert!(NameScan::parse(&[]).unwrap().is_none());
        assert!(NameScan::parse(b"not this section").unwrap().is_none());
    }

    #[test]
    fn a_future_version_reads_as_absent() {
        let mut bytes = section(&[("waffle house", 35.0, -88.0)]);
        bytes[4] = 2;

        assert!(NameScan::parse(&bytes).unwrap().is_none());
    }

    #[test]
    fn the_limit_is_honoured() {
        assert_eq!(scan().search("a", 1).len(), 1);
    }
}
