//! Trace one route: what each leg asks for and what it gets.
fn main() {
    let roads = std::env::args().nth(1).unwrap();
    let layer = ptiles_ffi::PtilesLayer::open(roads).unwrap();
    let stack = ptiles_ffi::PtilesStack::with_layers(
        Some(layer.clone()), None, None, None, None, None, None, None,
    );
    // Jackson -> Nashville, the ordinary 200 km drive.
    let legs = [
        ("whole route", 35.73377, -88.03220, 36.16270, -86.78160),
        ("first half", 35.73377, -88.03220, 35.94823, -87.40690),
        ("second half", 35.94823, -87.40690, 36.16270, -86.78160),
        ("first quarter", 35.73377, -88.03220, 35.84100, -87.71955),
        ("short hop 30km", 35.73377, -88.03220, 35.85000, -87.75000),
    ];
    for (label, la1, lo1, la2, lo2) in legs {
        let km = haversine(la1, lo1, la2, lo2) / 1000.0;
        let result = stack.offline_route(
            la1, lo1, la2, lo2, ptiles_ffi::OfflineRouteMode::Driving, false, false, 0.0,
        );
        match result {
            Ok(r) => println!(
                "{label:>16} {km:6.1} km  ->  routed {:.1} km, {} segments decoded",
                r.distance_m / 1000.0, r.decoded_segments,
            ),
            Err(e) => {
                let m = e.to_string();
                let short = m.split(';').next().unwrap_or(&m);
                println!("{label:>16} {km:6.1} km  ->  {short}");
            }
        }
        // What the endpoints snap to, which decides whether a leg can start.
        for (which, la, lo) in [("start", la1, lo1), ("end", la2, lo2)] {
            match layer.nearest_road(la, lo) {
                Ok(Some(n)) => println!(
                    "                    {which}: {} ({}) {:.0} m away",
                    n.name.unwrap_or_else(|| "unnamed".into()), n.road_class, n.distance_m,
                ),
                _ => println!("                    {which}: nothing near"),
            }
        }
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
