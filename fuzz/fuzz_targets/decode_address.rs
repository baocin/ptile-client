#![no_main]
//! Fuzz the address v2 index, merged-block cell slicing, and record decode.
use libfuzzer_sys::fuzz_target;
fuzz_target!(|data: &[u8]| {
    let _ = ptiles_core::parse_v2_index(data);
    let _ = ptiles_core::decode_address_cell(data);
    let _ = ptiles_core::address::merged_block_cell_slice(data, 0);
});
