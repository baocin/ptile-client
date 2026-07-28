//! Dump supported_formats_table() for regenerating SUPPORTED_FORMATS.md.
//! Run: cargo run -p ptiles-core --example dump-table > /tmp/table.txt
fn main() {
    print!("{}", ptiles_core::supported_formats_table());
}
