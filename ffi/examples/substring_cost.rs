//! What would a substring search over the whole name index cost?
//!
//! The index buckets by first letter, so a substring query only reads the
//! bucket its own first letter names. Reading every bucket is the simplest
//! way to make `affle` find `Waffle House`; this measures whether that is
//! affordable before anyone designs a cleverer index.
fn main() {
    let index = std::env::args().nth(1).unwrap();
    let layer = ptiles_ffi::PtilesLayer::open(index).unwrap();
    for query in ["affle", "waffle", "recycl", "cherokee", "a"] {
        let started = std::time::Instant::now();
        let hits = layer.search_business(query.to_string(), 40).unwrap();
        println!(
            "{query:>10}: {:>3} hits in {:>4} ms   {}",
            hits.len(),
            started.elapsed().as_millis(),
            hits.first().map(|h| h.name.clone()).unwrap_or_default(),
        );
    }
}
