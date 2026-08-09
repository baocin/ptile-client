//! FFI-surface tests over `.ptiles` files this test builds itself
//! (`ptiles_core::fixtures`).
//!
//! `integration.rs` needs the real corpus and skips whatever it lacks -- all
//! trails files, and the whole suite on a host with no data. These run
//! anywhere, and they cover what skipping hid: the trail lookups, the
//! per-cell slicing of a v2 merged block, and `PtilesStack::locate`
//! preferring the closer of a road and a trail.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ptiles_core::cell_for_coord;
use ptiles_core::fixtures::{camera_record, park_record, ptiles_v1, ptiles_v2_merged, trail_record};
use ptiles_ffi::{
    IndoorOutdoorReason, IndoorOutdoorState, PtilesError, PtilesLayer, PtilesStack,
};

const LAT: f64 = 36.16;
const LON: f64 = -86.78;

/// Write a fixture under `<state>.<layer>.ptiles` (the FFI infers the layer
/// from that name) in a directory of this test's own. Tests run in parallel
/// and each builds its own fixtures, so a shared filename would have one test
/// truncating a file another is reading.
fn write_fixture(owner: &str, name: &str, bytes: Vec<u8>) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("synthetic").join(owner);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join(name);
    std::fs::write(&path, bytes).expect("write fixture");
    path
}

fn open(path: PathBuf) -> Arc<PtilesLayer> {
    PtilesLayer::open(path.to_str().unwrap().to_string()).expect("open fixture layer")
}

/// A path running east through the query point, plus a trailhead ~22 m north
/// of it. Named `_v1` to also prove the FFI strips the version suffix real
/// snapshots carry.
fn trails_layer(owner: &str) -> Arc<PtilesLayer> {
    let cell = cell_for_coord(LAT, LON);
    let mut records = trail_record(
        2,
        0, // path
        5, // compacted
        1, // hiking
        &[(LAT, LON - 0.01), (LAT, LON + 0.01)],
        Some("Greenway"),
    );
    records.extend(trail_record(2, 6, 0, 0, &[(LAT + 0.0002, LON)], Some("North Gate")));
    open(write_fixture(
        owner,
        "XX.trails_v1.ptiles",
        ptiles_v1(b"PTILEST", &[(cell, records)]),
    ))
}

fn ring(lat: f64, lon: f64) -> Vec<(f64, f64)> {
    vec![
        (lat - 0.002, lon - 0.002),
        (lat - 0.002, lon + 0.002),
        (lat + 0.002, lon + 0.002),
        (lat + 0.002, lon - 0.002),
    ]
}

/// Two cells in one physical block -- the layout that returns garbage when a
/// decoder is handed the block whole.
fn merged_parks_layer(owner: &str) -> Arc<PtilesLayer> {
    let here = cell_for_coord(LAT, LON);
    let elsewhere = cell_for_coord(35.05, -85.31);
    let block = vec![
        (here, park_record(2, "park", &ring(LAT, LON), Some("Here Park"))),
        (
            elsewhere,
            park_record(2, "nature_reserve", &ring(35.05, -85.31), Some("Far Reserve")),
        ),
    ];
    open(write_fixture(
        owner,
        "XX.parks.ptiles",
        ptiles_v2_merged(b"PTILESP", &[block]),
    ))
}

#[test]
fn a_versioned_trails_filename_still_resolves_to_the_trails_layer() {
    let layer = trails_layer("versioned_name");
    assert_eq!(layer.metadata().layer, "trails");
    assert!(layer.covers(LAT, LON));
}

#[test]
fn trail_and_trailhead_are_separate_answers() {
    let layer = trails_layer("trail_and_trailhead");

    let way = layer
        .nearest_trail(LAT, LON, 0)
        .expect("query")
        .expect("a trail runs through the point");
    assert_eq!(way.kind, "trail");
    assert_eq!(way.name.as_deref(), Some("Greenway"));
    assert_eq!(way.class, "path");
    assert!(way.on_it);
    assert!(way.distance_m < 1.0);

    let head = layer
        .nearest_trailhead(LAT, LON, 0)
        .expect("query")
        .expect("the fixture has a trailhead");
    assert_eq!(head.kind, "trailhead");
    assert_eq!(head.name.as_deref(), Some("North Gate"));
    assert!((10.0..50.0).contains(&head.distance_m));
}

#[test]
fn trail_listing_carries_geometry_and_attributes() {
    let trails = trails_layer("trail_listing").trails(LAT, LON, 0).expect("trails query");
    assert_eq!(trails.len(), 2);
    assert_eq!(trails[0].surface, "compacted");
    assert_eq!(trails[0].sac_scale, "hiking");
    assert_eq!(trails[0].geometry.len(), 2);
    assert_eq!(trails[1].geom_type, 1, "the trailhead is a point");
    assert_eq!(trails[1].geometry.len(), 1);
}

#[test]
fn a_merged_block_is_sliced_to_the_queried_cell() {
    let layer = merged_parks_layer("merged_slice");

    let parks = layer.parks(LAT, LON, 0).expect("parks query");
    assert_eq!(parks.len(), 1, "only this cell's park: {parks:?}");
    assert_eq!(parks[0].name.as_deref(), Some("Here Park"));

    let at = layer.park_at(LAT, LON, 0).expect("query").expect("standing in it");
    assert!(at.inside);
    assert_eq!(at.distance_m, 0.0);
    assert_eq!(at.name.as_deref(), Some("Here Park"));

    // The other cell in the same block answers with its own park, not this
    // one -- the slice is per cell, not per block.
    let far = layer
        .park_at(35.05, -85.31, 0)
        .expect("query")
        .expect("its own park");
    assert_eq!(far.name.as_deref(), Some("Far Reserve"));
}

#[test]
fn stack_locate_prefers_the_closer_of_road_and_trail() {
    let stack = PtilesStack::with_layers(
        None,
        None,
        None,
        Some(trails_layer("stack_locate")),
        Some(merged_parks_layer("stack_locate")),
        None,
        None,
        None,
    );

    let got = stack.locate(LAT, LON, 0).expect("locate");
    let way = got.on_way.expect("the point sits on the trail");
    assert_eq!(way.kind, "trail", "no roads layer, so the trail wins");
    assert_eq!(way.name.as_deref(), Some("Greenway"));
    let park = got.park.expect("the parks layer answers too");
    assert!(park.inside);
    assert!(got.water.is_none(), "no water layer was supplied");
    assert!(got.address.is_none(), "no address layer was supplied");
}

#[test]
fn a_trails_file_refuses_park_and_rail_questions() {
    let layer = trails_layer("wrong_layer");
    assert!(matches!(
        layer.parks(LAT, LON, 0),
        Err(PtilesError::UnsupportedForLayer { .. })
    ));
    assert!(matches!(
        layer.nearest_rail(LAT, LON, 0),
        Err(PtilesError::UnsupportedForLayer { .. })
    ));
    assert!(matches!(
        layer.nearest_trail(LAT, LON, 2),
        Err(PtilesError::InvalidRing { ring: 2 })
    ));
}

#[test]
fn indoor_outdoor_keeps_missing_building_coverage_uncertain() {
    let stack = PtilesStack::new(None, None, None);
    let got = stack
        .indoor_outdoor(LAT, LON, 5.0)
        .expect("a missing optional layer is an uncertain answer, not an error");
    assert_eq!(got.state, IndoorOutdoorState::Uncertain);
    assert_eq!(got.reason, IndoorOutdoorReason::IncompleteCoverage);
    assert_eq!(got.building_osm_id, None);

    assert!(matches!(
        trails_layer("indoor_wrong_layer").indoor_outdoor(LAT, LON, 5.0),
        Err(PtilesError::UnsupportedForLayer { .. })
    ));
}

/// One camera ~22 m south of the query point, facing north with a 60-degree
/// cone -- pointed straight at you.
fn camera_layer(owner: &str) -> Arc<PtilesLayer> {
    let cell = cell_for_coord(LAT, LON);
    let records = camera_record(2, LAT - 0.0002, LON, 0, Some(0), Some(60), Some("At You"));
    open(write_fixture(
        owner,
        "XX.camera.ptiles",
        ptiles_v1(b"PTILESC", &[(cell, records)]),
    ))
}

/// A buildings layer is not something `fixtures` can build (the buildings
/// decoder wants a cell-relative framing of its own), so the occlusion half
/// is covered in `core::camera`'s own tests. This proves the wiring: the
/// camera reaches the FFI, and the stack answers with it.
#[test]
fn a_camera_aimed_at_you_is_reported_with_its_reasons() {
    let layer = camera_layer("camera_sees");

    let cameras = layer.cameras(LAT, LON, 0).expect("cameras query");
    assert_eq!(cameras.len(), 1);
    assert_eq!(cameras[0].camera_type, "fixed");
    assert_eq!(cameras[0].direction, Some(0));
    assert_eq!(cameras[0].angle, Some(60));

    let seen = layer.cameras_seeing(LAT, LON, 0, 50.0).expect("cameras_seeing");
    assert_eq!(seen.len(), 1);
    assert!(seen[0].sees);
    assert!(seen[0].aimed_at_you);
    assert!(!seen[0].aim_assumed, "direction and angle were both tagged");
    assert!(seen[0].line_of_sight, "no buildings were loaded, so nothing occludes");
    assert!(seen[0].distance_m > 15.0 && seen[0].distance_m < 30.0);
    assert!(seen[0].bearing_deg.abs() < 1.0, "due north, got {}", seen[0].bearing_deg);
}

#[test]
fn a_camera_out_of_range_is_not_reported() {
    let layer = camera_layer("camera_range");
    assert!(layer.cameras_seeing(LAT, LON, 0, 5.0).expect("query").is_empty());
}

#[test]
fn the_stack_answers_from_its_camera_layer_and_says_nothing_without_one() {
    let with_camera = PtilesStack::with_layers(
        None,
        None,
        None,
        None,
        None,
        None,
        Some(camera_layer("camera_stack")),
        None,
    );
    // range_m <= 0 means "use core's default", not "see nothing".
    let seen = with_camera.cameras_seeing(LAT, LON, 0, 0.0).expect("stack query");
    assert_eq!(seen.len(), 1);
    assert!(seen[0].sees);

    let without = PtilesStack::new(None, None, None);
    assert!(
        without.cameras_seeing(LAT, LON, 0, 0.0).expect("query").is_empty(),
        "no camera layer means no answer, not an error"
    );
}

#[test]
fn a_camera_file_refuses_a_trails_question() {
    let layer = camera_layer("camera_wrong_layer");
    assert!(matches!(
        layer.trails(LAT, LON, 0),
        Err(PtilesError::UnsupportedForLayer { .. })
    ));
}
