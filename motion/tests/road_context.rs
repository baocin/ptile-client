//! The road-aware half of the classifier, against a real decoded roads block.
//!
//! `movement.rs`'s own tests build `RoadContext` by hand, so they prove the
//! branch logic but not that it ever fires on real data: the priors match on
//! OSM `highway` strings, and if the decoder produced anything else -- a
//! numeric class, a different vocabulary, a prefix -- every road branch would
//! be silently dead and the unit tests would still pass. This walks synthetic
//! traces along polylines taken out of `test-fixtures/golden/roads.block.bin`
//! (a real 109 KB block from `TN.roads.ptiles`, downtown Nashville, 3,552
//! features and 129 mapped intersections) and resolves them the way a caller
//! does: `nearest_road` -> `RoadContext::from_nearest` -> `classify`.
//!
//! Traces are synthesized from the block's own geometry rather than replayed
//! from a GPX file because no committed trace overlaps this cell -- and walking
//! a known road at a known speed is a sharper test anyway: the expected answer
//! is known exactly, so a wrong one is a real failure rather than an argument
//! about ground truth.
//!
//! Skips silently if the fixture is absent, matching `core/tests/prefix_sweep.rs`.

use ptiles_core::{
    decode_road_block, haversine_distance_m, nearest_intersection, nearest_road, Intersection,
    RoadSegment,
};

use ptiles_motion::{
    classify, AccelStats, DebounceConfig, MovementType, RoadContext, TrafficControl, Vote,
    VoteDebouncer,
};

/// Threshold `nearest_road` is called with here. The default is 50 m; 25 m is
/// tighter than the widest plausible snap error and keeps a parallel road on
/// the far side of a block from answering.
const SNAP_THRESHOLD_M: f64 = 25.0;

fn block() -> Option<Vec<u8>> {
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test-fixtures/golden/roads.block.bin"
    );
    std::fs::read(p).ok()
}

/// Decoded `(roads, intersections)` from the golden block. `decode_road_block`,
/// not `decode_roads`: the latter drops the trailing intersection table, which
/// is half of what this file tests.
fn decoded() -> Option<(Vec<RoadSegment>, Vec<Intersection>)> {
    let bytes = block()?;
    Some(decode_road_block(&bytes, 2).expect("golden roads block must decode"))
}

/// The longest polyline of a given class, so the synthesized trace has room to
/// run without leaving the way.
fn longest_of<'a>(roads: &'a [RoadSegment], class: &str) -> Option<&'a RoadSegment> {
    roads
        .iter()
        .filter(|r| r.road_class == class && r.coords.len() >= 2)
        .max_by_key(|r| r.coords.len())
}

/// Points spaced `speed_mps * step_s` apart along a `[lon, lat]` polyline,
/// returned as `(lat, lon)`. Linear interpolation between vertices: at the
/// tens-of-metres scale of a city block that is indistinguishable from a
/// great-circle path, and it puts the points *exactly* on the way, which is
/// what makes the expected snap distance ~0.
fn walk(coords: &[[f64; 2]], speed_mps: f64, step_s: f64, max_points: usize) -> Vec<(f64, f64)> {
    let step_m = speed_mps * step_s;
    let mut out = vec![(coords[0][1], coords[0][0])];
    let mut carry = 0.0;
    for w in coords.windows(2) {
        let (alon, alat) = (w[0][0], w[0][1]);
        let (blon, blat) = (w[1][0], w[1][1]);
        let seg = haversine_distance_m(alat, alon, blat, blon);
        if !seg.is_finite() || seg <= 0.0 {
            continue;
        }
        let mut travelled = step_m - carry;
        while travelled <= seg && out.len() < max_points {
            let f = travelled / seg;
            out.push((alat + (blat - alat) * f, alon + (blon - alon) * f));
            travelled += step_m;
        }
        carry = (seg - (travelled - step_m)) % step_m;
        if out.len() >= max_points {
            break;
        }
    }
    out
}

/// Resolve one point against the block and classify it at `speed_mps`.
fn vote_at(
    roads: &[RoadSegment],
    lat: f64,
    lon: f64,
    speed_mps: f64,
) -> (Vote, Option<RoadContext>) {
    let ctx = nearest_road(lat, lon, roads, SNAP_THRESHOLD_M)
        .and_then(|near| RoadContext::from_nearest(roads, &near));
    // Accuracy 5 m: a good fix, so the 30 m gate stays open. No accelerometer,
    // which is the interesting case -- the road context is the only thing that
    // can beat the speed bands.
    let vote = classify(Some(speed_mps), Some(5.0), ctx.as_ref(), &AccelStats::EMPTY);
    (vote, ctx)
}

fn majority(votes: &[Vote]) -> MovementType {
    let mut counts: Vec<(MovementType, usize)> = Vec::new();
    for v in votes {
        match counts.iter_mut().find(|(t, _)| *t == v.movement) {
            Some((_, n)) => *n += 1,
            None => counts.push((v.movement, 1)),
        }
    }
    counts
        .iter()
        .max_by_key(|(_, n)| *n)
        .map(|(t, _)| *t)
        .unwrap_or(MovementType::Unknown)
}

#[test]
fn the_golden_block_decodes_to_the_classes_the_priors_match_on() {
    let Some((roads, ints)) = decoded() else {
        eprintln!("skipping: test-fixtures/golden/roads.block.bin not present");
        return;
    };
    assert_eq!(roads.len(), 3552, "golden block feature count");
    assert_eq!(ints.len(), 129, "golden block intersection count");

    // Every string the priors key on has to exist in real decoded output. If
    // the encoder's vocabulary ever changes, the road branches go quietly dead
    // and only this assertion notices.
    for class in ["footway", "steps", "pedestrian", "residential", "service", "motorway"] {
        assert!(
            roads.iter().any(|r| r.road_class == class),
            "no {class} in the golden block -- the {class} prior would be dead code"
        );
    }
    // `is_highway` also matches any `*_link` ramp.
    assert!(
        roads.iter().any(|r| r.road_class.ends_with("_link")),
        "no ramp classes, so the *_link branch is untested against real data"
    );
    // And the intersection table has to carry the queueing types, or the
    // signal-sticky window can never trigger in the field.
    assert!(
        ints.iter().any(|i| matches!(i.intersection_type, 1 | 2 | 3)),
        "no signals/stop/give_way nodes: {:?}",
        ints.iter().map(|i| i.intersection_type).collect::<Vec<_>>()
    );
}

#[test]
fn a_stroll_on_a_real_footway_is_seen_as_walking() {
    // The payoff case. 1.3 m/s is below the tree's 2.2 m/s walking floor, so
    // speed alone reads it as Stationary -- exactly what
    // `gpx_replay.rs::speed_alone_cannot_see_a_stroll_but_can_see_a_jog` pins
    // on a real 94-minute walk. Snapped to a real footway, the same speed
    // votes Walking at 0.90.
    let Some((roads, _)) = decoded() else { return };
    let way = longest_of(&roads, "footway").expect("a footway in the block");
    let points = walk(&way.coords, 1.3, 5.0, 40);
    assert!(points.len() >= 10, "footway too short to walk: {} points", points.len());

    let mut votes = Vec::new();
    let mut snapped_to_footpath = 0;
    for (lat, lon) in &points {
        let (vote, ctx) = vote_at(&roads, *lat, *lon, 1.3);
        if let Some(c) = &ctx {
            if matches!(c.road_class.as_str(), "footway" | "path" | "pedestrian" | "steps") {
                snapped_to_footpath += 1;
            }
            assert!(
                c.distance_m < 2.0,
                "generated point sits on the way, so the snap should be ~0, got {:.1} m to {}",
                c.distance_m,
                c.road_class
            );
        }
        votes.push(vote);
    }
    assert!(
        snapped_to_footpath * 2 > points.len(),
        "only {snapped_to_footpath}/{} points snapped to a footpath class",
        points.len()
    );
    assert_eq!(
        majority(&votes),
        MovementType::Walking,
        "a 1.3 m/s trace along a real footway must read as walking"
    );

    // Control: the identical speeds with no road context see nothing.
    assert_eq!(
        classify(Some(1.3), Some(5.0), None, &AccelStats::EMPTY).movement,
        MovementType::Stationary,
        "without the road prior this stroll is invisible -- that is the point"
    );
}

#[test]
fn crawling_on_a_real_residential_street_is_seen_as_driving() {
    // 3 m/s (~7 mph) is a car in a parking lot or a school zone. Speed alone
    // calls it Walking; the vehicular prior corrects it.
    let Some((roads, _)) = decoded() else { return };
    let way = longest_of(&roads, "residential").expect("a residential street in the block");
    let points = walk(&way.coords, 3.0, 5.0, 30);

    let mut votes = Vec::new();
    for (lat, lon) in &points {
        votes.push(vote_at(&roads, *lat, *lon, 3.0).0);
    }
    assert_eq!(
        majority(&votes),
        MovementType::Driving,
        "a 3 m/s trace along a real residential street must read as driving"
    );
    assert_eq!(
        classify(Some(3.0), Some(5.0), None, &AccelStats::EMPTY).movement,
        MovementType::Walking,
        "without the road prior the same speed reads as walking"
    );
}

#[test]
fn highway_speed_on_a_real_motorway_is_driving_at_high_confidence() {
    let Some((roads, _)) = decoded() else { return };
    let way = longest_of(&roads, "motorway").expect("a motorway in the block");
    let points = walk(&way.coords, 25.0, 2.0, 20);

    let mut best = 0.0f64;
    let mut votes = Vec::new();
    for (lat, lon) in &points {
        let (vote, _) = vote_at(&roads, *lat, *lon, 25.0);
        best = best.max(vote.confidence);
        votes.push(vote);
    }
    assert_eq!(majority(&votes), MovementType::Driving);
    assert!(
        best >= 0.95,
        "the motorway prior should fire at 0.95 somewhere along a motorway, best was {best}"
    );
}

#[test]
fn a_real_traffic_signal_holds_a_stopped_car_in_driving() {
    // End to end on real map data: stop at a mapped signal node, keep voting
    // Stationary, and the debounced state must stay Driving well past the 150 s
    // plain vehicle window because the intersection stretched it to 5 minutes.
    let Some((roads, ints)) = decoded() else { return };
    let signal = ints
        .iter()
        .find(|i| i.intersection_type == 1)
        .expect("a traffic_signals node in the block");
    let [lon, lat] = signal.coords();

    let near = nearest_intersection(lat, lon, &ints, SNAP_THRESHOLD_M)
        .expect("the node is at its own coordinates, so it must resolve");
    let control = TrafficControl::from_nearest(&near);
    assert!(near.distance_m < 1.0, "distance to itself: {}", near.distance_m);
    assert!(
        control.holds_traffic(DebounceConfig::default().signal_radius_m),
        "a signals node inside the radius must hold traffic: {control:?}"
    );

    // Drive up to the light on whatever way it sits on, then stop.
    let (drive_vote, road) = vote_at(&roads, lat, lon, 14.0);
    assert_eq!(drive_vote.movement, MovementType::Driving, "road {road:?}");
    let stop_vote = classify(Some(0.0), Some(5.0), road.as_ref(), &AccelStats::EMPTY);
    assert_eq!(stop_vote.movement, MovementType::Stationary);

    let mut d = VoteDebouncer::new(DebounceConfig::default());
    let mut t = 0u64;
    for _ in 0..20 {
        d.tick_at(&drive_vote, t, Some(&control));
        t += 1000;
    }
    assert_eq!(d.current(), MovementType::Driving);
    // 200 s at the light: past the 150 s vehicle window, inside the 300 s
    // signal window.
    for _ in 0..200 {
        d.tick_at(&stop_vote, t, Some(&control));
        t += 1000;
    }
    assert_eq!(
        d.current(),
        MovementType::Driving,
        "a real signal must keep a 200 s stop from reading as an arrival"
    );
    // Same stop with the intersection table ignored: it commits, which is the
    // behavior the map data is there to override.
    let mut blind = VoteDebouncer::new(DebounceConfig::default());
    let mut t = 0u64;
    for _ in 0..20 {
        blind.tick(&drive_vote, t);
        t += 1000;
    }
    for _ in 0..200 {
        blind.tick(&stop_vote, t);
        t += 1000;
    }
    assert_eq!(blind.current(), MovementType::Stationary);
}

#[test]
fn off_road_points_lose_the_prior_rather_than_snapping_wrongly() {
    // 2 km off the block's geometry: nothing should resolve, and the classifier
    // must fall back to speed rather than reporting a stale or nearest-anything
    // road. This is the "prior absent" path that every foot trace in
    // gpx_replay.rs actually runs on.
    let Some((roads, ints)) = decoded() else { return };
    let (lat, lon) = (36.1665 + 0.02, -86.7832 + 0.02);
    assert!(nearest_road(lat, lon, &roads, SNAP_THRESHOLD_M).is_none());
    assert!(nearest_intersection(lat, lon, &ints, SNAP_THRESHOLD_M).is_none());
    let (vote, ctx) = vote_at(&roads, lat, lon, 1.3);
    assert!(ctx.is_none());
    assert_eq!(vote.movement, MovementType::Stationary);
}
