//! Open every published layer and decode a real block from each.
//!
//! The synthetic matrix in `index_layout.rs` proves the detection logic; this
//! proves it against bytes an actual generator wrote. Both matter: the
//! synthetic tests would still pass if every real file turned out to use a
//! third layout nobody knew about.
//!
//! Fixtures are machine-local and each case skips when its file is absent, so
//! this suite is informative where the data lives and harmless where it
//! doesn't. `layer_coverage_is_asserted_somewhere` fails if *nothing* was
//! found, so an empty data directory can't masquerade as a green run.

#![cfg(feature = "std")]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use ptiles_core::file::{BlockOffsetBase, PtilesFile};
use ptiles_core::index::{ENTRY_SIZE_V1, ENTRY_SIZE_V2};
use ptiles_core::source::FileSource;

static OPENED: AtomicUsize = AtomicUsize::new(0);

/// Where per-state layers live, and where the freshly built national point
/// layers land (`scripts/build_points.py` writes to `ptiles/tiles`).
///
/// The committed corpus comes last on purpose. Where the full published layers
/// exist they are the better test -- megabytes of real index, thousands of
/// cells -- and this suite should use them. Where they don't (any CI runner,
/// any fresh clone) the corpus stands in, so `layer_coverage_is_asserted_somewhere`
/// still has something real to assert against instead of failing the build.
/// The corpus files are slices of these same published layers and detect the
/// same layout; see `conformance/slice.py`.
const SEARCH_DIRS: [&str; 4] = [
    "/home/aoi/kino/data/ptiles",
    "/home/aoi/kino/projects/ptiles/tiles",
    "/mnt/core/kino/ptiles/data/states",
    concat!(env!("CARGO_MANIFEST_DIR"), "/../conformance/corpus"),
];

fn find(name: &str) -> Option<PathBuf> {
    SEARCH_DIRS
        .iter()
        .map(|d| Path::new(d).join(name))
        .find(|p| p.exists())
}

struct Opened {
    entry_size: usize,
    offset_base: BlockOffsetBase,
    entries: usize,
    /// Decompressed size of the first block that had one.
    block_len: usize,
}

/// Open `name`, assert the detected layout matches expectation, and decode the
/// first block that has content.
fn check(name: &str, want_entry_size: usize) -> Option<Opened> {
    let path = find(name)?;
    let src = FileSource::open(&path).unwrap_or_else(|e| panic!("open {path:?}: {e}"));
    let file = PtilesFile::open(src).unwrap_or_else(|e| panic!("parse {path:?}: {e}"));
    let layout = file.layout();

    assert_eq!(
        layout.entry_size, want_entry_size,
        "{name}: detected {}-byte entries, expected {want_entry_size}",
        layout.entry_size
    );
    assert!(!file.index().is_empty(), "{name}: index is empty");

    let entry = file
        .index()
        .iter()
        .find(|e| e.block_length > 0)
        .unwrap_or_else(|| panic!("{name}: no entry names a non-empty block"));
    let block = file
        .read_block(entry.h3_cell)
        .unwrap_or_else(|e| panic!("{name}: read_block failed: {e}"))
        .unwrap_or_else(|| panic!("{name}: cell from the index resolved to nothing"));
    assert!(!block.is_empty(), "{name}: block decompressed to nothing");

    OPENED.fetch_add(1, Ordering::Relaxed);
    eprintln!(
        "{name}: {}-byte entries, {:?}, {} entries, first block {} B{}",
        layout.entry_size,
        layout.offset_base,
        file.index().len(),
        block.len(),
        if layout.header_is_inconsistent() {
            "  [header inconsistent]"
        } else {
            ""
        }
    );
    Some(Opened {
        entry_size: layout.entry_size,
        offset_base: layout.offset_base,
        entries: file.index().len(),
        block_len: block.len(),
    })
}

// ------------------------------------------------------- 19-byte-entry layers

#[test]
fn roads_is_v1_index() {
    check("TN.roads.ptiles", ENTRY_SIZE_V1);
}

#[test]
fn water_is_v1_index() {
    check("TN.water.ptiles", ENTRY_SIZE_V1);
}

#[test]
fn business_is_v1_index() {
    check("TN.business.ptiles", ENTRY_SIZE_V1);
}

#[test]
fn buildings_v8_is_v1_index_with_relative_offsets() {
    if let Some(o) = check("TN.buildings_v8.ptiles", ENTRY_SIZE_V1) {
        assert_eq!(
            o.offset_base,
            BlockOffsetBase::Relative,
            "buildings_v8 is the one layer observed storing relative offsets"
        );
    }
}

// ------------------------------------------------------- 38-byte-entry layers
//
// These are the layers the JS reader renders blank today and the Rust core
// could not open at all before this change.

#[test]
fn parks_is_v2_index() {
    check("TN.parks.ptiles", ENTRY_SIZE_V2);
}

#[test]
fn rail_is_v2_index() {
    check("TN.rail.ptiles", ENTRY_SIZE_V2);
}

#[test]
fn places_is_v2_index() {
    check("TN.places.ptiles", ENTRY_SIZE_V2);
}

// ------------------------------------------------ the freshly built national
// point layers, which are what signals/camera decoding is for

#[test]
fn national_signals_opens_and_decodes_records() {
    let Some(path) = find("US.signals.ptiles") else {
        eprintln!("skipping: US.signals.ptiles not built yet");
        return;
    };
    let file = PtilesFile::open(FileSource::open(&path).expect("open")).expect("parse");
    assert_eq!(file.layout().entry_size, ENTRY_SIZE_V2);
    assert!(
        !file.layout().header_is_inconsistent(),
        "a freshly built file must not need header correction; \
         if this fires, build_points.py has regressed"
    );

    let entry = file.index().iter().find(|e| e.block_length > 0).expect("a block");
    let block = file.read_cell(entry.h3_cell).expect("read").expect("present");
    let signals = ptiles_core::decode_signals(&block).expect("decode signals");
    assert!(!signals.is_empty(), "block must hold at least one signal");

    for s in signals.iter().take(64) {
        assert!(
            (-180.0..=180.0).contains(&s.lon) && (-90.0..=90.0).contains(&s.lat),
            "coordinate out of range: {s:?}"
        );
        assert!(!s.signal_type.is_empty());
        assert!(
            !s.signal_type.starts_with("unknown("),
            "signal_type byte outside the builder's table: {}",
            s.signal_type
        );
    }
    eprintln!(
        "US.signals: {} index entries, first block {} signals, e.g. {:?}",
        file.index().len(),
        signals.len(),
        signals[0]
    );
}

#[test]
fn national_camera_opens_and_decodes_records() {
    let Some(path) = find("US.camera.ptiles") else {
        eprintln!("skipping: US.camera.ptiles not built yet");
        return;
    };
    let file = PtilesFile::open(FileSource::open(&path).expect("open")).expect("parse");
    assert_eq!(file.layout().entry_size, ENTRY_SIZE_V2);
    assert!(!file.layout().header_is_inconsistent());

    let entry = file.index().iter().find(|e| e.block_length > 0).expect("a block");
    let block = file.read_cell(entry.h3_cell).expect("read").expect("present");
    let cams = ptiles_core::decode_cameras(&block).expect("decode cameras");
    assert!(!cams.is_empty());

    for c in cams.iter().take(64) {
        assert!(
            (-180.0..=180.0).contains(&c.lon) && (-90.0..=90.0).contains(&c.lat),
            "coordinate out of range: {c:?}"
        );
        assert!(c.direction.is_none_or(|d| d < 360 || d == 0xFFFF));
    }
    eprintln!(
        "US.camera: {} index entries, first block {} cameras, e.g. {:?}",
        file.index().len(),
        cams.len(),
        cams[0]
    );
}

/// Every decoded point must fall inside the H3 res-7 cell that indexes it.
/// `build_points.py --verify` asserts this when writing; asserting it on read
/// too means a generator that starts mis-assigning cells fails here rather
/// than quietly returning points from the wrong place.
#[test]
fn decoded_points_fall_in_the_cell_that_indexes_them() {
    let Some(path) = find("US.signals.ptiles") else {
        eprintln!("skipping: US.signals.ptiles not built yet");
        return;
    };
    let file = PtilesFile::open(FileSource::open(&path).expect("open")).expect("parse");

    let mut checked = 0usize;
    let step = (file.index().len() / 16).max(1);
    for entry in file.index().iter().step_by(step).take(16) {
        if entry.block_length == 0 {
            continue;
        }
        let block = file.read_cell(entry.h3_cell).expect("read").expect("present");
        let cell = h3o::CellIndex::try_from(entry.h3_cell).expect("index cell is a valid H3 index");
        for s in ptiles_core::decode_signals(&block).expect("decode") {
            let ll = h3o::LatLng::new(s.lat, s.lon).expect("finite coordinate");
            let got = ll.to_cell(cell.resolution());
            assert_eq!(
                got, cell,
                "{s:?} is indexed under {cell} but its own coordinates put it \
                 in {got}. Either the block was sliced wrong or the builder \
                 mis-assigned the cell."
            );
            checked += 1;
        }
    }
    assert!(
        checked > 0,
        "no point was checked -- this test stopped exercising anything"
    );
    eprintln!("cell containment: {checked} points confirmed in their own cell");
}

/// Guards the skip-if-absent pattern above: if the data directory is empty,
/// every test in this file passes without asserting anything, which looks
/// identical to real coverage. This one fails instead.
#[test]
fn layer_coverage_is_asserted_somewhere() {
    // Run the checks this test depends on, since test order isn't guaranteed.
    check("TN.roads.ptiles", ENTRY_SIZE_V1);
    check("TN.parks.ptiles", ENTRY_SIZE_V2);
    assert!(
        OPENED.load(Ordering::Relaxed) > 0,
        "no real .ptiles fixture was found in any of {SEARCH_DIRS:?} -- this \
         suite proved nothing. Point SEARCH_DIRS at the data or build it."
    );
}

// Silence dead-code warnings for fields kept for their diagnostic value.
#[allow(dead_code)]
fn _opened_fields_are_used(o: &Opened) -> (usize, usize) {
    (o.entries, o.block_len)
}
