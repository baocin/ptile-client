//! Search every business name in a state, and time it.
use ptiles_core::name_scan::NameScan;

fn main() {
    let path = std::env::args().nth(1).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    println!("section on disk: {:.2} MB", bytes.len() as f64 / 1e6);

    let opened = std::time::Instant::now();
    let scan = NameScan::parse(&bytes).unwrap().expect("a section");
    println!(
        "parsed {} names in {} ms (decompress, once per session)",
        scan.len(),
        opened.elapsed().as_millis(),
    );

    for query in ["affle", "waffle", "recycl", "cherokee", "ffee", "dudleys"] {
        let started = std::time::Instant::now();
        let hits = scan.search(query, 40);
        println!(
            "{query:>10}: {:>3} hits in {:>4} ms   {}",
            hits.len(),
            started.elapsed().as_millis(),
            hits.first().map(|h| h.name.clone()).unwrap_or_default(),
        );
    }
}
