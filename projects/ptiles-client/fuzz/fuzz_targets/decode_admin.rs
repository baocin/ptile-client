#![no_main]
//! Fuzz the admin sections (grid, string tables, polygons).
use libfuzzer_sys::fuzz_target;
fuzz_target!(|data: &[u8]| {
    let _ = ptiles_core::admin::decode_grid(data);
    let _ = ptiles_core::admin::decode_string_tables(data);
    let _ = ptiles_core::admin::decode_polygons(data);
});
