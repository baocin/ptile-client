//! Property test: every prefix of a real golden block must return `Ok` or
//! `Err` from its decoder — never panic. Generalizes the single-layer
//! `buildings.rs::every_prefix_of_a_valid_block_returns_ok_or_err_never_panics`
//! pattern to every layer at once, using the committed golden fixtures.

use std::path::Path;

fn block(name: &str) -> Option<Vec<u8>> {
    let p = format!(
        "{}/../test-fixtures/golden/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    if Path::new(&p).exists() {
        Some(std::fs::read(p).unwrap())
    } else {
        None
    }
}

/// Feed a bounded set of evenly-spaced prefixes (at most `MAX_SAMPLES`) to
/// `decode`; the only requirement is that it never panics. Bounded because a
/// truncated prefix can be decode-legal-but-slow (a huge but in-range count
/// header makes the decoder allocate/iterate a lot); the exhaustive
/// never-panics coverage lives in the `cargo-fuzz` targets, this is the fast
/// deterministic guard.
fn sweep(data: &[u8], mut decode: impl FnMut(&[u8])) {
    const MAX_SAMPLES: usize = 256;
    let len = data.len();
    let step = (len / MAX_SAMPLES).max(1);
    let mut n = 0;
    while n <= len {
        decode(&data[..n]);
        n += step;
    }
    decode(data); // always include the full block
}

#[test]
fn every_prefix_of_every_golden_block_never_panics() {
    if let Some(b) = block("water.block.bin") {
        sweep(&b, |d| {
            let _ = ptiles_core::decode_water(d);
        });
    }
    if let Some(b) = block("parks.block.bin") {
        sweep(&b, |d| {
            let _ = ptiles_core::decode_parks(d);
        });
    }
    if let Some(b) = block("rail.block.bin") {
        sweep(&b, |d| {
            let _ = ptiles_core::decode_rail(d);
        });
    }
    if let Some(b) = block("roads.block.bin") {
        sweep(&b, |d| {
            let _ = ptiles_core::decode_roads(d);
            let _ = ptiles_core::decode_road_block(d, 2);
        });
    }
    if let Some(b) = block("business.block.bin") {
        // Only the correct decoder for these bytes. (The name-index decoder is
        // a different format — exercised by its own fuzz target; feeding it
        // business bytes just makes it NFD-normalize huge misread strings.)
        sweep(&b, |d| {
            let _ = ptiles_core::decode_business(d);
        });
    }
    if let Some(b) = block("buildings_v8.block.bin") {
        sweep(&b, |d| {
            let _ = ptiles_core::decode_buildings(d, 36.16, -86.78);
        });
    }
}

#[test]
fn every_prefix_of_address_fixture_sections_never_panics() {
    // The address fixture is a whole file; sweep its raw bytes through the
    // structural parsers (header/index) and the cell/record decoders.
    if let Some(b) = block("address.ptiles") {
        sweep(&b, |d| {
            let _ = ptiles_core::Header::parse(d);
            let _ = ptiles_core::parse_v2_index(d);
            let _ = ptiles_core::decode_address_cell(d);
            let _ = ptiles_core::address::merged_block_cell_slice(d, 0);
        });
    }
}
