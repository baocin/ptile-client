//! Every published state address file, read the way a caller reads it.
//!
//! The v3 layer went from 34M records to 143M across 51 files, and until this
//! existed exactly one of those files had ever been opened by the client. The
//! failure this guards is the one this format keeps producing: a decode that
//! returns plausible records rather than an error. A wrong centre put every
//! record kilometres from its street with the street name still correct (see
//! `merged_block_cell_slice`), and nothing in the suite noticed.
//!
//! So the assertion that matters is geometric, not a count: a record must fall
//! inside the H3 cell that indexes it. That is false for any centre mistake,
//! any offset-sign flip, and any block/cell mismatch, and it cannot be
//! satisfied by a stub that returns empty vectors -- those fail the
//! non-emptiness check first.
//!
//! Both directions are then round-tripped per state:
//!   reverse  a record's own coordinates must find that record again
//!   forward  its number and street must find it back at the same point
//!
//! Skipped with a message when the data directory is absent, so CI stays green
//! without 1 GB of state files. Set `PTILES_ADDRESS_FULL=1` to check every
//! record in every file rather than a sample of cells.

use std::path::PathBuf;

use ptiles_core::address::{AddressFile, AddressSource};
use ptiles_core::{FileSource, cell_for_coord};

const STATES: [&str; 51] = [
    "AL", "AK", "AZ", "AR", "CA", "CO", "CT", "DE", "DC", "FL", "GA", "HI", "ID", "IL", "IN", "IA",
    "KS", "KY", "LA", "ME", "MD", "MA", "MI", "MN", "MS", "MO", "MT", "NE", "NV", "NH", "NJ", "NM",
    "NY", "NC", "ND", "OH", "OK", "OR", "PA", "RI", "SC", "SD", "TN", "TX", "UT", "VT", "VA", "WA",
    "WV", "WI", "WY",
];

/// Cells sampled per state unless `PTILES_ADDRESS_FULL` is set. Spread across
/// the index rather than taken from the front, so a file that is correct only
/// where the builder started cannot pass.
const SAMPLE_CELLS: usize = 40;

fn states_dir() -> PathBuf {
    PathBuf::from(
        std::env::var("PTILES_STATES_DIR")
            .unwrap_or_else(|_| "/mnt/core/kino/ptiles/data/v4/states".to_string()),
    )
}

struct StateReport {
    abbr: &'static str,
    cells: usize,
    records: usize,
    outside_cell: usize,
    missing_position: usize,
    empty_field: usize,
    /// Furthest any record sits from the centre of the cell indexing it.
    worst_offset_m: f64,
    sources: [usize; 4],
}

fn check_state(abbr: &'static str, path: PathBuf, full: bool) -> StateReport {
    let source = FileSource::open(&path).unwrap_or_else(|e| panic!("{abbr}: open {path:?}: {e}"));
    let file = AddressFile::open(source).unwrap_or_else(|e| panic!("{abbr}: parse: {e}"));
    let index = file.index().to_vec();
    assert!(!index.is_empty(), "{abbr}: empty index");

    let step = if full || index.len() <= SAMPLE_CELLS {
        1
    } else {
        index.len() / SAMPLE_CELLS
    };

    let mut rep = StateReport {
        abbr,
        cells: 0,
        records: 0,
        outside_cell: 0,
        missing_position: 0,
        empty_field: 0,
        worst_offset_m: 0.0,
        sources: [0; 4],
    };
    let mut probe: Option<(f64, f64, String, String)> = None;

    for entry in index.iter().step_by(step) {
        if entry.block_length == 0 {
            continue;
        }
        let records = file
            .addresses_in_cell(entry.h3_cell)
            .unwrap_or_else(|e| panic!("{abbr}: decode cell {:#x}: {e}", entry.h3_cell));
        assert!(
            !records.is_empty(),
            "{abbr}: cell {:#x} indexes {} features but decoded none",
            entry.h3_cell,
            entry.feature_count
        );
        rep.cells += 1;
        for r in &records {
            rep.records += 1;
            if r.housenumber.trim().is_empty() || r.street.trim().is_empty() {
                rep.empty_field += 1;
            }
            rep.sources[match r.source {
                AddressSource::Osm => 0,
                AddressSource::Nad => 1,
                AddressSource::OpenAddresses => 2,
                AddressSource::Unknown(_) => 3,
            }] += 1;
            match (r.lat, r.lon) {
                (Some(lat), Some(lon)) => {
                    if cell_for_coord(lat, lon) != entry.h3_cell {
                        rep.outside_cell += 1;
                    }
                    let (clat, clon) = ptiles_core::cell_center(entry.h3_cell);
                    let d = ptiles_core::haversine_distance_m(lat, lon, clat, clon);
                    if d > rep.worst_offset_m {
                        rep.worst_offset_m = d;
                    }
                    // Probe on a record that is unambiguously inside its own
                    // cell. A boundary record -- the h3 vs h3o disagreement
                    // above -- would make the round-trip below a test of which
                    // library is asked, not of this reader.
                    if probe.is_none() && cell_for_coord(lat, lon) == entry.h3_cell {
                        probe = Some((lat, lon, r.housenumber.clone(), r.street.clone()));
                    }
                }
                _ => rep.missing_position += 1,
            }
        }
    }

    assert!(rep.records > 0, "{abbr}: no records decoded");
    // Not zero-tolerance, and deliberately so. The builder assigns cells with
    // Python's h3 and the client resolves them with h3o, and the two disagree
    // about which side of a boundary a point falls on: Alabama's "120 Beech
    // Tree Lane" sits 1,264 m from its indexing cell's centre, right on the
    // edge, and each library answers differently. That is 1 record in 3,093
    // sampled -- noise, not a defect.
    //
    // A ratio still catches what matters. The block-centre bug this file was
    // written for displaced seven cells in every eight, so it reads as 87%
    // outside, not 0.03%. The distance bound catches the other shape of the
    // same mistake: a wrong centre that happens to land records in a
    // neighbouring cell rather than a distant one.
    let outside_ratio = rep.outside_cell as f64 / rep.records as f64;
    assert!(
        outside_ratio < 0.005,
        "{abbr}: {} of {} records ({:.2}%) fall outside the cell that indexes them",
        rep.outside_cell,
        rep.records,
        outside_ratio * 100.0
    );
    assert!(
        rep.worst_offset_m < 3_000.0,
        "{abbr}: a record sits {:.0} m from the centre of the cell indexing it \
         (a res-7 cell spans about 2.4 km, so this is a positioning error)",
        rep.worst_offset_m
    );
    assert_eq!(
        rep.missing_position, 0,
        "{abbr}: {} records have no position in a v3 file",
        rep.missing_position
    );

    // Round-trip both directions against a real record from this file.
    let (lat, lon, number, street) = probe.expect("a positioned record");

    // ring 1, as every real caller uses: a point near a cell edge has its
    // neighbours' addresses closer than its own cell's far side, and the demo
    // fetches the ring for exactly that reason.
    let back = file
        .addresses_at(lat, lon, 1)
        .unwrap_or_else(|e| panic!("{abbr}: reverse: {e}"));
    assert!(
        back.iter()
            .any(|r| r.housenumber == number && r.street == street),
        "{abbr}: reverse lookup at {lat},{lon} lost {number} {street}"
    );

    let found = file
        .search_address(&number, &street, Some((lat, lon)), 5)
        .unwrap_or_else(|e| panic!("{abbr}: forward: {e}"));
    let near = found.iter().any(|r| match (r.lat, r.lon) {
        (Some(rlat), Some(rlon)) => {
            (rlat - lat).abs() < 1e-4 && (rlon - lon).abs() < 1e-4 && r.housenumber == number
        }
        _ => false,
    });
    assert!(
        near,
        "{abbr}: forward search for {number} {street} did not return it at {lat},{lon}"
    );

    rep
}

#[test]
fn every_state_address_file_decodes_and_round_trips() {
    let dir = states_dir();
    if !dir.is_dir() {
        eprintln!("skipping: {dir:?} not present (set PTILES_STATES_DIR)");
        return;
    }
    let full = std::env::var("PTILES_ADDRESS_FULL").is_ok();

    let mut checked = 0;
    let mut missing = Vec::new();
    let (mut records, mut cells) = (0usize, 0usize);
    let mut sources = [0usize; 4];
    for abbr in STATES {
        // Named explicitly rather than globbed: the directory is NFS and has
        // been observed timing out on a listing.
        let path = dir.join(format!("{abbr}.address_v3.ptiles"));
        if !path.is_file() {
            missing.push(abbr);
            continue;
        }
        let rep = check_state(abbr, path, full);
        eprintln!(
            "  {} {:>10} records in {:>6} cells  (osm {} nad {} oa {} unknown {}){}",
            rep.abbr,
            rep.records,
            rep.cells,
            rep.sources[0],
            rep.sources[1],
            rep.sources[2],
            rep.sources[3],
            if rep.empty_field > 0 {
                format!("  {} empty field(s)", rep.empty_field)
            } else {
                String::new()
            }
        );
        records += rep.records;
        cells += rep.cells;
        for (i, n) in rep.sources.iter().enumerate() {
            sources[i] += n;
        }
        checked += 1;
    }
    eprintln!(
        "checked {checked} states, {records} records in {cells} cells \
         (osm {} nad {} oa {} unknown {})",
        sources[0], sources[1], sources[2], sources[3]
    );
    assert!(
        missing.is_empty(),
        "missing state address files: {missing:?}"
    );
    assert_eq!(checked, STATES.len(), "expected all 51 states");
}
