//! Route many random pairs and check the answers, not just that they exist.
//!
//! The earlier measurement had two weaknesses: every route started from the
//! same point, and "routed" counted any returned path. A route can come back
//! and still be wrong -- starting somewhere else, jumping a gap, doubling the
//! distance, or claiming a speed nothing drives.
use ptiles_core::file::PtilesFile;
use ptiles_core::source::FileSource;

fn main() {
    let roads = std::env::args().nth(1).unwrap();
    let business = std::env::args().nth(2).unwrap();
    let pairs: usize = std::env::args().nth(3).map_or(100, |s| s.parse().unwrap());

    let file = PtilesFile::open(FileSource::open(&business).unwrap()).unwrap();
    let version = file.header().version;
    let cells: Vec<u64> = file.index().iter().map(|e| e.h3_cell).collect();
    // Spread over the whole index, two independent strides so origins and
    // destinations are not drawn from the same neighbourhoods.
    let mut places: Vec<(f64, f64)> = Vec::new();
    for cell in cells.iter().step_by((cells.len() / (pairs * 3)).max(1)) {
        let Some(block) = file.read_block(*cell).unwrap() else { continue };
        let Ok(records) = ptiles_core::decode_business_versioned(&block, version, *cell) else {
            continue;
        };
        if let Some(b) = records.into_iter().find(|b| {
            !b.name.trim().is_empty()
                && (34.98..=36.68).contains(&b.lat)
                && (-90.31..=-81.65).contains(&b.lon)
        }) {
            places.push((b.lat, b.lon));
        }
    }

    let layer = ptiles_ffi::PtilesLayer::open(roads).unwrap();
    let probe = layer.clone();
    let stack = ptiles_ffi::PtilesStack::with_layers(
        Some(layer), None, None, None, None, None, None, None,
    );

    let (mut ok, mut failed) = (0usize, 0usize);
    let mut kinds: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let mut fail_km: Vec<f64> = Vec::new();
    let mut attempts: Vec<(f64, bool)> = Vec::new();
    let mut ratios: Vec<f64> = Vec::new();
    let mut problems: Vec<String> = Vec::new();
    let mut worst_gap = 0.0_f64;
    let mut slowest = f64::MAX;
    let mut fastest = 0.0_f64;

    // Deterministic pseudo-random pairing: a stride coprime with the length.
    let n = places.len();
    for i in 0..pairs.min(n) {
        let from = places[(i * 7) % n];
        let to = places[(i * 31 + 11) % n];
        let direct = haversine(from.0, from.1, to.0, to.1) / 1000.0;
        if direct < 1.0 { continue }
        match stack.offline_route(
            from.0, from.1, to.0, to.1,
            ptiles_ffi::OfflineRouteMode::Driving, false, false, 0.0,
        ) {
            Err(e) => {
                failed += 1;
                let m = e.to_string().to_lowercase().replace(' ', "");
                let kind = ["startnotsnapped", "endnotsnapped", "disconnected", "emptygraph",
                            "boundingbox", "nodebudget"]
                    .iter()
                    .find(|k| m.contains(*k))
                    .map_or("other".to_string(), |k| k.to_string());
                *kinds.entry(kind).or_default() += 1;
                fail_km.push(direct);
                attempts.push((direct, false));
            }
            Ok(r) => {
                ok += 1;
                attempts.push((direct, true));
                let km = r.distance_m / 1000.0;
                let path = &r.path;
                // 1. Does it start and end where it was asked to?
                let start_off = haversine(from.0, from.1, path[0].lat, path[0].lon);
                let last = &path[path.len() - 1];
                let end_off = haversine(to.0, to.1, last.lat, last.lon);
                if start_off > 1_500.0 {
                    problems.push(format!("starts {start_off:.0} m from the origin ({direct:.0} km trip)"));
                }
                if end_off > 1_500.0 {
                    problems.push(format!("ends {end_off:.0} m from the destination ({direct:.0} km trip)"));
                }
                // 2. Is it contiguous, or does it teleport?
                let mut gap = 0.0_f64;
                for w in path.windows(2) {
                    let d = haversine(w[0].lat, w[0].lon, w[1].lat, w[1].lon);
                    if d > gap { gap = d }
                }
                if gap > worst_gap { worst_gap = gap }
                // A long hop between consecutive points is not by itself a
                // fault: OSM only needs a vertex where a way bends, so a
                // straight motorway legitimately runs 6 km between vertices.
                // What matters is whether the straight line we draw actually
                // lies on a road. Sample the chord and ask.
                if gap > 2_000.0 {
                    let at = path
                        .windows(2)
                        .max_by(|x, y| {
                            haversine(x[0].lat, x[0].lon, x[1].lat, x[1].lon)
                                .partial_cmp(&haversine(y[0].lat, y[0].lon, y[1].lat, y[1].lon))
                                .unwrap()
                        })
                        .unwrap();
                    let mut worst_off = 0.0_f64;
                    let mut nothing_there = 0;
                    let (mut wlat, mut wlon) = (0.0, 0.0);
                    for step in 1..=4 {
                        let t = step as f64 / 5.0;
                        let lat = at[0].lat + (at[1].lat - at[0].lat) * t;
                        let lon = at[0].lon + (at[1].lon - at[0].lon) * t;
                        match probe.nearest_road(lat, lon) {
                            Ok(Some(n)) => {
                                if n.distance_m > worst_off {
                                    worst_off = n.distance_m;
                                    wlat = lat;
                                    wlon = lon;
                                }
                            }
                            _ => {
                                nothing_there += 1;
                                wlat = lat;
                                wlon = lon;
                            }
                        }
                    }
                    if nothing_there > 0 {
                        problems.push(format!(
                            "{:.1} km hop with no road under {nothing_there}/4 samples ({direct:.0} km trip) at {wlat:.5},{wlon:.5}",
                            gap / 1000.0,
                        ));
                    } else if worst_off > 150.0 {
                        problems.push(format!(
                            "{:.1} km hop drawn {:.0} m off the nearest way ({direct:.0} km trip) at {wlat:.5},{wlon:.5}",
                            gap / 1000.0, worst_off,
                        ));
                    }
                }
                // 3. Is the distance physically possible and not absurd?
                if km + 0.5 < direct {
                    problems.push(format!("shorter than the straight line: {km:.1} km for {direct:.1} km"));
                }
                let ratio = km / direct;
                ratios.push(ratio);
                if ratio > 3.0 {
                    problems.push(format!("{ratio:.1}x the straight line ({direct:.0} km -> {km:.0} km)"));
                }
                // 4. Does the claimed speed correspond to driving?
                let kmh = km / (r.duration_s / 3600.0);
                if kmh < slowest { slowest = kmh }
                if kmh > fastest { fastest = kmh }
                if !(5.0..=130.0).contains(&kmh) {
                    problems.push(format!("average speed {kmh:.0} km/h over {km:.0} km"));
                }
            }
        }
    }

    // Success by distance, since the corridor has a length limit and lumping
    // a 600 km trip in with a 20 km one hides which is which.
    println!("by distance:");
    for (lo, hi) in [(0.0, 50.0), (50.0, 150.0), (150.0, 300.0), (300.0, 400.0), (400.0, 9_999.0)] {
        let tried = attempts.iter().filter(|(d, _)| *d >= lo && *d < hi).count();
        let won = attempts.iter().filter(|(d, w)| *d >= lo && *d < hi && *w).count();
        if tried > 0 {
            println!(
                "  {lo:>5.0}-{hi:<5.0} km: {won:>3}/{tried:<3} routed ({:.0}%)",
                won as f64 / tried as f64 * 100.0,
            );
        }
    }

    ratios.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pick = |q: f64| ratios.get(((ratios.len() as f64 - 1.0) * q) as usize).copied().unwrap_or(0.0);
    println!("{} pairs: {ok} routed, {failed} failed", ok + failed);
    println!(
        "detour ratio  median {:.2}x  p90 {:.2}x  worst {:.2}x",
        pick(0.5), pick(0.9), ratios.last().copied().unwrap_or(0.0),
    );
    println!("average speed {slowest:.0}-{fastest:.0} km/h; largest gap between points {:.0} m", worst_gap);
    for (kind, count) in &kinds {
        println!("  failure {kind}: {count}");
    }
    fail_km.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if !fail_km.is_empty() {
        println!(
            "  failed trips span {:.0}-{:.0} km, median {:.0} km",
            fail_km[0], fail_km[fail_km.len() - 1], fail_km[fail_km.len() / 2],
        );
    }
    println!("{} suspect routes", problems.len());
    for p in problems.iter().take(12) {
        println!("   {p}");
    }
}

fn haversine(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let r = 6_371_000.0_f64;
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dp = (lat2 - lat1).to_radians();
    let dl = (lon2 - lon1).to_radians();
    let a = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * r * a.sqrt().asin()
}
