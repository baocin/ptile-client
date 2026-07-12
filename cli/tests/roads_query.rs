//! Integration test for the plan-addendum `roads` query (item 1): spawns the
//! `ptiles-cli` binary one-shot against the real TN roads fixture and checks
//! the JSON shape plus ring-0/ring-1 candidate-count monotonicity.
//!
//! Skips (with an eprintln) if the data file isn't present -- this repo's
//! tests don't ship the ~2GB of `.ptiles` fixtures, so CI/dev boxes without
//! them still pass the suite.

use std::path::Path;
use std::process::Command;

const DATA_FILE: &str = "/home/aoi/kino/data/ptiles/TN.roads.ptiles";

fn run_cli(args: &[&str]) -> serde_json::Value {
    let exe = env!("CARGO_BIN_EXE_ptiles-cli");
    let output = Command::new(exe)
        .args(args)
        .output()
        .expect("failed to spawn ptiles-cli");
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

#[test]
fn roads_query_shape_and_ring_monotonicity() {
    if !Path::new(DATA_FILE).exists() {
        eprintln!("skipping roads_query_shape_and_ring_monotonicity: {DATA_FILE} not present");
        return;
    }

    // Nashville-area point (see core/src/query.rs's own NASHVILLE_LAT/LON
    // test fixture for the same coordinates).
    let lat = "36.16";
    let lon = "-86.78";

    let ring0 = run_cli(&[
        "--path", DATA_FILE, "--lat", lat, "--lon", lon, "--query", "roads",
    ]);
    let roads0 = ring0
        .get("roads")
        .and_then(|v| v.as_array())
        .expect("response missing \"roads\" array");
    assert!(!roads0.is_empty(), "expected at least one road segment in the center cell");

    for segment in roads0 {
        assert!(segment.get("osm_id").is_some(), "segment missing osm_id: {segment}");
        assert!(segment.get("name").is_some(), "segment missing name (null ok): {segment}");
        assert!(segment.get("road_class").is_some(), "segment missing road_class: {segment}");
        let geometry = segment
            .get("geometry")
            .and_then(|v| v.as_array())
            .expect("segment missing geometry array");
        assert!(!geometry.is_empty(), "segment geometry is empty: {segment}");
        for point in geometry {
            let pair = point.as_array().expect("geometry point is not an array");
            assert_eq!(pair.len(), 2, "geometry point is not [lat, lon]: {point}");
        }
    }

    let count0 = ring0.get("candidate_count").and_then(|v| v.as_u64()).unwrap();

    let ring1 = run_cli(&[
        "--path", DATA_FILE, "--lat", lat, "--lon", lon, "--query", "roads", "--ring", "1",
    ]);
    let count1 = ring1.get("candidate_count").and_then(|v| v.as_u64()).unwrap();

    assert!(
        count1 >= count0,
        "ring-1 candidate count ({count1}) should be >= ring-0 ({count0})"
    );
}

#[test]
fn ring_greater_than_one_is_rejected() {
    if !Path::new(DATA_FILE).exists() {
        eprintln!("skipping ring_greater_than_one_is_rejected: {DATA_FILE} not present");
        return;
    }

    let exe = env!("CARGO_BIN_EXE_ptiles-cli");
    let output = Command::new(exe)
        .args([
            "--path", DATA_FILE, "--lat", "36.16", "--lon", "-86.78", "--query", "roads",
            "--ring", "2",
        ])
        .output()
        .expect("failed to spawn ptiles-cli");

    assert!(!output.status.success(), "expected non-zero exit for ring 2");
    let v: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|e| panic!("invalid JSON: {e}, stdout: {}", String::from_utf8_lossy(&output.stdout)));
    assert!(v.get("error").is_some(), "expected an \"error\" field, got {v}");
}

fn run_raw(args: &[&str]) -> std::process::Output {
    let exe = env!("CARGO_BIN_EXE_ptiles-cli");
    Command::new(exe).args(args).output().expect("failed to spawn ptiles-cli")
}

#[test]
fn missing_path_exits_nonzero() {
    // One-shot mode requires --path; omitting it must fail cleanly (exit 2),
    // not panic.
    let out = run_raw(&["--lat", "36.16", "--lon", "-86.78"]);
    assert!(!out.status.success(), "missing --path must exit non-zero");
}

#[test]
fn unknown_query_exits_nonzero() {
    let out = run_raw(&[
        "--path", "/data/TN.roads.ptiles", "--lat", "36.16", "--lon", "-86.78",
        "--query", "bogus",
    ]);
    assert!(!out.status.success(), "unknown --query must exit non-zero");
}

#[test]
fn unknown_layer_filename_exits_nonzero() {
    let out = run_raw(&[
        "--path", "/data/TN.water.ptiles", "--lat", "36.16", "--lon", "-86.78",
    ]);
    assert!(!out.status.success(), "un-inferable layer filename must exit non-zero");
}

#[test]
fn supported_formats_prints_and_exits_zero() {
    // No data file involved; --supported-formats is handled before any file
    // is required and must print something on stdout.
    let out = run_raw(&["--supported-formats"]);
    assert!(out.status.success(), "--supported-formats must exit zero");
    assert!(!out.stdout.is_empty(), "--supported-formats must print to stdout");
}

#[test]
fn cells_bounds_query_shape() {
    // Pure H3 geometry, no .ptiles file needed: --query cells --bounds ...
    // must emit a JSON object with a non-empty "cells" array.
    let out = run_raw(&[
        "--query", "cells", "--bounds", "36.10,-86.82,36.20,-86.74",
    ]);
    assert!(
        out.status.success(),
        "cells query failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    let cells = v.get("cells").and_then(|c| c.as_array()).expect("cells array");
    assert!(!cells.is_empty(), "expected at least one cell for a Nashville viewport");
}

#[test]
fn cells_query_bad_bounds_exits_nonzero() {
    let out = run_raw(&["--query", "cells", "--bounds", "1,2,3"]);
    assert!(!out.status.success(), "3-value --bounds must exit non-zero");
}

#[test]
fn intersection_query_shape() {
    if !Path::new(DATA_FILE).exists() {
        eprintln!("skipping intersection_query_shape: {DATA_FILE} not present");
        return;
    }

    // Query exactly at the golden fixture's first intersection (a signalized
    // junction in downtown Nashville): it must be found at ~0 m, type 1.
    let v = run_cli(&[
        "--path", DATA_FILE, "--lat", "36.16076", "--lon", "-86.79367",
        "--query", "intersection",
    ]);
    assert!(v.get("candidate_count").and_then(|c| c.as_u64()).is_some());
    let ni = v
        .get("nearest_intersection")
        .expect("missing nearest_intersection field");
    assert!(!ni.is_null(), "expected an intersection at the golden point, got null");
    for field in ["lat", "lon", "distance_m", "intersection_type"] {
        assert!(ni.get(field).is_some(), "nearest_intersection missing {field:?}: {ni}");
    }
    assert!(ni["distance_m"].as_f64().unwrap() < 1.0, "expected ~0m: {ni}");
    assert_eq!(ni["intersection_type"].as_u64().unwrap(), 1);
}

#[test]
fn intersection_query_far_point_is_null() {
    if !Path::new(DATA_FILE).exists() {
        eprintln!("skipping intersection_query_far_point_is_null: {DATA_FILE} not present");
        return;
    }
    // A point in the Cumberland River: query succeeds, nearest is null.
    let v = run_cli(&[
        "--path", DATA_FILE, "--lat", "36.16600", "--lon", "-86.77300",
        "--query", "intersection",
    ]);
    assert!(
        v.get("nearest_intersection").expect("field present").is_null(),
        "expected null for a river point: {v}"
    );
}

#[test]
fn nearest_road_query_shape() {
    if !Path::new(DATA_FILE).exists() {
        eprintln!("skipping nearest_road_query_shape: {DATA_FILE} not present");
        return;
    }

    let v = run_cli(&[
        "--path", DATA_FILE, "--lat", "36.16", "--lon", "-86.78", "--query", "road",
    ]);
    let nr = v.get("nearest_road").expect("missing nearest_road field");
    if nr.is_null() {
        // No segment within threshold of this exact point is acceptable;
        // the shape assertion below is what matters when one is found.
        return;
    }
    for field in ["osm_id", "name", "road_class", "snapped", "distance_m", "geometry"] {
        assert!(nr.get(field).is_some(), "nearest_road missing field {field:?}: {nr}");
    }
}
