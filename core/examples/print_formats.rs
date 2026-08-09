//! Regenerate SUPPORTED_FORMATS.md's generated table section:
//!   cargo run -p ptiles-core --example print_formats
fn main() {
    print!("{}", ptiles_core::supported_formats_table());
}
