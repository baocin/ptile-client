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
use ptiles_core::fixtures::{park_record, ptiles_v1, ptiles_v2_merged, trail_record};
use ptiles_ffi::{PtilesError, PtilesLayer, PtilesStack};

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
