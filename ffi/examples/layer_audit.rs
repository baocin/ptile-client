//! Data-quality sweep over a published business pack.
use ptiles_core::file::PtilesFile;
use ptiles_core::source::FileSource;
use std::collections::{BTreeMap, HashMap};

fn main() {
    let path = std::env::args().nth(1).unwrap();
    let file = PtilesFile::open(FileSource::open(&path).unwrap()).unwrap();
    let version = file.header().version;
    let cells: Vec<u64> = file.index().iter().map(|e| e.h3_cell).collect();

    let mut named = 0usize;
    let mut unnamed = 0usize;
    let mut uncategorised = 0usize;
    let mut null_island = 0usize;
    let mut outside_tn = 0usize;
    let mut address_named = 0usize;
    let mut phone_named = 0usize;
    let mut very_short = 0usize;
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut per_cat: BTreeMap<u8, (usize, usize)> = BTreeMap::new();
    // Inside the four terminals, after both filters.
    let airports = [(36.126, -86.677), (35.042, -89.977), (35.810, -83.990), (35.040, -85.200)];
    let mut inside_total = 0usize;
    let mut inside_flagged = 0usize;

    for cell in cells {
        let Some(block) = file.read_block(cell).unwrap() else { continue };
        let Ok(records) = ptiles_core::decode_business_versioned(&block, version, cell) else {
            continue;
        };
        for b in records {
            let name = b.name.trim().to_string();
            if name.is_empty() {
                unnamed += 1;
                continue;
            }
            named += 1;
            if b.category_idx == 0 {
                uncategorised += 1;
            }
            let flight = ptiles_core::flight_nodes::is_flight_node(&name);
            if b.category_idx != 0 {
                let e = per_cat.entry(b.category_idx).or_insert((0, 0));
                e.0 += 1;
                if flight { e.1 += 1 }
            }
            if b.lat.abs() < 0.01 && b.lon.abs() < 0.01 {
                null_island += 1;
            }
            if !(34.9..=36.7).contains(&b.lat) || !(-90.4..=-81.6).contains(&b.lon) {
                outside_tn += 1;
            }
            // "2934 winchester rd", "5905 clark ave. Chattanooga TN"
            let lower = name.to_lowercase();
            let starts_with_number = name.starts_with(|c: char| c.is_ascii_digit());
            if starts_with_number
                && [" rd", " road", " ave", " st ", " street", " dr", " blvd", " hwy", " lane", " ln"]
                    .iter()
                    .any(|s| lower.contains(s) || lower.ends_with(s.trim_end()))
            {
                address_named += 1;
            }
            let digits = name.chars().filter(char::is_ascii_digit).count();
            if digits >= 7 && name.chars().filter(|c| c.is_alphabetic()).count() <= 2 {
                phone_named += 1;
            }
            if name.chars().count() <= 2 {
                very_short += 1;
            }
            // Same name within ~11 m: the classic double-import.
            let key = format!("{}|{:.4}|{:.4}", lower, b.lat, b.lon);
            *seen.entry(key).or_default() += 1;

            if airports.iter().any(|(a, o)| (b.lat - a).abs() < 0.015 && (b.lon - o).abs() < 0.015) {
                inside_total += 1;
                if flight { inside_flagged += 1 }
            }
        }
    }

    let dupes: usize = seen.values().filter(|c| **c > 1).map(|c| c - 1).sum();
    let flight_cats: Vec<u8> = per_cat
        .iter()
        .filter(|(_, (t, f))| *t >= 50 && *f as f64 / *t as f64 >= 0.4)
        .map(|(idx, _)| *idx)
        .collect();
    let swept: usize = per_cat.iter().filter(|(i, _)| flight_cats.contains(i)).map(|(_, (t, _))| t).sum();

    println!("named {named}, unnamed {unnamed}");
    println!("uncategorised (cat 0): {uncategorised} ({:.0}%)", uncategorised as f64 / named as f64 * 100.0);
    println!("duplicate name at same spot: {dupes} ({:.1}%)", dupes as f64 / named as f64 * 100.0);
    println!("named after a street address: {address_named}");
    println!("named as a phone number: {phone_named}");
    println!("one- or two-character names: {very_short}");
    println!("at (0,0): {null_island}   outside the state bbox: {outside_tn}");
    println!("flight categories {flight_cats:?}, sweeping {swept} records");
    println!(
        "inside the four terminals: {inside_total} records, {inside_flagged} caught by name",
    );

    // Second pass, now that the flight categories are known: what noise
    // survives both filters inside a terminal, and what do the duplicates
    // look like?
    let mut residual: Vec<String> = Vec::new();
    let mut dupe_samples: Vec<(String, usize)> = Vec::new();
    let mut near_dupes = 0usize;
    let mut by_name: HashMap<String, Vec<(f64, f64)>> = HashMap::new();
    for cell in file.index().iter().map(|e| e.h3_cell).collect::<Vec<_>>() {
        let Some(block) = file.read_block(cell).unwrap() else { continue };
        let Ok(records) = ptiles_core::decode_business_versioned(&block, version, cell) else {
            continue;
        };
        for b in records {
            let name = b.name.trim().to_string();
            if name.is_empty() { continue }
            by_name.entry(name.to_lowercase()).or_default().push((b.lat, b.lon));
            let dropped = ptiles_core::flight_nodes::is_flight_node(&name)
                || flight_cats.contains(&b.category_idx);
            if !dropped
                && airports.iter().any(|(a, o)| (b.lat - a).abs() < 0.015 && (b.lon - o).abs() < 0.015)
                && residual.len() < 4000
            {
                residual.push(name);
            }
        }
    }
    // Same name, different spot, under ~150 m apart: one place imported twice
    // from two sources rather than two branches of a chain.
    for spots in by_name.values() {
        for i in 0..spots.len() {
            for j in i + 1..spots.len() {
                let dy = (spots[i].0 - spots[j].0).abs();
                let dx = (spots[i].1 - spots[j].1).abs();
                if dy < 0.0014 && dx < 0.0017 && (dy > 0.0 || dx > 0.0) {
                    near_dupes += 1;
                }
            }
        }
    }
    println!("  surviving both filters inside a terminal: {}", residual.len());
    residual.sort();
    residual.dedup();
    for name in residual.iter().step_by((residual.len() / 25).max(1)).take(25) {
        println!("    {name}");
    }
    println!("same name twice within 150 m (different coords): {near_dupes}");
    let _ = &mut dupe_samples;
}
