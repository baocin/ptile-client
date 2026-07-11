//! Integration test for `--query address` / `address-find` against the
//! committed synthetic golden fixture (always present, no big data file).

use std::process::Command;

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../test-fixtures/golden/address.ptiles");
// The fixture's cell 0x87264d106ffffff centers near here.
const LAT: &str = "36.1665";
const LON: &str = "-86.7832";

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_ptiles-cli"))
        .args(args)
        .output()
        .expect("spawn ptiles-cli")
}

#[test]
fn address_reverse_enumerates_cell() {
    let out = run(&["--path", FIXTURE, "--lat", LAT, "--lon", LON, "--query", "address"]);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    let addrs = v["addresses"].as_array().expect("addresses array");
    assert_eq!(v["count"].as_u64().unwrap(), addrs.len() as u64);
    assert!(addrs.iter().any(|a| a["street"] == "Broadway" && a["housenumber"] == "100"));
}

#[test]
fn address_forward_finds_by_number_and_street_case_insensitive() {
    let out = run(&[
        "--path", FIXTURE, "--lat", LAT, "--lon", LON,
        "--query", "address-find", "--number", "100", "--street", "BROADWAY",
    ]);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    assert_eq!(v["count"].as_u64().unwrap(), 1);
    assert_eq!(v["addresses"][0]["osm_id"].as_i64().unwrap(), 1440913532);
}

#[test]
fn address_find_missing_args_exits_nonzero() {
    // address-find without --street must fail cleanly.
    let out = run(&[
        "--path", FIXTURE, "--lat", LAT, "--lon", LON,
        "--query", "address-find", "--number", "100",
    ]);
    assert!(!out.status.success(), "missing --street must exit non-zero");
}
