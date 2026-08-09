//! Trail and merged-block queries against `.ptiles` files this test builds
//! itself (`ptiles_core::fixtures`).
//!
//! `nature_query.rs` runs against the real corpus and skips what that corpus
//! lacks -- which is every trails file, and (for a host with no data at all)
//! everything. These run anywhere, and they pin the two behaviours a missing
//! file was hiding: that the trail lookups answer at all, and that a v2
//! merged block is sliced per cell rather than decoded whole.

use std::path::{Path, PathBuf};
use std::process::Command;

use ptiles_core::cell_for_coord;
use ptiles_core::fixtures::{camera_record, park_record, ptiles_v1, ptiles_v2_merged, trail_record};

const LAT: f64 = 36.16;
const LON: f64 = -86.78;

fn run_cli(args: &[&str]) -> serde_json::Value {
    let exe = env!("CARGO_BIN_EXE_ptiles-cli");
    let output = Command::new(exe).args(args).output().expect("spawn ptiles-cli");
    assert!(
        output.status.success(),
        "ptiles-cli exited with {:?}, stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "invalid JSON from ptiles-cli: {e}\nstdout: {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

/// Write a fixture into the per-test-binary temp dir cargo provides, under a
/// `<state>.<layer>.ptiles` name -- the CLI infers the layer from it. Each
/// test gets its own directory: they run in parallel and each builds its own
/// fixtures, so a shared path would have one truncating a file another reads.
fn write_fixture(owner: &str, name: &str, bytes: Vec<u8>) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("synthetic_query").join(owner);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join(name);
    std::fs::write(&path, bytes).expect("write fixture");
    path
}

/// A trails file holding one path running east through the query point and a
/// trailhead sitting on it. Both live in the cell the query resolves to.
fn trails_fixture(owner: &str) -> PathBuf {
    let cell = cell_for_coord(LAT, LON);
    let mut records = trail_record(
        2,
        0, // path
        5, // compacted
        1, // hiking
        &[(LAT, LON - 0.01), (LAT, LON + 0.01)],
        Some("Greenway"),
    );
    records.extend(trail_record(
        2,
        6, // trailhead
        0,
        0,
        &[(LAT + 0.0002, LON)],
        Some("North Gate"),
    ));
    write_fixture(owner, "XX.trails_v1.ptiles", ptiles_v1(b"PTILEST", &[(cell, records)]))
}

fn query(path: &Path, kind: &str) -> serde_json::Value {
    run_cli(&[
        "--path",
        path.to_str().unwrap(),
        "--lat",
        &LAT.to_string(),
        "--lon",
        &LON.to_string(),
        "--query",
        kind,
    ])
}

#[test]
fn trail_lookup_finds_the_path_under_the_point() {
    let path = trails_fixture("trail_lookup");
    let got = query(&path, "trail");
    let trail = got["nearest_trail"]
        .as_object()
        .expect("a trail runs through the query point");
    assert_eq!(trail["kind"], "trail");
    assert_eq!(trail["name"], "Greenway");
    assert_eq!(trail["class"], "path");
    assert!(trail["on_it"].as_bool().unwrap(), "the point is on the line");
    assert!(trail["distance_m"].as_f64().unwrap() < 1.0);
}

#[test]
fn trailhead_lookup_finds_the_point_the_trail_lookup_skips() {
    let path = trails_fixture("trailhead_lookup");
    let got = query(&path, "trailhead");
    let head = got["nearest_trailhead"]
        .as_object()
        .expect("the fixture has a trailhead");
    assert_eq!(head["kind"], "trailhead");
    assert_eq!(head["name"], "North Gate");
    // ~22 m north: close, and definitely not the linestring.
    assert!((10.0..50.0).contains(&head["distance_m"].as_f64().unwrap()));
}

#[test]
fn trail_listing_returns_both_geometries_with_their_attributes() {
    let path = trails_fixture("trail_listing");
    let got = query(&path, "trails");
    let trails = got["trails"].as_array().expect("trails array");
    assert_eq!(trails.len(), 2);
    assert_eq!(trails[0]["surface"], "compacted");
    assert_eq!(trails[0]["sac_scale"], "hiking");
    assert_eq!(trails[0]["geom_type"], 0);
    assert_eq!(trails[1]["geom_type"], 1, "the trailhead is a point");
    // `path` is a way you walk, not built infrastructure.
    assert_eq!(trails[0]["developed"], false);
}

#[test]
fn a_merged_block_is_sliced_to_the_queried_cell() {
    // Two cells packed into one physical block, each with its own park. The
    // query must see only its own cell's park -- decoding the block whole
    // would return both, plus junk records from the block header.
    let here = cell_for_coord(LAT, LON);
    let elsewhere = cell_for_coord(35.05, -85.31);
    assert_ne!(here, elsewhere);

    let ring = |lat: f64, lon: f64| {
        vec![
            (lat - 0.002, lon - 0.002),
            (lat - 0.002, lon + 0.002),
            (lat + 0.002, lon + 0.002),
            (lat + 0.002, lon - 0.002),
        ]
    };
    let block = vec![
        (here, park_record(2, "park", &ring(LAT, LON), Some("Here Park"))),
        (
            elsewhere,
            park_record(2, "nature_reserve", &ring(35.05, -85.31), Some("Far Reserve")),
        ),
    ];
    let path = write_fixture("merged_slice", "XX.parks.ptiles", ptiles_v2_merged(b"PTILESP", &[block]));

    let listing = query(&path, "parks");
    let parks = listing["parks"].as_array().expect("parks array");
    assert_eq!(parks.len(), 1, "only this cell's park: {parks:?}");
    assert_eq!(parks[0]["name"], "Here Park");

    let at = query(&path, "park");
    let park = at["park"].as_object().expect("standing inside it");
    assert_eq!(park["name"], "Here Park");
    assert!(park["inside"].as_bool().unwrap());
    assert_eq!(park["distance_m"].as_f64().unwrap(), 0.0);
}

/// A camera file with two cameras ~22 m south of the query point: one facing
/// north (at you) and one facing south (away), both tagged with a 60-degree
/// field of view.
fn cameras_fixture(owner: &str) -> PathBuf {
    let cell = cell_for_coord(LAT, LON);
    let mut records = camera_record(2, LAT - 0.0002, LON, 0, Some(0), Some(60), Some("At You"));
    records.extend(camera_record(
        2,
        LAT - 0.0002,
        LON + 0.00001,
        0,
        Some(180),
        Some(60),
        Some("Facing Away"),
    ));
    write_fixture(owner, "XX.camera.ptiles", ptiles_v1(b"PTILESC", &[(cell, records)]))
}

#[test]
fn the_camera_query_answers_who_can_see_you_and_why() {
    let path = cameras_fixture("camera_query");
    let got = query(&path, "camera");
    assert_eq!(got["candidate_count"], 2);
    assert_eq!(
        got["occlusion_checked"], false,
        "a camera file alone knows nothing about what stands in the way"
    );

    let seen: Vec<&serde_json::Value> = got["seen_by"].as_array().expect("seen_by array").iter().collect();
    assert_eq!(seen.len(), 2, "both are in range, whatever they are pointed at");

    let at_you = seen.iter().find(|v| v["name"] == "At You").expect("the one facing north");
    assert_eq!(at_you["sees"], true);
    assert_eq!(at_you["aimed_at_you"], true);
    assert_eq!(at_you["aim_assumed"], false, "direction and angle were both tagged");
    assert!(at_you["bearing_deg"].as_f64().unwrap().abs() < 1.0, "due north");

    let away = seen.iter().find(|v| v["name"] == "Facing Away").expect("the one facing south");
    assert_eq!(away["sees"], false);
    assert_eq!(away["aimed_at_you"], false);
    // Still reported, with the reason -- the caller decides what to show.
    assert_eq!(away["line_of_sight"], true);
}

#[test]
fn the_cameras_listing_returns_the_tags_as_stored() {
    let path = cameras_fixture("cameras_listing");
    let got = query(&path, "cameras");
    let cameras = got["cameras"].as_array().expect("cameras array");
    assert_eq!(cameras.len(), 2);
    assert_eq!(cameras[0]["device_type"], "camera");
    assert_eq!(cameras[0]["placement"], "public");
    assert_eq!(cameras[0]["camera_type"], "fixed");
    assert_eq!(cameras[0]["direction"], 0);
    assert_eq!(cameras[0]["angle"], 60);
    assert_eq!(cameras[0]["name"], "At You");
}
