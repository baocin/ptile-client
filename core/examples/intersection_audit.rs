//! Settles GOAL.md section 2: are TN and NC still on the May snapshot, and are
//! the 9 stranded intersection entries still there?
//!
//! Counts intersections by control type in the *published* roads files, and
//! checks each one against the cell that indexes it. A stranded entry sits in
//! the wrong H3 cell and is therefore unreachable by any cell lookup -- it
//! decodes fine and can never be found, which is this format's usual failure
//! shape.
//!
//!   cargo run -p ptiles-core --example intersection_audit

use std::collections::BTreeMap;

use ptiles_core::{HttpSource, PtilesFile, cell_for_coord, decode_road_block};

fn main() {
    for st in ["TN", "NC"] {
        let url = format!("https://maps.mydatatimeline.com/maps/{st}.roads.ptiles");
        let file = match HttpSource::open(&url).map_err(|e| e.to_string()).and_then(|s| {
            PtilesFile::open(s).map_err(|e| e.to_string())
        }) {
            Ok(f) => f,
            Err(e) => {
                println!("{st}: unavailable: {e}");
                continue;
            }
        };

        let entries = file.index().to_vec();
        let mut types: BTreeMap<u8, usize> = BTreeMap::new();
        let (mut stranded, mut cells) = (0usize, 0usize);
        for entry in &entries {
            if entry.block_length == 0 {
                continue;
            }
            let block = match file.read_block(entry.h3_cell) {
                Ok(Some(b)) => b,
                _ => continue,
            };
            let (_, intersections) = match decode_road_block(&block, file.header().version) {
                Ok(v) => v,
                Err(_) => continue,
            };
            cells += 1;
            for i in &intersections {
                *types.entry(i.intersection_type).or_default() += 1;
                let (lat, lon) = (i.lat_micro as f64 / 1e5, i.lon_micro as f64 / 1e5);
                if cell_for_coord(lat, lon) != entry.h3_cell {
                    stranded += 1;
                }
            }
        }
        let t123: usize = [1u8, 2, 3].iter().map(|t| types.get(t).copied().unwrap_or(0)).sum();
        println!(
            "{st}: {cells} cells, types 1/2/3 = {t123}, type 4 = {}, all = {types:?}, stranded = {stranded}",
            types.get(&4).copied().unwrap_or(0)
        );
    }
}
