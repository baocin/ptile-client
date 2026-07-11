//! `cargo run -p ptiles-ffi --bin uniffi-bindgen --features uniffi/cli -- generate ...`
//! Standard UniFFI CLI entry point — proc-macro metadata is read straight out
//! of the compiled library, no UDL file involved.
fn main() {
    uniffi::uniffi_bindgen_main()
}
