//! Integration test exercising the FFI-layer wrapper functions directly
//! (`PtilesLayer`/`PtilesStack`, not `ptiles-core`) against real `.ptiles`
//! files under `/home/aoi/kino/data/ptiles/`. Skips gracefully (prints a
//! message, returns early) when a file is absent so `cargo test -p
//! ptiles-ffi` still passes on a host without the data pulled.

use ptiles_ffi::{
    intersection_holds_traffic, intersection_type_name, AddressLayer, AdminLayer, CandidateKind,
    Fix, LatLon, PtilesError, PtilesLayer, PtilesStack,
};

const DATA_DIR: &str = "/home/aoi/kino/data/ptiles";
const NASHVILLE_LAT: f64 = 36.16;
const NASHVILLE_LON: f64 = -86.78;

fn roads_path() -> String {
    format!("{DATA_DIR}/TN.roads.ptiles")
}
fn buildings_path() -> String {
    format!("{DATA_DIR}/TN.buildings_v8.ptiles")
}
fn admin_path() -> String {
    format!("{DATA_DIR}/US.admin.ptiles")
}
fn business_path() -> String {
    format!("{DATA_DIR}/TN.business.ptiles")
}
fn business_name_index_path() -> String {
    format!("{DATA_DIR}/TN.business_name_index.ptiles")
}
fn parks_path() -> String {
    format!("{DATA_DIR}/TN.parks.ptiles")
}
fn water_path() -> String {
    format!("{DATA_DIR}/TN.water.ptiles")
}
fn rail_path() -> String {
    format!("{DATA_DIR}/TN.rail.ptiles")
}
fn trails_path() -> String {
    format!("{DATA_DIR}/TN.trails_v1.ptiles")
}

/// A decoded class string must look like the format's own vocabulary
/// (`park`, `lake`, `station`). Parks and rail ship merged blocks, and
/// decoding one whole instead of slicing the cell out produces records that
/// exist but carry binary noise in their string fields -- which "the list is
/// non-empty" would happily accept.
fn assert_vocabulary(class: &str, what: &str) {
    assert!(
        !class.is_empty()
            && class
                .chars()
                .all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit()),
        "{what} class is not vocabulary: {class:?}"
    );
}

/// Tennessee is roughly 800 km wide; past that, the coordinates came out of
/// the wrong bytes.
fn assert_plausible_distance(d: f64, what: &str) {
    assert!(
        (0.0..800_000.0).contains(&d),
        "{what} distance is not plausible: {d} m"
    );
}

macro_rules! skip_if_absent {
    ($path:expr) => {
        if !std::path::Path::new(&$path).exists() {
            eprintln!("skipping: {} not present", $path);
            return;
        }
    };
}

#[test]
fn open_roads_layer_and_query_nearest_road() {
    skip_if_absent!(roads_path());
    let layer = PtilesLayer::open(roads_path()).expect("open roads layer");

    let nearest = layer
        .nearest_road(NASHVILLE_LAT, NASHVILLE_LON)
        .expect("nearest_road query should not error");

    // Downtown Nashville is dense with roads; there should be *something*
    // within the CLI's default 100m threshold, and its geometry should be a
    // real polyline (not empty).
    match nearest {
        Some(nr) => {
            assert!(nr.distance_m >= 0.0);
            assert!(!nr.geometry.is_empty(), "nearest road must include geometry");
            assert!(!nr.road_class.is_empty());
        }
        None => panic!("expected a nearby road in downtown Nashville, got None"),
    }
}

#[test]
fn roads_query_center_vs_ring1_grows_result_set() {
    skip_if_absent!(roads_path());
    let layer = PtilesLayer::open(roads_path()).expect("open roads layer");

    let center = layer
        .roads(NASHVILLE_LAT, NASHVILLE_LON, 0)
        .expect("ring-0 roads query");
    let ring1 = layer
        .roads(NASHVILLE_LAT, NASHVILLE_LON, 1)
        .expect("ring-1 roads query");

    assert!(!center.is_empty(), "expected road segments in the center cell");
    assert!(
        ring1.len() >= center.len(),
        "ring-1 result set ({}) should be >= center-only ({})",
        ring1.len(),
        center.len()
    );
}

#[test]
fn roads_query_rejects_ring_greater_than_one() {
    skip_if_absent!(roads_path());
    let layer = PtilesLayer::open(roads_path()).expect("open roads layer");
    let err = layer.roads(NASHVILLE_LAT, NASHVILLE_LON, 2);
    assert!(err.is_err(), "ring=2 must be rejected, matching CLI semantics");
}

#[test]
fn admin_layer_resolves_nashville() {
    skip_if_absent!(admin_path());
    let admin = AdminLayer::open(admin_path()).expect("open admin layer");
    let info = admin
        .admin_at(NASHVILLE_LAT, NASHVILLE_LON)
        .expect("Nashville should resolve");
    assert_eq!(info.state, "Tennessee");
    assert_eq!(info.county, "Davidson");
    assert_eq!(info.timezone, "America/Chicago");
}

#[test]
fn admin_layer_open_bad_path_errors() {
    let err = AdminLayer::open("/nonexistent/US.admin.ptiles".to_string());
    assert!(err.is_err(), "opening a missing admin file must error, not panic");
}

#[test]
fn address_layer_reads_golden_fixture() {
    // The committed synthetic golden fixture (always present).
    let fixture = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test-fixtures/golden/address.ptiles"
    );
    let addr = AddressLayer::open(fixture.to_string()).expect("open address fixture");
    // Reverse: the Nashville cell has 3 addresses incl. 100 Broadway.
    let recs = addr.addresses_at(36.1665, -86.7832, 0).expect("reverse lookup");
    assert!(recs.iter().any(|r| r.housenumber == "100" && r.street == "Broadway"));
    // Forward: case-insensitive.
    let hit = addr
        .find_address(36.1665, -86.7832, 0, "100".into(), "broadway".into())
        .expect("forward lookup");
    assert_eq!(hit.len(), 1);
    assert_eq!(hit[0].osm_id, 1440913532);
}

#[test]
fn address_layer_open_bad_path_errors() {
    assert!(AddressLayer::open("/nonexistent/TN.address.ptiles".to_string()).is_err());
}

#[test]
fn nearest_intersection_finds_a_downtown_junction() {
    skip_if_absent!(roads_path());
    let layer = PtilesLayer::open(roads_path()).expect("open roads layer");

    // Downtown Nashville has mapped signalized intersections; a generous
    // threshold should surface one with a valid control type.
    let nearest = layer
        .nearest_intersection(NASHVILLE_LAT, NASHVILLE_LON, 500.0)
        .expect("nearest_intersection query should not error");

    match nearest {
        Some(ni) => {
            assert!(ni.distance_m >= 0.0 && ni.distance_m <= 500.0);
            assert!(ni.lat.is_finite() && ni.lon.is_finite());
        }
        None => panic!("expected a nearby intersection in downtown Nashville, got None"),
    }
}

#[test]
fn nearest_intersection_tight_threshold_returns_none() {
    skip_if_absent!(roads_path());
    let layer = PtilesLayer::open(roads_path()).expect("open roads layer");
    // A point in the Cumberland River, with a 1m threshold: no intersection
    // that close, but the query must succeed and return None (not error).
    let nearest = layer
        .nearest_intersection(36.16600, -86.77300, 1.0)
        .expect("query should not error");
    assert!(nearest.is_none(), "no intersection within 1m of a river point");
}

#[test]
fn nearest_intersection_on_wrong_layer_errors() {
    skip_if_absent!(business_path());
    let layer = PtilesLayer::open(business_path()).expect("open business layer");
    let err = layer.nearest_intersection(NASHVILLE_LAT, NASHVILLE_LON, 100.0);
    assert!(err.is_err(), "nearest_intersection() on a business-layer file must error");
}

#[test]
fn building_query_on_wrong_layer_errors() {
    skip_if_absent!(roads_path());
    let layer = PtilesLayer::open(roads_path()).expect("open roads layer");
    let err = layer.building(NASHVILLE_LAT, NASHVILLE_LON);
    assert!(err.is_err(), "building() on a roads-layer file must error");
}

#[test]
fn open_buildings_layer_and_query_building() {
    skip_if_absent!(buildings_path());
    let layer = PtilesLayer::open(buildings_path()).expect("open buildings layer");
    // Just exercise the call path; a specific point may or may not be inside
    // a footprint, so only assert it doesn't error.
    let _ = layer
        .building(NASHVILLE_LAT, NASHVILLE_LON)
        .expect("building query should not error");
}

#[test]
fn open_business_layer_and_query_nearby() {
    skip_if_absent!(business_path());
    let layer = PtilesLayer::open(business_path()).expect("open business layer");
    let nearby = layer
        .businesses_near(NASHVILLE_LAT, NASHVILLE_LON, 1, 500.0)
        .expect("businesses_near should not error");
    // Downtown Nashville within 500m/ring-1 should have at least one
    // business in a 52 MB statewide file.
    assert!(!nearby.is_empty(), "expected at least one nearby business");
}

#[test]
fn search_business_finds_a_known_chain_in_tn() {
    skip_if_absent!(business_name_index_path());
    let layer = PtilesLayer::open(business_name_index_path()).expect("open business name index");

    let hits = layer
        .search_business("Waffle House".to_string(), 20)
        .expect("search_business should not error");
    assert!(!hits.is_empty(), "expected at least one Waffle House in TN");
    for hit in &hits {
        assert!(hit.name.to_lowercase().contains("waffle"));
        assert!((34.9..=36.7).contains(&hit.location.lat), "lat {} out of TN range", hit.location.lat);
        assert!((-90.4..=-81.6).contains(&hit.location.lon), "lon {} out of TN range", hit.location.lon);
    }
}

#[test]
fn search_business_respects_limit() {
    skip_if_absent!(business_name_index_path());
    let layer = PtilesLayer::open(business_name_index_path()).expect("open business name index");
    let hits = layer.search_business("s".to_string(), 5).expect("search_business should not error");
    assert!(hits.len() <= 5);
}

#[test]
fn search_business_on_wrong_layer_errors() {
    skip_if_absent!(business_path());
    let layer = PtilesLayer::open(business_path()).expect("open business layer");
    let err = layer.search_business("Waffle House".to_string(), 10);
    assert!(err.is_err(), "search_business() on the main business layer (not the name index) must error");
}

#[test]
fn ptiles_stack_scores_across_layers() {
    skip_if_absent!(roads_path());
    skip_if_absent!(buildings_path());

    let roads = PtilesLayer::open(roads_path()).expect("open roads layer");
    let buildings = PtilesLayer::open(buildings_path()).expect("open buildings layer");
    let stack = PtilesStack::new(Some(roads), Some(buildings), None);

    let fix = Fix {
        lat: NASHVILLE_LAT,
        lon: NASHVILLE_LON,
        horizontal_accuracy_m: 15.0,
        speed_mps: Some(8.0), // moving: road candidates should be weighted up
    };
    let candidates = stack.score(fix, 0).expect("scoring should not error");
    assert!(!candidates.is_empty(), "expected at least one ranked candidate");
    // Scores must be sorted descending.
    for pair in candidates.windows(2) {
        assert!(pair[0].score >= pair[1].score, "candidates must be sorted by score desc");
    }
    // At a brisk simulated speed, a road candidate should be plausible among
    // the top results (not asserting rank 0 -- depends on real geometry --
    // just that road candidates are present at all in a dense road network).
    assert!(
        candidates.iter().any(|c| c.kind == CandidateKind::Road),
        "expected at least one road candidate near downtown Nashville"
    );
}

#[test]
fn nearest_road_on_wrong_layer_errors() {
    skip_if_absent!(business_path());
    let layer = PtilesLayer::open(business_path()).expect("open business layer");
    let err = layer.nearest_road(NASHVILLE_LAT, NASHVILLE_LON);
    assert!(err.is_err(), "nearest_road() on a non-roads layer must error");
}

#[test]
fn roads_on_wrong_layer_errors() {
    skip_if_absent!(business_path());
    let layer = PtilesLayer::open(business_path()).expect("open business layer");
    let err = layer.roads(NASHVILLE_LAT, NASHVILLE_LON, 0);
    assert!(err.is_err(), "roads() on a non-roads layer must error");
}

#[test]
fn businesses_near_on_wrong_layer_errors() {
    skip_if_absent!(roads_path());
    let layer = PtilesLayer::open(roads_path()).expect("open roads layer");
    let err = layer.businesses_near(NASHVILLE_LAT, NASHVILLE_LON, 0, 200.0);
    assert!(err.is_err(), "businesses_near() on a non-business layer must error");
}

#[test]
fn businesses_near_rejects_ring_greater_than_one() {
    skip_if_absent!(business_path());
    let layer = PtilesLayer::open(business_path()).expect("open business layer");
    let err = layer.businesses_near(NASHVILLE_LAT, NASHVILLE_LON, 2, 200.0);
    assert!(err.is_err(), "ring=2 must be rejected on businesses_near");
}

#[test]
fn search_business_empty_query_returns_no_hits() {
    skip_if_absent!(business_name_index_path());
    let layer = PtilesLayer::open(business_name_index_path()).expect("open business name index");
    // Empty / whitespace-only query has nothing to rank against: core returns
    // an empty hit list rather than erroring, and the FFI must surface that as
    // Ok(empty), not an error.
    let hits = layer.search_business("   ".to_string(), 10).expect("empty query must not error");
    assert!(hits.is_empty(), "empty query should yield zero hits, got {}", hits.len());
}

#[test]
fn search_business_zero_limit_returns_no_hits() {
    skip_if_absent!(business_name_index_path());
    let layer = PtilesLayer::open(business_name_index_path()).expect("open business name index");
    let hits = layer.search_business("Waffle House".to_string(), 0).expect("limit 0 must not error");
    assert!(hits.is_empty(), "limit=0 should yield zero hits");
}

#[test]
fn building_happy_path_finds_or_none_without_error() {
    // Distinct from open_buildings_layer_and_query_building: assert the
    // building() call round-trips a Some result's fields when one is found,
    // rather than only that it doesn't error.
    skip_if_absent!(buildings_path());
    let layer = PtilesLayer::open(buildings_path()).expect("open buildings layer");
    if let Some(b) = layer.building(NASHVILLE_LAT, NASHVILLE_LON).expect("building query") {
        assert!(!b.building_type.is_empty(), "building_type must be populated");
        assert!((34.0..=37.0).contains(&b.centroid.lat), "centroid lat out of TN range");
    }
}

#[test]
fn stack_with_no_layers_scores_empty() {
    // PtilesStack::new happy path with all-None layers: score() must succeed
    // and return an empty candidate list (nothing to rank), not error.
    let stack = PtilesStack::new(None, None, None);
    let fix = Fix {
        lat: NASHVILLE_LAT,
        lon: NASHVILLE_LON,
        horizontal_accuracy_m: 10.0,
        speed_mps: None,
    };
    let candidates = stack.score(fix, 0).expect("empty stack must score without error");
    assert!(candidates.is_empty(), "an empty stack yields no candidates");
}

#[test]
fn stack_score_rejects_ring_greater_than_one() {
    let stack = PtilesStack::new(None, None, None);
    let fix = Fix {
        lat: NASHVILLE_LAT,
        lon: NASHVILLE_LON,
        horizontal_accuracy_m: 10.0,
        speed_mps: None,
    };
    let err = stack.score(fix, 2);
    assert!(err.is_err(), "ring=2 must be rejected on stack score");
}

#[test]
fn open_business_name_index_then_roads_query_errors() {
    // A business_name_index file is its own LayerKind; a lat/lon roads query
    // against it must be rejected as unsupported-for-layer.
    skip_if_absent!(business_name_index_path());
    let layer = PtilesLayer::open(business_name_index_path()).expect("open name index");
    assert!(
        layer.roads(NASHVILLE_LAT, NASHVILLE_LON, 0).is_err(),
        "roads() on a business_name_index layer must error"
    );
}

#[test]
fn open_invalid_bytes_errors() {
    // A correctly-named file (<state>.roads.ptiles, so layer inference
    // succeeds) whose bytes are not a valid ptiles container must fail at
    // PtilesFile::open (header/index parse) -> PtilesError::Open, exercising
    // the invalid-bytes path distinctly from the UnknownLayer path.
    let mut p = std::env::temp_dir();
    p.push(format!("ptiles_ffi_invalid_{}.roads.ptiles", std::process::id()));
    std::fs::write(&p, b"not a real ptiles file, just garbage bytes").expect("write temp file");
    let result = PtilesLayer::open(p.to_string_lossy().into_owned());
    let _ = std::fs::remove_file(&p);
    assert!(result.is_err(), "opening garbage bytes as a .ptiles file must error");
}

#[test]
fn parks_layer_lists_and_locates() {
    skip_if_absent!(parks_path());
    let layer = PtilesLayer::open(parks_path()).expect("open parks layer");

    let parks = layer.parks(NASHVILLE_LAT, NASHVILLE_LON, 1).expect("parks query");
    assert!(!parks.is_empty(), "downtown Nashville has parks in ring 1");
    for p in &parks {
        assert_vocabulary(&p.park_type, "park");
        assert!(p.geometry.len() >= 3, "a park polygon needs three vertices");
    }

    let at = layer.park_at(NASHVILLE_LAT, NASHVILLE_LON, 1).expect("park_at query");
    let at = at.expect("with parks in range there is always a nearest one");
    assert_eq!(at.kind, "park");
    assert_vocabulary(&at.class, "nearest park");
    assert_plausible_distance(at.distance_m, "nearest park");
    // Inside means zero distance to the boundary; outside means a real one.
    assert_eq!(at.inside, at.distance_m == 0.0);
}

#[test]
fn water_layer_lists_and_locates() {
    skip_if_absent!(water_path());
    let layer = PtilesLayer::open(water_path()).expect("open water layer");

    let water = layer.water(NASHVILLE_LAT, NASHVILLE_LON, 1).expect("water query");
    assert!(!water.is_empty(), "the Cumberland runs through downtown");
    // Reference geometries (geom_type 2) carry no coordinates; everything
    // else must.
    assert!(water
        .iter()
        .all(|w| w.geom_type == 2 || !w.geometry.is_empty()));

    let at = layer.water_at(NASHVILLE_LAT, NASHVILLE_LON, 1).expect("water_at query");
    if let Some(a) = at {
        assert_eq!(a.kind, "water");
        assert_vocabulary(&a.class, "nearest water");
        assert_plausible_distance(a.distance_m, "nearest water");
    }
}

#[test]
fn rail_layer_separates_track_from_station() {
    skip_if_absent!(rail_path());
    let layer = PtilesLayer::open(rail_path()).expect("open rail layer");

    let rail = layer.rail(NASHVILLE_LAT, NASHVILLE_LON, 1).expect("rail query");
    let track = layer
        .nearest_rail(NASHVILLE_LAT, NASHVILLE_LON, 1)
        .expect("nearest_rail query");
    let station = layer
        .nearest_station(NASHVILLE_LAT, NASHVILLE_LON, 1)
        .expect("nearest_station query");

    // Whatever the cell holds, a track answer must be a way and a station
    // answer must be a point -- never the other way round.
    for r in &rail {
        assert_vocabulary(&r.rail_type, "rail");
    }
    if let Some(t) = &track {
        assert_eq!(t.kind, "rail");
        assert!(rail.iter().any(|r| r.geom_type == 0));
        assert_plausible_distance(t.distance_m, "nearest rail");
    }
    if let Some(s) = &station {
        assert_eq!(s.kind, "station");
        assert!(rail.iter().any(|r| r.geom_type == 1));
        assert_plausible_distance(s.distance_m, "nearest station");
    }
}

#[test]
fn trails_layer_lists_and_locates() {
    skip_if_absent!(trails_path());
    let layer = PtilesLayer::open(trails_path()).expect("open trails layer");

    let trails = layer.trails(NASHVILLE_LAT, NASHVILLE_LON, 1).expect("trails query");
    let way = layer
        .nearest_trail(NASHVILLE_LAT, NASHVILLE_LON, 1)
        .expect("nearest_trail query");
    let head = layer
        .nearest_trailhead(NASHVILLE_LAT, NASHVILLE_LON, 1)
        .expect("nearest_trailhead query");

    if let Some(w) = &way {
        assert_eq!(w.kind, "trail");
        assert!(trails.iter().any(|t| t.geom_type == 0));
    }
    if let Some(h) = &head {
        assert_eq!(h.kind, "trailhead");
        assert!(trails.iter().any(|t| t.geom_type == 1));
    }
}

#[test]
fn layer_methods_reject_the_wrong_file() {
    skip_if_absent!(roads_path());
    let roads = PtilesLayer::open(roads_path()).expect("open roads layer");
    assert!(matches!(
        roads.trails(NASHVILLE_LAT, NASHVILLE_LON, 0),
        Err(PtilesError::UnsupportedForLayer { .. })
    ));
    assert!(matches!(
        roads.park_at(NASHVILLE_LAT, NASHVILLE_LON, 0),
        Err(PtilesError::UnsupportedForLayer { .. })
    ));
    assert!(matches!(
        roads.nearest_station(NASHVILLE_LAT, NASHVILLE_LON, 0),
        Err(PtilesError::UnsupportedForLayer { .. })
    ));
}

#[test]
fn stack_locate_answers_from_whichever_layers_it_holds() {
    skip_if_absent!(roads_path());
    skip_if_absent!(parks_path());
    let roads = PtilesLayer::open(roads_path()).expect("open roads layer");
    let parks = PtilesLayer::open(parks_path()).expect("open parks layer");
    let stack = PtilesStack::with_layers(
        Some(roads),
        None,
        None,
        None,
        Some(parks),
        None,
        None,
    );

    let got = stack.locate(NASHVILLE_LAT, NASHVILLE_LON, 0).expect("locate");
    let way = got.nearest_way.expect("downtown always has a road");
    assert_eq!(way.kind, "road", "no trails layer, so no trail can win");
    assert_eq!(way.on_it, got.on_way.is_some());
    assert!(got.address.is_none(), "no address layer was supplied");
    assert!(got.water.is_none(), "no water layer was supplied");
}

#[test]
fn empty_stack_locates_nothing_without_erroring() {
    let stack = PtilesStack::new(None, None, None);
    let got = stack.locate(NASHVILLE_LAT, NASHVILLE_LON, 0).expect("locate");
    assert!(got.nearest_way.is_none());
    assert!(got.on_way.is_none());
    assert!(got.park.is_none());
}

#[test]
fn open_empty_path_errors() {
    let err = PtilesLayer::open(String::new());
    assert!(err.is_err(), "empty path must error (no layer inferable)");
}

#[test]
fn open_unknown_layer_filename_errors() {
    // No file needs to exist -- LayerKind inference fails before any I/O.
    let err = PtilesLayer::open("/tmp/not-a-real-state.ptiles".to_string());
    assert!(err.is_err(), "a filename without <state>.<layer>.ptiles must error");
}

#[test]
fn open_missing_file_errors() {
    let err = PtilesLayer::open(format!("{DATA_DIR}/TN.roads.does-not-exist.ptiles"));
    // Layer inference on "roads.does-not-exist" fails (unknown 2nd token
    // isn't "roads"/"buildings_v8"/"business"), so this exercises the
    // UnknownLayer path deterministically without depending on the data dir.
    assert!(err.is_err());
}

// --- Gaps closed for the Android integration -------------------------------
//
// These prefer the committed conformance corpus over the machine-local data
// directory, so they run on a host with no data pulled -- the surfaces they
// cover (batch grouping, prefetch, metadata) are the ones an app depends on and
// are worth having green everywhere, not only where 33 MB layers exist.

fn corpus(name: &str) -> String {
    format!("{}/../conformance/corpus/{name}", env!("CARGO_MANIFEST_DIR"))
}

/// Corpus first, machine-local data second.
fn any_roads_path() -> Option<String> {
    for p in [corpus("TN.roads.ptiles"), roads_path()] {
        if std::path::Path::new(&p).exists() {
            return Some(p);
        }
    }
    None
}

fn any_buildings_path() -> Option<String> {
    for p in [corpus("TN.buildings_v8.ptiles"), buildings_path()] {
        if std::path::Path::new(&p).exists() {
            return Some(p);
        }
    }
    None
}

/// A coordinate the file actually holds a block for.
///
/// The header bbox is the whole state, but the corpus slice is 48 cells cut out
/// of it, so the bbox centre usually lands on a cell with no block -- being
/// inside the coverage box has never promised a block exists. The cell ids the
/// slice kept are recorded in `conformance/manifest.json`, so this reads the
/// first one and takes its centre; for a full machine-local layer, downtown
/// Nashville does.
fn a_covered_point(path: &str, layer: &PtilesLayer) -> (f64, f64) {
    if let Some(cell) = corpus_first_cell(path) {
        let (lat, lon) = ptiles_core::cell_center(cell);
        return (lat, lon);
    }
    let _ = layer;
    (NASHVILLE_LAT, NASHVILLE_LON)
}

/// A point that sits *on a feature* in the first block of `path`.
///
/// The centre of a covered cell is not good enough: an H3 res-7 cell is ~1.2 km
/// across and the corpus slice is rural west Tennessee, so the centre is often
/// several hundred metres from the nearest road -- past any sane snap threshold.
/// Asking the data where its features are keeps these tests meaningful on both
/// the 30 KB corpus slice and a full 33 MB layer.
fn first_feature_point(path: &str, buildings: bool) -> Option<(f64, f64)> {
    let src = ptiles_core::FileSource::open(path).ok()?;
    let file = ptiles_core::PtilesFile::open(src).ok()?;
    let version = file.header().version;
    for entry in file.index() {
        if entry.block_length == 0 {
            continue;
        }
        let Ok(Some(block)) = file.read_block(entry.h3_cell) else {
            continue;
        };
        if buildings {
            let (clat, clon) = ptiles_core::cell_center(entry.h3_cell);
            if let Ok(bs) = ptiles_core::decode_buildings(&block, clat, clon) {
                if let Some(b) = bs.first() {
                    return Some((b.centroid_lat, b.centroid_lon));
                }
            }
        } else {
            let _ = version;
            if let Ok(roads) = ptiles_core::decode_roads(&block) {
                if let Some(c) = roads.iter().find_map(|r| r.coords.first()) {
                    return Some((c[1], c[0]));
                }
            }
        }
    }
    None
}

/// `first_cell` for a corpus file, from the manifest. `None` for anything that
/// is not a corpus path.
fn corpus_first_cell(path: &str) -> Option<u64> {
    let name = path.rsplit('/').next()?;
    if !path.contains("conformance/corpus") {
        return None;
    }
    let manifest = std::fs::read_to_string(format!(
        "{}/../conformance/manifest.json",
        env!("CARGO_MANIFEST_DIR")
    ))
    .ok()?;
    // One field out of one object: a five-line scan beats a serde_json
    // dev-dependency this crate does not otherwise need.
    let start = manifest.find(&format!("\"{name}\""))?;
    let key = manifest[start..].find("\"first_cell\"")? + start;
    let open = manifest[key..].find(':')? + key + 1;
    let hex = manifest[open..].trim_start().trim_start_matches('"');
    let hex = &hex[..hex.find('"')?];
    u64::from_str_radix(hex, 16).ok()
}

#[test]
fn metadata_reports_coverage_and_provenance() {
    let Some(path) = any_roads_path() else { return };
    let layer = PtilesLayer::open(path.clone()).expect("open roads");
    let m = layer.metadata();

    assert_eq!(m.layer, "roads");
    assert_eq!(m.path, path);
    assert!(m.version >= 1, "version {}", m.version);
    assert!(m.block_count > 0, "a layer with no blocks is not useful");
    assert!(m.min_lat < m.max_lat && m.min_lon < m.max_lon, "empty bbox: {m:?}");
    assert!(m.byte_length.unwrap_or(0) > 0);
    // Local file: no HTTP validators. That is the honest answer -- the format
    // itself carries no build date, so provenance is only available remotely.
    assert_eq!(m.last_modified, None);
    assert_eq!(m.etag, None);

    // Coverage is answerable without any read.
    let (lat, lon) = a_covered_point(&path, &layer);
    assert!(layer.covers(lat, lon));
    assert!(!layer.covers(0.0, 0.0), "null island is not in Tennessee");
    assert!(!layer.covers(m.max_lat + 1.0, lon));
}

#[test]
fn batch_queries_agree_with_the_single_point_ones() {
    let Some(path) = any_roads_path() else { return };
    let layer = PtilesLayer::open(path.clone()).expect("open roads");
    let Some((lat, lon)) = first_feature_point(&path, false) else { return };
    // A short walk along one road, plus one point far outside coverage.
    let points: Vec<LatLon> = (0..8)
        .map(|i| LatLon {
            lat: lat + i as f64 * 0.0002,
            lon: lon + i as f64 * 0.0002,
        })
        .chain([LatLon { lat: 0.0, lon: 0.0 }])
        .collect();

    let batch = layer
        .nearest_roads_at(points.clone(), 0.0)
        .expect("batch nearest_road");
    assert_eq!(batch.len(), points.len(), "one answer per input, in order");
    assert!(batch.last().unwrap().is_none(), "null island has no road");

    for (i, p) in points.iter().enumerate() {
        let single = layer.nearest_road(p.lat, p.lon).expect("single nearest_road");
        match (&batch[i], &single) {
            (Some(b), Some(s)) => {
                assert_eq!(b.osm_id, s.osm_id, "point {i} disagrees");
                assert!((b.distance_m - s.distance_m).abs() < 1e-9);
            }
            (None, None) => {}
            (b, s) => panic!("point {i}: batch {b:?} vs single {s:?}"),
        }
    }
}

#[test]
fn a_batch_over_one_cell_reads_one_block() {
    // The point of the batch API. Eight points inside one H3 cell must cost one
    // block, not eight -- that ratio is what makes per-point enrichment of a
    // day's trace (~12,000 points, a few dozen cells) possible at all.
    let Some(path) = any_roads_path() else { return };
    let layer = PtilesLayer::open(path.clone()).expect("open roads");
    let Some((lat, lon)) = first_feature_point(&path, false) else { return };
    let points: Vec<LatLon> = (0..8)
        .map(|i| LatLon { lat: lat + i as f64 * 0.00005, lon })
        .collect();

    assert_eq!(layer.cached_block_count(), 0);
    let found = layer.nearest_roads_at(points, 0.0).expect("batch");
    let after = layer.cached_block_count();
    assert!(
        (1..=2).contains(&after),
        "8 points in ~1 cell should touch 1-2 blocks, touched {after}"
    );
    // And the batch actually answered, or the block count above proves nothing.
    assert!(
        found.iter().any(|r| r.is_some()),
        "no roads found at a covered point -- this test would pass on an empty layer"
    );

    // And a second pass adds nothing: the blocks are already decompressed.
    let again = vec![LatLon { lat, lon }];
    layer.nearest_roads_at(again, 0.0).expect("second batch");
    assert_eq!(layer.cached_block_count(), after, "second pass re-read blocks");

    layer.clear_cache();
    assert_eq!(layer.cached_block_count(), 0);
}

#[test]
fn buildings_batch_matches_single_and_groups() {
    let Some(path) = any_buildings_path() else { return };
    let layer = PtilesLayer::open(path.clone()).expect("open buildings");
    let Some((lat, lon)) = first_feature_point(&path, true) else { return };
    // Tight spacing: these must stay inside the one building's cell, and within
    // the 50 m centroid fallback for at least the first point.
    let points: Vec<LatLon> = (0..6)
        .map(|i| LatLon { lat: lat + i as f64 * 0.00002, lon })
        .collect();

    let batch = layer.buildings_at(points.clone()).expect("batch buildings");
    assert_eq!(batch.len(), points.len());
    assert!(
        batch.iter().any(|b| b.is_some()),
        "no buildings at a covered point -- the comparison below would be vacuous"
    );
    for (i, p) in points.iter().enumerate() {
        let single = layer.building(p.lat, p.lon).expect("single building");
        assert_eq!(
            batch[i].as_ref().map(|b| b.osm_id),
            single.as_ref().map(|b| b.osm_id),
            "point {i} disagrees"
        );
    }
    assert!(layer.cached_block_count() <= 2, "grouping failed");
}

#[test]
fn batch_queries_reject_the_wrong_layer() {
    let Some(path) = any_buildings_path() else { return };
    let layer = PtilesLayer::open(path.clone()).expect("open buildings");
    let p = vec![LatLon { lat: 36.16, lon: -86.78 }];
    assert!(layer.nearest_roads_at(p.clone(), 0.0).is_err());
    assert!(layer.nearest_intersections_at(p, 0.0).is_err());
}

#[test]
fn prefetch_bbox_warms_the_region_then_queries_are_free() {
    let Some(path) = any_roads_path() else { return };
    let layer = PtilesLayer::open(path.clone()).expect("open roads");

    // A box around a real feature, not the whole layer: the cell cap (512 res-7
    // cells, ~2,600 km^2) means a state-sized prefetch is deliberately refused,
    // which `prefetch_refuses_an_oversized_bbox` covers.
    let Some((lat, lon)) = first_feature_point(&path, false) else { return };
    let warmed = layer
        .prefetch_bbox(lat - 0.05, lon - 0.05, lat + 0.05, lon + 0.05)
        .expect("prefetch a city-sized box");
    assert!(warmed > 0, "the middle of the layer's coverage should hold blocks");
    let cached = layer.cached_block_count();
    assert!(cached >= warmed, "absent cells are cached too");

    // Every query inside the region now hits memory.
    layer.nearest_road(lat, lon).expect("query after prefetch");
    assert_eq!(layer.cached_block_count(), cached, "prefetch missed a block");
}

#[test]
fn prefetch_refuses_an_oversized_bbox() {
    // A whole-hemisphere prefetch is an error, not a silent truncation: a
    // partial prefetch that reports success is worse than a refusal, because
    // the caller then trusts a region it does not have.
    let Some(path) = any_roads_path() else { return };
    let layer = PtilesLayer::open(path.clone()).expect("open roads");
    match layer.prefetch_bbox(-89.0, -179.0, 89.0, 179.0) {
        Err(PtilesError::InvalidBounds { message }) => {
            assert!(message.contains("too large"), "{message}");
        }
        other => panic!("expected InvalidBounds, got {:?}", other.map(|n| n)),
    }
    // A malformed box is the same class of error, not a panic.
    assert!(matches!(
        layer.prefetch_bbox(f64::NAN, 0.0, 1.0, 1.0),
        Err(PtilesError::InvalidBounds { .. })
    ));
}

#[test]
fn an_unreachable_host_is_a_network_error_not_a_missing_file() {
    // The distinction the Android offline fallback had to guess at. Port 1 on
    // localhost refuses connections, so this needs no network and no server:
    // the failure is transport-level, and it must not look like "no such
    // layer".
    let err = match PtilesLayer::open("http://127.0.0.1:1/TN.roads.ptiles".to_string()) {
        Err(e) => e,
        Ok(_) => panic!("connection to port 1 must fail"),
    };
    assert!(
        matches!(err, PtilesError::Network { .. }),
        "expected Network, got {err:?}"
    );
    // And the message says which of the two it was, for a log reader.
    assert!(err.to_string().contains("network error"), "{err}");
}

#[test]
fn the_intersection_vocabulary_is_the_formats_own() {
    assert_eq!(intersection_type_name(1), "traffic_signals");
    assert_eq!(intersection_type_name(2), "stop");
    assert_eq!(intersection_type_name(3), "give_way");
    assert_eq!(intersection_type_name(4), "roundabout");
    // 0 and anything unrecognised: the node is mapped, its control is not
    // stated. Naming it anything more specific would be invention.
    assert_eq!(intersection_type_name(0), "junction");
    assert_eq!(intersection_type_name(200), "junction");

    assert!(intersection_holds_traffic(1));
    assert!(intersection_holds_traffic(2));
    assert!(intersection_holds_traffic(3));
    assert!(!intersection_holds_traffic(4), "a roundabout does not queue");
    assert!(!intersection_holds_traffic(0));
}

#[test]
fn a_real_intersection_names_itself() {
    let Some(path) = any_roads_path() else { return };
    let layer = PtilesLayer::open(path.clone()).expect("open roads");
    let Some((lat, lon)) = first_feature_point(&path, false) else { return };
    // Whatever the corpus slice holds nearby; the claim is only that its type
    // byte is nameable, which is what a caller holding the integer needs.
    if let Some(ix) = layer
        .nearest_intersection(lat, lon, 5000.0)
        .expect("nearest_intersection")
    {
        let name = intersection_type_name(ix.intersection_type);
        assert!(!name.is_empty());
        assert!(
            ["traffic_signals", "stop", "give_way", "roundabout", "junction"].contains(&name.as_str()),
            "unexpected name {name}"
        );
    }
}
