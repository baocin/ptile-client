//! PTLR: the three-zoom-band roads container.
//!
//! Every roads file the builders emit is this format, and until now nothing in
//! this crate could read one -- `PTILESR`, the container it replaced, holds
//! only the motorway/trunk/primary subset. A client without PTLR has no
//! residential streets, no service roads, no paths: it cannot route.
//!
//! It is not a PTiles file. There is no 256-byte PTiles header, no H3 index,
//! no per-cell blocks -- three zstd frames, each holding every road in the
//! region at one level of detail:
//!
//! ```text
//! magic        4   b"PTLR"
//! version      1   (+3 reserved)
//! z04 triple  @8   offset(u64) comp(u32) decomp(u32)   res 4, zoom 5-9
//! z05 triple  @24  ...                                 res 5, zoom 10-12
//! z07 triple  @40  ...                                 res 7, zoom 13+
//! road_count  @56  == the z07 count
//! dict_lens   @60  z04, z05, z07 (u32 each)
//! counts      @72  z04, z05, z07 (u32 each)
//! bbox        @84  min_lon, min_lat, max_lon, max_lat (i32 micro-degrees)
//! boundary    @100 offset(u64) @108 length(u32)
//! dictionaries at 256, concatenated in band order, split by dict_lens
//! ```
//!
//! Records, within a decompressed band:
//!
//! ```text
//! varint osm_id delta (plain, NOT zigzag -- PBF ways are id-ascending)
//! u16 vertex_count
//! i32 first_lon, i32 first_lat (micro-degrees)
//! (vertex_count - 1) zigzag varint delta pairs
//! u8  road class index
//! u8  flags: 0x01 name (u16-prefixed), 0x02 ref (u16-prefixed)
//! ```
//!
//! **Ids are deltas along the whole band.** A record decoded from the middle
//! of one has no running total, so it must be given the absolute id from the
//! index walk. Getting this wrong does not fail -- it returns small integers
//! that look like ids, and the Python reader shipped that way until Tokyo
//! Station's nearest road came back as osm_id 3.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::collections::BTreeMap;

use crate::boundary::{decode_boundary, Ring};
use crate::codec::{decode_varint, zigzag_decode, DecodeError};
use crate::roads::RoadSegment;

pub const PTLR_MAGIC: &[u8; 4] = b"PTLR";
pub const PTLR_HEADER_SIZE: usize = 256;

/// Zoom bands, in the order their dictionaries and counts are stored.
pub const BANDS: [Band; 3] = [Band::Z04, Band::Z05, Band::Z07];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Band {
    /// res 4, zoom 5-9: motorway/trunk/primary only, simplified to 500 m.
    Z04,
    /// res 5, zoom 10-12: every road, simplified to 200 m.
    Z05,
    /// res 7, zoom 13+: every road at full precision.
    Z07,
}

impl Band {
    fn index(self) -> usize {
        match self {
            Band::Z04 => 0,
            Band::Z05 => 1,
            Band::Z07 => 2,
        }
    }

    fn header_offset(self) -> usize {
        match self {
            Band::Z04 => 8,
            Band::Z05 => 24,
            Band::Z07 => 40,
        }
    }
}

/// Road class index, matching `PTLR_ROAD_CLASSES` in `ptiles/roads.py`, which
/// is the reverse of `ROAD_CLASS_INDEX` in `scripts/build_roads.py`. That map
/// is many-to-one -- `motorway_link` encodes as `motorway` -- so this list is
/// shorter than the tag vocabulary it came from.
pub const PTLR_ROAD_CLASSES: [&str; 16] = [
    "motorway",
    "trunk",
    "primary",
    "secondary",
    "tertiary",
    "unclassified",
    "residential",
    "service",
    "living_street",
    "track",
    "path",
    "pedestrian",
    "steps",
    "construction",
    "rest_area",
    "services",
];

#[derive(Clone, Copy, Debug)]
struct BandSpec {
    offset: u64,
    comp_len: u32,
    decomp_len: u32,
    dict_len: u32,
    count: u32,
}

/// A parsed PTLR header.
#[derive(Clone, Debug)]
pub struct PtlrHeader {
    pub version: u8,
    pub road_count: u32,
    /// (min_lat, min_lon, max_lat, max_lon), or None when the file records no
    /// bbox -- v1 did not, and a reader must then consult the file for every
    /// query rather than assume it covers nothing.
    pub bounds: Option<(f64, f64, f64, f64)>,
    pub boundary_offset: u64,
    pub boundary_length: u32,
    bands: [BandSpec; 3],
}

fn read_u32(data: &[u8], at: usize) -> Result<u32, DecodeError> {
    data.get(at..at + 4)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
        .ok_or(DecodeError::UnexpectedEof {
            offset: at,
            needed: 4,
        })
}

fn read_u64(data: &[u8], at: usize) -> Result<u64, DecodeError> {
    data.get(at..at + 8)
        .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
        .ok_or(DecodeError::UnexpectedEof {
            offset: at,
            needed: 8,
        })
}

fn read_i32(data: &[u8], at: usize) -> Result<i32, DecodeError> {
    read_u32(data, at).map(|v| v as i32)
}

impl PtlrHeader {
    pub fn parse(data: &[u8]) -> Result<PtlrHeader, DecodeError> {
        if data.len() < PTLR_HEADER_SIZE {
            return Err(DecodeError::UnexpectedEof {
                offset: 0,
                needed: PTLR_HEADER_SIZE,
            });
        }
        if &data[0..4] != PTLR_MAGIC {
            let mut found = [0u8; 4];
            found.copy_from_slice(&data[0..4]);
            return Err(DecodeError::WrongMagic {
                expected: PTLR_MAGIC,
                found,
            });
        }
        let version = data[4];

        let mut bands = [BandSpec {
            offset: 0,
            comp_len: 0,
            decomp_len: 0,
            dict_len: 0,
            count: 0,
        }; 3];
        for band in BANDS {
            let at = band.header_offset();
            let i = band.index();
            bands[i].offset = read_u64(data, at)?;
            bands[i].comp_len = read_u32(data, at + 8)?;
            bands[i].decomp_len = read_u32(data, at + 12)?;
            // v2 added the per-band dictionary lengths and counts. Without
            // them the three concatenated dictionaries cannot be split, so a
            // v1 file's frames cannot be decompressed at all -- the Python
            // reader recovers them by scanning for the zstd dictionary magic,
            // which is worth mirroring here only if a v1 file turns up.
            if version >= 2 {
                bands[i].dict_len = read_u32(data, 60 + i * 4)?;
                bands[i].count = read_u32(data, 72 + i * 4)?;
            }
        }

        let road_count = read_u32(data, 56)?;
        let (bounds, boundary_offset, boundary_length) = if version >= 2 {
            let min_lon = read_i32(data, 84)?;
            let min_lat = read_i32(data, 88)?;
            let max_lon = read_i32(data, 92)?;
            let max_lat = read_i32(data, 96)?;
            let bounds = if (min_lon, min_lat, max_lon, max_lat) == (0, 0, 0, 0) {
                None
            } else {
                Some((
                    f64::from(min_lat) / 100_000.0,
                    f64::from(min_lon) / 100_000.0,
                    f64::from(max_lat) / 100_000.0,
                    f64::from(max_lon) / 100_000.0,
                ))
            };
            // PTLR's own bbox occupies @84, where the PTiles layers keep their
            // boundary fields, so the polygon pair sits at @100/@108 instead.
            (bounds, read_u64(data, 100)?, read_u32(data, 108)?)
        } else {
            (None, 0, 0)
        };

        Ok(PtlrHeader {
            version,
            road_count,
            bounds,
            boundary_offset,
            boundary_length,
            bands,
        })
    }

    /// Byte range of one band's compressed frame.
    pub fn band_range(&self, band: Band) -> (u64, u32) {
        let b = self.bands[band.index()];
        (b.offset, b.comp_len)
    }

    /// Where this band's dictionary sits, given that they are concatenated at
    /// 256 in band order.
    pub fn dict_range(&self, band: Band) -> (u64, u32) {
        let mut start = PTLR_HEADER_SIZE as u64;
        for b in BANDS {
            if b == band {
                return (start, self.bands[b.index()].dict_len);
            }
            start += u64::from(self.bands[b.index()].dict_len);
        }
        (start, 0)
    }

    /// Roads in one band, as the builder counted them.
    pub fn band_count(&self, band: Band) -> u32 {
        self.bands[band.index()].count
    }
}

/// Decode one record. `absolute_id`, when given, replaces the delta-derived id
/// -- see the module docs on why that matters.
pub fn decode_record(
    data: &[u8],
    pos: usize,
    prev_osm_id: i64,
    absolute_id: Option<i64>,
) -> Result<(RoadSegment, usize, i64), DecodeError> {
    let start = pos;
    let mut p = pos;

    let (delta, consumed) = decode_varint(data, p)?;
    p += consumed;
    let osm_id = prev_osm_id + delta as i64;

    let count = data
        .get(p..p + 2)
        .map(|b| u16::from_le_bytes(b.try_into().unwrap()))
        .ok_or(DecodeError::UnexpectedEof {
            offset: p,
            needed: 2,
        })? as usize;
    p += 2;

    let mut lon = read_i32(data, p)?;
    let mut lat = read_i32(data, p + 4)?;
    p += 8;

    let mut coords = Vec::with_capacity(count.min(1 << 14));
    coords.push([f64::from(lon) / 100_000.0, f64::from(lat) / 100_000.0]);
    for _ in 1..count {
        let (dlon, consumed) = decode_varint(data, p)?;
        p += consumed;
        let (dlat, consumed) = decode_varint(data, p)?;
        p += consumed;
        lon = lon.wrapping_add(zigzag_decode(dlon) as i32);
        lat = lat.wrapping_add(zigzag_decode(dlat) as i32);
        coords.push([f64::from(lon) / 100_000.0, f64::from(lat) / 100_000.0]);
    }

    let cls_idx = *data.get(p).ok_or(DecodeError::UnexpectedEof {
        offset: p,
        needed: 1,
    })? as usize;
    p += 1;
    let flags = *data.get(p).ok_or(DecodeError::UnexpectedEof {
        offset: p,
        needed: 1,
    })?;
    p += 1;

    let mut name = None;
    let mut ref_tag = None;
    for (bit, slot) in [(0x01u8, &mut name), (0x02u8, &mut ref_tag)] {
        if flags & bit != 0 {
            let (s, consumed) = crate::codec::decode_string_u16(data, p)?;
            p += consumed;
            *slot = Some(s);
        }
    }

    let road_class = PTLR_ROAD_CLASSES
        .get(cls_idx)
        .map(|s| String::from(*s))
        .unwrap_or_else(|| String::from("unknown"));

    Ok((
        RoadSegment {
            // RoadSegment carries a u64 id, matching the PTiles layers.
            osm_id: absolute_id.unwrap_or(osm_id).max(0) as u64,
            road_class,
            coords,
            name,
            ref_tag,
            oneway: None,
            speed_limit_kmh: None,
            lanes: None,
            surface: None,
            bridge_tunnel: None,
        },
        p - start,
        osm_id,
    ))
}

/// Index bucket size in micro-degrees: 0.01 degrees, roughly 1.1 km. Small on
/// purpose -- a point query decodes every road in the buckets it searches, and
/// 5 km buckets meant tens of thousands of roads per query in central Tokyo.
const GRID: i32 = 1000;

/// An in-memory index of one decompressed band.
///
/// PTLR carries no spatial index, so one is built by walking the band once and
/// bucketing each road by the cell of its first vertex. Each entry keeps the
/// running absolute osm id alongside the record offset, which is the only
/// place that number exists.
pub struct BandIndex {
    buckets: BTreeMap<(i32, i32), Vec<(u32, i64)>>,
}

impl BandIndex {
    /// Walk a decompressed band and bucket every record in it.
    pub fn build(raw: &[u8]) -> BandIndex {
        let mut buckets: BTreeMap<(i32, i32), Vec<(u32, i64)>> = BTreeMap::new();
        let mut pos = 0usize;
        let mut prev = 0i64;
        while pos < raw.len() {
            let start = pos;
            // Walk the record without allocating its geometry: only the first
            // vertex and the running id are needed here, and a full decode of
            // every road in a country-sized file would cost far more than the
            // query that follows it.
            let (delta, consumed) = match decode_varint(raw, pos) {
                Ok(v) => v,
                Err(_) => break,
            };
            pos += consumed;
            prev += delta as i64;
            let Some(count_bytes) = raw.get(pos..pos + 2) else {
                break;
            };
            let count = u16::from_le_bytes(count_bytes.try_into().unwrap()) as usize;
            pos += 2;
            let (Ok(lon), Ok(lat)) = (read_i32(raw, pos), read_i32(raw, pos + 4)) else {
                break;
            };
            pos += 8;
            let mut ok = true;
            for _ in 1..count {
                match (decode_varint(raw, pos), ()) {
                    (Ok((_, c)), ()) => pos += c,
                    _ => {
                        ok = false;
                        break;
                    }
                }
                match decode_varint(raw, pos) {
                    Ok((_, c)) => pos += c,
                    Err(_) => {
                        ok = false;
                        break;
                    }
                }
            }
            if !ok || pos + 2 > raw.len() {
                break;
            }
            pos += 1; // road class
            let flags = raw[pos];
            pos += 1;
            for bit in [0x01u8, 0x02] {
                if flags & bit != 0 {
                    let Some(len_bytes) = raw.get(pos..pos + 2) else {
                        ok = false;
                        break;
                    };
                    let n = u16::from_le_bytes(len_bytes.try_into().unwrap()) as usize;
                    pos += 2 + n;
                }
            }
            if !ok || pos > raw.len() {
                break;
            }
            buckets
                .entry((lon.div_euclid(GRID), lat.div_euclid(GRID)))
                .or_default()
                .push((start as u32, prev));
        }
        BandIndex { buckets }
    }

    /// Record (offset, absolute id) pairs in the cells around a point.
    pub fn candidates(&self, lat: f64, lon: f64, rings: i32) -> Vec<(u32, i64)> {
        let gx = ((lon * 100_000.0) as i32).div_euclid(GRID);
        let gy = ((lat * 100_000.0) as i32).div_euclid(GRID);
        let mut out = Vec::new();
        for dx in -rings..=rings {
            for dy in -rings..=rings {
                if let Some(bucket) = self.buckets.get(&(gx + dx, gy + dy)) {
                    out.extend_from_slice(bucket);
                }
            }
        }
        out
    }

    /// Pairs whose first vertex falls in a bounding box.
    pub fn in_bounds(&self, min_lat: f64, min_lon: f64, max_lat: f64, max_lon: f64) -> Vec<(u32, i64)> {
        let x0 = ((min_lon * 100_000.0) as i32).div_euclid(GRID);
        let x1 = ((max_lon * 100_000.0) as i32).div_euclid(GRID);
        let y0 = ((min_lat * 100_000.0) as i32).div_euclid(GRID);
        let y1 = ((max_lat * 100_000.0) as i32).div_euclid(GRID);
        let mut out = Vec::new();
        for gx in x0..=x1 {
            for gy in y0..=y1 {
                if let Some(bucket) = self.buckets.get(&(gx, gy)) {
                    out.extend_from_slice(bucket);
                }
            }
        }
        out
    }

    pub fn len(&self) -> usize {
        self.buckets.values().map(|b| b.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.buckets.is_empty()
    }
}

/// Decode the boundary polygon from a PTLR file's own bytes.
pub fn boundary_from(header: &PtlrHeader, file_bytes: &[u8]) -> Result<Vec<Ring>, DecodeError> {
    if header.boundary_offset == 0 || header.boundary_length == 0 {
        return Ok(Vec::new());
    }
    let start = header.boundary_offset as usize;
    let end = start + header.boundary_length as usize;
    match file_bytes.get(start..end) {
        Some(blob) => decode_boundary(blob),
        None => Ok(Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn varint(mut v: u64, out: &mut Vec<u8>) {
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(b);
                break;
            }
            out.push(b | 0x80);
        }
    }

    fn zigzag(v: i32) -> u64 {
        ((v << 1) ^ (v >> 31)) as u32 as u64
    }

    /// One record, in the layout `scripts/build_roads.py::encode_road` writes.
    fn record(delta: u64, coords: &[[i32; 2]], cls: u8, name: Option<&str>) -> Vec<u8> {
        let mut out = Vec::new();
        varint(delta, &mut out);
        out.extend_from_slice(&(coords.len() as u16).to_le_bytes());
        out.extend_from_slice(&coords[0][0].to_le_bytes());
        out.extend_from_slice(&coords[0][1].to_le_bytes());
        let (mut plon, mut plat) = (coords[0][0], coords[0][1]);
        for c in &coords[1..] {
            varint(zigzag(c[0] - plon), &mut out);
            varint(zigzag(c[1] - plat), &mut out);
            plon = c[0];
            plat = c[1];
        }
        out.push(cls);
        out.push(if name.is_some() { 0x01 } else { 0x00 });
        if let Some(n) = name {
            out.extend_from_slice(&(n.len() as u16).to_le_bytes());
            out.extend_from_slice(n.as_bytes());
        }
        out
    }

    #[test]
    fn decodes_a_record() {
        let raw = record(
            42,
            &[[-8_679_300, 3_616_270], [-8_679_100, 3_616_400]],
            2,
            Some("Broadway"),
        );
        let (road, consumed, osm_id) = decode_record(&raw, 0, 0, None).unwrap();
        assert_eq!(consumed, raw.len());
        assert_eq!(osm_id, 42);
        assert_eq!(road.road_class, "primary");
        assert_eq!(road.name.as_deref(), Some("Broadway"));
        assert_eq!(road.coords.len(), 2);
        assert!((road.coords[0][0] - -86.793).abs() < 1e-6);
    }

    #[test]
    fn ids_are_deltas_and_the_index_carries_the_running_total() {
        let mut band = record(100, &[[-8_679_300, 3_616_270], [-8_679_100, 3_616_400]], 0, None);
        band.extend(record(40, &[[-8_679_000, 3_616_500], [-8_678_900, 3_616_600]], 6, None));

        // Decoded cold from the middle, the second record reports its delta.
        let (bare, _, _) = decode_record(&band, 0, 0, None).unwrap();
        assert_eq!(bare.osm_id, 100);

        let index = BandIndex::build(&band);
        assert_eq!(index.len(), 2, "both records indexed");
        let mut ids: Vec<i64> = index
            .candidates(36.1627, -86.7930, 2)
            .into_iter()
            .map(|(offset, abs)| {
                let (road, _, _) = decode_record(&band, offset as usize, 0, Some(abs)).unwrap();
                road.osm_id as i64
            })
            .collect();
        ids.sort_unstable();
        assert_eq!(ids, vec![100, 140], "absolute ids, not deltas");
    }

    #[test]
    fn header_reads_bands_dicts_and_bounds() {
        let mut head = vec![0u8; PTLR_HEADER_SIZE];
        head[0..4].copy_from_slice(PTLR_MAGIC);
        head[4] = 3;
        // z05 frame at 4096, 100 compressed, 300 decompressed
        head[24..32].copy_from_slice(&4096u64.to_le_bytes());
        head[32..36].copy_from_slice(&100u32.to_le_bytes());
        head[36..40].copy_from_slice(&300u32.to_le_bytes());
        head[56..60].copy_from_slice(&7u32.to_le_bytes()); // road_count
        head[60..64].copy_from_slice(&10u32.to_le_bytes()); // z04 dict
        head[64..68].copy_from_slice(&20u32.to_le_bytes()); // z05 dict
        head[68..72].copy_from_slice(&30u32.to_le_bytes()); // z07 dict
        head[76..80].copy_from_slice(&7u32.to_le_bytes()); // z05 count
        head[84..88].copy_from_slice(&(-9_030_000i32).to_le_bytes()); // min_lon
        head[88..92].copy_from_slice(&3_498_000i32.to_le_bytes()); // min_lat
        head[92..96].copy_from_slice(&(-8_150_000i32).to_le_bytes()); // max_lon
        head[96..100].copy_from_slice(&3_668_000i32.to_le_bytes()); // max_lat
        head[100..108].copy_from_slice(&9000u64.to_le_bytes());
        head[108..112].copy_from_slice(&512u32.to_le_bytes());

        let h = PtlrHeader::parse(&head).unwrap();
        assert_eq!(h.version, 3);
        assert_eq!(h.road_count, 7);
        assert_eq!(h.band_range(Band::Z05), (4096, 100));
        assert_eq!(h.band_count(Band::Z05), 7);
        // Dictionaries are concatenated at 256 in band order.
        assert_eq!(h.dict_range(Band::Z04), (256, 10));
        assert_eq!(h.dict_range(Band::Z05), (266, 20));
        assert_eq!(h.dict_range(Band::Z07), (286, 30));
        let (min_lat, min_lon, max_lat, max_lon) = h.bounds.unwrap();
        assert!((min_lat - 34.98).abs() < 1e-6);
        assert!((max_lon - -81.5).abs() < 1e-6);
        assert_eq!((h.boundary_offset, h.boundary_length), (9000, 512));
    }

    #[test]
    fn rejects_a_file_that_is_not_ptlr() {
        let mut head = vec![0u8; PTLR_HEADER_SIZE];
        head[0..7].copy_from_slice(b"PTILESR");
        assert!(matches!(
            PtlrHeader::parse(&head),
            Err(DecodeError::WrongMagic { .. })
        ));
    }

    #[test]
    fn a_v1_header_reports_no_bounds_rather_than_a_bogus_box() {
        let mut head = vec![0u8; PTLR_HEADER_SIZE];
        head[0..4].copy_from_slice(PTLR_MAGIC);
        head[4] = 1;
        let h = PtlrHeader::parse(&head).unwrap();
        assert!(h.bounds.is_none(), "v1 recorded no bbox; do not invent one");
        assert_eq!(h.dict_range(Band::Z04), (256, 0));
    }

    #[test]
    fn a_truncated_band_stops_the_index_walk_without_panicking() {
        let mut band = record(1, &[[0, 0], [10, 10]], 0, None);
        band.truncate(band.len() - 1);
        let index = BandIndex::build(&band);
        assert!(index.is_empty() || index.len() <= 1);
    }
}

// ---------------------------------------------------------------------------
// Source-backed reader
// ---------------------------------------------------------------------------

use crate::file::{decompress_with_dict_fallback, FileError};
use crate::source::PtilesSource;

/// An open PTLR roads file.
///
/// Bands are decompressed on first use and kept, because the in-memory index
/// has to walk the whole band anyway: a query that decompressed per call would
/// pay 66 MB of zstd for every click. Opening costs only the 256-byte header,
/// which matters when a client holds a dozen regions at once and queries one.
pub struct PtlrFile<S: PtilesSource> {
    source: S,
    header: PtlrHeader,
    bands: BTreeMap<usize, Vec<u8>>,
    indexes: BTreeMap<usize, BandIndex>,
    boundary: Option<Vec<Ring>>,
}

impl<S: PtilesSource> PtlrFile<S> {
    pub fn open(source: S) -> Result<Self, FileError> {
        let mut head = [0u8; PTLR_HEADER_SIZE];
        source
            .read_exact_at(0, &mut head)
            .map_err(FileError::Source)?;
        let header = PtlrHeader::parse(&head).map_err(FileError::Decode)?;
        Ok(PtlrFile {
            source,
            header,
            bands: BTreeMap::new(),
            indexes: BTreeMap::new(),
            boundary: None,
        })
    }

    pub fn header(&self) -> &PtlrHeader {
        &self.header
    }

    /// The region polygon, or an empty list when the file carries none.
    pub fn boundary(&mut self) -> Result<&[Ring], FileError> {
        if self.boundary.is_none() {
            let rings = if self.header.boundary_offset == 0 || self.header.boundary_length == 0 {
                Vec::new()
            } else {
                let mut blob = alloc::vec![0u8; self.header.boundary_length as usize];
                self.source
                    .read_exact_at(self.header.boundary_offset, &mut blob)
                    .map_err(FileError::Source)?;
                decode_boundary(&blob).map_err(FileError::Decode)?
            };
            self.boundary = Some(rings);
        }
        Ok(self.boundary.as_deref().unwrap_or(&[]))
    }

    /// Whether this file is the one that owns a point: the polygon decides
    /// when there is one, the bbox otherwise. Callers holding several regional
    /// files (Japan's eight buildings extracts overlap at their seams) need
    /// this to pick among them.
    pub fn covers(&mut self, lat: f64, lon: f64) -> Result<bool, FileError> {
        if !self.boundary()?.is_empty() {
            let rings = self.boundary()?;
            return Ok(crate::boundary::point_in_rings(lon, lat, rings));
        }
        Ok(match self.header.bounds {
            // No bbox recorded (a v1 file): the file must be consulted rather
            // than assumed empty.
            None => true,
            Some((s, w, n, e)) => lat >= s && lat <= n && lon >= w && lon <= e,
        })
    }

    fn band_bytes(&mut self, band: Band) -> Result<&[u8], FileError> {
        let key = band.index();
        if !self.bands.contains_key(&key) {
            let (offset, comp_len) = self.header.band_range(band);
            if comp_len == 0 {
                self.bands.insert(key, Vec::new());
            } else {
                let (dict_off, dict_len) = self.header.dict_range(band);
                let mut dict = alloc::vec![0u8; dict_len as usize];
                if dict_len > 0 {
                    self.source
                        .read_exact_at(dict_off, &mut dict)
                        .map_err(FileError::Source)?;
                }
                let mut frame = alloc::vec![0u8; comp_len as usize];
                self.source
                    .read_exact_at(offset, &mut frame)
                    .map_err(FileError::Source)?;
                let raw = decompress_with_dict_fallback(&frame, &dict).map_err(|message| {
                    FileError::Decompress {
                        offset,
                        message,
                    }
                })?;
                self.bands.insert(key, raw);
            }
        }
        Ok(self.bands.get(&key).map(|v| v.as_slice()).unwrap_or(&[]))
    }

    fn index_for(&mut self, band: Band) -> Result<(), FileError> {
        let key = band.index();
        if !self.indexes.contains_key(&key) {
            let raw = self.band_bytes(band)?.to_vec();
            self.indexes.insert(key, BandIndex::build(&raw));
        }
        Ok(())
    }

    /// Roads whose first vertex is in the cells around a point.
    ///
    /// `rings` widens the search: 2 covers roughly 2 km, enough that a road
    /// whose first vertex is far away but whose geometry passes nearby is
    /// still found. Raise it for a sparse rural query, at proportional cost.
    pub fn roads_near(
        &mut self,
        lat: f64,
        lon: f64,
        rings: i32,
        band: Band,
    ) -> Result<Vec<RoadSegment>, FileError> {
        self.index_for(band)?;
        let pairs = self.indexes[&band.index()].candidates(lat, lon, rings);
        self.decode_pairs(band, &pairs)
    }

    /// Roads whose first vertex is in a bounding box.
    pub fn roads_in_bounds(
        &mut self,
        min_lat: f64,
        min_lon: f64,
        max_lat: f64,
        max_lon: f64,
        band: Band,
    ) -> Result<Vec<RoadSegment>, FileError> {
        self.index_for(band)?;
        let pairs = self.indexes[&band.index()].in_bounds(min_lat, min_lon, max_lat, max_lon);
        self.decode_pairs(band, &pairs)
    }

    fn decode_pairs(
        &mut self,
        band: Band,
        pairs: &[(u32, i64)],
    ) -> Result<Vec<RoadSegment>, FileError> {
        let raw = self.band_bytes(band)?;
        let mut out = Vec::with_capacity(pairs.len());
        for (offset, absolute_id) in pairs {
            // A record that will not decode is skipped rather than failing the
            // query: the rest of the band is independently addressable, and a
            // client that returns nothing because one road is malformed is
            // indistinguishable from one that found no roads.
            if let Ok((road, _, _)) = decode_record(raw, *offset as usize, 0, Some(*absolute_id)) {
                out.push(road);
            }
        }
        Ok(out)
    }
}
