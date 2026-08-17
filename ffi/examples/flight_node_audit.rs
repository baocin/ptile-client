//! Measure the flight-node rule against a published pack.
//!
//! `cargo run -p ptiles-ffi --example flight_node_audit --release -- <business.ptiles> [survivors]`
use ptiles_core::file::PtilesFile;
use ptiles_core::source::FileSource;

/// (lat, lon, name) of the airports whose records this samples.
const AIRPORTS: [(f64, f64, &str); 4] = [
    (36.126, -86.677, "BNA"),
    (35.042, -89.977, "MEM"),
    (35.810, -83.990, "TYS"),
    (35.040, -85.200, "CHA"),
];

fn main() {
    let path = std::env::args().nth(1).unwrap();
    let survivors = std::env::args().nth(2).is_some();
    let file = PtilesFile::open(FileSource::open(&path).unwrap()).unwrap();
    let version = file.header().version;
    let cells: Vec<u64> = file.index().iter().map(|e| e.h3_cell).collect();
    let (mut named, mut dropped) = (0usize, 0usize);
    // Everything within ~1.5 km of an airport reference point: the terminal
    // and its apron, not the hotels and car parks on the ring road.
    let mut inside: Vec<(String, bool, &str)> = Vec::new();
    for cell in cells {
        let Some(block) = file.read_block(cell).unwrap() else { continue };
        let Ok(records) = ptiles_core::decode_business_versioned(&block, version, cell) else {
            continue;
        };
        for b in records {
            if b.name.trim().is_empty() {
                continue;
            }
            named += 1;
            let flagged = ptiles_core::flight_nodes::is_flight_node(&b.name);
            if flagged {
                dropped += 1;
            }
            if let Some((_, _, code)) = AIRPORTS
                .iter()
                .find(|(a, o, _)| (b.lat - a).abs() < 0.015 && (b.lon - o).abs() < 0.015)
            {
                inside.push((b.name.clone(), flagged, code));
            }
        }
    }
    let flagged_inside = inside.iter().filter(|(_, f, _)| *f).count();
    println!("named {named}, flagged {dropped}");
    println!(
        "inside the four terminals: {} records, {} flagged, {} kept",
        inside.len(),
        flagged_inside,
        inside.len() - flagged_inside,
    );
    if survivors {
        println!("--- kept inside a terminal (the recall question) ---");
        let mut kept: Vec<&(String, bool, &str)> = inside.iter().filter(|(_, f, _)| !f).collect();
        kept.sort();
        for (name, _, code) in kept {
            println!("  {code}  {name}");
        }
    }
}
