//! Does the airport junk share a category the real venues do not?
use ptiles_core::file::PtilesFile;
use ptiles_core::source::FileSource;
use std::collections::BTreeMap;

fn main() {
    let path = std::env::args().nth(1).unwrap();
    let file = PtilesFile::open(FileSource::open(&path).unwrap()).unwrap();
    let version = file.header().version;
    let cells: Vec<u64> = file.index().iter().map(|e| e.h3_cell).collect();
    let mut flagged: BTreeMap<u8, usize> = BTreeMap::new();
    let mut altitude: BTreeMap<u8, usize> = BTreeMap::new();
    let mut all: BTreeMap<u8, usize> = BTreeMap::new();
    // "10,000 Feet", "36,000 feet in the air", "35000 Feet Above Chicago"
    let looks_airborne = |n: &str| {
        let l = n.to_ascii_lowercase();
        (l.contains("feet") || l.contains(" ft") || l.contains("000'"))
            && l.chars().next().is_some_and(|c| c.is_ascii_digit())
    };
    for cell in cells {
        let Some(block) = file.read_block(cell).unwrap() else { continue };
        let Ok(records) = ptiles_core::decode_business_versioned(&block, version, cell) else {
            continue;
        };
        for b in records {
            if b.name.trim().is_empty() { continue }
            *all.entry(b.category_idx).or_default() += 1;
            if ptiles_core::flight_nodes::is_flight_node(&b.name) {
                *flagged.entry(b.category_idx).or_default() += 1;
            } else if looks_airborne(&b.name) {
                *altitude.entry(b.category_idx).or_default() += 1;
            }
        }
    }
    let top = |label: &str, m: &BTreeMap<u8, usize>| {
        let mut v: Vec<_> = m.iter().collect();
        v.sort_by_key(|(_, c)| std::cmp::Reverse(**c));
        let total: usize = m.values().sum();
        print!("{label} (total {total}): ");
        for (idx, count) in v.iter().take(6) {
            let share = **count as f64 / total as f64 * 100.0;
            print!("cat{idx}={count} ({share:.0}%)  ");
        }
        println!();
    };
    // Which categories would the builder's share test flag, and what is in
    // them? Mirrors ptiles/flightnodes.py::flight_categories, on indices here
    // because a pack carries no labels.
    let mut per_cat: BTreeMap<u8, (usize, usize)> = BTreeMap::new();
    for cell in file.index().iter().map(|e| e.h3_cell).collect::<Vec<_>>() {
        let Some(block) = file.read_block(cell).unwrap() else { continue };
        let Ok(records) = ptiles_core::decode_business_versioned(&block, version, cell) else {
            continue;
        };
        for b in records {
            if b.name.trim().is_empty() || b.category_idx == 0 { continue }
            let e = per_cat.entry(b.category_idx).or_insert((0, 0));
            e.0 += 1;
            if ptiles_core::flight_nodes::is_flight_node(&b.name) { e.1 += 1 }
        }
    }
    println!("--- categories the 40%/50-record test would drop ---");
    let mut swept = 0usize;
    for (idx, (total, flights)) in &per_cat {
        let share = *flights as f64 / *total as f64;
        if *total >= 50 && share >= 0.4 {
            println!("  cat{idx}: {total} records, {flights} flight-named ({:.0}%)", share * 100.0);
            swept += total;
        }
    }
    println!("  total swept: {swept}");
    // What is category 94, exactly?
    let mut cat94: Vec<(String, bool)> = Vec::new();
    for cell in file.index().iter().map(|e| e.h3_cell).collect::<Vec<_>>() {
        let Some(block) = file.read_block(cell).unwrap() else { continue };
        let Ok(records) = ptiles_core::decode_business_versioned(&block, version, cell) else {
            continue;
        };
        for b in records {
            if b.category_idx == 94 && !b.name.trim().is_empty() {
                cat94.push((b.name.clone(), ptiles_core::flight_nodes::is_flight_node(&b.name)));
            }
        }
    }
    let flagged94 = cat94.iter().filter(|(_, f)| *f).count();
    println!(
        "category 94: {} records, {} already flagged, {} kept",
        cat94.len(), flagged94, cat94.len() - flagged94,
    );
    println!("--- a sample of category 94 the name rule keeps ---");
    let mut kept: Vec<&String> = cat94.iter().filter(|(_, f)| !f).map(|(n, _)| n).collect();
    kept.sort();
    kept.dedup();
    for name in kept.iter().step_by((kept.len() / 40).max(1)).take(40) {
        println!("  {name}");
    }
    top("flagged flights/gates", &flagged);
    top("airborne check-ins", &altitude);
    top("every named record", &all);
}
