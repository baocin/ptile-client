//! Is a gap in a route a gap in the road data?
use ptiles_core::file::PtilesFile;
use ptiles_core::source::FileSource;

fn main() {
    let roads = std::env::args().nth(1).unwrap();
    let layer = ptiles_ffi::PtilesLayer::open(roads.clone()).unwrap();
    // The two ends of a 6 km jump seen in two independent routes.
    let a: (f64, f64) = std::env::args()
        .nth(2)
        .map(|s| {
            let mut it = s.split(',');
            (it.next().unwrap().parse().unwrap(), it.next().unwrap().parse().unwrap())
        })
        .unwrap_or((35.57954, -89.11288));
    let b = (a.0 + 0.0005, a.1 + 0.0005);
    // Does a tiny offset find a road the exact point did not? That would make
    // the miss an artefact of which cell the probe lands in, not a hole.
    for (dy, dx) in [(0.0, 0.0), (0.002, 0.0), (-0.002, 0.0), (0.0, 0.002), (0.0, -0.002)] {
        match layer.nearest_road(a.0 + dy, a.1 + dx) {
            Ok(Some(n)) => println!(
                "  offset {dy:+.3},{dx:+.3}: {} ({}) {:.0} m",
                n.name.clone().unwrap_or_else(|| "unnamed".into()), n.road_class, n.distance_m,
            ),
            other => println!("  offset {dy:+.3},{dx:+.3}: {other:?}"),
        }
    }
    for (label, lat, lon) in [("gap start", a.0, a.1), ("gap end", b.0, b.1)] {
        match layer.nearest_road(lat, lon) {
            Ok(Some(n)) => println!(
                "{label}: {} ({}) {:.0} m away, snapped {:.5},{:.5}",
                n.name.clone().unwrap_or_else(|| "unnamed".into()),
                n.road_class, n.distance_m, n.snapped_lat, n.snapped_lon,
            ),
            other => println!("{label}: {other:?}"),
        }
    }

    // If the road is straight there, points along the chord sit on it. A
    // 6 km hop is then honest geometry -- OSM only needs a vertex where a way
    // bends -- and not a hole in the data.
    println!("along the chord:");
    for step in 1..=5 {
        let t = step as f64 / 6.0;
        let lat = a.0 + (b.0 - a.0) * t;
        let lon = a.1 + (b.1 - a.1) * t;
        match layer.nearest_road(lat, lon) {
            Ok(Some(n)) => println!(
                "  {:.0}% along: {} ({}) {:.0} m away",
                t * 100.0,
                n.name.clone().unwrap_or_else(|| "unnamed".into()),
                n.road_class,
                n.distance_m,
            ),
            other => println!("  {:.0}% along: {other:?}", t * 100.0),
        }
    }

    // Walk the raw segments around the gap and find the longest hop between
    // consecutive vertices: if the data itself has a 6 km step, the route is
    // drawing exactly what it was given.
    let file = PtilesFile::open(FileSource::open(&roads).unwrap()).unwrap();
    let version = file.header().version;
    let mut worst: Option<(f64, String, [f64; 2], [f64; 2])> = None;
    let mut checked = 0usize;
    for cell in file.index().iter().map(|e| e.h3_cell).collect::<Vec<_>>() {
        let Some((clat, clon)) = ptiles_core::query::try_cell_center(cell) else { continue };
        if (clat - a.0).abs() > 0.15 || (clon - a.1).abs() > 0.15 {
            continue;
        }
        let Some(block) = file.read_block(cell).unwrap() else { continue };
        let Ok((segments, _)) = ptiles_core::decode_road_block(&block, version) else { continue };
        for seg in segments {
            checked += 1;
            for w in seg.coords.windows(2) {
                let d = haversine(w[0][1], w[0][0], w[1][1], w[1][0]);
                if worst.as_ref().is_none_or(|(best, _, _, _)| d > *best) {
                    worst = Some((
                        d,
                        format!("{} ({})", seg.name.clone().unwrap_or_else(|| "unnamed".into()), seg.road_class),
                        w[0], w[1],
                    ));
                }
            }
        }
    }
    println!("{checked} segments near the gap");
    if let Some((d, what, p, q)) = worst {
        println!(
            "longest hop between consecutive vertices: {:.0} m on {what}  ({:.5},{:.5} -> {:.5},{:.5})",
            d, p[1], p[0], q[1], q[0],
        );
    }
}

fn haversine(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let r = 6_371_000.0_f64;
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dp = (lat2 - lat1).to_radians();
    let dl = (lon2 - lon1).to_radians();
    let x = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * r * x.sqrt().asin()
}
