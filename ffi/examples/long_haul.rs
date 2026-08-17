//! Long routes: the length of a state, and across a state line.
//!
//! `cargo run -p ptiles-ffi --example long_haul --release -- <roads dir> <TN roads path>`
use std::sync::Arc;

struct Run {
    label: &'static str,
    from: (f64, f64),
    to: (f64, f64),
    pack: String,
}

fn main() {
    let tn = std::env::args().nth(1).unwrap();
    let runs = vec![
        // Within Tennessee, increasingly long. The state is ~700 km end to end,
        // so this is a full day's drive at the top.
        Run { label: "Jackson -> Nashville", from: (35.73377, -88.03220), to: (36.16270, -86.78160), pack: tn.clone() },
        Run { label: "Nashville -> Chattanooga", from: (36.16270, -86.78160), to: (35.04560, -85.30970), pack: tn.clone() },
        Run { label: "Memphis -> Nashville", from: (35.14950, -90.04900), to: (36.16270, -86.78160), pack: tn.clone() },
        Run { label: "Nashville -> Knoxville", from: (36.16270, -86.78160), to: (35.96060, -83.92070), pack: tn.clone() },
        Run { label: "Chattanooga -> Bristol", from: (35.04560, -85.30970), to: (36.59510, -82.18870), pack: tn.clone() },
        Run { label: "Memphis -> Knoxville", from: (35.14950, -90.04900), to: (35.96060, -83.92070), pack: tn.clone() },
        Run { label: "Memphis -> Bristol (end to end)", from: (35.14950, -90.04900), to: (36.59510, -82.18870), pack: tn.clone() },
        // Over the line with only the near state's pack, which is what a user
        // meets driving out of the state they downloaded.
        Run { label: "Clarksville -> Hopkinsville KY (30 km)", from: (36.52980, -87.35950), to: (36.86560, -87.48860), pack: tn.clone() },
        Run { label: "Nashville -> Bowling Green KY", from: (36.16270, -86.78160), to: (36.99030, -86.44360), pack: tn.clone() },
        Run { label: "Memphis -> Southaven MS (15 km)", from: (35.14950, -90.04900), to: (34.98900, -90.01260), pack: tn.clone() },
    ];

    for run in runs {
        let layer: Option<Arc<ptiles_ffi::PtilesLayer>> =
            match ptiles_ffi::PtilesLayer::open(run.pack.clone()) {
                Ok(l) => Some(l),
                Err(e) => {
                    println!("{:>46}  cannot open: {e}", run.label);
                    None
                }
            };
        let Some(layer) = layer else { continue };
        let stack = ptiles_ffi::PtilesStack::with_layers(
            Some(layer), None, None, None, None, None, None, None,
        );
        let km = haversine(run.from.0, run.from.1, run.to.0, run.to.1) / 1000.0;
        let started = std::time::Instant::now();
        let result = stack.offline_route(
            run.from.0, run.from.1, run.to.0, run.to.1,
            ptiles_ffi::OfflineRouteMode::Driving, false, false, 0.0,
        );
        let took = started.elapsed().as_secs_f64();
        match result {
            Ok(r) => println!(
                "{:>46}  {km:5.0} km -> {:5.0} km ({:.2}x) {:>4.0} min  {took:5.1}s",
                run.label, r.distance_m / 1000.0, r.distance_m / 1000.0 / km, r.duration_s / 60.0,
            ),
            Err(e) => {
                let m = e.to_string();
                let head = m.split(';').next().unwrap_or(&m);
                let head = head.strip_prefix("bad bounding box: ").unwrap_or(head);
                println!("{:>46}  {km:5.0} km -> {head}  {took:5.1}s", run.label);
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
