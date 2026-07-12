#![no_main]

use libfuzzer_sys::fuzz_target;

// Fixed dummy cell center coords (SPEC: v8 buildings are stored relative to
// the H3 cell center). Any finite lat/lon works for fuzzing decode robustness.
const DUMMY_LAT: f64 = 36.16;
const DUMMY_LON: f64 = -86.78;

fuzz_target!(|data: &[u8]| {
    let _ = ptiles_core::decode_buildings(data, DUMMY_LAT, DUMMY_LON);
});
