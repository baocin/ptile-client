//! Drift guard: `SUPPORTED_FORMATS.md`'s generated table section must match
//! `ptiles_core::supported_formats()` (built from the `SUPPORTED_FORMATS`
//! const) verbatim. Task: Addendum 2 decision 3 ("a test asserting doc and
//! const agree so it can't drift").
//!
//! `std`-only (reads a file at a repo-relative path) -- fine, since this test
//! binary is only built with the default `std` feature.

use std::fs;
use std::path::Path;

const BEGIN_MARKER: &str = "<!-- BEGIN GENERATED SUPPORTED_FORMATS TABLE -->";
const END_MARKER: &str = "<!-- END GENERATED SUPPORTED_FORMATS TABLE -->";

#[test]
fn doc_matches_generated_table() {
    let doc_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../SUPPORTED_FORMATS.md");
    let doc = fs::read_to_string(&doc_path)
        .unwrap_or_else(|e| panic!("failed to read {doc_path:?}: {e}"));

    let begin = doc
        .find(BEGIN_MARKER)
        .unwrap_or_else(|| panic!("{doc_path:?} missing {BEGIN_MARKER:?}"))
        + BEGIN_MARKER.len();
    let end = doc
        .find(END_MARKER)
        .unwrap_or_else(|| panic!("{doc_path:?} missing {END_MARKER:?}"));
    assert!(begin <= end, "generated-section markers out of order in {doc_path:?}");

    let doc_section = doc[begin..end].trim();
    let generated = ptiles_core::supported_formats_table();
    let generated = generated.trim();

    assert_eq!(
        doc_section, generated,
        "SUPPORTED_FORMATS.md's generated section has drifted from \
         ptiles_core::SUPPORTED_FORMATS -- regenerate the doc from \
         supported_formats_table()"
    );
}
