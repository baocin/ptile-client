//! GPX snap integration test: for every point of the `tn-middle-tennessee`
//! GPX track (test-fixtures/parsed.json), resolve its H3 res-7 cell, read
//! and decode the corresponding block(s) from the real `TN.roads.ptiles`
//! file, and confirm `nearest_road` snaps within 100 m.
//!
//! Skips gracefully (eprintln + return) if either the GPX fixture or the
//! real `.ptiles` data file is absent, so machines without
//! `~/kino/data/ptiles/` populated still build/pass.

use std::collections::HashMap;
use std::path::Path;

use ptiles_core::{
    cell_for_coord, decode_roads, nearest_road, neighbor_cells, FileSource, PtilesFile,
    RoadSegment,
};

const SNAP_THRESHOLD_M: f64 = 100.0;
/// Only bother expanding to neighbor cells when the center-cell result is
/// missing or worse than this -- avoids paying for 6x extra block
/// reads/decodes on every point when the center cell alone already gives a
/// good snap.
const NEIGHBOR_EXPAND_THRESHOLD_M: f64 = 30.0;

#[test]
fn gpx_track_snaps_to_roads_within_100m() {
    let gpx_path = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../test-fixtures/parsed.json"));
    if !gpx_path.exists() {
        eprintln!("skipping: GPX fixture not present at {gpx_path:?}");
        return;
    }
    let roads_path = Path::new("/home/aoi/kino/data/ptiles/TN.roads.ptiles");
    if !roads_path.exists() {
        eprintln!("skipping: roads fixture not present at {roads_path:?}");
        return;
    }

    let raw = std::fs::read_to_string(gpx_path).expect("read parsed.json");
    let tracks: serde_json::Value = serde_json::from_str(&raw).expect("parse parsed.json as JSON");
    let tracks = tracks.as_array().expect("parsed.json must be a JSON array of tracks");

    let track = tracks
        .iter()
        .find(|t| t.get("label").and_then(|v| v.as_str()) == Some("tn-middle-tennessee"))
        .expect("tn-middle-tennessee track must be present in parsed.json");

    let points = track
        .get("points")
        .and_then(|v| v.as_array())
        .expect("track must have a points array");
    assert_eq!(points.len(), 1187, "expected 1187 points in tn-middle-tennessee track");

    let src = FileSource::open(roads_path).expect("open TN.roads.ptiles");
    let file = PtilesFile::open(src).expect("parse TN.roads.ptiles header/dict/index");

    // Cache decoded blocks per cell -- points cluster tightly in H3 res-7
    // cells (~1.2km edge), so repeated cell hits are the common case.
    let mut block_cache: HashMap<u64, Vec<RoadSegment>> = HashMap::new();
    let mut missing_cells: HashMap<u64, ()> = HashMap::new();

    let mut worst_distance_m = 0.0f64;
    let mut worst_point = (0.0f64, 0.0f64);
    let mut unsnapped: Vec<(usize, f64, f64)> = Vec::new();
    let mut total_snapped = 0usize;

    for (i, pt) in points.iter().enumerate() {
        let arr = pt.as_array().expect("point must be a [lat, lon] pair");
        let lat = arr[0].as_f64().expect("lat must be a number");
        let lon = arr[1].as_f64().expect("lon must be a number");

        let cell = cell_for_coord(lat, lon);
        assert_ne!(cell, 0, "point ({lat}, {lon}) must resolve to a valid H3 cell");

        let mut best = load_roads(&file, cell, &mut block_cache, &mut missing_cells)
            .and_then(|roads| nearest_road(lat, lon, roads, SNAP_THRESHOLD_M));

        // Expand to ring-1 neighbors when the center cell gave nothing, or
        // only a marginal snap -- the nearest road may sit just across a
        // cell boundary (SPEC.md step 6).
        let needs_expansion = match &best {
            None => true,
            Some(nr) => nr.distance_m > NEIGHBOR_EXPAND_THRESHOLD_M,
        };
        if needs_expansion {
            for neighbor in neighbor_cells(cell) {
                if let Some(roads) = load_roads(&file, neighbor, &mut block_cache, &mut missing_cells) {
                    if let Some(nr) = nearest_road(lat, lon, roads, SNAP_THRESHOLD_M) {
                        if best.is_none_or(|b| nr.distance_m < b.distance_m) {
                            best = Some(nr);
                        }
                    }
                }
            }
        }

        match best {
            Some(nr) => {
                total_snapped += 1;
                if nr.distance_m > worst_distance_m {
                    worst_distance_m = nr.distance_m;
                    worst_point = (lat, lon);
                }
            }
            None => unsnapped.push((i, lat, lon)),
        }
    }

    println!(
        "gpx_snap: {}/{} points snapped within {}m, worst-case distance = {:.2}m at ({:.5}, {:.5})",
        total_snapped,
        points.len(),
        SNAP_THRESHOLD_M,
        worst_distance_m,
        worst_point.0,
        worst_point.1
    );
    if !unsnapped.is_empty() {
        println!(
            "gpx_snap: {} points did not snap within {}m (first few: {:?})",
            unsnapped.len(),
            SNAP_THRESHOLD_M,
            &unsnapped[..unsnapped.len().min(5)]
        );
    }

    assert!(
        unsnapped.is_empty(),
        "{} of {} points failed to snap within {}m; worst-case among snapped = {:.2}m; unsnapped sample: {:?}",
        unsnapped.len(),
        points.len(),
        SNAP_THRESHOLD_M,
        worst_distance_m,
        &unsnapped[..unsnapped.len().min(5)]
    );
    assert!(
        worst_distance_m <= SNAP_THRESHOLD_M,
        "worst-case snap distance {worst_distance_m:.2}m exceeds {SNAP_THRESHOLD_M}m threshold"
    );
}

/// Read + decode the roads block for `cell`, memoized in `cache`. Returns
/// `None` if the cell has no block in the index (memoized in
/// `missing_cells` to avoid repeat index lookups for empty/sparse areas).
fn load_roads<'a>(
    file: &PtilesFile<FileSource>,
    cell: u64,
    cache: &'a mut HashMap<u64, Vec<RoadSegment>>,
    missing_cells: &mut HashMap<u64, ()>,
) -> Option<&'a [RoadSegment]> {
    if missing_cells.contains_key(&cell) {
        return None;
    }
    if let std::collections::hash_map::Entry::Vacant(e) = cache.entry(cell) {
        match file.read_block(cell).expect("read_block must not error") {
            Some(block) => {
                let roads = decode_roads(&block).expect("decode_roads must parse a real block");
                e.insert(roads);
            }
            None => {
                missing_cells.insert(cell, ());
                return None;
            }
        }
    }
    cache.get(&cell).map(|v| v.as_slice())
}
