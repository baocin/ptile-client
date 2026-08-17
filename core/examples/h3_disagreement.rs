//! How far apart do the builder's h3 (Python) and the client's h3o land?
//!
//! The builder assigns each record a cell; this resolves the record's own
//! coordinates and compares. A disagreement matters only if the resolved cell
//! is NOT a neighbour of the stored one, because every real caller reads the
//! ring, not the single cell.
fn main() {
    let dir = "/mnt/core/kino/ptiles/data/v4/states";
    for abbr in ["AL", "TN", "NY", "CA", "TX", "MT"] {
        let path = format!("{dir}/{abbr}.address_v3.ptiles");
        let src = match ptiles_core::FileSource::open(&path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let f = ptiles_core::address::AddressFile::open(src).unwrap();
        let index = f.index().to_vec();
        let step = (index.len() / 60).max(1);
        let (mut n, mut mismatch, mut non_neighbour) = (0usize, 0usize, 0usize);
        let mut worst = 0.0f64;
        for e in index.iter().step_by(step) {
            if e.block_length == 0 {
                continue;
            }
            for r in f.addresses_in_cell(e.h3_cell).unwrap() {
                let (Some(lat), Some(lon)) = (r.lat, r.lon) else { continue };
                n += 1;
                let got = ptiles_core::cell_for_coord(lat, lon);
                if got == e.h3_cell {
                    continue;
                }
                mismatch += 1;
                // Is the cell we resolved to adjacent to the one that stored it?
                let ring = ptiles_core::neighbor_cells(e.h3_cell);
                if !ring.contains(&got) {
                    non_neighbour += 1;
                }
                let (clat, clon) = ptiles_core::cell_center(e.h3_cell);
                let d = ptiles_core::haversine_distance_m(lat, lon, clat, clon);
                if d > worst {
                    worst = d;
                }
            }
        }
        println!(
            "{abbr}: {n} records, {mismatch} disagree ({:.4}%), {non_neighbour} not adjacent, \
             worst distance to stored cell centre {worst:.0} m",
            100.0 * mismatch as f64 / n.max(1) as f64
        );
    }
}
