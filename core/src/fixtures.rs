//! Synthetic `.ptiles` files, for tests that must not depend on a corpus.
//!
//! Behind the `fixtures` feature and never compiled into a shipping build.
//! It exists because the interesting cases are the ones the local corpus
//! happens not to cover: there is no trails file to query, and only two
//! layers (parks, rail) carry the v2 merged-block layout that a record
//! decoder silently misreads when handed a whole block. Both were reachable
//! only by "skipped: file not present", which is a test that proves nothing.
//!
//! Blocks are written as zstd frames holding a single *raw* (uncompressed)
//! block, which is a legal frame every decoder accepts, so building a file
//! needs no compressor. Written against `header.rs`, `index.rs` and
//! `merged.rs` — the readers are the specification here, and a fixture that
//! drifts from them fails loudly at `PtilesFile::open`.

use alloc::vec::Vec;

use crate::header::HEADER_SIZE;
use crate::index::{ENTRY_SIZE_V1, ENTRY_SIZE_V2};

/// A zstd frame whose payload is one raw block: valid to any decoder, and
/// buildable without a compressor.
pub fn raw_zstd_frame(content: &[u8]) -> Vec<u8> {
    // Frame_Header_Descriptor: Single_Segment_flag (0x20) so no
    // Window_Descriptor follows, plus FCS_Field_Size = 2 (bits 6-7 == 0b10),
    // i.e. a 4-byte Frame_Content_Size. The fixed 4-byte size keeps this
    // correct for payloads of any length rather than only tiny ones.
    let mut frame = alloc::vec![0x28u8, 0xB5, 0x2F, 0xFD, 0x20 | 0x80];
    frame.extend_from_slice(&(content.len() as u32).to_le_bytes());
    // Block_Header (3 bytes LE): Last_Block = 1, Block_Type = 0 (Raw),
    // Block_Size = content.len().
    let block_header: u32 = ((content.len() as u32) << 3) | 1;
    frame.extend_from_slice(&block_header.to_le_bytes()[0..3]);
    frame.extend_from_slice(content);
    frame
}

/// Coverage box written into every fixture header, wide enough to contain
/// the coordinates the record builders below produce. `PtilesLayer::covers`
/// consults it, so a tighter box would make a fixture answer "nothing here".
pub struct Bounds {
    pub min_lat: f32,
    pub min_lon: f32,
    pub max_lat: f32,
    pub max_lon: f32,
}

impl Default for Bounds {
    fn default() -> Self {
        Bounds {
            min_lat: -90.0,
            min_lon: -180.0,
            max_lat: 90.0,
            max_lon: 180.0,
        }
    }
}

fn header(magic: &[u8; 7], index_len: usize, blocks_offset: u64, blocks: u32) -> Vec<u8> {
    let bounds = Bounds::default();
    let mut buf = alloc::vec![0u8; HEADER_SIZE];
    buf[0..7].copy_from_slice(magic);
    buf[8] = 1; // schema version
    buf[12..16].copy_from_slice(&bounds.min_lat.to_le_bytes());
    buf[16..20].copy_from_slice(&bounds.min_lon.to_le_bytes());
    buf[20..24].copy_from_slice(&bounds.max_lat.to_le_bytes());
    buf[24..28].copy_from_slice(&bounds.max_lon.to_le_bytes());
    buf[36..40].copy_from_slice(&blocks.to_le_bytes());
    buf[52..60].copy_from_slice(&(HEADER_SIZE as u64).to_le_bytes());
    buf[60..64].copy_from_slice(&(index_len as u32).to_le_bytes());
    buf[64..72].copy_from_slice(&blocks_offset.to_le_bytes());
    buf
}

/// A complete `.ptiles` file with a v1 (19-byte) index: one cell, one block,
/// the layout roads/water/business/buildings use.
pub fn ptiles_v1(magic: &[u8; 7], cells: &[(u64, Vec<u8>)]) -> Vec<u8> {
    let frames: Vec<Vec<u8>> = cells.iter().map(|(_, rec)| raw_zstd_frame(rec)).collect();
    let index_len = 4 + cells.len() * ENTRY_SIZE_V1;
    let blocks_offset = (HEADER_SIZE + index_len) as u64;

    let mut index = Vec::with_capacity(index_len);
    index.extend_from_slice(&(cells.len() as u32).to_le_bytes());
    let mut running = 0u64;
    for ((cell, _), frame) in cells.iter().zip(&frames) {
        index.extend_from_slice(&cell.to_le_bytes());
        // Offsets are relative to `blocks_offset`; `IndexLayout` detects which
        // base a file uses, and relative is what the published files carry.
        index.extend_from_slice(&running.to_le_bytes()[0..6]);
        index.extend_from_slice(&(frame.len() as u32).to_le_bytes()[0..3]);
        index.extend_from_slice(&1u16.to_le_bytes());
        running += frame.len() as u64;
    }

    let mut out = header(magic, index_len, blocks_offset, cells.len() as u32);
    out.extend_from_slice(&index);
    for frame in &frames {
        out.extend_from_slice(frame);
    }
    out
}

/// One merged block's payload: the cell table `merged::cell_slice` reads,
/// followed by each cell's records.
pub fn merged_block(cells: &[(u64, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&0i32.to_le_bytes()); // center_lon, unused by readers
    out.extend_from_slice(&0i32.to_le_bytes()); // center_lat
    out.extend_from_slice(&(cells.len() as u32).to_le_bytes());
    let mut offset = 0u32;
    for (cell, records) in cells {
        out.extend_from_slice(&cell.to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += records.len() as u32;
    }
    for (_, records) in cells {
        out.extend_from_slice(records);
    }
    out
}

/// A complete `.ptiles` file with a v2 (38-byte) index and merged blocks —
/// the parks/rail/places/signals layout, where one physical block holds
/// several cells and a decoder handed the whole thing produces garbage
/// rather than an error.
pub fn ptiles_v2_merged(magic: &[u8; 7], blocks: &[Vec<(u64, Vec<u8>)>]) -> Vec<u8> {
    let frames: Vec<Vec<u8>> = blocks
        .iter()
        .map(|cells| raw_zstd_frame(&merged_block(cells)))
        .collect();
    let entries: usize = blocks.iter().map(|b| b.len()).sum();
    let index_len = 4 + entries * ENTRY_SIZE_V2;
    let blocks_offset = (HEADER_SIZE + index_len) as u64;

    // The index is one entry per *cell*, sorted by cell id so the reader's
    // binary search finds them; several entries can name the same block,
    // which is the whole point of the layout.
    let mut rows: Vec<(u64, u64, u32, u16)> = Vec::new();
    let mut running = 0u64;
    for (cells, frame) in blocks.iter().zip(&frames) {
        for (i, (cell, _)) in cells.iter().enumerate() {
            rows.push((*cell, running, frame.len() as u32, i as u16));
        }
        running += frame.len() as u64;
    }
    rows.sort_by_key(|r| r.0);

    let mut index = Vec::with_capacity(index_len);
    index.extend_from_slice(&(entries as u32).to_le_bytes());
    for (cell, offset, length, cell_index) in rows {
        index.extend_from_slice(&cell.to_le_bytes());
        index.extend_from_slice(&[0u8; 16]); // bbox, zero in real files too
        index.extend_from_slice(&offset.to_le_bytes()[0..6]);
        index.extend_from_slice(&length.to_le_bytes()[0..2]);
        index.push((offset >> 48) as u8);
        index.push((length >> 16) as u8);
        index.extend_from_slice(&1u16.to_le_bytes()); // feature_count
        index.extend_from_slice(&cell_index.to_le_bytes());
    }

    let mut out = header(magic, index_len, blocks_offset, blocks.len() as u32);
    out.extend_from_slice(&index);
    for frame in &frames {
        out.extend_from_slice(frame);
    }
    out
}

fn push_varint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let byte = (v & 0x7F) as u8;
        v >>= 7;
        if v == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn push_zigzag(out: &mut Vec<u8>, v: i64) {
    push_varint(out, ((v << 1) ^ (v >> 63)) as u64);
}

fn micro(deg: f64) -> i32 {
    // `crate::math::round`, not the inherent method: that one exists only
    // when std is linked, and this crate is no_std-optional.
    crate::math::round(deg * 100_000.0) as i32
}

/// One trail record: zigzag-delta osm_id, geometry, then the indexed
/// type/surface/SAC bytes and the optional name. `coords` are `(lat, lon)`
/// pairs; a single pair encodes a trailhead point, two or more a way.
///
/// Indices are the ones in `codec::tables` — `trail_type` 0 is `path` and 6
/// is `trailhead`.
pub fn trail_record(
    osm_delta: i64,
    trail_type: u8,
    surface: u8,
    sac_scale: u8,
    coords: &[(f64, f64)],
    name: Option<&str>,
) -> Vec<u8> {
    let mut out = Vec::new();
    push_zigzag(&mut out, osm_delta);

    if coords.len() == 1 {
        out.push(1); // geom_type: point
        out.extend_from_slice(&micro(coords[0].1).to_le_bytes());
        out.extend_from_slice(&micro(coords[0].0).to_le_bytes());
    } else {
        out.push(0); // geom_type: linestring
        out.extend_from_slice(&(coords.len() as u16).to_le_bytes());
        if !coords.is_empty() {
            let (mut prev_lon, mut prev_lat) = (micro(coords[0].1), micro(coords[0].0));
            out.extend_from_slice(&prev_lon.to_le_bytes());
            out.extend_from_slice(&prev_lat.to_le_bytes());
            for (lat, lon) in &coords[1..] {
                let (lon, lat) = (micro(*lon), micro(*lat));
                push_zigzag(&mut out, (lon - prev_lon) as i64);
                push_zigzag(&mut out, (lat - prev_lat) as i64);
                prev_lon = lon;
                prev_lat = lat;
            }
        }
    }

    out.push(trail_type);
    out.push(surface);
    out.push(sac_scale);
    match name {
        Some(n) => {
            out.push(0x01); // flags: name present
            out.extend_from_slice(&(n.len() as u16).to_le_bytes());
            out.extend_from_slice(n.as_bytes());
        }
        None => out.push(0x00),
    }
    out
}

/// One park record: zigzag-delta osm_id, u8 vertex count, delta coordinates,
/// a u8-length park type, then the optional name. `coords` are `(lat, lon)`.
pub fn park_record(osm_delta: i64, park_type: &str, coords: &[(f64, f64)], name: Option<&str>) -> Vec<u8> {
    let mut out = Vec::new();
    push_zigzag(&mut out, osm_delta);
    out.push(coords.len() as u8);
    if !coords.is_empty() {
        let (mut prev_lon, mut prev_lat) = (micro(coords[0].1), micro(coords[0].0));
        out.extend_from_slice(&prev_lon.to_le_bytes());
        out.extend_from_slice(&prev_lat.to_le_bytes());
        for (lat, lon) in &coords[1..] {
            let (lon, lat) = (micro(*lon), micro(*lat));
            push_zigzag(&mut out, (lon - prev_lon) as i64);
            push_zigzag(&mut out, (lat - prev_lat) as i64);
            prev_lon = lon;
            prev_lat = lat;
        }
    }
    out.push(park_type.len() as u8);
    out.extend_from_slice(park_type.as_bytes());
    match name {
        Some(n) => {
            out.push(0x01);
            out.extend_from_slice(&(n.len() as u16).to_le_bytes());
            out.extend_from_slice(n.as_bytes());
        }
        None => out.push(0x00),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::MemorySource;
    use crate::{PtilesFile, decode_parks, decode_trails};

    // A cell id whose value does not matter to the readers, only its
    // presence in the index and (for v2) in the block's cell table.
    const CELL_A: u64 = 0x87264d106ffffff;
    const CELL_B: u64 = 0x87264d10fffffff;

    fn a_path() -> Vec<u8> {
        trail_record(
            2,
            0,
            5,
            1,
            &[(36.0, -86.80), (36.0, -86.79)],
            Some("Greenway"),
        )
    }

    #[test]
    fn a_v1_fixture_round_trips_through_the_real_reader() {
        let file = ptiles_v1(b"PTILEST", &[(CELL_A, a_path())]);
        let f = PtilesFile::open(MemorySource::new(file)).expect("fixture must be a valid file");
        assert!(!f.has_merged_blocks());
        let block = f.read_cell(CELL_A).unwrap().expect("cell present");
        let trails = decode_trails(&block).unwrap();
        assert_eq!(trails.len(), 1);
        assert_eq!(trails[0].name.as_deref(), Some("Greenway"));
        assert_eq!(trails[0].trail_type, "path");
        assert_eq!(trails[0].surface, "compacted");
        assert_eq!(trails[0].sac_scale, "hiking");
        assert_eq!(trails[0].coords.len(), 2);
        assert!(f.read_cell(CELL_B).unwrap().is_none(), "an unindexed cell is a miss");
    }

    #[test]
    fn a_v2_fixture_needs_slicing_and_the_reader_does_it() {
        let park_a = park_record(2, "park", &[(36.0, -86.80), (36.01, -86.80), (36.01, -86.79)], Some("A"));
        let park_b = park_record(4, "nature_reserve", &[(35.0, -85.80), (35.01, -85.80), (35.01, -85.79)], Some("B"));
        let file = ptiles_v2_merged(
            b"PTILESP",
            &[alloc::vec![(CELL_A, park_a), (CELL_B, park_b)]],
        );
        let f = PtilesFile::open(MemorySource::new(file)).expect("fixture must be a valid file");
        assert!(f.has_merged_blocks(), "a 38-byte index means merged blocks");

        // Sliced: each cell decodes to exactly its own park.
        for (cell, name) in [(CELL_A, "A"), (CELL_B, "B")] {
            let parks = decode_parks(&f.read_cell(cell).unwrap().unwrap()).unwrap();
            assert_eq!(parks.len(), 1, "{name}");
            assert_eq!(parks[0].name.as_deref(), Some(name));
        }

        // Unsliced is the failure this whole layout invites: the block's
        // header decodes as records, so the caller gets junk, not an error.
        let whole = f.read_block(CELL_A).unwrap().unwrap();
        let junk = decode_parks(&whole).unwrap();
        assert!(
            junk.first().map(|p| p.name.as_deref()) != Some(Some("A")),
            "decoding a merged block whole must not accidentally look right"
        );
    }
}
