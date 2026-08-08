//! Integration test for the trail/park/water/rail queries: spawns the
//! `ptiles-cli` binary one-shot against the real TN fixtures and checks the
//! JSON shape of both halves of each layer's pair -- the singular lookup
//! ("which one am I in/on") and the plural listing.
//!
//! Skips (with an eprintln) per file, same as `roads_query.rs`: this repo
//! ships no `.ptiles` fixtures, and not every corpus has every layer (there
//! is no trails file in the local corpus at the time of writing).

use std::path::Path;
use std::process::Command;

const DATA_DIR: &str = "/home/aoi/kino/data/ptiles";
const LAT: &str = "36.16";
const LON: &str = "-86.78";

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

/// `Some(path)` when the layer file exists, else `None` after printing why
/// the test is skipping.
fn layer(file: &str) -> Option<String> {
    let path = format!("{DATA_DIR}/{file}");
    if Path::new(&path).exists() {
        Some(path)
    } else {
        eprintln!("skipping: {path} not present");
        None
    }
}

fn query(path: &str, kind: &str) -> serde_json::Value {
    run_cli(&["--path", path, "--lat", LAT, "--lon", LON, "--query", kind, "--ring", "1"])
}

/// A decoded class string must look like the format's own vocabulary
/// (`park`, `lake`, `station`) -- lowercase ASCII words.
///
/// This is the check that catches a mis-sliced block: parks and rail ship
/// merged blocks, and decoding one whole produces records that *exist* and
/// carry binary noise in their string fields, so "the array is non-empty"
/// passes while the answer is nonsense.
fn assert_vocabulary(class: &str, what: &str) {
    assert!(
        !class.is_empty()
            && class
                .chars()
                .all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit()),
        "{what} class is not vocabulary: {class:?}"
    );
}

/// Tennessee is roughly 800 km wide; anything past that came from a decoder
/// reading coordinates out of the wrong bytes.
fn assert_plausible_distance(d: f64, what: &str) {
    assert!(
        (0.0..800_000.0).contains(&d),
        "{what} distance is not plausible: {d} m"
    );
}

#[test]
fn parks_lookup_and_listing() {
    let Some(path) = layer("TN.parks.ptiles") else {
        return;
    };

    let listing = query(&path, "parks");
    let parks = listing
        .get("parks")
        .and_then(|v| v.as_array())
        .expect("response missing \"parks\" array");
    assert!(!parks.is_empty(), "ring 1 around downtown Nashville has parks");
    for p in parks {
        assert_vocabulary(p["park_type"].as_str().expect("park_type string"), "park");
        let ring = p["geometry"].as_array().expect("park missing geometry");
        assert!(ring.len() >= 3, "a park polygon needs three vertices: {p}");
    }

    let at = query(&path, "park");
    let park = at.get("park").expect("response missing \"park\" key");
    // With parks in range there is always a nearest one, and containment and
    // a zero boundary distance must agree.
    let park = park.as_object().expect("a park in range");
    assert_eq!(park["kind"], "park");
    assert_vocabulary(park["class"].as_str().unwrap(), "nearest park");
    assert_plausible_distance(park["distance_m"].as_f64().unwrap(), "nearest park");
    assert_eq!(park["inside"].as_bool().unwrap(), park["distance_m"].as_f64().unwrap() == 0.0);
}

#[test]
fn water_lookup_and_listing() {
    let Some(path) = layer("TN.water.ptiles") else {
        return;
    };

    let listing = query(&path, "waters");
    let features = listing
        .get("water_features")
        .and_then(|v| v.as_array())
        .expect("response missing \"water_features\" array");
    assert!(!features.is_empty(), "the Cumberland runs through downtown");

    let at = query(&path, "water");
    assert!(at.get("water").is_some(), "response missing \"water\" key (null ok)");
    if let Some(w) = at["water"].as_object() {
        assert_eq!(w["kind"], "water");
        assert_vocabulary(w["class"].as_str().unwrap(), "nearest water");
        assert_plausible_distance(w["distance_m"].as_f64().unwrap(), "nearest water");
    }
}

#[test]
fn rail_track_and_station_are_different_answers() {
    let Some(path) = layer("TN.rail.ptiles") else {
        return;
    };

    let listing = query(&path, "rails");
    for r in listing["rail"].as_array().expect("response missing \"rail\" array") {
        assert_vocabulary(r["rail_type"].as_str().expect("rail_type string"), "rail");
    }

    let track = query(&path, "rail");
    assert!(track.get("nearest_rail").is_some(), "missing \"nearest_rail\" (null ok)");
    let station = query(&path, "station");
    assert!(station.get("nearest_station").is_some(), "missing \"nearest_station\" (null ok)");

    // Whatever the cell holds, the two never answer with each other's shape.
    if let Some(t) = track["nearest_rail"].as_object() {
        assert_eq!(t["kind"], "rail");
        assert!(t.get("snapped").is_some(), "a way answer snaps to a centreline");
    }
    if let Some(s) = station["nearest_station"].as_object() {
        assert_eq!(s["kind"], "station");
        assert!(s.get("lat").is_some(), "a point answer carries its position");
        assert_vocabulary(s["class"].as_str().unwrap(), "nearest station");
        assert_plausible_distance(s["distance_m"].as_f64().unwrap(), "nearest station");
    }
}

#[test]
fn trails_lookup_and_listing() {
    let Some(path) = layer("TN.trails_v1.ptiles") else {
        return;
    };

    let listing = query(&path, "trails");
    assert!(listing.get("trails").is_some(), "response missing \"trails\" array");

    let on = query(&path, "trail");
    assert!(on.get("nearest_trail").is_some(), "missing \"nearest_trail\" (null ok)");
    if let Some(t) = on["nearest_trail"].as_object() {
        assert_eq!(t["kind"], "trail");
    }

    let head = query(&path, "trailhead");
    assert!(head.get("nearest_trailhead").is_some(), "missing \"nearest_trailhead\"");
    if let Some(h) = head["nearest_trailhead"].as_object() {
        assert_eq!(h["kind"], "trailhead");
    }
}
