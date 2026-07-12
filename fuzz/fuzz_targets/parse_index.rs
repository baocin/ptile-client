#![no_main]
use libfuzzer_sys::fuzz_target;
fuzz_target!(|data: &[u8]| {
    if let Ok(entries) = ptiles_core::parse_index(data) {
        let _ = ptiles_core::index_binary_search(&entries, 0);
    }
    // v2 (address) index shares this byte surface.
    let _ = ptiles_core::parse_v2_index(data);
});
