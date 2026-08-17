//! Dump business names and coordinates as TSV, for offline analysis.
use ptiles_core::file::PtilesFile;
use ptiles_core::source::FileSource;

fn main() {
    let path = std::env::args().nth(1).unwrap();
    let file = PtilesFile::open(FileSource::open(&path).unwrap()).unwrap();
    let version = file.header().version;
    for cell in file.index().iter().map(|e| e.h3_cell).collect::<Vec<_>>() {
        let Some(block) = file.read_block(cell).unwrap() else { continue };
        let Ok(records) = ptiles_core::decode_business_versioned(&block, version, cell) else {
            continue;
        };
        for b in records {
            let name = b.name.trim().replace('\t', " ");
            if name.is_empty() {
                continue;
            }
            println!("{name}\t{:.6}\t{:.6}\t{}", b.lat, b.lon, b.category_idx);
        }
    }
}
