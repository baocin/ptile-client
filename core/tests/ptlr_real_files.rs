//! PTLR against the files the builders actually produce.
//!
//! The unit tests in `core::ptlr` work from records this crate encodes itself,
//! which proves the decoder is self-consistent and nothing else. These open
//! real published-shape files -- one US state, one Japanese region -- and
//! check the two things a synthetic record cannot: that the container's zstd
//! dictionaries split where the header says, and that the ids coming back are
//! real OSM way ids rather than the deltas they are stored as.
//!
//! Skipped, not failed, when the build tree is not present: this repo is
//! cloned without 40 GB of map data.

#![cfg(feature = "std")]

use std::path::{Path, PathBuf};

use ptiles_core::ptlr::{Band, PtlrFile};
use ptiles_core::source::FileSource;

fn build_dir() -> PathBuf {
    PathBuf::from(
        std::env::var("PTILES_OUT")
            .unwrap_or_else(|_| String::from("/mnt/core/kino/ptiles/data/v5/states")),
    )
}

/// Highest-versioned roads file for a scope, or None when the tree is absent.
fn roads_file(scope: &str) -> Option<PathBuf> {
    let dir = build_dir();
    let mut found: Vec<PathBuf> = std::fs::read_dir(&dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(&format!("{scope}.roads_v")) && n.ends_with(".ptiles"))
        })
        .collect();
    found.sort();
    found.pop()
}

fn open(path: &Path) -> PtlrFile<FileSource> {
    PtlrFile::open(FileSource::open(path).expect("open roads file")).expect("parse PTLR header")
}

#[test]
fn tennessee_roads_decode_with_real_way_ids() {
    let Some(path) = roads_file("TN") else {
        eprintln!("skipping: no TN roads file in the build tree");
        return;
    };
    let mut file = open(&path);
    assert!(file.header().version >= 2, "v1 has no dictionary lengths");
    assert!(file.header().road_count > 100_000, "a state's worth of roads");

    // Downtown Nashville.
    let roads = file
        .roads_near(36.1627, -86.7816, 2, Band::Z05)
        .expect("query");
    assert!(!roads.is_empty(), "downtown Nashville should hold roads");

    let ids: Vec<u64> = roads.iter().map(|r| r.osm_id).collect();
    assert!(
        ids.iter().max().copied().unwrap_or(0) > 1_000_000,
        "ids look like deltas, not OSM way ids: {:?}",
        &ids[..ids.len().min(5)]
    );

    // Every class, not the motorway subset the old container held.
    let classes: std::collections::BTreeSet<&str> =
        roads.iter().map(|r| r.road_class.as_str()).collect();
    assert!(
        classes.iter().any(|c| *c == "residential" || *c == "service" || *c == "footway"),
        "PTLR should carry minor roads too, saw {classes:?}"
    );

    // Geometry lands where Nashville is, not on Null Island.
    let first = roads[0].coords[0];
    assert!((first[0] - -86.78).abs() < 1.0 && (first[1] - 36.16).abs() < 1.0);
}

#[test]
fn a_region_file_knows_which_points_it_owns() {
    let Some(path) = roads_file("TN") else {
        eprintln!("skipping: no TN roads file in the build tree");
        return;
    };
    let mut file = open(&path);
    assert!(file.covers(36.1627, -86.7816).expect("covers"), "Nashville");
    assert!(
        !file.covers(40.7831, -73.9712).expect("covers"),
        "Manhattan is not in Tennessee's file, bbox overlap or not"
    );
    // Bristol straddles the Virginia line; the polygon is stored with an
    // outward bias precisely so a border town is never owned by nobody.
    assert!(file.covers(36.5951, -82.1887).expect("covers"), "Bristol TN");
}

#[test]
fn tokyo_roads_decode_from_the_kanto_file() {
    let Some(path) = roads_file("JP-KANTO") else {
        eprintln!("skipping: no JP-KANTO roads file in the build tree");
        return;
    };
    let mut file = open(&path);
    let roads = file
        .roads_near(35.6812, 139.7671, 2, Band::Z05)
        .expect("query");
    assert!(!roads.is_empty(), "Tokyo Station should hold roads");
    assert!(
        roads.iter().any(|r| r.name.is_some()),
        "some Tokyo roads are named"
    );
    assert!(
        roads.iter().map(|r| r.osm_id).max().unwrap_or(0) > 1_000_000,
        "absolute ids, not deltas"
    );
}

#[test]
fn the_highway_band_holds_fewer_roads_than_the_detailed_one() {
    let Some(path) = roads_file("TN") else {
        eprintln!("skipping: no TN roads file in the build tree");
        return;
    };
    let file = open(&path);
    let z04 = file.header().band_count(Band::Z04);
    let z07 = file.header().band_count(Band::Z07);
    // Z04 is filtered to motorway/trunk/primary. If these are equal, the band
    // filter is not running -- which is how the container's own road_count
    // came to undercount the file by 95% before it was fixed.
    assert!(z04 < z07, "z04 {z04} should be a subset of z07 {z07}");
    assert_eq!(
        file.header().road_count,
        z07,
        "road_count is the z07 count, not the filtered band's"
    );
}
