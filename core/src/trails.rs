//! Trail block decoder (`.trails_v1.ptiles`, schema v1).
//!
//! Framing follows `scripts/build_trails.py::enc`, which is `rail.rs`'s record
//! layout plus two attribute bytes: zigzag-delta osm_id, geom_type byte
//! (1 = point/trailhead, else linestring), indexed trail_type, indexed
//! surface, indexed SAC hiking scale, flags, optional u16-length name.
//!
//! The two extra bytes are why trails cannot be decoded by `decode_rail`:
//! reading a trail record with the rail decoder consumes the surface byte as
//! flags and then misreads the rest of the block.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::business_search::{fold_loose, score_match};
use crate::codec::{
    DecodeError, decode_varint, read_i32, read_u8, read_u16, tables, zigzag_decode,
};
use crate::file::{FileError, PtilesFile};
use crate::proximity::haversine_distance_m;
use crate::source::PtilesSource;

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TrailFeature {
    pub osm_id: i64,
    pub trail_type: String,
    pub geom_type: u8, // 0 = linestring, 1 = point/trailhead
    pub coords: Vec<[f64; 2]>,
    pub surface: String,
    pub sac_scale: String,
    pub name: Option<String>,
}

fn lookup(table: &[(u8, &str)], idx: u8) -> String {
    table
        .iter()
        .find(|(i, _)| *i == idx)
        .map(|(_, s)| String::from(*s))
        .unwrap_or_else(|| alloc::format!("unknown({idx})"))
}

fn decode_trail_record(
    data: &[u8],
    pos: usize,
    prev_osm_id: i64,
) -> Result<(TrailFeature, usize, i64), DecodeError> {
    let start = pos;
    let mut p = pos;

    let (delta_raw, consumed) = decode_varint(data, p)?;
    p += consumed;
    let osm_id = prev_osm_id.wrapping_add(zigzag_decode(delta_raw));

    let geom_type = read_u8(data, p)?;
    p += 1;

    let coords = if geom_type == 1 {
        let lon = read_i32(data, p)?;
        let lat = read_i32(data, p + 4)?;
        p += 8;
        alloc::vec![[lon as f64 / 100_000.0, lat as f64 / 100_000.0]]
    } else {
        let vertex_count = read_u16(data, p)? as usize;
        p += 2;
        // Zero-vertex linestrings carry no coordinate bytes at all, so the
        // 8-byte first-vertex header must be skipped too (same guard as rail).
        if vertex_count > 0 {
            let first_lon = read_i32(data, p)?;
            let first_lat = read_i32(data, p + 4)?;
            p += 8;
            let (coords, consumed) =
                crate::codec::decode_coordinates(data, p, first_lon, first_lat, vertex_count)?;
            p += consumed;
            coords
        } else {
            Vec::new()
        }
    };

    let trail_type = lookup(tables::TRAIL_TYPE_REVERSE, read_u8(data, p)?);
    p += 1;
    let surface = lookup(tables::TRAIL_SURFACE_REVERSE, read_u8(data, p)?);
    p += 1;
    let sac_scale = lookup(tables::SAC_SCALE_REVERSE, read_u8(data, p)?);
    p += 1;

    let flags = read_u8(data, p)?;
    p += 1;

    let mut name = None;
    if flags & 0x01 != 0 {
        let (s, consumed) = crate::codec::decode_string_u16(data, p)?;
        name = Some(s);
        p += consumed;
    }

    Ok((
        TrailFeature {
            osm_id,
            trail_type,
            geom_type,
            coords,
            surface,
            sac_scale,
            name,
        },
        p - start,
        osm_id,
    ))
}

/// Whether a trail type is developed infrastructure rather than a natural way.
///
/// `cycleway` and `footway` are built, surfaced routes -- a greenway or a park
/// path with a hard surface. `path`, `track`, `bridleway` and `steps` are the
/// walking/riding kind. A renderer usually wants to draw the two differently,
/// and the split is a property of the layer's type vocabulary, not of any one
/// renderer, so it lives here rather than being re-derived by each caller.
///
/// A trailhead is a point, not a way, and is not developed either way; it
/// answers false so a caller styling lines is never handed a surprise.
pub fn trail_is_developed(trail_type: &str) -> bool {
    matches!(trail_type, "cycleway" | "footway")
}

/// Decode a decompressed trail block into its features. Sequential records,
/// no length prefix — a record that fails to decode stops the scan.
pub fn decode_trails(data: &[u8]) -> Result<Vec<TrailFeature>, DecodeError> {
    let mut features = Vec::new();
    let mut pos = 0usize;
    let mut prev_osm_id = 0i64;

    while pos < data.len() {
        match decode_trail_record(data, pos, prev_osm_id) {
            Ok((feat, consumed, new_prev)) => {
                prev_osm_id = new_prev;
                pos += consumed.max(1);
                features.push(feat);
            }
            Err(_) => break,
        }
    }

    Ok(features)
}

/// One named trail matched by [`search_trails`], reported at the point on it
/// nearest the searcher.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TrailHit {
    pub name: String,
    /// The vertex of the trail nearest the origin -- for a long trail, the end
    /// you would actually walk in from, not an arbitrary first point.
    pub lat: f64,
    pub lon: f64,
    pub distance_m: f64,
    pub is_trailhead: bool,
    /// Same scale as [`crate::BusinessHit::score`]: 2 exact, 1 prefix, 0 substring.
    pub score: u8,
}

/// Name search across an entire `{ST}.trails_v1.ptiles` file.
///
/// Trails have no name-index sidecar -- nothing like
/// `{ST}.business_name_index.ptiles` is built for them -- so a spatial sweep
/// was the only reach a caller had, and a trail beyond the sweep was invisible
/// rather than merely distant. This is the brute-force answer
/// ([`crate::search_business_brute_force`] is the precedent), and it is
/// affordable because the layer is small: Tennessee's is 2.9 MB against the
/// business layer's 54 MB.
///
/// Unlike the business brute force there is no early exit at `limit`. Block
/// order is geographic, so stopping early would return whichever corner of the
/// state the index happens to start in and call it the best match. The whole
/// file is scanned, then hits are ranked by match quality and distance and
/// truncated -- which is only sane because the file is small enough to read in
/// full. `limit` bounds what crosses the FFI, not what is read.
///
/// One row per trail *name*: a single path decodes as many segments and often
/// appears in several blocks, and the row kept is the one whose vertex is
/// nearest `(origin_lat, origin_lon)`.
pub fn search_trails<S: PtilesSource>(
    file: &PtilesFile<S>,
    query: &str,
    origin_lat: f64,
    origin_lon: f64,
    limit: usize,
) -> Result<Vec<TrailHit>, FileError> {
    // The loose fold, not the index one: this scan consults no bucket, so it
    // can afford to ignore punctuation -- which is most of the difference
    // between what a person types and what OSM stored.
    let query_folded = fold_loose(query.trim());
    if query_folded.is_empty() || limit == 0 {
        return Ok(Vec::new());
    }

    // Keyed by folded name so the dedupe is a map lookup, not a linear scan of
    // what is already held: a one-letter query matches thousands of records.
    let mut best: BTreeMap<String, TrailHit> = BTreeMap::new();

    // One decompression per *block*, not per index entry. A trails file packs
    // several cells into one merged block, so the 7,246 entries of the
    // Tennessee index name only 906 blocks -- reading per entry decompressed
    // each of them eight times over, and cost 280 ms against 46.
    let mut blocks: Vec<(u64, u64)> = Vec::new(); // (block offset, a cell in it)
    for entry in file.index() {
        if blocks.last().map(|(off, _)| *off) != Some(entry.block_offset) {
            blocks.push((entry.block_offset, entry.h3_cell));
        }
    }
    let merged = file.has_merged_blocks();

    for (_, cell) in blocks {
        let Some(block) = file.read_block(cell)? else {
            continue;
        };
        // A merged block opens with a cell table, so its records have to be
        // sliced out cell by cell; decoding the raw block reads that table as
        // trail records and yields garbage names.
        let mut records = Vec::new();
        if merged {
            for id in crate::merged::cell_ids(&block)? {
                if let Some(slice) = crate::merged::cell_slice(&block, id)? {
                    records.append(&mut decode_trails(slice)?);
                }
            }
        } else {
            records = decode_trails(&block)?;
        }
        for trail in records {
            let Some(name) = trail.name.filter(|n| !n.trim().is_empty()) else {
                continue;
            };
            let folded = fold_loose(&name);
            let Some(score) = score_match(&folded, &query_folded) else {
                continue;
            };
            let Some((lon, lat, distance_m)) = trail
                .coords
                .iter()
                .map(|c| (c[0], c[1], haversine_distance_m(origin_lat, origin_lon, c[1], c[0])))
                .min_by(|a, b| a.2.total_cmp(&b.2))
            else {
                continue;
            };
            let hit = TrailHit {
                name,
                lat,
                lon,
                distance_m,
                is_trailhead: trail.geom_type == 1,
                score,
            };
            match best.get(&folded) {
                Some(held) if held.distance_m <= hit.distance_m => {}
                _ => {
                    best.insert(folded, hit);
                }
            }
        }
    }

    let mut hits: Vec<TrailHit> = best.into_values().collect();
    hits.sort_by(|a, b| b.score.cmp(&a.score).then(a.distance_m.total_cmp(&b.distance_m)));
    hits.truncate(limit);
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_path() -> std::vec::Vec<u8> {
        let mut d = std::vec::Vec::new();
        d.push(0x02); // osm delta zigzag(1) = 2
        d.push(0); // geom_type = linestring
        d.extend_from_slice(&2u16.to_le_bytes()); // vertex_count
        d.extend_from_slice(&(-11_385_000i32).to_le_bytes()); // first lon micro
        d.extend_from_slice(&4_868_000i32.to_le_bytes()); // first lat micro
        d.push(0x0A); // dlon zigzag(5) = 10
        d.push(0x05); // dlat zigzag(-3) = 5
        d.push(0); // trail_type 0 = path
        d.push(8); // surface 8 = ground
        d.push(2); // sac 2 = mountain_hiking
        d.push(0x01); // flags: name present
        d.extend_from_slice(&9u16.to_le_bytes());
        d.extend_from_slice(b"Highline ");
        d
    }

    #[test]
    fn developed_split_covers_every_type_in_the_table() {
        // Every type the builder can emit must classify without a surprise,
        // so a new type added to the table cannot silently fall through.
        for (_, name) in tables::TRAIL_TYPE_REVERSE {
            let d = trail_is_developed(name);
            let expected = matches!(*name, "cycleway" | "footway");
            assert_eq!(d, expected, "{name} classified wrong");
        }
        assert!(!trail_is_developed("trailhead"));
        assert!(!trail_is_developed("unknown(200)"));
    }

    #[test]
    fn empty_block_decodes_to_empty_vec() {
        assert_eq!(decode_trails(&[]).unwrap(), Vec::new());
    }

    #[test]
    fn linestring_path_decoding_is_exact() {
        let feats = decode_trails(&synthetic_path()).unwrap();
        assert_eq!(feats.len(), 1);
        let f = &feats[0];
        assert_eq!(f.osm_id, 1);
        assert_eq!(f.geom_type, 0);
        assert_eq!(f.trail_type, "path");
        assert_eq!(f.surface, "ground");
        assert_eq!(f.sac_scale, "mountain_hiking");
        assert_eq!(f.coords.len(), 2);
        assert_eq!(f.coords[0], [-113.85, 48.68]);
        assert_eq!(f.coords[1], [-113.84995, 48.67997]);
        assert_eq!(f.name.as_deref(), Some("Highline "));
    }

    #[test]
    fn point_trailhead_decoding_is_exact() {
        let mut d = std::vec::Vec::new();
        d.extend_from_slice(&[0xC8, 0x01]); // zigzag(100)
        d.push(1); // point
        d.extend_from_slice(&(-11_360_000i32).to_le_bytes());
        d.extend_from_slice(&4_850_000i32.to_le_bytes());
        d.push(6); // trailhead
        d.push(0); // surface unset
        d.push(0); // sac unset
        d.push(0x00); // no name
        let feats = decode_trails(&d).unwrap();
        assert_eq!(feats.len(), 1);
        assert_eq!(feats[0].osm_id, 100);
        assert_eq!(feats[0].trail_type, "trailhead");
        assert_eq!(feats[0].surface, "");
        assert_eq!(feats[0].coords, std::vec![[-113.6, 48.5]]);
    }

    #[test]
    fn cross_record_osm_delta_accumulates() {
        let mut d = synthetic_path();
        d.extend(synthetic_path());
        let feats = decode_trails(&d).unwrap();
        assert_eq!(feats.len(), 2);
        assert_eq!(feats[0].osm_id, 1);
        assert_eq!(feats[1].osm_id, 2);
    }

    #[test]
    fn unknown_indices_fall_back_rather_than_panic() {
        let mut d = std::vec::Vec::new();
        d.push(0x02);
        d.push(1); // point
        d.extend_from_slice(&0i32.to_le_bytes());
        d.extend_from_slice(&0i32.to_le_bytes());
        d.push(200); // trail_type out of range
        d.push(201); // surface out of range
        d.push(202); // sac out of range
        d.push(0x00);
        let feats = decode_trails(&d).unwrap();
        assert_eq!(feats[0].trail_type, "unknown(200)");
        assert_eq!(feats[0].surface, "unknown(201)");
        assert_eq!(feats[0].sac_scale, "unknown(202)");
    }

    #[test]
    fn empty_geometry_linestring_yields_no_coords() {
        let mut d = std::vec::Vec::new();
        d.push(0x02);
        d.push(0); // linestring
        d.extend_from_slice(&0u16.to_le_bytes()); // vertex_count 0
        d.push(0); // path
        d.push(0);
        d.push(0);
        d.push(0x00);
        let feats = decode_trails(&d).unwrap();
        assert_eq!(feats.len(), 1);
        assert!(feats[0].coords.is_empty());
        assert_eq!(feats[0].trail_type, "path");
    }

    #[test]
    fn truncation_returns_err_not_panic() {
        let d = [0xC8u8, 0x01, 0x01]; // point claims 8 coord bytes, supplies none
        assert!(decode_trail_record(&d, 0, 0).is_err());
    }

    #[test]
    fn truncated_block_stops_gracefully() {
        let full = synthetic_path();
        for cut in [1usize, 3, 10, full.len() - 1] {
            let feats = decode_trails(&full[..cut]).unwrap();
            assert!(feats.len() <= 1);
        }
    }

    #[cfg(feature = "std")]
    fn two_block_file() -> PtilesFile<crate::source::MemorySource> {
        use crate::fixtures::{ptiles_v1, trail_record};
        // Two cells, and the far one first: block order is the trap the
        // whole-file scan exists to avoid.
        let far = trail_record(1, 0, 8, 0, &[(36.5, -82.0), (36.51, -82.01)], Some("Cumberland Trail"));
        let near = [
            trail_record(1, 0, 8, 0, &[(35.61, -88.81), (35.62, -88.82)], Some("Cypress Greenway")),
            // The same path in two stretches: one row, at the nearer vertex.
            trail_record(1, 0, 8, 0, &[(35.90, -88.90)], Some("Cumberland Trail")),
            trail_record(1, 0, 8, 0, &[(35.70, -88.85)], Some("Cumberland Trail")),
            trail_record(1, 0, 8, 0, &[(35.63, -88.83)], None),
        ]
        .concat();
        let bytes = ptiles_v1(b"PTILEST", &[(1u64, far), (2u64, near)]);
        PtilesFile::open(crate::source::MemorySource(bytes)).expect("open synthetic trails file")
    }

    #[cfg(feature = "std")]
    #[test]
    fn scan_reaches_a_block_a_spatial_sweep_would_never_touch() {
        let file = two_block_file();
        let hits = search_trails(&file, "cumberland", 35.6145, -88.8139, 10).unwrap();
        assert_eq!(hits.len(), 1, "one row per trail name, not one per segment");
        assert_eq!(hits[0].name, "Cumberland Trail");
        // The nearest of the three "Cumberland Trail" records wins, and it is
        // ~10 km out -- past any ring sweep, and still found.
        assert!((hits[0].lat - 35.70).abs() < 1e-9, "{hits:?}");
        assert!(hits[0].distance_m > 1_000.0 && hits[0].distance_m < 30_000.0, "{hits:?}");
    }

    #[cfg(feature = "std")]
    #[test]
    fn punctuation_and_case_do_not_hide_a_trail() {
        use crate::fixtures::{ptiles_v1, trail_record};
        let rec = trail_record(1, 0, 8, 0, &[(35.7, -88.8)], Some("St. Mary's Loop"));
        let file = PtilesFile::open(crate::source::MemorySource(ptiles_v1(
            b"PTILEST",
            &[(1u64, rec)],
        )))
        .unwrap();
        for q in ["st marys loop", "ST. MARY'S", "st. marys"] {
            assert_eq!(
                search_trails(&file, q, 35.6, -88.8, 5).unwrap().len(),
                1,
                "{q} found nothing",
            );
        }
    }

    #[cfg(feature = "std")]
    #[test]
    fn unnamed_ways_and_non_matches_stay_out() {
        let file = two_block_file();
        assert!(search_trails(&file, "greenway", 35.6145, -88.8139, 10).unwrap().len() == 1);
        assert!(search_trails(&file, "zzz", 35.6145, -88.8139, 10).unwrap().is_empty());
        assert!(search_trails(&file, "   ", 35.6145, -88.8139, 10).unwrap().is_empty());
        assert!(search_trails(&file, "trail", 35.6145, -88.8139, 0).unwrap().is_empty());
    }

    // The reason trails needs its own decoder: the rail decoder reads the
    // surface byte where it expects flags, so it cannot round-trip a trail.
    #[test]
    fn rail_decoder_misreads_a_trail_record() {
        let trail = decode_trails(&synthetic_path()).unwrap();
        let as_rail = crate::rail::decode_rail(&synthetic_path()).unwrap();
        assert_eq!(trail[0].name.as_deref(), Some("Highline "));
        assert!(as_rail.is_empty() || as_rail[0].name.as_deref() != Some("Highline "));
    }
}

