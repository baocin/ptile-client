//! What each routing failure actually means.
//!
//! "Offline route failed: Disconnected" is one message for two unrelated
//! situations: there is genuinely no road joining these two points, or there
//! is one and we did not load it. The first is a fact about the world and the
//! honest answer is to say so; the second is a fact about our corridor or our
//! packs and is fixable without the user doing anything. Telling them apart is
//! the difference between "no road goes there" and "download Georgia".
//!
//! Same for snapping. `EndNotSnapped` means the destination sits further from
//! any routable way than the snap radius allows -- which happens for a real
//! reason (a POI pinned on a building centroid, a trailhead reached by a
//! `track`) far more often than because the place is unreachable.
//!
//! These are synthetic graphs on purpose: each one isolates a single cause, so
//! a failure here names the mechanism rather than a coordinate in Tennessee.

#![cfg(feature = "std")]

use ptiles_core::{
    CorridorPrefs, RoadSegment, RouteFailure, RoutePrefs, RouteProfile, route_in_corridor,
    route_roads_diagnostic,
};

fn seg(class: &str, coords: Vec<[f64; 2]>) -> RoadSegment {
    RoadSegment {
        osm_id: 0,
        road_class: class.to_string(),
        coords,
        name: None,
        ref_tag: None,
        oneway: None,
        speed_limit_kmh: None,
        lanes: None,
        surface: None,
        bridge_tunnel: None,
    }
}

/// A straight run of road, `n` vertices from `(lat, lon)` heading east.
fn run(class: &str, lat: f64, lon: f64, n: usize, step: f64) -> RoadSegment {
    seg(class, (0..n).map(|i| [lon + i as f64 * step, lat]).collect())
}

const PREFS: RoutePrefs = RoutePrefs {
    profile: RouteProfile::Driving,
    avoid_highways: false,
    avoid_intersections: false,
};

// --- the two meanings of Disconnected -------------------------------------

/// Genuinely disconnected: two roads that do not meet, and no third road
/// exists anywhere. Widening the corridor cannot help, and the honest answer
/// is that there is no route.
#[test]
fn disconnected_with_no_joining_road_is_a_fact_about_the_world() {
    let west = run("residential", 36.0, -86.000, 4, 0.001);
    let east = run("residential", 36.0, -85.900, 4, 0.001);

    let failure = route_roads_diagnostic(
        &[west, east], &[], 36.0, -86.0, 36.0, -85.9, 60.0, PREFS,
    );

    assert_eq!(failure, Err(RouteFailure::Disconnected));
}

/// The same failure, produced instead by loading only part of the road
/// network. The joining road exists; it was simply not in the segments handed
/// to the router. This is the case that a wider corridor rescues, and the
/// reason `route_in_corridor` retries rather than reporting straight away.
#[test]
fn disconnected_from_a_partial_load_is_rescued_by_loading_more() {
    let west = run("residential", 36.0, -86.000, 4, 0.001);
    let link = run("residential", 36.0, -85.997, 100, 0.001);
    let east = run("residential", 36.0, -85.900, 4, 0.001);

    let without_link = route_roads_diagnostic(
        &[west.clone(), east.clone()], &[], 36.0, -86.0, 36.0, -85.9, 60.0, PREFS,
    );
    let with_link = route_roads_diagnostic(
        &[west, link, east], &[], 36.0, -86.0, 36.0, -85.9, 60.0, PREFS,
    );

    assert_eq!(without_link, Err(RouteFailure::Disconnected));
    assert!(with_link.is_ok(), "the link road joins them: {with_link:?}");
}

/// A corridor that returns nothing at all is not "disconnected" -- it is a
/// pack that is missing or does not cover the ground. The distinct failure is
/// what lets the app say "download the state" instead of "no route exists".
#[test]
fn an_empty_corridor_is_a_missing_pack_not_a_missing_road() {
    let failure = route_roads_diagnostic(&[], &[], 36.0, -86.0, 36.1, -86.1, 60.0, PREFS);

    assert_eq!(failure, Err(RouteFailure::EmptyGraph));
}

/// Roads on both sides but a hole in the middle, which is what a route across
/// a state line looks like when only one state's pack is installed: the near
/// half loads, the far half is silent, and the failure is indistinguishable
/// from a genuine gap without knowing which cells returned nothing.
#[test]
fn a_gap_where_the_next_pack_would_be_looks_exactly_like_a_genuine_gap() {
    let home_state = run("primary", 36.0, -86.00, 30, 0.001);
    // Nothing between -85.97 and -85.90: the neighbouring pack is not
    // installed, so its cells decode to no segments at all.
    let far_side = run("primary", 36.0, -85.90, 30, 0.001);

    let failure = route_roads_diagnostic(
        &[home_state, far_side], &[], 36.0, -86.0, 36.0, -85.88, 60.0, PREFS,
    );

    assert_eq!(
        failure,
        Err(RouteFailure::Disconnected),
        "the router cannot tell a missing pack from a missing road; the caller has to",
    );
}

// --- snapping --------------------------------------------------------------

#[test]
fn a_start_further_than_the_snap_radius_says_so() {
    let road = run("residential", 36.0, -86.0, 4, 0.001);

    let failure = route_roads_diagnostic(
        &[road], &[], 36.05, -86.0, 36.0, -86.002, 60.0, PREFS,
    );

    assert_eq!(failure, Err(RouteFailure::StartNotSnapped));
}

#[test]
fn an_end_further_than_the_snap_radius_says_so() {
    let road = run("residential", 36.0, -86.0, 4, 0.001);

    let failure = route_roads_diagnostic(
        &[road], &[], 36.0, -86.0, 36.05, -86.0, 60.0, PREFS,
    );

    assert_eq!(failure, Err(RouteFailure::EndNotSnapped));
}

/// The same request succeeds with a wider radius, which is why an unsnapped
/// endpoint is worth retrying rather than reporting. 220 m is a normal
/// distance between a POI pinned on a building and the road serving it.
#[test]
fn a_wider_snap_radius_rescues_an_endpoint_beside_the_road() {
    let road = run("residential", 36.0, -86.0, 6, 0.001);
    let two_hundred_m_north = 36.0 + 0.0018;

    let tight = route_roads_diagnostic(
        &[road.clone()], &[], 36.0, -86.0, two_hundred_m_north, -85.998, 60.0, PREFS,
    );
    let generous = route_roads_diagnostic(
        &[road], &[], 36.0, -86.0, two_hundred_m_north, -85.998, 400.0, PREFS,
    );

    assert_eq!(tight, Err(RouteFailure::EndNotSnapped));
    assert!(generous.is_ok(), "200 m from the road is reachable: {generous:?}");
}

/// A trailhead served by a forest track is unroutable by car not because it is
/// far away but because `track` is not a driving class. The failure is
/// `EndNotSnapped`, and no snap radius fixes it -- the class filter does.
#[test]
fn a_destination_served_only_by_a_track_never_snaps_for_driving() {
    let track = run("track", 36.0, -86.0, 6, 0.001);

    let driving = route_roads_diagnostic(
        &[track.clone()], &[], 36.0, -86.0, 36.0, -85.995, 2_000.0, PREFS,
    );
    let on_foot = route_roads_diagnostic(
        &[track], &[], 36.0, -86.0, 36.0, -85.995, 2_000.0,
        RoutePrefs { profile: RouteProfile::Foot, ..PREFS },
    );

    assert_eq!(driving, Err(RouteFailure::EmptyGraph), "no driving edge survives the filter");
    assert!(on_foot.is_ok(), "the same track walks fine: {on_foot:?}");
}

// --- what the corridor loader does about it --------------------------------

/// The widening retry only fires for `Disconnected`. An unsnapped endpoint is
/// reported immediately, because a wider corridor adds roads further away, not
/// closer, and cannot bring one nearer the destination.
#[test]
fn the_corridor_retries_a_disconnection_and_not_an_unsnapped_endpoint() {
    let mut fetches = 0;
    let err = route_in_corridor(
        36.0, -86.0, 36.0, -85.9, PREFS, &CorridorPrefs::default(),
        |_cells: &[u64]| -> Result<Vec<RoadSegment>, ()> {
            fetches += 1;
            Ok(vec![
                run("residential", 36.0, -86.000, 4, 0.001),
                run("residential", 36.0, -85.900, 4, 0.001),
            ])
        },
    );
    assert!(err.is_err(), "two islands cannot be joined");
    let disconnected_fetches = fetches;

    fetches = 0;
    let _ = route_in_corridor(
        36.0, -86.0, 36.05, -85.9, PREFS, &CorridorPrefs::default(),
        |_cells: &[u64]| -> Result<Vec<RoadSegment>, ()> {
            fetches += 1;
            Ok(vec![run("residential", 36.0, -86.0, 4, 0.001)])
        },
    );

    assert!(
        disconnected_fetches > fetches,
        "a disconnection is retried wider ({disconnected_fetches} fetches), \
         an unsnapped endpoint is not ({fetches})",
    );
}
