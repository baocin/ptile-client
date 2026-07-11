#![no_main]
//! Fuzz the whole-file open + block-read chain (header + index + zstd/dict
//! decompress + block decode) — the highest-value structural surface, and the
//! only path that touches `ruzstd`.
use libfuzzer_sys::fuzz_target;
use ptiles_core::{MemorySource, PtilesFile};

fuzz_target!(|data: &[u8]| {
    if let Ok(f) = PtilesFile::open(MemorySource::new(data.to_vec())) {
        for e in f.index().iter().take(16) {
            let _ = f.read_block(e.h3_cell);
        }
    }
});
