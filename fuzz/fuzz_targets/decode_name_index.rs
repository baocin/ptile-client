#![no_main]
//! Fuzz the business name-index block decode + match path.
use libfuzzer_sys::fuzz_target;
fuzz_target!(|data: &[u8]| {
    let _ = ptiles_core::match_business_name_block(data, "a", 10);
});
