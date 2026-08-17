//! Search every business name in a state, through the file's own header.
use ptiles_core::file::PtilesFile;
use ptiles_core::name_scan::NameScan;
use ptiles_core::source::FileSource;

fn main() {
    let path = std::env::args().nth(1).unwrap();
    let opened = std::time::Instant::now();
    let file = PtilesFile::open(FileSource::open(&path).unwrap()).unwrap();
    let scan = NameScan::read(&file).unwrap().expect("a scan section");
    println!(
        "{} names, read and decompressed in {} ms (once per session)",
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
            hits.first().map(|h| format!("{} at {:.4},{:.4}", h.name, h.lat, h.lon)).unwrap_or_default(),
        );
    }
}
