//! Integration test exercising the FFI-layer wrapper functions directly
//! (`PtilesLayer`/`PtilesStack`, not `ptiles-core`) against real `.ptiles`
//! files under `/home/aoi/kino/data/ptiles/`. Skips gracefully (prints a
//! message, returns early) when a file is absent so `cargo test -p
//! ptiles-ffi` still passes on a host without the data pulled.

use ptiles_ffi::{AddressLayer, AdminLayer, CandidateKind, Fix, PtilesLayer, PtilesStack};

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
