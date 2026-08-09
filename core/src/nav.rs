//! Following a route: the turn queue, and where you are on it.
//!
//! None of this needs the router changed. A route is a polyline; a turn is a
//! bearing change along it; snapping is `point_to_linestring_distance_m`,
//! which already reports which segment and where on it; and off-route is that
//! distance against a threshold. The road names come from the segments the
//! caller already loaded to route with.
//!
//! The one thing a GPS fix cannot give you is where you are *pointed* when you
//! are stopped or crawling, and `coords.heading` is absent or noise at exactly
//! those speeds. [`NavState::bearing_deg`] answers it from the route instead:
//! the bearing of the next [`LOOKAHEAD_M`] of road you are about to drive.
//! That is the vector to rotate a map by and to compare a fix against.

use alloc::string::String;
use alloc::vec::Vec;

use crate::camera::bearing_to;
use crate::proximity::{haversine_distance_m, point_to_linestring_distance_m};
use crate::roads::RoadSegment;

/// How far ahead the predicted bearing looks. Long enough that a single noisy
/// vertex does not swing it, short enough that it turns *with* you rather than
/// pointing at where the road goes after the bend.
pub const LOOKAHEAD_M: f64 = 60.0;

/// Smoothing window for measuring a turn. Bearings are taken between points
/// this far either side of a vertex, not between adjacent vertices: OSM
/// geometry has 3 m vertices on curves, and adjacent-vertex bearings turn a
/// smooth sweep into a dozen 8-degree "turns".
const TURN_WINDOW_M: f64 = 25.0;

/// Below this, a bearing change is the road bending, not a turn to announce.
pub const MIN_TURN_DEG: f64 = 25.0;

/// Two turns closer than this are one manoeuvre -- a staggered junction, or
/// the several small lefts a roundabout is made of.
const TURN_MERGE_M: f64 = 20.0;

/// How far off the line counts as off-route, before accuracy is considered.
/// A two-lane road with verges is ~12 m kerb to kerb and a phone fix is good
/// to ~5 m in the open, so 35 m is comfortably outside "on the road" without
/// firing on every underpass.
pub const OFF_ROUTE_M: f64 = 35.0;

/// Which way a manoeuvre goes. Named rather than a signed angle because the
/// phrasing, the icon and the voice line all key off the name, and each would
/// otherwise re-derive the thresholds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Maneuver {
    Depart,
    Continue,
    SlightLeft,
    Left,
    SharpLeft,
    SlightRight,
    Right,
    SharpRight,
    UTurn,
    Arrive,
}

impl Maneuver {
    /// The manoeuvre a signed bearing change describes. Positive is right
    /// (clockwise), matching compass bearings.
    pub fn from_delta(delta_deg: f64) -> Maneuver {
        let a = delta_deg.abs();
        let right = delta_deg > 0.0;
        if a >= 150.0 {
            Maneuver::UTurn
        } else if a >= 100.0 {
            if right { Maneuver::SharpRight } else { Maneuver::SharpLeft }
        } else if a >= 60.0 {
            if right { Maneuver::Right } else { Maneuver::Left }
        } else if a >= MIN_TURN_DEG {
            if right { Maneuver::SlightRight } else { Maneuver::SlightLeft }
        } else {
            Maneuver::Continue
        }
    }

    /// Lower case, stable, for a UI to key an icon or a phrase off.
    pub fn as_str(self) -> &'static str {
        match self {
            Maneuver::Depart => "depart",
            Maneuver::Continue => "continue",
            Maneuver::SlightLeft => "slight_left",
            Maneuver::Left => "left",
            Maneuver::SharpLeft => "sharp_left",
            Maneuver::SlightRight => "slight_right",
            Maneuver::Right => "right",
            Maneuver::SharpRight => "sharp_right",
            Maneuver::UTurn => "u_turn",
            Maneuver::Arrive => "arrive",
        }
    }
}

/// One entry in the turn queue.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Turn {
    pub maneuver: Maneuver,
    /// Signed bearing change, degrees. Positive is right.
    pub delta_deg: f64,
    /// Index into the route path where the manoeuvre happens.
    pub index: usize,
    /// Metres from the route start to this manoeuvre.
    pub along_m: f64,
    /// Where the manoeuvre is, `(lat, lon)`.
    pub lat: f64,
    pub lon: f64,
    /// The road being turned *onto*, when a segment was near enough to name
    /// it. `None` is honest: an unnamed service road is common, and inventing
    /// "turn onto the road" helps nobody.
    pub road_name: Option<String>,
    /// Route number of that road (`US-431`), when it carries one.
    pub road_ref: Option<String>,
    /// Its class (`residential`, `motorway_link`), which is how a caller tells
    /// a slip road from a street.
    pub road_class: Option<String>,
    /// Where to look to name this turn: a point on the road being joined,
    /// 15 m past the corner. At the corner itself both roads are equally
    /// near, so this is the disambiguation.
    ///
    /// Kept on the turn so naming does not have to happen when the route is
    /// built. A caller can leave every name empty, then fetch the one cell
    /// containing this point as the turn comes up and name it from that --
    /// which is one block read per turn instead of holding a whole
    /// corridor's roads in memory for the length of a drive.
    pub probe_lat: f64,
    pub probe_lon: f64,
}

/// Where you are on the route, and which way you are pointed.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct NavState {
    /// The fix, pulled onto the route.
    pub lat: f64,
    pub lon: f64,
    /// How far the raw fix was from the route.
    pub offset_m: f64,
    /// Distance travelled along the route to here.
    pub along_m: f64,
    /// Distance still to drive.
    pub remaining_m: f64,
    /// Bearing of the next [`LOOKAHEAD_M`] of route, degrees clockwise from
    /// north. The predicted heading: what to rotate the map to, and what to
    /// compare a GPS heading against.
    pub bearing_deg: f64,
    /// Index of the path vertex the snap landed on, to pass back next fix.
    pub index: usize,
    /// Index into the turn queue of the next manoeuvre, if any remain.
    pub next_turn: Option<usize>,
    /// Metres to that manoeuvre.
    pub distance_to_turn_m: f64,
    /// True when the fix is far enough off the line to doubt the route.
    /// One fix does not make a wrong turn -- see the doc comment on
    /// [`navigate`].
    pub off_route: bool,
}

/// Cumulative distance to each vertex of a path. Computed once per route and
/// passed back in, because recomputing it per GPS fix is the difference
/// between a constant-time update and walking the whole route at 1 Hz.
pub fn cumulative_m(path: &[[f64; 2]]) -> Vec<f64> {
    let mut out = Vec::with_capacity(path.len());
    let mut total = 0.0;
    out.push(0.0);
    for w in path.windows(2) {
        total += haversine_distance_m(w[0][1], w[0][0], w[1][1], w[1][0]);
        out.push(total);
    }
    out
}

/// Bearing at a vertex, measured across a window rather than between adjacent
/// points. Returns `(incoming, outgoing)`.
fn windowed_bearings(path: &[[f64; 2]], cum: &[f64], i: usize) -> Option<(f64, f64)> {
    let here = cum[i];
    // Walk back to the first point at least TURN_WINDOW_M behind, and forward
    // to the first at least that far ahead. Clamped to the ends.
    let mut back = i;
    while back > 0 && here - cum[back] < TURN_WINDOW_M {
        back -= 1;
    }
    let mut fwd = i;
    while fwd + 1 < path.len() && cum[fwd] - here < TURN_WINDOW_M {
        fwd += 1;
    }
    if back == i || fwd == i {
        return None;
    }
    let incoming = bearing_to(path[back][1], path[back][0], path[i][1], path[i][0]);
    let outgoing = bearing_to(path[i][1], path[i][0], path[fwd][1], path[fwd][0]);
    Some((incoming, outgoing))
}

/// Signed difference between two bearings, in `(-180, 180]`. Positive is a
/// turn to the right.
pub fn bearing_delta(from_deg: f64, to_deg: f64) -> f64 {
    let mut d = (to_deg - from_deg) % 360.0;
    if d > 180.0 {
        d -= 360.0;
    }
    if d <= -180.0 {
        d += 360.0;
    }
    d
}

/// The turns a route contains, in order.
///
/// Purely geometric: a turn is a bearing change along the path, measured
/// across [`TURN_WINDOW_M`] so a curve does not read as a sequence of small
/// turns. `roads` is only used to *name* the result -- the road nearest each
/// manoeuvre, within `name_radius_m` -- so passing an empty slice gives the
/// same turns with no names on them.
///
/// The first entry is always `Depart` and the last always `Arrive`, so a
/// caller can drive the whole queue without special-casing the ends.
pub fn turn_queue(path: &[[f64; 2]], roads: &[RoadSegment], name_radius_m: f64) -> Vec<Turn> {
    if path.len() < 2 {
        return Vec::new();
    }
    let cum = cumulative_m(path);
    let total = *cum.last().unwrap_or(&0.0);

    let probe_0 = probe_point(path, &cum, 0, PROBE_AHEAD_M);
    let mut turns = Vec::new();
    turns.push(Turn {
        maneuver: Maneuver::Depart,
        delta_deg: 0.0,
        index: 0,
        along_m: 0.0,
        lat: path[0][1],
        lon: path[0][0],
        road_name: None,
        road_ref: None,
        road_class: None,
        probe_lat: probe_0.0,
        probe_lon: probe_0.1,
    });

    for i in 1..path.len().saturating_sub(1) {
        let Some((incoming, outgoing)) = windowed_bearings(path, &cum, i) else {
            continue;
        };
        let delta = bearing_delta(incoming, outgoing);
        if delta.abs() < MIN_TURN_DEG {
            continue;
        }
        // Merge with the previous turn when they are the same manoeuvre a few
        // metres apart: one junction, not three.
        if let Some(prev) = turns.last_mut() {
            if prev.maneuver != Maneuver::Depart
                && cum[i] - prev.along_m < TURN_MERGE_M
                && (prev.delta_deg > 0.0) == (delta > 0.0)
            {
                prev.delta_deg += delta;
                prev.maneuver = Maneuver::from_delta(prev.delta_deg);
                continue;
            }
        }
        let (probe_lat, probe_lon) = probe_point(path, &cum, i, PROBE_AHEAD_M);
        turns.push(Turn {
            maneuver: Maneuver::from_delta(delta),
            delta_deg: delta,
            index: i,
            along_m: cum[i],
            lat: path[i][1],
            lon: path[i][0],
            road_name: None,
            road_ref: None,
            road_class: None,
            probe_lat,
            probe_lon,
        });
    }

    let last = path.len() - 1;
    turns.push(Turn {
        maneuver: Maneuver::Arrive,
        delta_deg: 0.0,
        index: last,
        along_m: total,
        lat: path[last][1],
        lon: path[last][0],
        road_name: None,
        road_ref: None,
        road_class: None,
        probe_lat: path[last][1],
        probe_lon: path[last][0],
    });

    // Name each manoeuvre after the road it turns *onto*: sample a little past
    // the turn, not at it, since the turn point sits on both roads.
    if !roads.is_empty() {
        for t in turns.iter_mut() {
            name_turn(t, roads, name_radius_m);
        }
    }
    turns
}

/// How far past a corner to sample when naming the turn.
const PROBE_AHEAD_M: f64 = 15.0;

/// Name a single turn from roads loaded near it. Returns true when it found
/// one.
///
/// The lazy half of [`turn_queue`]: build the queue with no roads at all, then
/// as each turn comes within announcing distance, read the one cell holding
/// its `probe_lat`/`probe_lon`, decode it, and call this. A drive then costs
/// one block per turn -- almost always already cached, since it is a cell the
/// route passes through -- instead of keeping every road in the corridor
/// alive for the whole trip.
///
/// Name turns *before* the first announcement, not during it: a manoeuvre
/// announced as "turn left" at 2 km and "turn left onto Broadway" at 200 m
/// reads as two different turns.
pub fn name_turn(turn: &mut Turn, roads: &[RoadSegment], radius_m: f64) -> bool {
    match nearest_named_road(turn.probe_lat, turn.probe_lon, roads, radius_m) {
        Some(r) => {
            turn.road_name = r.name.clone();
            turn.road_ref = r.ref_tag.clone();
            turn.road_class = Some(r.road_class.clone());
            true
        }
        None => false,
    }
}

/// A point `ahead_m` further along the path from vertex `i`, clamped to the
/// end. Returned `(lat, lon)`.
fn probe_point(path: &[[f64; 2]], cum: &[f64], i: usize, ahead_m: f64) -> (f64, f64) {
    let target = cum[i] + ahead_m;
    let mut j = i;
    while j + 1 < path.len() && cum[j] < target {
        j += 1;
    }
    (path[j][1], path[j][0])
}

/// The nearest road to a point within `radius_m`, preferring one that has a
/// name: an unnamed service stub often sits closer to a junction than the
/// street the driver is turning onto.
fn nearest_named_road<'a>(
    lat: f64,
    lon: f64,
    roads: &'a [RoadSegment],
    radius_m: f64,
) -> Option<&'a RoadSegment> {
    let mut best: Option<(&RoadSegment, f64)> = None;
    for r in roads {
        let Some((_, proj)) = point_to_linestring_distance_m(lat, lon, &r.coords) else {
            continue;
        };
        if proj.distance_m > radius_m {
            continue;
        }
        // A named road within the radius beats a nearer unnamed one; among
        // equals, nearest wins.
        let score = proj.distance_m + if r.name.is_some() { 0.0 } else { radius_m };
        match best {
            Some((_, b)) if b <= score => {}
            _ => best = Some((r, score)),
        }
    }
    best.map(|(r, _)| r)
}

/// Where a fix puts you on the route.
///
/// `cum` comes from [`cumulative_m`] for this path; `last_index` is the
/// `index` from the previous call, or 0 to start. The search is windowed
/// around it -- forward 500 m, back 100 m -- so a route that doubles back on
/// itself cannot snap to the wrong leg, and a long route costs the same per
/// fix as a short one. A fix that finds nothing acceptable in the window
/// falls back to searching the whole path, which is what recovers after a
/// tunnel or a long GPS gap.
///
/// `off_route` is a property of *this* fix, not a decision: a single bad fix
/// in a parking garage is not a wrong turn. A caller should require it on
/// several consecutive fixes before rerouting, and should scale its own
/// patience by how bad the accuracy is.
pub fn navigate(
    path: &[[f64; 2]],
    cum: &[f64],
    turns: &[Turn],
    lat: f64,
    lon: f64,
    accuracy_m: f64,
    last_index: usize,
) -> Option<NavState> {
    if path.len() < 2 || cum.len() != path.len() {
        return None;
    }
    let threshold = OFF_ROUTE_M.max(3.0 * accuracy_m.max(0.0));

    let (index, snapped, offset_m) = snap_windowed(path, cum, lat, lon, last_index, threshold)?;

    // Distance along: to the vertex the segment starts at, plus the bit of
    // that segment actually covered.
    let along_m = cum[index]
        + haversine_distance_m(path[index][1], path[index][0], snapped.0, snapped.1);
    let total = *cum.last().unwrap_or(&0.0);

    let bearing_deg = lookahead_bearing(path, cum, index, snapped, along_m);

    let next_turn = turns
        .iter()
        .position(|t| t.along_m > along_m + 1.0 && t.maneuver != Maneuver::Depart);
    let distance_to_turn_m = next_turn
        .map(|i| turns[i].along_m - along_m)
        .unwrap_or_else(|| (total - along_m).max(0.0));

    Some(NavState {
        lat: snapped.0,
        lon: snapped.1,
        offset_m,
        along_m,
        remaining_m: (total - along_m).max(0.0),
        bearing_deg,
        index,
        next_turn,
        distance_to_turn_m,
        off_route: offset_m > threshold,
    })
}

/// Snap within a window around `last_index`, falling back to the whole path.
/// Returns `(segment index, (lat, lon), distance)`.
fn snap_windowed(
    path: &[[f64; 2]],
    cum: &[f64],
    lat: f64,
    lon: f64,
    last_index: usize,
    threshold: f64,
) -> Option<(usize, (f64, f64), f64)> {
    let here = cum.get(last_index).copied().unwrap_or(0.0);
    let lo = cum.partition_point(|&c| c < here - 100.0).saturating_sub(1);
    let hi = cum.partition_point(|&c| c <= here + 500.0).min(path.len());

    let windowed = (lo + 1 < hi)
        .then(|| snap_range(path, lat, lon, lo, hi))
        .flatten();
    match windowed {
        Some(w) if w.2 <= threshold => Some(w),
        // Outside the window's reach: either genuinely off-route or back after
        // a gap. A full scan tells the two apart, and only runs when it must.
        other => snap_range(path, lat, lon, 0, path.len()).or(other),
    }
}

fn snap_range(
    path: &[[f64; 2]],
    lat: f64,
    lon: f64,
    lo: usize,
    hi: usize,
) -> Option<(usize, (f64, f64), f64)> {
    let slice = path.get(lo..hi)?;
    let (seg, proj) = point_to_linestring_distance_m(lat, lon, slice)?;
    Some((lo + seg, proj.snapped, proj.distance_m))
}

/// Bearing over the next [`LOOKAHEAD_M`] of route from the snapped point --
/// the predicted heading. Falls back to the final segment's bearing at the end
/// of the route, where there is nothing left to look ahead at.
fn lookahead_bearing(
    path: &[[f64; 2]],
    cum: &[f64],
    index: usize,
    snapped: (f64, f64),
    along_m: f64,
) -> f64 {
    let target = along_m + LOOKAHEAD_M;
    let mut j = index + 1;
    while j + 1 < path.len() && cum[j] < target {
        j += 1;
    }
    let ahead = path[j.min(path.len() - 1)];
    let b = bearing_to(snapped.0, snapped.1, ahead[1], ahead[0]);
    // Degenerate only when the snap landed exactly on the point ahead, which
    // happens at the very end; use the last real segment instead.
    if ahead[1] == snapped.0 && ahead[0] == snapped.1 && path.len() >= 2 {
        let n = path.len();
        return bearing_to(path[n - 2][1], path[n - 2][0], path[n - 1][1], path[n - 1][0]);
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// Metres per degree of longitude at latitude 36, which is what puts the
    /// landmarks in these fixtures where they are.
    const M_PER_DEG: f64 = 90_060.0;

    fn east(km: f64) -> f64 {
        -86.0 + (km * 1000.0) / M_PER_DEG
    }

    fn north(km: f64) -> f64 {
        36.0 + (km * 1000.0) / 111_320.0
    }

    /// East 1 km, then north 1 km: one right-angle left turn.
    fn l_shape() -> Vec<[f64; 2]> {
        let mut p = vec![];
        for i in 0..=10 {
            p.push([east(i as f64 * 0.1), 36.0]);
        }
        for i in 1..=10 {
            p.push([east(1.0), north(i as f64 * 0.1)]);
        }
        p
    }

    fn road(name: &str, class: &str, coords: Vec<[f64; 2]>) -> RoadSegment {
        RoadSegment {
            osm_id: 1,
            road_class: String::from(class),
            coords,
            name: Some(String::from(name)),
            ref_tag: None,
            oneway: None,
            speed_limit_kmh: None,
            lanes: None,
            surface: None,
            bridge_tunnel: None,
        }
    }

    #[test]
    fn a_straight_road_has_no_turns_between_depart_and_arrive() {
        let path: Vec<[f64; 2]> = (0..=20).map(|i| [east(i as f64 * 0.1), 36.0]).collect();
        let q = turn_queue(&path, &[], 30.0);
        assert_eq!(q.len(), 2, "{q:?}");
        assert_eq!(q[0].maneuver, Maneuver::Depart);
        assert_eq!(q[1].maneuver, Maneuver::Arrive);
        assert!((q[1].along_m - 2000.0).abs() < 20.0, "along {}", q[1].along_m);
    }

    #[test]
    fn a_right_angle_reads_as_one_left_turn_at_the_corner() {
        let path = l_shape();
        let q = turn_queue(&path, &[], 30.0);
        assert_eq!(q.len(), 3, "depart, one turn, arrive: {q:?}");
        let t = &q[1];
        assert_eq!(t.maneuver, Maneuver::Left);
        assert!(t.delta_deg < -80.0 && t.delta_deg > -100.0, "delta {}", t.delta_deg);
        assert!((t.along_m - 1000.0).abs() < 30.0, "along {}", t.along_m);
    }

    #[test]
    fn a_gentle_curve_is_not_a_sequence_of_turns() {
        // A 90-degree sweep over 500 m: a curve, not a manoeuvre. Adjacent
        // vertex bearings would call this ten separate turns.
        let mut path = vec![];
        for i in 0..=50 {
            let t = i as f64 / 50.0;
            let ang = t * core::f64::consts::FRAC_PI_2;
            path.push([east(0.5 * crate::math::sin(ang)), north(0.5 * (1.0 - crate::math::cos(ang)))]);
        }
        let q = turn_queue(&path, &[], 30.0);
        assert_eq!(q.len(), 2, "a curve is not a turn queue: {q:?}");
    }

    #[test]
    fn turns_are_named_after_the_road_they_join() {
        let path = l_shape();
        let roads = vec![
            road("Broadway", "residential", (0..=10).map(|i| [east(i as f64 * 0.1), 36.0]).collect()),
            road("4th Avenue", "residential", (0..=10).map(|i| [east(1.0), north(i as f64 * 0.1)]).collect()),
        ];
        let q = turn_queue(&path, &roads, 30.0);
        assert_eq!(q[1].maneuver, Maneuver::Left);
        assert_eq!(
            q[1].road_name.as_deref(),
            Some("4th Avenue"),
            "named for the road turned onto, not the one left behind"
        );
        assert_eq!(q[0].road_name.as_deref(), Some("Broadway"), "departing on Broadway");
    }

    #[test]
    fn naming_a_turn_late_gives_the_same_answer_as_naming_it_early() {
        // The whole point of the lazy path: a queue built with no roads at
        // all, named one turn at a time from a cell fetched as that turn comes
        // up, must not disagree with the queue built with every road in hand.
        let path = l_shape();
        let roads = vec![
            road("Broadway", "residential", (0..=10).map(|i| [east(i as f64 * 0.1), 36.0]).collect()),
            road("4th Avenue", "residential", (0..=10).map(|i| [east(1.0), north(i as f64 * 0.1)]).collect()),
        ];
        let eager = turn_queue(&path, &roads, 30.0);

        let mut lazy = turn_queue(&path, &[], 30.0);
        assert!(lazy.iter().all(|t| t.road_name.is_none()), "starts unnamed");
        for t in lazy.iter_mut() {
            // A caller would fetch only the cell holding the probe; here the
            // whole set stands in for it, which is the same input.
            assert!(name_turn(t, &roads, 30.0));
        }
        assert_eq!(lazy, eager);
    }

    #[test]
    fn the_probe_point_sits_on_the_road_being_joined_not_the_one_left() {
        let path = l_shape();
        let q = turn_queue(&path, &[], 30.0);
        let turn = &q[1];
        // The corner is at (36.0, east(1.0)); the probe must be north of it,
        // on the new leg, or naming picks whichever road is nearer the corner.
        assert!(
            turn.probe_lat > turn.lat + 0.0001,
            "probe {:?} should be up the new leg from the corner {:?}",
            (turn.probe_lat, turn.probe_lon),
            (turn.lat, turn.lon)
        );
        assert!((turn.probe_lon - turn.lon).abs() < 1e-6);
    }

    #[test]
    fn naming_a_turn_with_nothing_near_leaves_it_alone() {
        let path = l_shape();
        let mut q = turn_queue(&path, &[], 30.0);
        let far = road("Elsewhere", "residential", vec![[-87.0, 35.0], [-87.0, 35.1]]);
        assert!(!name_turn(&mut q[1], &[far], 30.0));
        assert!(q[1].road_name.is_none(), "an unnamed turn stays unnamed rather than borrowing");
    }

    #[test]
    fn an_unnamed_road_leaves_the_name_empty_rather_than_inventing_one() {
        let path = l_shape();
        let mut unnamed = road("x", "service", (0..=10).map(|i| [east(1.0), north(i as f64 * 0.1)]).collect());
        unnamed.name = None;
        let q = turn_queue(&path, &[unnamed], 30.0);
        assert!(q[1].road_name.is_none());
        assert_eq!(q[1].road_class.as_deref(), Some("service"), "class still reported");
    }

    #[test]
    fn snapping_reports_position_along_and_the_predicted_heading() {
        let path = l_shape();
        let cum = cumulative_m(&path);
        let q = turn_queue(&path, &[], 30.0);

        // 300 m along the eastbound leg, 10 m north of it.
        let st = navigate(&path, &cum, &q, north(0.01), east(0.3), 5.0, 0).expect("snap");
        assert!(st.offset_m > 5.0 && st.offset_m < 15.0, "offset {}", st.offset_m);
        assert!((st.along_m - 300.0).abs() < 20.0, "along {}", st.along_m);
        assert!(!st.off_route);
        // Driving east: bearing 90.
        assert!((st.bearing_deg - 90.0).abs() < 5.0, "bearing {}", st.bearing_deg);
        assert_eq!(st.next_turn, Some(1));
        assert!((st.distance_to_turn_m - 700.0).abs() < 30.0, "to turn {}", st.distance_to_turn_m);
    }

    #[test]
    fn the_predicted_heading_turns_with_the_route_before_the_corner() {
        let path = l_shape();
        let cum = cumulative_m(&path);
        let q = turn_queue(&path, &[], 30.0);

        // 30 m short of the corner: the lookahead already sees the new leg, so
        // the heading has begun to swing north. This is the whole point of a
        // predicted vector -- a GPS heading here still says due east.
        let st = navigate(&path, &cum, &q, 36.0, east(0.97), 5.0, 0).expect("snap");
        assert!(
            st.bearing_deg < 80.0,
            "heading should lead into the turn, got {}",
            st.bearing_deg
        );

        // Past the corner it is due north.
        let after = navigate(&path, &cum, &q, north(0.3), east(1.0), 5.0, st.index).expect("snap");
        assert!(after.bearing_deg < 5.0 || after.bearing_deg > 355.0, "bearing {}", after.bearing_deg);
        assert_eq!(after.next_turn, Some(2), "only Arrive remains");
    }

    #[test]
    fn off_route_scales_with_gps_accuracy() {
        let path = l_shape();
        let cum = cumulative_m(&path);
        let q = turn_queue(&path, &[], 30.0);

        // 60 m off the line. With a good fix that is a wrong turn.
        let good = navigate(&path, &cum, &q, north(0.06), east(0.3), 5.0, 0).expect("snap");
        assert!(good.off_route, "offset {}", good.offset_m);

        // The same 60 m with a 30 m accuracy fix is an urban canyon, not a
        // wrong turn: 3 x 30 = 90 m of doubt.
        let vague = navigate(&path, &cum, &q, north(0.06), east(0.3), 30.0, 0).expect("snap");
        assert!(!vague.off_route, "a vague fix must not fire on its own noise");
    }

    #[test]
    fn a_route_that_doubles_back_snaps_to_the_leg_you_are_on() {
        // Out 1 km east and back along the same line, 30 m apart. Both legs
        // are within snapping distance of the same fix; only the window says
        // which one you are on.
        let mut path: Vec<[f64; 2]> = (0..=20).map(|i| [east(i as f64 * 0.05), 36.0]).collect();
        path.extend((0..=20).map(|i| [east(1.0 - i as f64 * 0.05), north(0.03)]));
        let cum = cumulative_m(&path);
        let q = turn_queue(&path, &[], 30.0);

        let outbound = navigate(&path, &cum, &q, 36.0002, east(0.5), 5.0, 0).expect("snap");
        assert!(outbound.along_m < 1000.0, "outbound along {}", outbound.along_m);

        // Same place, but arriving with the return leg as context.
        let back_index = cum.partition_point(|&c| c < 1400.0);
        let inbound = navigate(&path, &cum, &q, 36.0002, east(0.5), 5.0, back_index).expect("snap");
        assert!(inbound.along_m > 1000.0, "inbound along {}", inbound.along_m);
    }

    #[test]
    fn a_fix_far_from_everything_is_off_route_not_a_panic() {
        let path = l_shape();
        let cum = cumulative_m(&path);
        let q = turn_queue(&path, &[], 30.0);
        let lost = navigate(&path, &cum, &q, 35.0, -87.0, 5.0, 0).expect("still answers");
        assert!(lost.off_route);
        assert!(lost.offset_m > 10_000.0);
    }

    #[test]
    fn arriving_leaves_no_next_turn_and_no_distance_remaining() {
        let path = l_shape();
        let cum = cumulative_m(&path);
        let q = turn_queue(&path, &[], 30.0);
        let end = navigate(&path, &cum, &q, north(1.0), east(1.0), 5.0, path.len() - 2)
            .expect("snap");
        assert!(end.remaining_m < 20.0, "remaining {}", end.remaining_m);
        assert!(end.distance_to_turn_m < 20.0);
    }

    #[test]
    fn a_degenerate_route_answers_nothing_rather_than_guessing() {
        assert!(turn_queue(&[], &[], 30.0).is_empty());
        assert!(turn_queue(&[[-86.0, 36.0]], &[], 30.0).is_empty());
        let one = vec![[-86.0, 36.0]];
        assert!(navigate(&one, &cumulative_m(&one), &[], 36.0, -86.0, 5.0, 0).is_none());
    }

    #[test]
    fn bearing_delta_wraps_the_short_way() {
        assert_eq!(bearing_delta(10.0, 20.0), 10.0);
        assert_eq!(bearing_delta(350.0, 10.0), 20.0, "across north, not -340");
        assert_eq!(bearing_delta(10.0, 350.0), -20.0);
        assert_eq!(bearing_delta(0.0, 180.0), 180.0);
    }
}
