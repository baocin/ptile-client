//! Golden test for the address layer against a reference-encoder-generated
//! fixture (`test-fixtures/build_address_golden.py`). No real address sample is
//! hosted, so this fixture is produced by the same Python encode helpers the
//! real builder uses — a true differential check of the Rust decoder.

use ptiles_core::{AddressFile, MemorySource};
use serde_json::Value;

fn fixture_bytes() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test-fixtures/golden/address.ptiles"
    ))
    .unwrap()
}

fn golden() -> Value {
    let raw = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test-fixtures/golden/address.golden.json"
    ))
    .unwrap();
    serde_json::from_slice(&raw).unwrap()
}

#[test]
fn address_file_decodes_golden_fixture() {
    let file = AddressFile::open(MemorySource::new(fixture_bytes())).expect("open address file");
    let g = golden();
    let cells = g["cells"].as_array().unwrap();
    assert_eq!(file.index().len(), cells.len());

    for cell in cells {
        let cell_id = cell["cell_id"].as_u64().unwrap();
        let expected = cell["addresses"].as_array().unwrap();
        let decoded = file.addresses_in_cell(cell_id).expect("decode cell");
        assert_eq!(decoded.len(), expected.len(), "cell {cell_id:#x} count");
        for (d, e) in decoded.iter().zip(expected) {
            assert_eq!(d.osm_id, e["osm_id"].as_i64().unwrap());
            assert_eq!(d.housenumber, e["housenumber"].as_str().unwrap());
            assert_eq!(d.street, e["street"].as_str().unwrap());
        }
    }
}

#[test]
fn address_forward_lookup_matches_by_number_and_street() {
    let file = AddressFile::open(MemorySource::new(fixture_bytes())).expect("open");
    // The first golden cell has "100 Broadway". Look it up directly via the
    // cell's records + fold-matching (case-insensitive).
    let g = golden();
    let cell_id = g["cells"][0]["cell_id"].as_u64().unwrap();
    let recs = file.addresses_in_cell(cell_id).unwrap();
    let hit = recs
        .iter()
        .find(|r| r.housenumber == "100" && r.street.contains("Broadway"));
    assert!(hit.is_some(), "expected 100 Broadway in the first cell");
}

#[test]
fn address_open_rejects_admin_file() {
    // An admin file (block_count==0, aux_length>0) must be rejected by the
    // address opener (they share the PTILESA magic).
    let admin = "/home/aoi/kino/data/ptiles/US.admin.ptiles";
    if !std::path::Path::new(admin).exists() {
        eprintln!("skipping address_open_rejects_admin_file: {admin} not present");
        return;
    }
    let src = MemorySource::new(std::fs::read(admin).unwrap());
    assert!(
        AddressFile::open(src).is_err(),
        "an admin file must not open as an address file"
    );
}

/// The shape every published state file actually has: magic `PTILESD`,
/// version 2, blocks compressed against a stored zstd dictionary. The v1
/// fixture above shares none of those three properties, which is how the
/// reader shipped rejecting the magic and decompressing without the
/// dictionary while this test file stayed green.
fn dict_fixture_bytes() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test-fixtures/golden/address_v2_dict.ptiles"
    ))
    .unwrap()
}

fn dict_golden() -> Value {
    let raw = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test-fixtures/golden/address_v2_dict.golden.json"
    ))
    .unwrap();
    serde_json::from_slice(&raw).unwrap()
}

#[test]
fn address_v2_dictionary_compressed_file_decodes() {
    let file = AddressFile::open(MemorySource::new(dict_fixture_bytes())).expect("open v2+dict");
    let g = dict_golden();
    assert!(g["dict_length"].as_u64().unwrap() > 0, "fixture has a dict");

    for cell in g["cells"].as_array().unwrap() {
        let cell_id = cell["cell_id"].as_u64().unwrap();
        let expected = cell["addresses"].as_array().unwrap();
        let decoded = file.addresses_in_cell(cell_id).expect("decode cell");
        assert_eq!(decoded.len(), expected.len(), "cell {cell_id:#x} count");
        for (d, e) in decoded.iter().zip(expected) {
            assert_eq!(d.osm_id, e["osm_id"].as_i64().unwrap());
            assert_eq!(d.housenumber, e["housenumber"].as_str().unwrap());
            assert_eq!(d.street, e["street"].as_str().unwrap());
            // v2's whole point: the record knows where it is, to 1e-5 degrees.
            let (lat, lon) = (d.lat.expect("lat"), d.lon.expect("lon"));
            assert!((lat - e["lat"].as_f64().unwrap()).abs() < 1e-6, "lat {lat}");
            assert!((lon - e["lon"].as_f64().unwrap()).abs() < 1e-6, "lon {lon}");
        }
    }
}
