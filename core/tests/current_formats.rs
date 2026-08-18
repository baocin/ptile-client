//! Every layer in a current build, opened and decoded by this crate.
//!
//! The unit tests encode their own records, so they prove the decoders are
//! self-consistent and nothing more. These read what the builders actually
//! wrote -- buildings v10, business v5, places/parks/rail/trails/ev v2, water
//! v2, address v4 -- because every format bug this project has had looked fine
//! against synthetic bytes and wrong against real ones.
//!
//! Skipped when the build tree is absent; the repo is cloned without it.

#![cfg(feature = "std")]


use std::path::PathBuf;

use ptiles_core::file::PtilesFile;
use ptiles_core::source::FileSource;

fn build_dir() -> PathBuf {
    PathBuf::from(
        std::env::var("PTILES_OUT")
            .unwrap_or_else(|_| String::from("/mnt/core/kino/ptiles/data/v5/states")),
    )
}

/// Highest-versioned file for a scope and layer, e.g. ("TN", "buildings").
fn layer_file(scope: &str, layer: &str) -> Option<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(build_dir())
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
                return false;
            };
            let Some(rest) = name.strip_prefix(&format!("{scope}.")) else {
                return false;
            };
            let Some(stem) = rest.strip_suffix(".ptiles") else {
                return false;
            };
            // `business` must not match `business_name_index`.
            stem == layer || stem.strip_prefix(layer).is_some_and(|s| s.starts_with("_v"))
        })
        .collect();
    found.sort();
    found.pop()
}

macro_rules! skip_unless {
    ($opt:expr, $what:expr) => {
        match $opt {
            Some(v) => v,
            None => {
                eprintln!("skipping: no {} in the build tree", $what);
                return;
            }
        }
    };
}

/// AddressSource has no Ord, so collect distinct sources by their Debug form.
fn alloc_fmt(source: &ptiles_core::address::AddressSource) -> String {
    format!("{source:?}")
}

fn open(path: PathBuf) -> PtilesFile<FileSource> {
    PtilesFile::open(FileSource::open(&path).expect("open")).unwrap_or_else(|e| {
        panic!("{} did not open: {e}", path.display());
    })
}

/// Decode every block of a file and hand the records to `f`.
fn for_each_block<F: FnMut(&[u8])>(file: &PtilesFile<FileSource>, limit: usize, mut f: F) {
    for entry in file.index().iter().take(limit) {
        if let Ok(Some(block)) = file.read_block(entry.h3_cell) {
            f(&block);
        }
    }
}

#[test]
fn buildings_v10_carry_alternative_names() {
    let path = skip_unless!(layer_file("JP-KANTO", "buildings"), "JP-KANTO buildings");
    let file = open(path);
    assert!(
        file.header().version >= 10,
        "expected a v10 build, got v{}",
        file.header().version
    );

    let mut decoded = 0usize;
    let mut with_name_en = 0usize;
    let mut with_business_tag = 0usize;
    for entry in file.index().iter().take(40) {
        let Ok(Some(block)) = file.read_cell(entry.h3_cell) else {
            continue;
        };
        let (lat, lon) = ptiles_core::query::cell_center(entry.h3_cell);
        let buildings =
            ptiles_core::buildings::decode_buildings(&block, lat, lon).expect("decode block");
        decoded += buildings.len();
        with_name_en += buildings.iter().filter(|b| b.name_en.is_some()).count();
        with_business_tag += buildings.iter().filter(|b| b.business_tag.is_some()).count();
    }
    assert!(decoded > 0, "no buildings decoded from a v10 file");
    // Measured at 11.8% of *named* buildings on Shikoku; over 40 blocks of
    // Kanto a handful is enough to prove the flags3 walk lands on real strings
    // rather than on the middle of another field.
    assert!(
        with_name_en > 0,
        "{decoded} buildings decoded and not one name:en -- flags3 is not being read"
    );
    eprintln!("kanto sample: {decoded} buildings, {with_name_en} name:en, {with_business_tag} business_tag");
}

#[test]
fn business_v5_carries_name_en_and_chain_count() {
    let path = skip_unless!(layer_file("TN", "business"), "TN business");
    let file = open(path);
    assert!(file.header().version >= 5, "expected a v5 build");

    let mut total = 0usize;
    let mut chains = 0usize;
    for entry in file.index().iter().take(40) {
        let Ok(Some(block)) = file.read_cell(entry.h3_cell) else {
            continue;
        };
        let records = ptiles_core::business::decode_business_versioned(
            &block,
            file.header().version,
            entry.h3_cell,
        )
        .expect("decode business block");
        total += records.len();
        chains += records.iter().filter(|r| r.chain_count.is_some()).count();
        for r in &records {
            assert!(r.lat.abs() <= 90.0 && r.lon.abs() <= 180.0, "sane position");
        }
    }
    assert!(total > 0, "no businesses decoded from a v5 file");
    assert!(
        chains > 0,
        "{total} businesses and no chain_count -- 0x80 is being skipped, not read"
    );
}

#[test]
fn places_v2_decode_with_english_names() {
    let path = skip_unless!(layer_file("JP", "places"), "JP places");
    let file = open(path);
    assert!(file.header().version >= 2);

    let mut names = 0usize;
    let mut english = 0usize;
    for entry in file.index().iter().take(60) {
        let Ok(Some(block)) = file.read_cell(entry.h3_cell) else {
            continue;
        };
        let places = ptiles_core::places::decode_places(&block).expect("decode places");
        names += places.len();
        english += places.iter().filter(|p| p.name_en.is_some()).count();
    }
    assert!(names > 0, "no places decoded");
    // 75.5% of named Japanese places carry name:en -- if none do, the flag
    // walk is wrong, not the data.
    assert!(english > 0, "{names} places and no name:en");
}

#[test]
fn the_field_walking_layers_decode_a_whole_block_each() {
    // parks, rail, trails and ev have no per-record length prefix, so a
    // decoder that mis-reads one field does not fail -- it walks off into the
    // next record and returns a shorter list. Decoding a full block and
    // getting a plausible count is the check that catches that.
    for (layer, min_expected) in [("parks", 1usize), ("rail", 1), ("trails", 1), ("ev", 1)] {
        let Some(path) = layer_file("TN", layer) else {
            eprintln!("skipping {layer}: not in the build tree");
            continue;
        };
        let file = open(path);
        assert!(
            file.header().version >= 2,
            "{layer} should be v2 in a current build"
        );
        let mut count = 0usize;
        for entry in file.index().iter().take(20) {
            let Ok(Some(block)) = file.read_cell(entry.h3_cell) else {
                continue;
            };
            count += match layer {
                "parks" => ptiles_core::parks::decode_parks(&block).unwrap().len(),
                "rail" => ptiles_core::rail::decode_rail(&block).unwrap().len(),
                "trails" => ptiles_core::trails::decode_trails(&block).unwrap().len(),
                "ev" => ptiles_core::ev::decode_chargers(&block).unwrap().len(),
                _ => unreachable!(),
            };
        }
        assert!(count >= min_expected, "{layer}: decoded {count} records");
        eprintln!("{layer}: {count} records over 20 blocks");
    }
}

#[test]
fn address_v4_carries_units_and_provenance() {
    let path = skip_unless!(layer_file("TN", "address"), "TN address");
    let file = open(path);
    assert!(file.header().version >= 4, "expected a v4 build");

    let mut total = 0usize;
    let mut with_unit = 0usize;
    let mut sources: Vec<String> = Vec::new();
    for entry in file.index().iter().take(40) {
        let Ok(Some(block)) = file.read_cell(entry.h3_cell) else {
            continue;
        };
        let (lat, lon) = ptiles_core::query::cell_center(entry.h3_cell);
        let centre = ((lon * 100_000.0) as i32, (lat * 100_000.0) as i32);
        let records = ptiles_core::address::decode_address_cell(
            &block,
            Some(centre),
            file.header().version,
        )
        .expect("decode address block");
        total += records.len();
        with_unit += records.iter().filter(|r| !r.unit.is_empty()).count();
        for r in &records {
            let s = alloc_fmt(&r.source);
            if !sources.contains(&s) {
                sources.push(s);
            }
        }
    }
    assert!(total > 0, "no addresses decoded from a v4 file");
    assert!(
        with_unit > 0,
        "{total} addresses and no units -- v4's unit field is not being read"
    );
    assert!(
        sources.len() > 1,
        "a merged v4 file should carry more than one source, saw {sources:?}"
    );
}

#[test]
fn every_file_in_the_build_opens() {
    let dir = build_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        eprintln!("skipping: no build tree at {}", dir.display());
        return;
    };
    let mut opened = 0usize;
    let mut refused: Vec<String> = Vec::new();
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if !name.ends_with(".ptiles") {
            continue;
        }
        // Roads are the PTLR container, not a PTiles file; ptlr_real_files.rs
        // covers them.
        if name.contains(".roads_v") {
            continue;
        }
        match FileSource::open(&path).map_err(|e| e.to_string()).and_then(|s| {
            PtilesFile::open(s).map(|_| ()).map_err(|e| e.to_string())
        }) {
            Ok(()) => opened += 1,
            Err(e) => refused.push(format!("{name}: {e}")),
        }
    }
    assert!(
        refused.is_empty(),
        "{} of {} files refused by this build: {:#?}",
        refused.len(),
        opened + refused.len(),
        refused
    );
    eprintln!("{opened} files opened");
}
