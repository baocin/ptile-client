//! Index layout detection: entry width x offset base, plus the header-lies
//! cases.
//!
//! Every `.ptiles` file in the wild pairs one of two index entry widths (19 or
//! 38 bytes) with one of two offset bases (absolute or relative to
//! `blocks_offset`), and the header that describes all of it may be wrong. The
//! historical failure was silent: a 38-byte index read as 19-byte yields
//! entries whose offset and length come from the zeroed bbox field, so every
//! block looks empty and the layer renders nothing with no error anywhere.
//!
//! These tests build files for each combination and assert **which layout was
//! detected**, not only that the bytes came back. A reader that lands on the
//! right block through the wrong reasoning is one generator change from
//! breaking, and asserting only the payload would let it pass.

use ptiles_core::file::{BlockOffsetBase, PtilesFile};
use ptiles_core::index::{
    ENTRY_SIZE_V1, ENTRY_SIZE_V2, EntrySizeSource, detect_entry_size, parse_index_detected,
};
use ptiles_core::source::MemorySource;

const HEADER_SIZE: usize = 256;

/// A minimal, spec-valid zstd frame holding `payload` in a single raw
/// (uncompressed) block.
///
/// Hand-built rather than pulled from an encoder crate: these tests are about
/// index geometry, not compression, and the decode still runs through the real
/// `ruzstd` path. `payload` must be under 256 bytes, which the
/// Single_Segment frame header below encodes in one byte.
///
///   magic          28 b5 2f fd
///   frame header   0x20 -- Single_Segment set, so no window descriptor and
///                  the content size is the single byte that follows
///   content size   u8
///   block header   3 bytes LE: (size << 3) | (Raw << 1) | last_block
///   payload        verbatim
fn zstd_frame(payload: &[u8]) -> Vec<u8> {
    assert!(payload.len() < 256, "fixture payloads stay in one byte");
    let mut f = vec![0x28, 0xB5, 0x2F, 0xFD, 0x20, payload.len() as u8];
    let block_header: u32 = ((payload.len() as u32) << 3) | (0 << 1) | 1;
    f.extend_from_slice(&block_header.to_le_bytes()[0..3]);
    f.extend_from_slice(payload);
    f
}

#[test]
fn fixture_frames_decode_through_the_real_zstd_path() {
    // If this fails every other test in this file is meaningless.
    let (buf, cells) = build_file(Layout::new(ENTRY_SIZE_V1), &[b"roundtrip"]);
    let f = open(buf);
    assert_eq!(f.read_block(cells[0]).unwrap().unwrap(), b"roundtrip");
}

fn entry_v1(h3_cell: u64, block_offset: u64, block_length: u32, feature_count: u16) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&h3_cell.to_le_bytes());
    b.extend_from_slice(&block_offset.to_le_bytes()[0..6]);
    b.extend_from_slice(&block_length.to_le_bytes()[0..3]);
    b.extend_from_slice(&feature_count.to_le_bytes());
    assert_eq!(b.len(), ENTRY_SIZE_V1);
    b
}

/// The 38-byte entry, matching `scripts/shared.py::encode_index_entry_v2`.
/// Note the bbox occupies bytes 8..24 and is written as zeros by every real
/// builder — that is precisely the region a 19-byte parse mistakes for the
/// offset and length.
fn entry_v2(h3_cell: u64, block_offset: u64, block_length: u32, feature_count: u16) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&h3_cell.to_le_bytes());
    b.extend_from_slice(&[0u8; 16]); // min_lon, min_lat, max_lon, max_lat
    b.extend_from_slice(&block_offset.to_le_bytes()[0..6]);
    b.extend_from_slice(&(block_length as u16).to_le_bytes());
    b.push(((block_offset >> 48) & 0xFF) as u8);
    b.push(((block_length >> 16) & 0xFF) as u8);
    b.extend_from_slice(&feature_count.to_le_bytes());
    b.extend_from_slice(&0u16.to_le_bytes()); // cell_index
    assert_eq!(b.len(), ENTRY_SIZE_V2);
    b
}

#[derive(Clone, Copy)]
struct Layout {
    entry_size: usize,
    relative: bool,
    /// Bytes to add to the header's `blocks_offset` and to every stored
    /// absolute offset, reproducing a generator that computed both from a
    /// wrong index stride.
    overshoot: u64,
    /// Stride to declare in the header, overriding the true one.
    declared_stride: Option<usize>,
}

impl Layout {
    fn new(entry_size: usize) -> Self {
        Layout { entry_size, relative: false, overshoot: 0, declared_stride: None }
    }
    fn relative(mut self) -> Self {
        self.relative = true;
        self
    }
    fn overshoot(mut self, n: u64) -> Self {
        self.overshoot = n;
        self
    }
    fn declared_stride(mut self, n: usize) -> Self {
        self.declared_stride = Some(n);
        self
    }
}

/// Build a complete single-layer file (water, PTILESW v1) whose index uses the
/// requested layout and whose blocks hold `payloads`, one per cell.
/// Cells are `100, 200, ...` in ascending order.
fn build_file(layout: Layout, payloads: &[&[u8]]) -> (Vec<u8>, Vec<u64>) {
    let frames: Vec<Vec<u8>> = payloads.iter().map(|p| zstd_frame(p)).collect();
    let cells: Vec<u64> = (0..payloads.len()).map(|i| 100 + i as u64 * 100).collect();

    let index_offset = HEADER_SIZE as u64;
    let true_index_len = 4 + payloads.len() * layout.entry_size;
    let real_blocks_offset = index_offset + true_index_len as u64;
    // A generator with a wrong stride computes blocks_offset from it, and then
    // every absolute block offset inherits the same error.
    let header_blocks_offset = real_blocks_offset + layout.overshoot;

    let mut index = Vec::new();
    index.extend_from_slice(&(payloads.len() as u32).to_le_bytes());
    let mut running = 0u64;
    for (i, frame) in frames.iter().enumerate() {
        let stored = if layout.relative {
            running
        } else {
            header_blocks_offset + running
        };
        let e = if layout.entry_size == ENTRY_SIZE_V2 {
            entry_v2(cells[i], stored, frame.len() as u32, 1)
        } else {
            entry_v1(cells[i], stored, frame.len() as u32, 1)
        };
        index.extend_from_slice(&e);
        running += frame.len() as u64;
    }

    let declared_index_len = match layout.declared_stride {
        Some(s) => 4 + payloads.len() * s,
        None => true_index_len,
    };

    let mut buf = vec![0u8; HEADER_SIZE];
    buf[0..7].copy_from_slice(b"PTILESW");
    buf[8] = 1; // version
    buf[28..36].copy_from_slice(&(payloads.len() as u64).to_le_bytes());
    buf[36..40].copy_from_slice(&(frames.len() as u32).to_le_bytes());
    buf[40..48].copy_from_slice(&(HEADER_SIZE as u64).to_le_bytes()); // dict_offset
    buf[48..52].copy_from_slice(&0u32.to_le_bytes()); // dict_length: none
    buf[52..60].copy_from_slice(&index_offset.to_le_bytes());
    buf[60..64].copy_from_slice(&(declared_index_len as u32).to_le_bytes());
    buf[64..72].copy_from_slice(&header_blocks_offset.to_le_bytes());

    buf.extend_from_slice(&index);
    // Only the real index bytes were appended; if the header over-declares the
    // index length, the block region still starts where the entries end.
    for frame in &frames {
        buf.extend_from_slice(frame);
    }
    (buf, cells)
}

fn open(buf: Vec<u8>) -> PtilesFile<MemorySource> {
    PtilesFile::open(MemorySource::new(buf)).expect("file must open")
}

// ---------------------------------------------------------------- the matrix

#[test]
fn v1_absolute() {
    let (buf, cells) = build_file(Layout::new(ENTRY_SIZE_V1), &[b"alpha", b"beta"]);
    let f = open(buf);
    assert_eq!(f.layout().entry_size, ENTRY_SIZE_V1);
    assert_eq!(f.layout().offset_base, BlockOffsetBase::Absolute);
    assert_eq!(f.layout().entry_size_source, EntrySizeSource::DeclaredLength);
    assert!(!f.layout().header_is_inconsistent());
    assert_eq!(f.read_block(cells[1]).unwrap().unwrap(), b"beta");
}

#[test]
fn v1_relative() {
    let (buf, cells) = build_file(Layout::new(ENTRY_SIZE_V1).relative(), &[b"alpha", b"beta"]);
    let f = open(buf);
    assert_eq!(f.layout().entry_size, ENTRY_SIZE_V1);
    assert_eq!(f.layout().offset_base, BlockOffsetBase::Relative);
    assert_eq!(f.read_block(cells[1]).unwrap().unwrap(), b"beta");
}

#[test]
fn v2_absolute() {
    let (buf, cells) = build_file(Layout::new(ENTRY_SIZE_V2), &[b"alpha", b"beta"]);
    let f = open(buf);
    assert_eq!(f.layout().entry_size, ENTRY_SIZE_V2);
    assert_eq!(f.layout().offset_base, BlockOffsetBase::Absolute);
    assert_eq!(f.read_block(cells[1]).unwrap().unwrap(), b"beta");
}

#[test]
fn v2_relative() {
    let (buf, cells) = build_file(Layout::new(ENTRY_SIZE_V2).relative(), &[b"alpha", b"beta"]);
    let f = open(buf);
    assert_eq!(f.layout().entry_size, ENTRY_SIZE_V2);
    assert_eq!(f.layout().offset_base, BlockOffsetBase::Relative);
    assert_eq!(f.read_block(cells[0]).unwrap().unwrap(), b"alpha");
}

/// The exact shape of the published `US.signals.ptiles` / `US.camera.ptiles`:
/// 38-byte entries, but `index_length` written at a 42-byte stride, so
/// `blocks_offset` and every absolute offset overshoot by `count * 4`.
#[test]
fn v2_with_42_byte_declared_stride_is_corrected() {
    let payloads: [&[u8]; 3] = [b"alpha", b"beta", b"gamma"];
    let overshoot = payloads.len() as u64 * 4;
    let (buf, cells) = build_file(
        Layout::new(ENTRY_SIZE_V2).declared_stride(42).overshoot(overshoot),
        &payloads,
    );
    let f = open(buf);

    assert_eq!(f.layout().entry_size, ENTRY_SIZE_V2);
    assert_eq!(
        f.layout().entry_size_source,
        EntrySizeSource::Probed,
        "42 is not a width we know, so the declared stride must be rejected \
         and the width probed"
    );
    assert_eq!(f.layout().declared_stride, Some(42));
    assert_eq!(
        f.layout().offset_base,
        BlockOffsetBase::AbsoluteCorrected { overshoot }
    );
    assert!(
        f.layout().header_is_inconsistent(),
        "a file this malformed must be reported as such, not silently absorbed"
    );

    for (i, want) in payloads.iter().enumerate() {
        assert_eq!(&f.read_block(cells[i]).unwrap().unwrap(), want);
    }
}

/// Detection must not be satisfied by a width that merely divides evenly and
/// happens to be one we know. Exercised at the detection layer, because a
/// header that declares a *smaller* stride than the truth also truncates the
/// index read itself — that case is covered separately below.
#[test]
fn declared_stride_that_divides_but_is_wrong_is_rejected() {
    let mut index = Vec::new();
    index.extend_from_slice(&2u32.to_le_bytes());
    index.extend_from_slice(&entry_v2(100, 900_000, 512, 7));
    index.extend_from_slice(&entry_v2(200, 900_512, 256, 3));

    // Claim 19-byte entries: 4 + 2*19 = 42 divides evenly and 19 is a width we
    // support, so only structural validation can reject it.
    let (size, source, declared) =
        detect_entry_size(&index, Some(4 + 2 * ENTRY_SIZE_V1)).expect("must detect");
    assert_eq!(declared, Some(ENTRY_SIZE_V1));
    assert_eq!(
        size, ENTRY_SIZE_V2,
        "19 divides evenly and is known, but parsing at 19 gives entry 0 a \
         zero-length block, so the declared stride must lose to the bytes"
    );
    assert_eq!(source, EntrySizeSource::Probed);
}

/// A header that under-declares `index_length` truncates the index before any
/// detection can run. Unlike the 42-byte over-declaration (which merely reads
/// a few spare bytes and is recoverable), this loses data. It must fail with a
/// named error rather than parse a prefix and silently serve a partial index.
#[test]
fn under_declared_index_length_fails_loudly() {
    let (buf, _) = build_file(
        Layout::new(ENTRY_SIZE_V2).declared_stride(ENTRY_SIZE_V1),
        &[b"alpha", b"beta"],
    );
    let msg = match PtilesFile::open(MemorySource::new(buf)) {
        Ok(f) => panic!(
            "opened with a truncated index: {} entries at width {}",
            f.index().len(),
            f.layout().entry_size
        ),
        Err(e) => format!("{e}"),
    };
    assert!(
        msg.to_lowercase().contains("eof") || msg.to_lowercase().contains("parse"),
        "error should name the shortfall: {msg}"
    );
}

/// The regression itself: a v2 index parsed as v1 produces entries whose
/// offset and length come from the zeroed bbox. Assert that shape explicitly,
/// so if detection ever regresses the failure is named rather than silent.
#[test]
fn v2_index_read_as_v1_yields_empty_blocks() {
    let mut index = Vec::new();
    index.extend_from_slice(&2u32.to_le_bytes());
    index.extend_from_slice(&entry_v2(100, 900_000, 512, 7));
    index.extend_from_slice(&entry_v2(200, 900_512, 256, 3));

    let as_v1 = ptiles_core::index::parse_index(&index).expect("parses, wrongly");
    assert_eq!(as_v1.len(), 2);
    assert_eq!(
        (as_v1[0].block_offset, as_v1[0].block_length),
        (0, 0),
        "this is the silent-empty failure mode detection exists to prevent"
    );

    let detected = parse_index_detected(&index, None).expect("detect");
    assert_eq!(detected.entry_size, ENTRY_SIZE_V2);
    assert_eq!(detected.entries[0].block_offset, 900_000);
    assert_eq!(detected.entries[0].block_length, 512);
    assert_eq!(detected.entries[0].feature_count, 7);
}

// ----------------------------------------------------------- adversarial input

#[test]
fn entries_out_of_sort_order_are_rejected_at_that_width() {
    let mut index = Vec::new();
    index.extend_from_slice(&2u32.to_le_bytes());
    index.extend_from_slice(&entry_v1(500, 1000, 10, 1));
    index.extend_from_slice(&entry_v1(100, 1010, 10, 1)); // descending
    // 19 fails the sort check; 38 doesn't fit. Nothing valid remains.
    assert!(detect_entry_size(&index, None).is_err());
}

#[test]
fn all_zero_length_blocks_are_rejected_at_that_width() {
    let mut index = Vec::new();
    index.extend_from_slice(&2u32.to_le_bytes());
    index.extend_from_slice(&entry_v1(100, 0, 0, 0));
    index.extend_from_slice(&entry_v1(200, 0, 0, 0));
    assert!(
        detect_entry_size(&index, None).is_err(),
        "an index where no cell has data is indistinguishable from a \
         mis-parsed one; refuse rather than return silence"
    );
}

#[test]
fn empty_index_is_valid_at_any_width() {
    let index = 0u32.to_le_bytes();
    let (_, source, _) = detect_entry_size(&index, Some(4)).expect("zero entries is legal");
    assert_eq!(source, EntrySizeSource::DeclaredLength);
    let p = parse_index_detected(&index, None).unwrap();
    assert!(p.entries.is_empty());
}

#[test]
fn huge_entry_count_does_not_overflow_or_over_allocate() {
    let index = u32::MAX.to_le_bytes();
    assert!(detect_entry_size(&index, None).is_err());
    assert!(parse_index_detected(&index, None).is_err());
    assert!(parse_index_detected(&index, Some(usize::MAX)).is_err());
}

#[test]
fn truncated_index_at_every_length_never_panics() {
    let (buf, _) = build_file(Layout::new(ENTRY_SIZE_V2), &[b"alpha", b"beta", b"gamma"]);
    let index_start = HEADER_SIZE;
    let index_end = index_start + 4 + 3 * ENTRY_SIZE_V2;
    for cut in index_start..=index_end {
        let slice = &buf[index_start..cut.min(buf.len())];
        // Either a clean parse or a clean error; never a panic.
        let _ = parse_index_detected(slice, None);
        let _ = detect_entry_size(slice, None);
    }
}

#[test]
fn declared_index_length_shorter_than_entries_does_not_read_past_the_buffer() {
    let mut index = Vec::new();
    index.extend_from_slice(&3u32.to_le_bytes());
    index.extend_from_slice(&entry_v1(100, 1000, 10, 1)); // only 1 of 3 present
    assert!(parse_index_detected(&index, Some(4 + 3 * ENTRY_SIZE_V1)).is_err());
}

#[test]
fn block_offset_past_eof_is_an_error_not_a_panic() {
    let (mut buf, cells) = build_file(Layout::new(ENTRY_SIZE_V1), &[b"alpha"]);
    // Rewrite entry 0's 6-byte offset to just under the 2^48 ceiling.
    let off = HEADER_SIZE + 4 + 8;
    buf[off..off + 6].copy_from_slice(&((1u64 << 48) - 1).to_le_bytes()[0..6]);
    let f = PtilesFile::open(MemorySource::new(buf)).expect("header still parses");
    assert!(
        f.read_block(cells[0]).is_err(),
        "an offset past EOF must be a named error"
    );
}

#[test]
fn corrupt_block_bytes_produce_a_decompress_error_naming_the_offset() {
    let (mut buf, cells) = build_file(Layout::new(ENTRY_SIZE_V1), &[b"alpha"]);
    let blocks_start = HEADER_SIZE + 4 + ENTRY_SIZE_V1;
    for b in buf[blocks_start..].iter_mut() {
        *b ^= 0xFF;
    }
    let f = open(buf);
    let msg = match f.read_block(cells[0]) {
        Ok(_) => panic!("garbage must not decode"),
        Err(e) => format!("{e}"),
    };
    assert!(
        msg.contains("zstd") || msg.contains("decompress"),
        "error should name the failure: {msg}"
    );
}

#[test]
fn unknown_magic_fails_closed() {
    let (mut buf, _) = build_file(Layout::new(ENTRY_SIZE_V1), &[b"alpha"]);
    buf[0..7].copy_from_slice(b"PTILESZ");
    let msg = match PtilesFile::open(MemorySource::new(buf)) {
        Ok(_) => panic!("an unlisted magic must not open"),
        Err(e) => format!("{e}"),
    };
    assert!(
        msg.contains("PTILESZ") || msg.to_lowercase().contains("version"),
        "error must name what it found: {msg}"
    );
}

#[test]
fn unsupported_version_fails_closed_and_names_both_sides() {
    let (mut buf, _) = build_file(Layout::new(ENTRY_SIZE_V1), &[b"alpha"]);
    buf[8] = 99; // water is v1 only
    let msg = match PtilesFile::open(MemorySource::new(buf)) {
        Ok(_) => panic!("an unlisted version must not open"),
        Err(e) => format!("{e}"),
    };
    assert!(msg.contains("99"), "error must name the version found: {msg}");
}
