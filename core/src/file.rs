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
//! Index layout is detected per file, never assumed. Two things vary
//! independently and both are properties of the generator that wrote the file,
//! not of the layer:
//!
//! - **Entry width**, 19 or 38 bytes — see `index.rs`. Detected from the
//!   header's `index_length` when that divides evenly into a known width, and
//!   by structural probing when it doesn't.
//! - **Offset base**, absolute or relative to `blocks_offset` — every reader
//!   in the Python reference (`ptiles/buildings.py`, `roads.py`, `water.py`,
//!   `business.py`, `places.py`, `reader.py`) uses the same rule:
//!   `relative = index[0].block_offset < header.blocks_offset`. Layers whose
//!   `blocks_offset` is 0 (or whose first block offset already exceeds it)
//!   look "absolute" only by coincidence; the rule handles both uniformly.
//!
//! On top of those, `blocks_offset` itself can be **wrong**. The published
//! `US.signals.ptiles` and `US.camera.ptiles` had `index_length` computed at a
//! 42-byte stride while the encoder emitted 38-byte entries, so the header's
//! `blocks_offset` — and every absolute `block_offset` derived from it —
//! overshot the real block region by `count * 4` bytes (432,692 and 145,580
//! respectively) and not one block was reachable. `open()` recomputes where
//! the index actually ends and, when the header disagrees, applies the
//! difference as a correction. See [`BlockOffsetBase`].
//!
//! Everything here is structural and costs no extra reads: the header and the
//! whole index section are already in memory before any of it runs.

use alloc::string::String;
use alloc::vec::Vec;

use ruzstd::decoding::{BlockDecodingStrategy, Dictionary, FrameDecoder};

use crate::header::{HEADER_SIZE, Header};
use crate::index::{
    EntrySizeSource, IndexEntry, binary_search, parse_index_detected,
};
use crate::source::{PtilesSource, SourceError};
use crate::versions::{UnsupportedVersion, check_supported};

/// Errors from opening a `.ptiles` file or reading one of its blocks.
#[derive(thiserror::Error, Debug)]
pub enum FileError {
    #[error("source read failed: {0}")]
    Source(#[from] SourceError),
    #[error("header/index parse failed: {0}")]
    Decode(#[from] crate::codec::DecodeError),
    #[error("bad magic prefix: {found:?} (expected `PTILES` + layer byte)")]
    BadMagic { found: [u8; 7] },
    /// Header parsed fine, but its magic/version pair is not in
    /// `SUPPORTED_FORMATS` -- fails closed rather than guessing at forward
    /// compatibility (Addendum 2, decision 2). Not `#[from]`-derived: `no_std`
    /// builds have no `core::error::Error` impl for `UnsupportedVersion` to
    /// satisfy thiserror's `AsDynError` bound, so the conversion is manual
    /// (see the `From` impl below).
    #[error("{0}")]
    UnsupportedVersion(UnsupportedVersion),
    #[error(
        "zstd decompress failed for block at offset {offset} (dict and plain both failed): {message}"
    )]
    Decompress { offset: u64, message: String },
}

impl From<UnsupportedVersion> for FileError {
    fn from(e: UnsupportedVersion) -> Self {
        FileError::UnsupportedVersion(e)
    }
}

/// How a stored `block_offset` becomes an absolute file offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockOffsetBase {
    /// Stored offsets are already absolute and the header agrees with the
    /// index. The overwhelmingly common case.
    Absolute,
    /// Stored offsets are relative to `header.blocks_offset`. Observed on
    /// `buildings_v8`.
    Relative,
    /// Stored offsets are absolute but were computed from a `blocks_offset`
    /// that overshoots where the index actually ends; subtract the difference.
    /// Observed on the published `US.signals`/`US.camera`.
    AbsoluteCorrected { overshoot: u64 },
}

/// What `open()` concluded about a file's index layout. Exposed so callers and
/// tests can assert the decision rather than only its consequences: a reader
/// that lands on the right bytes via the wrong reasoning is one generator
/// change away from breaking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IndexLayout {
    pub entry_size: usize,
    pub entry_size_source: EntrySizeSource,
    pub offset_base: BlockOffsetBase,
    /// Entry width implied by the header's `index_length`, when it divided
    /// evenly. `Some(42)` on the two broken published files.
    pub declared_stride: Option<usize>,
}

impl IndexLayout {
    /// True when the header's own numbers contradict the bytes that follow
    /// them. Worth surfacing: the file is readable, but whatever wrote it has
    /// a bug and other files from the same generator are suspect.
    pub fn header_is_inconsistent(&self) -> bool {
        matches!(self.offset_base, BlockOffsetBase::AbsoluteCorrected { .. })
            || self
                .declared_stride
                .is_some_and(|s| s != self.entry_size)
    }
}

/// An open `.ptiles` file: header, spatial index, and (if present) zstd
/// dictionary, backed by any `PtilesSource`. Not tied to `std::fs::File` —
/// works with `MemorySource` in `no_std`/wasm/MCU contexts too.
pub struct PtilesFile<S: PtilesSource> {
    source: S,
    header: Header,
    index: Vec<IndexEntry>,
    dict: Vec<u8>,
    layout: IndexLayout,
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

        check_supported(&header.magic, header.version)?;

        // If the source can report its length, validate the header's declared
        // regions against it *before* allocating buffers sized from those
        // (untrusted) length fields. A corrupt/hostile header could otherwise
        // name a multi-GB `index_length`/`dict_length` and force a huge
        // allocation (or abort) before the read itself would have failed.
        let source_len = source.len();
        check_region(source_len, header.dict_offset, header.dict_length as u64)?;
        check_region(source_len, header.index_offset, header.index_length as u64)?;

        let dict = if header.dict_length > 0 {
            let mut buf = alloc::vec![0u8; header.dict_length as usize];
            source.read_exact_at(header.dict_offset, &mut buf)?;
            buf
        } else {
            Vec::new()
        };

        let mut index_buf = alloc::vec![0u8; header.index_length as usize];
        source.read_exact_at(header.index_offset, &mut index_buf)?;
        let parsed =
            parse_index_detected(&index_buf, Some(header.index_length as usize))?;
        let index = parsed.entries;

        // Where the index actually ends, from the entries as parsed rather
        // than from the header's arithmetic about them.
        let real_blocks_offset = header
            .index_offset
            .saturating_add(4)
            .saturating_add((index.len() as u64).saturating_mul(parsed.entry_size as u64));

        let offset_base = if index
            .first()
            .is_some_and(|e| e.block_offset < header.blocks_offset)
        {
            // Same rule the Python reference readers use.
            BlockOffsetBase::Relative
        } else if header.blocks_offset > real_blocks_offset {
            // The header claims the blocks start later than the index really
            // ends. Absolute offsets were derived from that same wrong number,
            // so they carry the identical overshoot.
            BlockOffsetBase::AbsoluteCorrected {
                overshoot: header.blocks_offset - real_blocks_offset,
            }
        } else {
            BlockOffsetBase::Absolute
        };

        Ok(PtilesFile {
            source,
            header,
            index,
            dict,
            layout: IndexLayout {
                entry_size: parsed.entry_size,
                entry_size_source: parsed.entry_size_source,
                offset_base,
                declared_stride: parsed.declared_stride,
            },
        })
    }

    /// What `open()` concluded about this file's index layout.
    pub fn layout(&self) -> IndexLayout {
        self.layout
    }

    /// True when this file's blocks pack several cells together and must be
    /// sliced before their records can be read. Tied to the index width: a
    /// 38-byte index and merged blocks are two halves of the same generator
    /// format.
    pub fn has_merged_blocks(&self) -> bool {
        self.layout.entry_size == crate::index::ENTRY_SIZE_V2
    }

    /// Read the record bytes belonging to `cell` -- the input every layer's
    /// `decode_*` expects.
    ///
    /// Prefer this over [`read_block`](Self::read_block). On a v1 layer the two
    /// are identical, but on a v2 layer `read_block` returns a *merged* block
    /// holding several cells behind a header, and feeding that to a record
    /// decoder produces a run of garbage records before the stream
    /// resynchronises rather than an error. `read_cell` slices the requested
    /// cell out first.
    ///
    /// `Ok(None)` if the cell is not in the index, or is indexed to a block
    /// that turns out not to contain it.
    pub fn read_cell(&self, cell: u64) -> Result<Option<Vec<u8>>, FileError> {
        let Some(block) = self.read_block(cell)? else {
            return Ok(None);
        };
        if !self.has_merged_blocks() {
            return Ok(Some(block));
        }
        Ok(crate::merged::cell_slice(&block, cell)?.map(|s| s.to_vec()))
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    /// The underlying source, e.g. to inspect `HttpSource::request_count()`
    /// for telemetry/tests. Not otherwise needed for normal use.
    pub fn source(&self) -> &S {
        &self.source
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

        // Use checked arithmetic: a corrupt index entry could name a
        // `block_offset` near u64::MAX that wraps when `blocks_offset` is added.
        // Wrapping would produce a bogus-but-in-range offset and read the wrong
        // bytes; surface it as an OutOfBounds error instead.
        let oob = || SourceError::OutOfBounds {
            offset: entry.block_offset,
            needed: entry.block_length as usize,
            len: self.source.len().unwrap_or(0),
        };
        let abs_offset = match self.layout.offset_base {
            BlockOffsetBase::Relative => self
                .header
                .blocks_offset
                .checked_add(entry.block_offset)
                .ok_or_else(oob)?,
            BlockOffsetBase::Absolute => entry.block_offset,
            BlockOffsetBase::AbsoluteCorrected { overshoot } => entry
                .block_offset
                .checked_sub(overshoot)
                .ok_or_else(oob)?,
        };

        // Guard the block-length allocation against a corrupt index entry the
        // same way `open` guards the dict/index buffers.
        check_region(self.source.len(), abs_offset, entry.block_length as u64)?;

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

/// Validate that a declared `[offset, offset+length)` region fits within a
/// source of known length, before any buffer sized from `length` is allocated.
/// If the source length is unknown (`None`), skip the check and let the read
/// itself fail. Also catches `offset + length` overflow.
fn check_region(source_len: Option<u64>, offset: u64, length: u64) -> Result<(), SourceError> {
    let Some(total) = source_len else {
        return Ok(());
    };
    let fits = offset.checked_add(length).is_some_and(|end| end <= total);
    if fits {
        Ok(())
    } else {
        Err(SourceError::OutOfBounds {
            offset,
            // `length` is a file-format field (u32-derived); clamp for the
            // usize field without risking a panic on 32-bit targets.
            needed: usize::try_from(length).unwrap_or(usize::MAX),
            len: total,
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

/// Plain (dict-less) zstd decompress of a whole frame. Used by the `admin`
/// and `address` layers, whose header sections (string tables / polygons)
/// are plain-zstd blobs rather than dictionary-compressed per-cell blocks.
/// Returns `Err` (never panics) on a malformed frame.
pub(crate) fn zstd_decompress(compressed: &[u8]) -> Result<Vec<u8>, crate::codec::DecodeError> {
    let mut decoder = FrameDecoder::new();
    try_decode_all(&mut decoder, compressed).ok_or(crate::codec::DecodeError::UnexpectedEof {
        offset: 0,
        needed: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::MemorySource;

    /// A minimal, valid single-frame zstd stream (raw/uncompressed block) that
    /// `ruzstd` decodes back to `content`. Built by hand because the workspace
    /// has no zstd *encoder* (ruzstd is decode-only) — see this module's docs.
    fn raw_zstd_frame(content: &[u8]) -> Vec<u8> {
        assert!(content.len() < 256, "helper only encodes tiny payloads");
        let mut frame = alloc::vec![0x28u8, 0xB5, 0x2F, 0xFD];
        // Frame_Header_Descriptor: Single_Segment_flag set (0x20) => no
        // Window_Descriptor, and a 1-byte Frame_Content_Size follows.
        frame.push(0x20);
        frame.push(content.len() as u8); // Frame_Content_Size (1 byte)
        // Block_Header (3 bytes LE): Last_Block=1, Block_Type=0 (Raw),
        // Block_Size=content.len().
        let block_header: u32 = ((content.len() as u32) << 3) | 1;
        frame.extend_from_slice(&block_header.to_le_bytes()[0..3]);
        frame.extend_from_slice(content);
        frame
    }

    fn encode_index_entry(
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

    /// Assemble a complete, valid in-memory `.ptiles` file with one index
    /// entry (cell `100`) whose block decompresses to `content`. Offsets are
    /// absolute (block_offset >= blocks_offset), so `relative_offsets` is false.
    fn build_valid_file(content: &[u8]) -> Vec<u8> {
        let frame = raw_zstd_frame(content);
        let index_offset = HEADER_SIZE as u64;
        let index = {
            let mut v = Vec::new();
            v.extend_from_slice(&1u32.to_le_bytes()); // entry_count = 1
            // block placed immediately after the index section
            v.extend_from_slice(&encode_index_entry(
                100,
                index_offset + 4 + 19,
                frame.len() as u32,
                1,
            ));
            v
        };
        let blocks_offset = index_offset + index.len() as u64;

        let mut buf = alloc::vec![0u8; HEADER_SIZE];
        buf[0..7].copy_from_slice(b"PTILESN");
        buf[8] = 1;
        buf[52..60].copy_from_slice(&index_offset.to_le_bytes());
        buf[60..64].copy_from_slice(&(index.len() as u32).to_le_bytes());
        buf[64..72].copy_from_slice(&blocks_offset.to_le_bytes());
        buf.extend_from_slice(&index);
        buf.extend_from_slice(&frame);
        buf
    }

    #[test]
    fn open_valid_in_memory_file_and_read_block_hit_and_miss() {
        let content = b"hello ptiles world";
        let src = MemorySource::new(build_valid_file(content));
        let file = PtilesFile::open(src).expect("valid file must open");

        assert_eq!(file.index().len(), 1);
        assert_eq!(
            file.layout().offset_base,
            BlockOffsetBase::Absolute,
            "offsets here are absolute"
        );

        // Hit: cell present in the index decompresses to the original content.
        let block = file
            .read_block(100)
            .expect("read_block must succeed")
            .expect("cell 100 is in the index");
        assert_eq!(block, content);

        // Miss: a cell not in the index returns Ok(None), not an error.
        assert_eq!(file.read_block(101).unwrap(), None);
        assert_eq!(file.read_block(0).unwrap(), None);
    }

    #[test]
    fn open_rejects_corrupt_index_length_without_huge_alloc() {
        // Valid-looking header, but index_length claims ~1 TiB while the file
        // is only the header. Must error (via the region guard) rather than
        // attempt the allocation or panic.
        let mut buf = alloc::vec![0u8; HEADER_SIZE];
        buf[0..7].copy_from_slice(b"PTILESN");
        buf[8] = 1;
        buf[52..60].copy_from_slice(&(HEADER_SIZE as u64).to_le_bytes()); // index_offset
        buf[60..64].copy_from_slice(&u32::MAX.to_le_bytes()); // index_length ~4 GiB
        buf[64..72].copy_from_slice(&(HEADER_SIZE as u64).to_le_bytes());
        let src = MemorySource::new(buf);
        assert!(matches!(
            PtilesFile::open(src),
            Err(FileError::Source(SourceError::OutOfBounds { .. }))
        ));
    }

    #[test]
    fn read_block_rejects_corrupt_block_length_without_huge_alloc() {
        // Start from a valid file, then corrupt the index entry's block_length
        // (3-byte field) to its max (~16 MiB) while the actual block is tiny.
        // read_block must error via check_region, not attempt the allocation.
        let mut buf = build_valid_file(b"hi");
        // Index starts at HEADER_SIZE; entry begins after the u32 entry_count.
        // Layout: h3_cell(8) + block_offset(6) + block_length(3) + feat(2).
        let block_len_off = HEADER_SIZE + 4 + 8 + 6;
        buf[block_len_off] = 0xFF;
        buf[block_len_off + 1] = 0xFF;
        buf[block_len_off + 2] = 0xFF;
        let file = PtilesFile::open(MemorySource::new(buf)).expect("header/index still valid");
        assert!(matches!(
            file.read_block(100),
            Err(FileError::Source(SourceError::OutOfBounds { .. }))
        ));
    }

    #[test]
    fn open_rejects_corrupt_dict_region() {
        // dict_length points past EOF: caught before allocation/read.
        let mut buf = alloc::vec![0u8; HEADER_SIZE];
        buf[0..7].copy_from_slice(b"PTILESN");
        buf[8] = 1;
        buf[40..48].copy_from_slice(&0u64.to_le_bytes()); // dict_offset
        buf[48..52].copy_from_slice(&10_000u32.to_le_bytes()); // dict_length > file
        buf[52..60].copy_from_slice(&(HEADER_SIZE as u64).to_le_bytes());
        buf[64..72].copy_from_slice(&(HEADER_SIZE as u64).to_le_bytes());
        let src = MemorySource::new(buf);
        assert!(matches!(
            PtilesFile::open(src),
            Err(FileError::Source(SourceError::OutOfBounds { .. }))
        ));
    }

    #[test]
    fn open_rejects_index_region_past_eof() {
        // Header declares an index that would run off the end of the file
        // (EOF mid-read). Must be an Err, not a panic.
        let mut buf = alloc::vec![0u8; HEADER_SIZE];
        buf[0..7].copy_from_slice(b"PTILESN");
        buf[8] = 1;
        buf[52..60].copy_from_slice(&(HEADER_SIZE as u64).to_le_bytes());
        buf[60..64].copy_from_slice(&23u32.to_le_bytes()); // needs 23 bytes but none follow
        buf[64..72].copy_from_slice(&(HEADER_SIZE as u64).to_le_bytes());
        let src = MemorySource::new(buf);
        assert!(PtilesFile::open(src).is_err());
    }

    #[test]
    fn read_block_errors_when_block_region_past_eof() {
        // Build a valid header+index, but truncate the file so the block the
        // index points at is missing. read_block must return Err, not panic.
        let mut file_bytes = build_valid_file(b"data");
        // Drop the trailing block frame entirely.
        let blocks_offset = u64::from_le_bytes(file_bytes[64..72].try_into().unwrap());
        file_bytes.truncate(blocks_offset as usize);
        let src = MemorySource::new(file_bytes);
        let file = PtilesFile::open(src).expect("header+index still valid");
        assert!(matches!(
            file.read_block(100),
            Err(FileError::Source(SourceError::OutOfBounds { .. }))
        ));
    }

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

        let features =
            crate::water::decode_water(&block).expect("decode_water must parse the real block");
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
        assert_eq!(
            file.layout().offset_base,
            BlockOffsetBase::Relative,
            "TN.buildings_v8.ptiles is expected to use relative block offsets"
        );

        let cell = file.index()[0].h3_cell;
        let block = file
            .read_block(cell)
            .expect("read_block should succeed")
            .expect("cell from the index must resolve to a block");
        assert!(!block.is_empty(), "decompressed block must be non-empty");
    }

    /// Every real `.ptiles` file under `~/kino/data/ptiles/` must open
    /// successfully -- this is the version-gating happy path for all seven
    /// magics in `SUPPORTED_FORMATS` at once. Skips (passes trivially) if the
    /// data dir isn't present, matching the other real-file tests here.
    #[cfg(feature = "std")]
    #[test]
    fn opens_every_real_ptiles_file() {
        let files: &[(&str, &str)] = &[
            (
                "/home/aoi/kino/data/ptiles/TN.buildings_v8.ptiles",
                "PTILESF",
            ),
            ("/home/aoi/kino/data/ptiles/TN.roads.ptiles", "PTILESR"),
            ("/home/aoi/kino/data/ptiles/TN.business.ptiles", "PTILESB"),
            ("/home/aoi/kino/data/ptiles/TN.water.ptiles", "PTILESW"),
            ("/home/aoi/kino/data/ptiles/TN.places.ptiles", "PTILESP"),
            ("/home/aoi/kino/data/ptiles/TN.parks.ptiles", "PTILESN"),
            ("/home/aoi/kino/data/ptiles/TN.rail.ptiles", "PTILEST"),
        ];
        for (path, expected_magic) in files {
            let path = std::path::Path::new(path);
            if !path.exists() {
                eprintln!("skipping: fixture not present at {path:?}");
                continue;
            }
            let src = crate::source::FileSource::open(path).expect("open");
            let file = PtilesFile::open(src).unwrap_or_else(|e| {
                panic!("PtilesFile::open should accept the real file {path:?}: {e}")
            });
            assert_eq!(file.header().magic_str(), *expected_magic);
        }
    }

    /// A header whose version byte has been bumped past what
    /// `SUPPORTED_FORMATS` lists must be rejected with `UnsupportedVersion`,
    /// not silently accepted -- the fail-closed contract from Addendum 2.
    #[test]
    fn open_rejects_bumped_version() {
        // Real TN.buildings_v9.ptiles header is magic PTILESF version 9; bump
        // to 10, which is not in SUPPORTED_FORMATS (only {8, 9}).
        let mut buf = alloc::vec![0u8; HEADER_SIZE];
        buf[0..7].copy_from_slice(b"PTILESF");
        buf[8] = 10;
        buf[64..72].copy_from_slice(&256u64.to_le_bytes()); // blocks_offset
        let src = MemorySource::new(buf);
        match PtilesFile::open(src) {
            Err(FileError::UnsupportedVersion(e)) => {
                assert_eq!(e.found, 10);
                assert_eq!(e.supported, alloc::vec![8, 9]);
            }
            Ok(_) => panic!("expected UnsupportedVersion, got Ok"),
            Err(other) => {
                panic!("expected UnsupportedVersion, got a different FileError variant: {other}")
            }
        }
    }

    /// A magic this client has never seen a real sample of (e.g. admin) must
    /// also be rejected, with an empty `supported` list rather than a panic
    /// or silent accept.
    #[test]
    fn open_rejects_unrecognized_magic_with_known_prefix() {
        let mut buf = alloc::vec![0u8; HEADER_SIZE];
        buf[0..7].copy_from_slice(b"PTILESZ"); // PTILES-prefixed but not in SUPPORTED_FORMATS
        buf[8] = 1;
        buf[64..72].copy_from_slice(&256u64.to_le_bytes());
        let src = MemorySource::new(buf);
        match PtilesFile::open(src) {
            Err(FileError::UnsupportedVersion(e)) => {
                assert!(e.supported.is_empty());
            }
            Ok(_) => panic!("expected UnsupportedVersion, got Ok"),
            Err(other) => {
                panic!("expected UnsupportedVersion, got a different FileError variant: {other}")
            }
        }
    }
}
