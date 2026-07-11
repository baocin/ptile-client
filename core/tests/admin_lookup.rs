//! End-to-end admin lookup against the real `US.admin.ptiles` sample.
//! Skips (with an eprintln) when the ~31 MB file isn't present, like the
//! other data-dependent integration tests.

use std::path::Path;

use ptiles_core::{AdminFile, FileSource};

const ADMIN_FILE: &str = "/home/aoi/kino/data/ptiles/US.admin.ptiles";

#[test]
fn admin_lookup_resolves_nashville() {
    if !Path::new(ADMIN_FILE).exists() {
        eprintln!("skipping admin_lookup_resolves_nashville: {ADMIN_FILE} not present");
        return;
    }
    let source = FileSource::open(ADMIN_FILE).expect("open admin source");
    let admin = AdminFile::open(source).expect("parse admin file");

    // Downtown Nashville.
    let info = admin
        .admin_at(36.1627, -86.7816)
        .expect("Nashville should resolve to a grid entry");
    assert_eq!(info.country, "United States");
    assert_eq!(info.state, "Tennessee");
    assert_eq!(info.county, "Davidson", "got {info:?}");
    assert!(info.zip.starts_with("37"), "expected a TN zip, got {:?}", info.zip);
    assert_eq!(info.timezone, "America/Chicago");
}

#[test]
fn admin_lookup_open_grows_polygons() {
    if !Path::new(ADMIN_FILE).exists() {
        eprintln!("skipping admin_lookup_open_grows_polygons: {ADMIN_FILE} not present");
        return;
    }
    let source = FileSource::open(ADMIN_FILE).expect("open admin source");
    let admin = AdminFile::open(source).expect("parse admin file");
    let polys = admin.polygons().expect("decode polygons");
    assert!(!polys.is_empty(), "admin file should carry boundary polygons");
    assert!(polys.iter().all(|p| p.admin_level == 4));
}
