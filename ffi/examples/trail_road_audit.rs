//! Name and tagging quality in the trails and roads layers.
use ptiles_core::file::PtilesFile;
use ptiles_core::source::FileSource;
use std::collections::HashMap;

fn main() {
    let trails_path = std::env::args().nth(1).unwrap();
    let roads_path = std::env::args().nth(2).unwrap();

    let f = PtilesFile::open(FileSource::open(&trails_path).unwrap()).unwrap();
    let (mut total, mut unnamed) = (0usize, 0usize);
    let mut names: HashMap<String, usize> = HashMap::new();
    for cell in f.index().iter().map(|e| e.h3_cell).collect::<Vec<_>>() {
        let Some(block) = f.read_block(cell).unwrap() else { continue };
        let Ok(trails) = ptiles_core::decode_trails(&block) else { continue };
        for t in trails {
            total += 1;
            match t.name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
                None => unnamed += 1,
                Some(n) => *names.entry(n.to_lowercase()).or_default() += 1,
            }
        }
    }
    println!("trails: {total} segments, {unnamed} unnamed ({:.0}%), {} distinct names",
        unnamed as f64 / total as f64 * 100.0, names.len());
    let mut common: Vec<(&String, &usize)> = names.iter().collect();
    common.sort_by_key(|(_, c)| std::cmp::Reverse(**c));
    print!("  most repeated: ");
    for (n, c) in common.iter().take(5) { print!("{n} x{c};  ") }
    println!();

    let f = PtilesFile::open(FileSource::open(&roads_path).unwrap()).unwrap();
    let rversion = f.header().version;
    let (mut rtotal, mut runnamed, mut nospeed) = (0usize, 0usize, 0usize);
    let mut classes: HashMap<String, usize> = HashMap::new();
    for cell in f.index().iter().map(|e| e.h3_cell).collect::<Vec<_>>().iter().take(4000) {
        let Some(block) = f.read_block(*cell).unwrap() else { continue };
        let Ok((roads, _)) = ptiles_core::decode_road_block(&block, rversion) else { continue };
        for r in roads {
            rtotal += 1;
            if r.name.as_deref().map(str::trim).unwrap_or("").is_empty() { runnamed += 1 }
            if r.speed_limit_kmh.is_none() { nospeed += 1 }
            *classes.entry(r.road_class.clone()).or_default() += 1;
        }
    }
    println!("roads (first 4000 cells): {rtotal} segments, {runnamed} unnamed ({:.0}%), {nospeed} with no speed limit ({:.0}%)",
        runnamed as f64 / rtotal as f64 * 100.0, nospeed as f64 / rtotal as f64 * 100.0);
    let mut cl: Vec<(&String, &usize)> = classes.iter().collect();
    cl.sort_by_key(|(_, c)| std::cmp::Reverse(**c));
    print!("  classes: ");
    for (n, c) in cl.iter().take(8) { print!("{n}={c}  ") }
    println!();
}
