//! PtilesFile<S: PtilesSource>: open, parse header/dict/index, read_block().
//! Implements the dict-then-plain decompress fallback (mirrors
//! `~/kino/projects/ptiles/ptiles/compression.py:22-37`):
//!
//! ```python
//! def decompress_block(data: bytes, dict_data: bytes) -> bytes | None:
//!     try:
//!         d = zstd.ZstdCompressionDict(dict_data)
//!         dctx = zstd.ZstdDecompressor(dict_data=d)
//!         return dctx.decompress(data)
//!     except Exception:
//!         return None
//!
//! def decompress_fallback(data: bytes) -> bytes | None:
//!     try:
//!         return zstd.ZstdDecompressor().decompress(data)
//!     except Exception:
//!         return None
//! ```
//!
//! `ruzstd` has no one-shot "decode with dict" function — this wraps
//! `ruzstd::decoding::{FrameDecoder, Dictionary}` to reproduce the same
//! try-dict-then-try-plain fallback.
//!
//! Scope note: this module implements the SPEC.md v1 index/block format only.
//! Some real files use an undocumented v2 "merged block" format (see
//! `index.rs` doc comment) — `PtilesFile` will fail to find cells in those
//! files' indexes (`parse_index` will misinterpret v2 entries as v1 and
//! either error on a length mismatch or return corrupt entries). That's
//! flagged as a follow-up, not fixed here — see task report.
//!
//! Block offset relativity: every reader in the Python reference
//! (`ptiles/buildings.py`, `roads.py`, `water.py`, `business.py`,
//! `places.py`, `reader.py`) detects whether `IndexEntry::block_offset`
//! values are absolute file offsets or relative to `header.blocks_offset`,
//! using the same rule: `relative = index[0].block_offset < header.blocks_offset`.
//! This is a per-file property, not a per-layer one — `PtilesFile::open`
//! runs the same detection and `read_block` adds `blocks_offset` back in
//! when relative. Layers whose `blocks_offset` happens to be 0 (or whose
//! first block offset happens to already exceed it) look "absolute" only
//! by coincidence; the general rule handles both cases uniformly.

use alloc::string::String;
use alloc::vec::Vec;

use ruzstd::decoding::{BlockDecodingStrategy, Dictionary, FrameDecoder};

use crate::header::{Header, HEADER_SIZE};
use crate::index::{binary_search, parse_index, IndexEntry};
use crate::source::{PtilesSource, SourceError};

/// Errors from opening a `.ptiles` file or reading one of its blocks.
#[derive(thiserror::Error, Debug)]
pub enum FileError {
    #[error("source read failed: {0}")]
    Source(#[from] SourceError),
    #[error("header/index parse failed: {0}")]
    Decode(#[from] crate::codec::DecodeError),
    #[error("bad magic prefix: {found:?} (expected `PTILES` + layer byte)")]
    BadMagic { found: [u8; 7] },
    #[error("zstd decompress failed for block at offset {offset} (dict and plain both failed): {message}")]
    Decompress { offset: u64, message: String },
}

/// An open `.ptiles` file: header, spatial index, and (if present) zstd
/// dictionary, backed by any `PtilesSource`. Not tied to `std::fs::File` —
/// works with `MemorySource` in `no_std`/wasm/MCU contexts too.
pub struct PtilesFile<S: PtilesSource> {
    source: S,
    header: Header,
    index: Vec<IndexEntry>,
    dict: Vec<u8>,
    /// True if `IndexEntry::block_offset` values are relative to
    /// `header.blocks_offset` rather than absolute file offsets. Detected
    /// in `open()` — see this module's doc comment.
    relative_offsets: bool,
}

impl<S: PtilesSource> PtilesFile<S> {
    /// Open a `.ptiles` file: parse the 256-byte header, load the zstd
    /// dictionary (if `dict_length > 0`), and parse the spatial index.
    pub fn open(source: S) -> Result<Self, FileError> {
        let mut header_buf = [0u8; HEADER_SIZE];
        source.read_exact_at(0, &mut header_buf)?;
        let header = Header::parse(&header_buf)?;

        if &header.magic[0..6] != b"PTILES" {
            return Err(FileError::BadMagic {
                found: header.magic,
            });
        }

        let dict = if header.dict_length > 0 {
            let mut buf = alloc::vec![0u8; header.dict_length as usize];
            source.read_exact_at(header.dict_offset, &mut buf)?;
            buf
        } else {
            Vec::new()
        };

        let mut index_buf = alloc::vec![0u8; header.index_length as usize];
        source.read_exact_at(header.index_offset, &mut index_buf)?;
        let index = parse_index(&index_buf)?;

        // Same detection the Python reference readers use (buildings.py,
        // roads.py, water.py, business.py, places.py, reader.py): if the
        // first block's offset is less than blocks_offset, offsets are
        // relative to blocks_offset, not absolute file offsets.
        let relative_offsets = index
            .first()
            .is_some_and(|e| e.block_offset < header.blocks_offset);

        Ok(PtilesFile {
            source,
            header,
            index,
            dict,
            relative_offsets,
        })
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    pub fn index(&self) -> &[IndexEntry] {
        &self.index
    }

    /// Read and decompress the data block for a given H3 (res-7) cell.
    /// Returns `Ok(None)` if the cell has no entry in the spatial index.
    /// Decompression tries the layer's zstd dictionary first (if any),
    /// falling back to plain (dictionary-less) decompress — mirrors
    /// `ptiles/compression.py`'s `decompress_block`/`decompress_fallback` pair.
    pub fn read_block(&self, cell: u64) -> Result<Option<Vec<u8>>, FileError> {
        let Some(entry) = binary_search(&self.index, cell) else {
            return Ok(None);
        };

        let abs_offset = if self.relative_offsets {
            self.header.blocks_offset + entry.block_offset
        } else {
            entry.block_offset
        };

        let mut compressed = alloc::vec![0u8; entry.block_length as usize];
        self.source.read_exact_at(abs_offset, &mut compressed)?;

        decompress_with_dict_fallback(&compressed, &self.dict)
            .map(Some)
            .map_err(|message| FileError::Decompress {
                offset: abs_offset,
                message,
            })
    }
}

/// Try zstd decompress with the layer dictionary, falling back to plain
/// (dict-less) decompress on failure — matches
/// `ptiles/compression.py::decompress_block` / `decompress_fallback`.
fn decompress_with_dict_fallback(compressed: &[u8], dict: &[u8]) -> Result<Vec<u8>, String> {
    if !dict.is_empty() {
        if let Ok(parsed_dict) = Dictionary::decode_dict(dict) {
            let mut decoder = FrameDecoder::new();
            if decoder.add_dict(parsed_dict).is_ok() {
                if let Some(out) = try_decode_all(&mut decoder, compressed) {
                    return Ok(out);
                }
            }
        }
        // fall through to dict-less attempt on any failure above, matching
        // the Python reference's broad `except Exception: return None` +
        // separate dict-less retry.
    }

    let mut decoder = FrameDecoder::new();
    try_decode_all(&mut decoder, compressed)
        .ok_or_else(|| String::from("zstd decompress failed (dict and plain both failed)"))
}

fn try_decode_all(decoder: &mut FrameDecoder, compressed: &[u8]) -> Option<Vec<u8>> {
    let mut input: &[u8] = compressed;
    decoder.reset(&mut input).ok()?;
    decoder
        .decode_blocks(&mut input, BlockDecodingStrategy::All)
        .ok()?;
    decoder.collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::MemorySource;

    #[test]
    fn open_rejects_bad_magic() {
        let mut buf = alloc::vec![0u8; HEADER_SIZE];
        buf[0..7].copy_from_slice(b"NOTPTLS");
        let src = MemorySource::new(buf);
        assert!(matches!(
            PtilesFile::open(src),
            Err(FileError::BadMagic { .. })
        ));
    }

    #[test]
    fn open_rejects_truncated_header() {
        let src = MemorySource::new(alloc::vec![0u8; 10]);
        assert!(PtilesFile::open(src).is_err());
    }

    #[test]
    fn read_block_returns_none_for_missing_cell() {
        // Build a minimal valid header: magic + version, dict_length=0,
        // index at offset 256 with 0 entries, blocks_offset right after.
        let mut buf = alloc::vec![0u8; HEADER_SIZE];
        buf[0..7].copy_from_slice(b"PTILESN");
        buf[8] = 1;
        // dict_offset/dict_length = 0 (no dict)
        // index_offset = 256, index_length = 4 (just entry_count = 0)
        buf[52..60].copy_from_slice(&256u64.to_le_bytes());
        buf[60..64].copy_from_slice(&4u32.to_le_bytes());
        buf[64..72].copy_from_slice(&260u64.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes()); // empty index: entry_count = 0

        let src = MemorySource::new(buf);
        let file = PtilesFile::open(src).unwrap();
        assert_eq!(file.index().len(), 0);
        assert_eq!(file.read_block(12345).unwrap(), None);
    }

    /// Integration test against a real `.ptiles` file (Phase 1 step 6 in the
    /// plan). `TN.water.ptiles` uses the SPEC.md v1 index format (verified by
    /// hand during this task: `index_length == 4 + block_count * 19` exactly,
    /// unlike `TN.rail.ptiles`/`TN.parks.ptiles`/`TN.places.ptiles`, which use
    /// an undocumented v2 "merged block" format this module doesn't support
    /// yet — see this file's module doc and `index.rs`).
    ///
    /// Skips (passes trivially) if the fixture isn't present, so `cargo test`
    /// doesn't fail in environments without `~/kino/data/ptiles/` populated.
    #[cfg(feature = "std")]
    #[test]
    fn opens_real_water_file_and_decodes_a_block() {
        let path = std::path::Path::new("/home/aoi/kino/data/ptiles/TN.water.ptiles");
        if !path.exists() {
            eprintln!("skipping: fixture not present at {path:?}");
            return;
        }

        let src = crate::source::FileSource::open(path).expect("open TN.water.ptiles");
        let file = PtilesFile::open(src).expect("parse header/dict/index");

        assert_eq!(file.header().magic_str(), "PTILESW");
        assert!(!file.index().is_empty(), "index must have entries");
        assert!(!file.dict.is_empty(), "water layer is trained with a dict");

        // Pick a cell known to be populated: the first index entry.
        let cell = file.index()[0].h3_cell;
        let block = file
            .read_block(cell)
            .expect("read_block should succeed")
            .expect("cell from the index must resolve to a block");
        assert!(!block.is_empty(), "decompressed block must be non-empty");

        let features = crate::water::decode_water(&block).expect("decode_water must parse the real block");
        assert!(
            !features.is_empty(),
            "real water block should decode to at least one feature"
        );
    }

    /// Regression test for the relative-block-offset bug: `TN.buildings_v8.ptiles`
    /// stores `IndexEntry::block_offset` relative to `header.blocks_offset`
    /// (confirmed against the Python reference's `BuildingsReader`), unlike
    /// e.g. `TN.water.ptiles` above where offsets are already absolute.
    /// `PtilesFile::open`/`read_block` must detect and handle both.
    ///
    /// Skips (passes trivially) if the fixture isn't present.
    #[cfg(feature = "std")]
    #[test]
    fn opens_real_buildings_v8_file_and_decodes_a_block() {
        let path = std::path::Path::new("/home/aoi/kino/data/ptiles/TN.buildings_v8.ptiles");
        if !path.exists() {
            eprintln!("skipping: fixture not present at {path:?}");
            return;
        }

        let src = crate::source::FileSource::open(path).expect("open TN.buildings_v8.ptiles");
        let file = PtilesFile::open(src).expect("parse header/dict/index");

        assert!(!file.index().is_empty(), "index must have entries");
        assert!(
            file.relative_offsets,
            "TN.buildings_v8.ptiles is expected to use relative block offsets"
        );

        let cell = file.index()[0].h3_cell;
        let block = file
            .read_block(cell)
            .expect("read_block should succeed")
            .expect("cell from the index must resolve to a block");
        assert!(!block.is_empty(), "decompressed block must be non-empty");
    }
}
