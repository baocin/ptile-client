//! What would a substring search over the whole name index cost?
//!
//! The index buckets by first letter, so a substring query only reads the
//! bucket its own first letter names. Reading every bucket is the simplest
//! way to make `affle` find `Waffle House`; this measures whether that is
//! affordable before anyone designs a cleverer index.
fn main() {
    let index = std::env::args().nth(1).unwrap();
    let layer = ptiles_ffi::PtilesLayer::open(index).unwrap();
    for query in ["affle", "recycl", "cherokee", "ffee"] {
        let fast_at = std::time::Instant::now();
        let fast = layer.search_business(query.to_string(), 40).unwrap();
        let fast_ms = fast_at.elapsed().as_millis();
        let wide_at = std::time::Instant::now();
        let wide = layer.search_business_everywhere(query.to_string(), 40).unwrap();
        let wide_ms = wide_at.elapsed().as_millis();
        let known: std::collections::HashSet<String> =
            fast.iter().map(|h| h.name.clone()).collect();
        let added: Vec<String> = wide
            .iter()
            .map(|h| h.name.clone())
            .filter(|n| !known.contains(n))
            .collect();
        println!(
            "{query:>10}: fast {:>2} hits {fast_ms:>4} ms | everywhere {:>2} hits {wide_ms:>4} ms | {} new, e.g. {:?}",
            fast.len(),
            wide.len(),
            added.len(),
            added.iter().take(3).collect::<Vec<_>>(),
        );
    }
}
