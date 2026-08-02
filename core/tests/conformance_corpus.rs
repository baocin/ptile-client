//! The Rust half of the cross-language conformance corpus.
//!
//! `real_layers.rs` opens whatever published data happens to exist on the
//! machine, and skips where it doesn't. That makes it strong here and silent
//! on a CI runner -- which is exactly how CI stayed red: its
//! `layer_coverage_is_asserted_somewhere` guard fired because nothing was
//! found.
//!
//! This suite is the opposite. `conformance/corpus/` is committed, so it runs
//! everywhere and never skips. Each file is a slice of a real published layer
//! (see `conformance/slice.py`), carrying the same header, the same index
//! entries, and the same detected layout as the file it came from.
//!
//! What it pins is the *layout decision*, not just the decoded output: entry
//! width, why that width was chosen, offset base, and the declared stride.
//! A reader that lands on the right bytes by the wrong reasoning passes an
//! output-only test and fails this one -- which matters, because every format
//! bug this codebase has had was a layout decision that was wrong in a way
//! the output only revealed later, as silently-empty layers.

#![cfg(feature = "std")]

use std::path::{Path, PathBuf};

use ptiles_core::file::{BlockOffsetBase, PtilesFile};
use ptiles_core::index::EntrySizeSource;
use ptiles_core::source::FileSource;

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../conformance/corpus")
}

fn manifest() -> serde_json::Value {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../conformance/manifest.json");
    let raw = std::fs::read_to_string(&p)
        .unwrap_or_else(|e| panic!("read {p:?}: {e} -- regenerate with conformance/slice.py"));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse {p:?}: {e}"))
}

/// `manifest.json` records the layout as strings; map them onto the enums so a
/// renamed variant is a compile error rather than a silently-passing string
/// comparison.
fn want_offset_base(name: &str, overshoot: u64) -> BlockOffsetBase {
    match name {
        "Absolute" => BlockOffsetBase::Absolute,
        "Relative" => BlockOffsetBase::Relative,
        "AbsoluteCorrected" => BlockOffsetBase::AbsoluteCorrected { overshoot },
        other => panic!("manifest names an unknown offset base {other:?}"),
    }
}

fn want_entry_size_source(name: &str) -> EntrySizeSource {
    match name {
        "DeclaredLength" => EntrySizeSource::DeclaredLength,
        "Probed" => EntrySizeSource::Probed,
        "Forced" => EntrySizeSource::Forced,
        other => panic!("manifest names an unknown entry-size source {other:?}"),
    }
}

#[test]
fn every_corpus_file_detects_the_layout_its_manifest_records() {
    let m = manifest();
    let files = m["files"].as_object().expect("manifest.files is an object");
    assert!(!files.is_empty(), "manifest lists no files");

    let mut checked = 0;
    for (name, want) in files {
        let path = corpus_dir().join(name);
        let src = FileSource::open(&path).unwrap_or_else(|e| {
            panic!("open {path:?}: {e} -- corpus file missing; run conformance/slice.py")
        });
        let file = PtilesFile::open(src).unwrap_or_else(|e| panic!("parse {name}: {e}"));
        let layout = file.layout();

        let want_size = want["entry_size"].as_u64().unwrap() as usize;
        assert_eq!(
            layout.entry_size, want_size,
            "{name}: detected {}-byte entries, manifest says {want_size}",
            layout.entry_size
        );

        let want_base = want_offset_base(
            want["offset_base"].as_str().unwrap(),
            want["overshoot"].as_u64().unwrap(),
        );
        assert_eq!(
            layout.offset_base, want_base,
            "{name}: offset base {:?}, manifest says {want_base:?}",
            layout.offset_base
        );

        assert_eq!(
            layout.entry_size_source,
            want_entry_size_source(want["entry_size_source"].as_str().unwrap()),
            "{name}: the width was chosen for a different reason than recorded"
        );

        let want_stride = want["declared_stride"].as_u64().map(|s| s as usize);
        assert_eq!(
            layout.declared_stride, want_stride,
            "{name}: declared stride {:?}, manifest says {want_stride:?}",
            layout.declared_stride
        );

        let want_count = want["entry_count"].as_u64().unwrap() as usize;
        assert_eq!(
            file.index().len(),
            want_count,
            "{name}: index has {} entries, manifest says {want_count}",
            file.index().len()
        );

        checked += 1;
    }

    assert!(checked > 0, "no corpus file was checked");
    eprintln!("conformance: {checked} corpus files matched their manifest layout");
}

/// Detecting the layout is only half of it -- the offsets have to land on real
/// bytes. Every entry must decompress, which is what actually failed when the
/// 42-byte stride shipped: layout looked plausible and no block was reachable.
#[test]
fn every_corpus_entry_resolves_to_a_decodable_block() {
    let m = manifest();
    let mut blocks = 0;

    for (name, _) in m["files"].as_object().unwrap() {
        let path = corpus_dir().join(name);
        let src = FileSource::open(&path).unwrap_or_else(|e| panic!("open {path:?}: {e}"));
        let file = PtilesFile::open(src).unwrap_or_else(|e| panic!("parse {name}: {e}"));

        let cells: Vec<u64> = file.index().iter().map(|e| e.h3_cell).collect();
        for cell in cells {
            let got = file
                .read_block(cell)
                .unwrap_or_else(|e| panic!("{name}: read_block({cell:x}): {e}"));
            let block = got.unwrap_or_else(|| panic!("{name}: cell {cell:x} is in the index but read_block returned None"));
            assert!(
                !block.is_empty(),
                "{name}: cell {cell:x} decompressed to nothing"
            );
            blocks += 1;
        }
    }

    assert!(blocks > 0, "no block was decoded");
    eprintln!("conformance: {blocks} blocks decompressed across the corpus");
}

/// The two `stride42` files are the published `US.signals`/`US.camera` bug
/// preserved as bytes: `index_length` computed at 42 bytes while the encoder
/// emitted 38. Read as 19-byte entries they look structurally plausible and
/// yield `block_length == 0` for every cell -- the silent-empty failure. This
/// asserts the reader still calls it, since it is the one case in the corpus
/// that no synthetic fixture caught in time.
#[test]
fn the_forty_two_byte_stride_files_are_still_detected_as_broken() {
    let m = manifest();
    let mut seen = 0;

    for (name, want) in m["files"].as_object().unwrap() {
        if !name.contains("stride42") {
            continue;
        }
        seen += 1;

        assert_eq!(
            want["declared_stride"].as_u64(),
            Some(42),
            "{name}: this file exists to carry a 42-byte declared stride"
        );

        let path = corpus_dir().join(name);
        let src = FileSource::open(&path).unwrap_or_else(|e| panic!("open {path:?}: {e}"));
        let file = PtilesFile::open(src).unwrap_or_else(|e| panic!("parse {name}: {e}"));
        let layout = file.layout();

        assert_eq!(layout.entry_size, 38, "{name}: entries are 38 bytes wide");
        assert_eq!(
            layout.entry_size_source,
            EntrySizeSource::Probed,
            "{name}: 42 divides evenly but is not a known width, so the width \
             must come from probing, not from the header"
        );
        assert!(
            matches!(layout.offset_base, BlockOffsetBase::AbsoluteCorrected { .. }),
            "{name}: offsets were derived from the overshooting blocks_offset \
             and need correcting, got {:?}",
            layout.offset_base
        );
        assert!(
            layout.header_is_inconsistent(),
            "{name}: the header contradicts its own index and should say so"
        );
    }

    assert_eq!(
        seen, 2,
        "expected both stride42 files in the corpus; found {seen}"
    );
}

/// The PTCI coarse index in `aux` maps a cell to a *position* in the real
/// index, so it is only useful if those positions still name the cell the
/// sample claims. Nothing checked that before: the coarse reader lived only in
/// `demo/index.html`, where a stale sample surfaces as "cell not in this file"
/// rather than as an error -- indistinguishable from sparse coverage.
#[test]
fn the_coarse_index_agrees_with_the_index_it_points_into() {
    let m = manifest();
    let mut files = 0;
    let mut samples = 0;

    for (name, want) in m["files"].as_object().unwrap() {
        if want["aux_length"].as_u64().unwrap_or(0) == 0 {
            continue;
        }
        let path = corpus_dir().join(name);
        let raw = std::fs::read(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
        let header = ptiles_core::Header::parse(&raw).expect("header");
        let at = header.aux_offset as usize;
        let aux = &raw[at..at + header.aux_length as usize];

        let Some(coarse) = ptiles_core::parse_coarse_index(aux)
            .unwrap_or_else(|e| panic!("{name}: aux announced PTCI but did not parse: {e}"))
        else {
            continue; // aux holds something that is not a coarse index
        };
        files += 1;

        let file = PtilesFile::open(FileSource::open(&path).unwrap()).unwrap();
        let index = file.index();

        assert_eq!(
            coarse.entry_count as usize,
            index.len(),
            "{name}: coarse index claims {} entries, the real index has {}",
            coarse.entry_count,
            index.len()
        );

        for s in &coarse.samples {
            let pos = s.entry_index as usize;
            assert!(
                pos < index.len(),
                "{name}: sample points at entry {pos}, past the {}-entry index",
                index.len()
            );
            assert_eq!(
                index[pos].h3_cell, s.h3_cell,
                "{name}: sample says entry {pos} is cell {:x}, but it is {:x}",
                s.h3_cell, index[pos].h3_cell
            );
            samples += 1;
        }

        // Every cell in the index must fall inside the bracket its own sample
        // produces -- that is the whole contract, and it is what a partial-index
        // reader relies on when it fetches only that run.
        for (i, e) in index.iter().enumerate() {
            let b = coarse
                .bracket(e.h3_cell)
                .unwrap_or_else(|| panic!("{name}: cell {:x} bracketed to nothing", e.h3_cell));
            assert!(
                (b.start as usize) <= i && i <= (b.end as usize),
                "{name}: entry {i} (cell {:x}) is outside its own bracket {}..={}",
                e.h3_cell,
                b.start,
                b.end
            );
        }
    }

    assert!(
        files >= 2,
        "expected the rebuilt signals and camera slices to carry a coarse index; found {files}"
    );
    assert!(samples > 0, "no coarse samples were checked");
    eprintln!("conformance: {samples} coarse samples verified across {files} files");
}

/// Guards the corpus the way `real_layers.rs` guards the data directory: if
/// `conformance/corpus/` were emptied, every loop above would iterate zero
/// times over a manifest that also happened to be empty, and pass. This fails
/// instead.
#[test]
fn the_corpus_is_not_empty() {
    let dir = corpus_dir();
    let found: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {dir:?}: {e} -- run conformance/slice.py"))
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "ptiles"))
        .collect();

    assert!(
        found.len() >= 8,
        "conformance/corpus/ holds {} .ptiles files; the corpus is supposed to \
         cover both entry widths and all three offset bases, so this is too few \
         to have proved anything. Run conformance/slice.py.",
        found.len()
    );

    let listed = manifest()["files"].as_object().unwrap().len();
    assert_eq!(
        found.len(),
        listed,
        "corpus/ holds {} files but manifest.json lists {listed} -- they were \
         generated at different times",
        found.len()
    );
}
