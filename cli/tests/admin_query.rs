//! Integration test for `--query admin` against the real US admin sample.
//! Skips when the file isn't present.

use std::path::Path;
use std::process::Command;

const ADMIN_FILE: &str = "/home/aoi/kino/data/ptiles/US.admin.ptiles";

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_ptiles-cli"))
        .args(args)
        .output()
        .expect("spawn ptiles-cli")
}

#[test]
fn admin_query_resolves_nashville_jurisdiction() {
    if !Path::new(ADMIN_FILE).exists() {
        eprintln!("skipping admin_query_resolves_nashville_jurisdiction: {ADMIN_FILE} not present");
        return;
    }
    let out = run(&[
        "--path", ADMIN_FILE, "--lat", "36.1627", "--lon", "-86.7816", "--query", "admin",
    ]);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    let admin = v.get("admin").expect("admin field");
    assert_eq!(admin["state"], "Tennessee");
    assert_eq!(admin["county"], "Davidson");
    assert_eq!(admin["timezone"], "America/Chicago");
}

#[test]
fn admin_query_bad_path_exits_nonzero() {
    let out = run(&[
        "--path", "/nonexistent/US.admin.ptiles", "--lat", "36.16", "--lon", "-86.78",
        "--query", "admin",
    ]);
    assert!(!out.status.success(), "missing admin file must exit non-zero");
}
