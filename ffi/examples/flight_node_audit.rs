//! Measure the flight-node rule against a published pack.
use ptiles_core::file::PtilesFile;
use ptiles_core::source::FileSource;

fn main() {
    let path = std::env::args().nth(1).unwrap();
    let file = PtilesFile::open(FileSource::open(&path).unwrap()).unwrap();
    let version = file.header().version;
    let cells: Vec<u64> = file.index().iter().map(|e| e.h3_cell).collect();
    let (mut named, mut dropped) = (0usize, 0usize);
    let mut samples: Vec<String> = Vec::new();
    let mut far: Vec<(String, f64, f64)> = Vec::new();
    let airports = [(36.126, -86.677), (35.042, -89.977), (35.81, -83.99), (35.04, -85.20)];
    for cell in cells {
        let Some(block) = file.read_block(cell).unwrap() else { continue };
        let Ok(records) = ptiles_core::decode_business_versioned(&block, version, cell) else { continue };
        for b in records {
            if b.name.trim().is_empty() { continue }
            named += 1;
            if ptiles_core::flight_nodes::is_flight_node(&b.name) {
                dropped += 1;
                if samples.len() < 10 { samples.push(b.name.clone()); }
                let near = airports.iter().any(|(a, c)| (b.lat - a).abs() + (b.lon - c).abs() < 0.3);
                if !near && far.len() < 15 { far.push((b.name.clone(), b.lat, b.lon)); }
            }
        }
    }
    println!("named {named}, dropped {dropped}");
    for s in &samples { println!("  drop: {s}"); }
    println!("dropped away from a TN airport (first 15): {}", far.len());
    for (n, lat, lon) in &far { println!("  far: {n}  ({lat:.3}, {lon:.3})"); }
}
