//! How often does routing fail on a real pack, and which failure is it?
//!
//! Destinations are real businesses, so the endpoints are the ones a user
//! actually picks -- POIs pinned on building centroids and car parks -- not
//! points snapped to the road network first, which would flatter the result.
use ptiles_core::file::PtilesFile;
use ptiles_core::source::FileSource;
use std::collections::BTreeMap;

fn main() {
    let roads = std::env::args().nth(1).unwrap();
    let business = std::env::args().nth(2).unwrap();
    let sample: usize = std::env::args().nth(3).map_or(200, |s| s.parse().unwrap());

    // Spread destinations over the whole state rather than one city: every
    // Nth record across the index.
    let file = PtilesFile::open(FileSource::open(&business).unwrap()).unwrap();
    let version = file.header().version;
    let cells: Vec<u64> = file.index().iter().map(|e| e.h3_cell).collect();
    let step = (cells.len() / sample).max(1);
    let mut targets: Vec<(String, f64, f64)> = Vec::new();
    for cell in cells.iter().step_by(step) {
        let Some(block) = file.read_block(*cell).unwrap() else { continue };
        let Ok(records) = ptiles_core::decode_business_versioned(&block, version, *cell) else {
            continue;
        };
        // Inside the state the pack covers. 1,670 records in TN.business sit
        // outside it -- an 8,000 km "destination" is a bad record, not a
        // routing failure, and counting it as one flatters nothing.
        if let Some(b) = records.into_iter().find(|b| {
            !b.name.trim().is_empty()
                && (34.98..=36.68).contains(&b.lat)
                && (-90.31..=-81.65).contains(&b.lon)
        }) {
            targets.push((b.name, b.lat, b.lon));
        }
        if targets.len() >= sample { break }
    }

    let roads_layer = ptiles_ffi::PtilesLayer::open(roads).unwrap();
    let stack = ptiles_ffi::PtilesStack::with_layers(
        Some(roads_layer.clone()), None, None, None, None, None, None, None,
    );
    // Jackson, TN: where the app's fallback anchor sits.
    let (from_lat, from_lon) = (35.73377, -88.03220);
    let mut outcomes: BTreeMap<String, usize> = BTreeMap::new();
    let mut failures: Vec<(String, f64, String)> = Vec::new();
    for (name, lat, lon) in &targets {
        let km = haversine(from_lat, from_lon, *lat, *lon) / 1000.0;
        match route_split(&stack, &roads_layer, from_lat, from_lon, *lat, *lon, 3) {
            Ok(_) => *outcomes.entry("routed".into()).or_default() += 1,
            Err(e) => {
                let kind = classify(&e);
                *outcomes.entry(kind.clone()).or_default() += 1;
                if failures.len() < 2000 { failures.push((name.clone(), km, kind)) }
            }
        }
    }

    let total = targets.len();
    println!("{total} destinations from Jackson, TN");
    for (kind, count) in &outcomes {
        println!("  {kind}: {count} ({:.0}%)", *count as f64 / total as f64 * 100.0);
    }
    // Does distance predict failure?
    for band in [(0.0, 25.0), (25.0, 75.0), (75.0, 150.0), (150.0, 1000.0)] {
        let in_band: Vec<&(String, f64, String)> =
            failures.iter().filter(|(_, km, _)| *km >= band.0 && *km < band.1).collect();
        let attempted = targets
            .iter()
            .filter(|(_, la, lo)| {
                let km = haversine(from_lat, from_lon, *la, *lo) / 1000.0;
                km >= band.0 && km < band.1
            })
            .count();
        if attempted > 0 {
            println!(
                "  {:.0}-{:.0} km: {} of {} failed ({:.0}%)",
                band.0, band.1, in_band.len(), attempted,
                in_band.len() as f64 / attempted as f64 * 100.0,
            );
        }
    }
    println!("--- a sample of failures ---");
    for (name, km, kind) in failures.iter().take(15) {
        println!("  {kind:>18}  {km:6.1} km  {name}");
    }
}

/// What the Android client does around `offline_route`: halve a leg that the
/// corridor refuses and route each half. Replicated here so the measurement
/// reflects what a user meets rather than the raw FFI.
fn route_split(
    stack: &std::sync::Arc<ptiles_ffi::PtilesStack>,
    roads: &std::sync::Arc<ptiles_ffi::PtilesLayer>,
    lat1: f64, lon1: f64, lat2: f64, lon2: f64,
    splits_left: u32,
) -> Result<(), String> {
    // The client's snap ladder: the profile default, then 500 m, then a km.
    let mut last = String::new();
    for snap in [0.0, 500.0, 1000.0] {
        match stack.offline_route(
            lat1, lon1, lat2, lon2, ptiles_ffi::OfflineRouteMode::Driving, false, false, snap,
        ) {
            Ok(_) => return Ok(()),
            Err(e) => {
                last = e.to_string();
                if !last.to_lowercase().replace(' ', "").contains("notsnapped") {
                    break;
                }
            }
        }
    }
    {
        {
            let message = last;
            let splittable = {
                let m = message.to_lowercase();
                m.contains("disconnected") || (m.contains("bounding box") && m.contains("cells"))
            };
            if splits_left == 0 || !splittable {
                return Err(message);
            }
            // Snapped, as the client does: a raw midpoint lands in a field
            // and fails to snap, which is a failure of the split and not of
            // the route.
            // Split on the arterial network, not on whatever lane is nearest
            // the geometric midpoint. A midpoint in a field snaps to a farm
            // track, and a leg that starts on a farm track is disconnected
            // from the highway the rest of the route needs.
            let (mut mid_lat, mut mid_lon) = ((lat1 + lat2) / 2.0, (lon1 + lon2) / 2.0);
            let mut best: Option<(f64, f64, f64)> = None;
            for (dy, dx) in [
                (0.0, 0.0), (0.02, 0.0), (-0.02, 0.0), (0.0, 0.02), (0.0, -0.02),
                (0.04, 0.0), (-0.04, 0.0), (0.0, 0.04), (0.0, -0.04),
                (0.02, 0.02), (-0.02, -0.02), (0.02, -0.02), (-0.02, 0.02),
            ] {
                let (probe_lat, probe_lon) = (mid_lat + dy, mid_lon + dx);
                if let Ok(Some(near)) = roads.nearest_road(probe_lat, probe_lon) {
                    let major = matches!(
                        near.road_class.as_str(),
                        "motorway" | "trunk" | "primary" | "secondary"
                    );
                    if !major { continue }
                    let off = dy.abs() + dx.abs();
                    if best.as_ref().is_none_or(|(_, _, b)| off < *b) {
                        best = Some((near.snapped_lat, near.snapped_lon, off));
                    }
                }
            }
            if let Some((la, lo, _)) = best {
                mid_lat = la;
                mid_lon = lo;
            } else if let Ok(Some(near)) = roads.nearest_road(mid_lat, mid_lon) {
                mid_lat = near.snapped_lat;
                mid_lon = near.snapped_lon;
            }
            route_split(stack, roads, lat1, lon1, mid_lat, mid_lon, splits_left - 1)?;
            route_split(stack, roads, mid_lat, mid_lon, lat2, lon2, splits_left - 1)
        }
    }
}

fn classify(message: &str) -> String {
    let m = message.to_lowercase();
    for kind in ["startnotsnapped", "endnotsnapped", "disconnected", "emptygraph", "bounding box", "nodebudget"] {
        if m.replace(' ', "").contains(kind) {
            return kind.to_string();
        }
    }
    format!("other: {message}")
}

fn haversine(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let r = 6_371_000.0_f64;
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dp = (lat2 - lat1).to_radians();
    let dl = (lon2 - lon1).to_radians();
    let a = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * r * a.sqrt().asin()
}
