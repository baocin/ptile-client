//! Nearest-road geometry: haversine distance, point-to-segment distance,
//! and the "what road am I near?" query from SPEC.md ("Query: What road am
//! I near?", ptiles/SPEC.md:292-320).
//!
//! SPEC.md's own `point_to_segment_distance` reference implementation works
//! in raw microdegrees and notes that's only approximate ("multiply by
//! ~0.9 m/microdegree... for precise results, use the Haversine formula").
//! The deployed demo (steele.red/ptiles/index.html) instead uses full
//! Haversine for every point-distance it computes (building/business
//! nearest-lookups) and per the plan's disagreement rule the demo wins on
//! decoding/query *behavior*. So here: point-to-segment distance is done by
//! projecting the segment onto a local equirectangular tangent plane
//! centered at the query point (meters, accurate at the sub-kilometer
//! scales `nearest_road` operates at) and running the same clamped
//! projection formula SPEC.md gives, rather than truncating to raw
//! microdegree deltas.

use crate::roads::{Intersection, RoadSegment};

use crate::math;

/// Mean Earth radius in meters (matches SPEC.md's reverse-geocode reference
/// and the demo's Haversine calls, both of which use 6,371,000 m).
const EARTH_RADIUS_M: f64 = 6_371_000.0;

/// Great-circle distance between two lat/lon points, in meters.
pub fn haversine_distance_m(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let d_lat = (lat2 - lat1).to_radians();
    let d_lon = (lon2 - lon1).to_radians();
    let sin_half_lat = math::sin(d_lat / 2.0);
    let sin_half_lon = math::sin(d_lon / 2.0);
    let a = sin_half_lat * sin_half_lat
        + math::cos(lat1.to_radians()) * math::cos(lat2.to_radians()) * sin_half_lon * sin_half_lon;
    let c = 2.0 * math::atan2(math::sqrt(a), math::sqrt(1.0 - a));
    EARTH_RADIUS_M * c
}

/// Normalize a longitude *difference* (degrees) into `[-180, 180]` so a pair
/// straddling the antimeridian (e.g. `+179.9` and `-179.9`) reads as the
/// 0.2-degree gap it physically is, not a ~360-degree jump. Without this,
/// `project_m` would compute kilometre-scale bogus distances near +/-180 lon.
fn normalize_lon_delta(mut d: f64) -> f64 {
    // Guard against NaN/inf: only wrap finite values.
    if !d.is_finite() {
        return d;
    }
    while d > 180.0 {
        d -= 360.0;
    }
    while d < -180.0 {
        d += 360.0;
    }
    d
}

/// Project a lat/lon point to meters on a local equirectangular tangent
/// plane centered at `(origin_lat, origin_lon)`. Accurate for the
/// sub-kilometer distances this module is used at; degrades over long
/// ranges (that's what `haversine_distance_m` is for).
fn project_m(origin_lat: f64, origin_lon: f64, lat: f64, lon: f64) -> (f64, f64) {
    let x = normalize_lon_delta(lon - origin_lon).to_radians()
        * math::cos(origin_lat.to_radians())
        * EARTH_RADIUS_M;
    let y = (lat - origin_lat).to_radians() * EARTH_RADIUS_M;
    (x, y)
}

/// Result of snapping a point onto a single line segment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SegmentProjection {
    /// Distance from the query point to the snapped point, in meters.
    pub distance_m: f64,
    /// Fractional position along the segment, clamped to `[0, 1]`
    /// (`0.0` = segment start, `1.0` = segment end).
    pub t: f64,
    /// Snapped point, `(lat, lon)` degrees.
    pub snapped: (f64, f64),
}

/// Distance from point `(p_lat, p_lon)` to the line segment
/// `(a_lat, a_lon)-(b_lat, b_lon)`, all in degrees. Mirrors SPEC.md's
/// `point_to_segment_distance`, but works in a local-meters projection
/// instead of raw microdegrees so the result is directly comparable
/// against Haversine-computed distances and meter thresholds (e.g. the
/// default 50 m nearest-road cutoff).
pub fn point_to_segment_distance_m(
    p_lat: f64,
    p_lon: f64,
    a_lat: f64,
    a_lon: f64,
    b_lat: f64,
    b_lon: f64,
) -> SegmentProjection {
    // Project everything relative to the query point so the tangent plane
    // is centered as close as possible to where accuracy matters most.
    let (px, py) = (0.0f64, 0.0f64);
    let (ax, ay) = project_m(p_lat, p_lon, a_lat, a_lon);
    let (bx, by) = project_m(p_lat, p_lon, b_lat, b_lon);

    let dx = bx - ax;
    let dy = by - ay;

    if dx == 0.0 && dy == 0.0 {
        let d = math::sqrt((px - ax) * (px - ax) + (py - ay) * (py - ay));
        return SegmentProjection {
            distance_m: d,
            t: 0.0,
            snapped: (a_lat, a_lon),
        };
    }

    let t_raw = ((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy);
    let t = t_raw.clamp(0.0, 1.0);

    let sx = ax + t * dx;
    let sy = ay + t * dy;
    let distance_m = math::sqrt((px - sx) * (px - sx) + (py - sy) * (py - sy));

    // Un-project the snapped point back to lat/lon by linear interpolation
    // along the original (unprojected) endpoints -- exact for how the
    // projection is constructed (it's affine in lat/lon for small spans).
    let snapped_lat = a_lat + t * (b_lat - a_lat);
    let snapped_lon = a_lon + t * (b_lon - a_lon);

    SegmentProjection {
        distance_m,
        t,
        snapped: (snapped_lat, snapped_lon),
    }
}

/// Minimum distance from a point to any segment of a linestring, plus which
/// segment and where on it. `coords` are `[lon, lat]` pairs (matches
/// `RoadSegment::coords` and every other decoder's coordinate order).
/// Returns `None` for degenerate linestrings (fewer than 2 points).
pub fn point_to_linestring_distance_m(
    p_lat: f64,
    p_lon: f64,
    coords: &[[f64; 2]],
) -> Option<(usize, SegmentProjection)> {
    if coords.len() < 2 {
        return None;
    }
    let mut best: Option<(usize, SegmentProjection)> = None;
    for i in 0..coords.len() - 1 {
        let [lon1, lat1] = coords[i];
        let [lon2, lat2] = coords[i + 1];
        let proj = point_to_segment_distance_m(p_lat, p_lon, lat1, lon1, lat2, lon2);
        // Skip NaN/inf distances (NaN inputs, projection blow-ups) so a bad
        // segment can't lodge itself as "best" and shadow finite ones -- a
        // NaN comparison is always false, so it would never be replaced.
        if !proj.distance_m.is_finite() {
            continue;
        }
        if best.is_none_or(|(_, b)| proj.distance_m < b.distance_m) {
            best = Some((i, proj));
        }
    }
    best
}

/// Whether `(lat, lon)` falls inside a closed ring. `coords` are `[lon, lat]`
/// pairs, the order every decoder emits; the ring may be explicitly closed
/// (last vertex equal to the first) or not.
///
/// Ray casting in raw degrees, deliberately: containment is a topological
/// question, and re-projecting to metres changes nothing about which side of
/// an edge a point lands on at the sub-degree spans a block covers. Lived in
/// `cli/src/main.rs` and `ffi/src/lib.rs` as two copies before landing here,
/// which is where the park/water lookups need it.
pub fn point_in_polygon(lat: f64, lon: f64, coords: &[[f64; 2]]) -> bool {
    if coords.len() < 3 {
        return false;
    }
    let mut inside = false;
    let n = coords.len();
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (coords[i][0], coords[i][1]);
        let (xj, yj) = (coords[j][0], coords[j][1]);
        if ((yi > lat) != (yj > lat)) && (lon < (xj - xi) * (lat - yi) / (yj - yi) + xi) {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Where segment `a`→`b` crosses segment `c`→`d`, as a fraction of the way
/// along `a`→`b`, or `None` when they do not cross. All points are
/// `[lon, lat]`.
///
/// Degrees, not metres, deliberately: whether two segments cross and where
/// along the first they do are both preserved by the affine
/// degrees-to-local-metres map, so projecting first would cost trigonometry
/// and change no answer. Convert the returned fraction to a distance with the
/// segment's own length when you need metres.
///
/// Touching endpoints count as a crossing; parallel segments never do, even
/// when collinear and overlapping, since there is no single crossing point to
/// name.
pub fn segment_crossing(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> Option<f64> {
    let r = [b[0] - a[0], b[1] - a[1]];
    let s = [d[0] - c[0], d[1] - c[1]];
    let denom = r[0] * s[1] - r[1] * s[0];
    if denom == 0.0 || !denom.is_finite() {
        return None;
    }
    let ac = [c[0] - a[0], c[1] - a[1]];
    let t = (ac[0] * s[1] - ac[1] * s[0]) / denom;
    let u = (ac[0] * r[1] - ac[1] * r[0]) / denom;
    ((0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u)).then_some(t)
}

/// Distance from a point to a ring's boundary, in metres. A ring is a closed
/// linestring, so the boundary distance is the linestring distance with the
/// closing edge included — without it, a point just outside the gap between
/// the last and first vertex measures far too far.
pub fn point_to_ring_distance_m(lat: f64, lon: f64, coords: &[[f64; 2]]) -> Option<f64> {
    if coords.len() < 2 {
        return None;
    }
    let open = coords.first() != coords.last();
    let direct = point_to_linestring_distance_m(lat, lon, coords).map(|(_, p)| p.distance_m);
    if !open {
        return direct;
    }
    let [flon, flat] = coords[0];
    let [llon, llat] = coords[coords.len() - 1];
    let closing = point_to_segment_distance_m(lat, lon, llat, llon, flat, flon).distance_m;
    match (direct, closing.is_finite().then_some(closing)) {
        (Some(d), Some(c)) => Some(d.min(c)),
        (d, c) => d.or(c),
    }
}

/// A road segment found by [`nearest_road`], with the snapped point and
/// distance to the query location.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NearestRoad {
    /// Index into the `roads` slice passed to `nearest_road`.
    pub road_index: usize,
    /// Index of the segment-pair within that road's linestring (`coords[i]`
    /// to `coords[i+1]`) the snap landed on.
    pub segment_index: usize,
    /// Snapped point, `(lat, lon)` degrees.
    pub snapped: (f64, f64),
    /// Distance from the query point to the snapped point, in meters.
    pub distance_m: f64,
}

/// Default nearest-road search threshold in meters (SPEC.md:298: "Return
/// nearest segment within threshold (default 50 m)").
pub const DEFAULT_THRESHOLD_M: f64 = 50.0;

/// Find the closest road segment to `(lat, lon)` among `roads`, per
/// SPEC.md's "What road am I near?" query (steps 4-5: compute min distance
/// to each segment's linestring, return nearest within threshold).
///
/// Does not do H3 lookup or neighbor-cell expansion itself (steps 1-2, 6)
/// — those live in `query.rs`; callers pass in the road segments already
/// fetched for the relevant cell(s).
pub fn nearest_road(
    lat: f64,
    lon: f64,
    roads: &[RoadSegment],
    threshold_m: f64,
) -> Option<NearestRoad> {
    let mut best: Option<NearestRoad> = None;
    for (road_index, road) in roads.iter().enumerate() {
        if let Some((segment_index, proj)) = point_to_linestring_distance_m(lat, lon, &road.coords)
        {
            if proj.distance_m <= threshold_m && best.is_none_or(|b| proj.distance_m < b.distance_m)
            {
                best = Some(NearestRoad {
                    road_index,
                    segment_index,
                    snapped: proj.snapped,
                    distance_m: proj.distance_m,
                });
            }
        }
    }
    best
}

/// An intersection found by [`nearest_intersection`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NearestIntersection {
    /// Index into the `intersections` slice passed to `nearest_intersection`.
    pub index: usize,
    /// Distance from the query point to the intersection, in meters.
    pub distance_m: f64,
    /// Traffic-control classification: 1 = traffic_signals, 2 = stop,
    /// 3 = give_way, 4 = roundabout (0/other = untyped).
    pub intersection_type: u8,
}

/// Find the closest labeled intersection to `(lat, lon)` within `threshold_m`
/// — the "am I at an intersection?" query. Point-to-point analogue of
/// [`nearest_road`]: `.roads.ptiles` v2 blocks carry an intersection table
/// (see [`crate::decode_road_block`]); callers pass the decoded intersections
/// for the relevant cell(s).
///
/// Answers "is there a mapped intersection point near me (and what control
/// type)?". It does NOT report junction degree — the format stores no
/// topology, so a 4-way junction and a tagged road endpoint are
/// indistinguishable from this data alone.
pub fn nearest_intersection(
    lat: f64,
    lon: f64,
    intersections: &[Intersection],
    threshold_m: f64,
) -> Option<NearestIntersection> {
    let mut best: Option<NearestIntersection> = None;
    for (index, ix) in intersections.iter().enumerate() {
        let [ix_lon, ix_lat] = ix.coords();
        let distance_m = haversine_distance_m(lat, lon, ix_lat, ix_lon);
        if distance_m.is_finite()
            && distance_m <= threshold_m
            && best.is_none_or(|b| distance_m < b.distance_m)
        {
            best = Some(NearestIntersection {
                index,
                distance_m,
                intersection_type: ix.intersection_type,
            });
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;
    use alloc::vec;

    fn road(coords: Vec<[f64; 2]>) -> RoadSegment {
        RoadSegment {
            osm_id: 1,
            road_class: String::from("residential"),
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

    #[test]
    fn haversine_known_pair() {
        // Nashville (36.16, -86.78) to Memphis (35.15, -90.05) ~ 300 km.
        let d = haversine_distance_m(36.16, -86.78, 35.15, -90.05);
        assert!((280_000.0..320_000.0).contains(&d), "d={d}");
    }

    #[test]
    fn haversine_zero_distance() {
        let d = haversine_distance_m(36.16, -86.78, 36.16, -86.78);
        assert!(d.abs() < 1e-6);
    }

    #[test]
    fn point_to_segment_projects_onto_middle() {
        // Segment running east along the equator; point directly north of
        // the midpoint should project to t=0.5.
        let proj = point_to_segment_distance_m(0.001, 0.5, 0.0, 0.0, 0.0, 1.0);
        assert!((proj.t - 0.5).abs() < 0.01, "t={}", proj.t);
        assert!(proj.distance_m > 0.0 && proj.distance_m < 200.0);
    }

    #[test]
    fn point_to_segment_clamps_beyond_start() {
        // Point "behind" the segment start should clamp to t=0.
        let proj = point_to_segment_distance_m(0.0, -1.0, 0.0, 0.0, 0.0, 1.0);
        assert_eq!(proj.t, 0.0);
        assert_eq!(proj.snapped, (0.0, 0.0));
    }

    #[test]
    fn point_to_segment_clamps_beyond_end() {
        // Point beyond the segment end should clamp to t=1.
        let proj = point_to_segment_distance_m(0.0, 2.0, 0.0, 0.0, 0.0, 1.0);
        assert_eq!(proj.t, 1.0);
        assert_eq!(proj.snapped, (0.0, 1.0));
    }

    #[test]
    fn point_to_segment_degenerate_zero_length() {
        // a == b: falls back to point distance, t=0.
        let proj = point_to_segment_distance_m(0.001, 0.001, 0.0, 0.0, 0.0, 0.0);
        assert_eq!(proj.t, 0.0);
        assert_eq!(proj.snapped, (0.0, 0.0));
        assert!(proj.distance_m > 0.0);
    }

    #[test]
    fn nearest_road_picks_closest_within_threshold() {
        let roads = vec![
            road(vec![[-86.80, 36.16], [-86.79, 36.16]]), // ~1.1km away, outside threshold
            road(vec![[-86.7801, 36.1601], [-86.7799, 36.1601]]), // right at the query pt
        ];
        let result = nearest_road(36.16, -86.78, &roads, DEFAULT_THRESHOLD_M).unwrap();
        assert_eq!(result.road_index, 1);
        assert!(result.distance_m < 20.0, "distance_m={}", result.distance_m);
    }

    #[test]
    fn nearest_road_none_when_all_beyond_threshold() {
        let roads = vec![road(vec![[-90.0, 35.0], [-90.01, 35.0]])];
        assert!(nearest_road(36.16, -86.78, &roads, DEFAULT_THRESHOLD_M).is_none());
    }

    #[test]
    fn point_to_linestring_empty_and_single_point() {
        assert!(point_to_linestring_distance_m(0.0, 0.0, &[]).is_none());
        assert!(point_to_linestring_distance_m(0.0, 0.0, &[[0.0, 0.0]]).is_none());
    }

    // --- distance math: hand-computed expected values ---

    #[test]
    fn haversine_one_degree_lat_is_about_111km() {
        // One degree of latitude is ~111.19 km on a sphere of radius 6371 km
        // (R * 1deg-in-rad = 6_371_000 * pi/180 = 111_194.9 m).
        let d = haversine_distance_m(0.0, 0.0, 1.0, 0.0);
        assert!((d - 111_194.9).abs() < 1.0, "d={d}");
    }

    #[test]
    fn haversine_lon_shrinks_with_latitude() {
        // A degree of longitude at 60N is ~half its equatorial length
        // (cos 60 = 0.5).
        let eq = haversine_distance_m(0.0, 0.0, 0.0, 1.0);
        let hi = haversine_distance_m(60.0, 0.0, 60.0, 1.0);
        assert!((hi / eq - 0.5).abs() < 0.01, "eq={eq} hi={hi}");
    }

    #[test]
    fn haversine_is_symmetric() {
        let a = haversine_distance_m(36.16, -86.78, 35.15, -90.05);
        let b = haversine_distance_m(35.15, -90.05, 36.16, -86.78);
        assert!((a - b).abs() < 1e-6);
    }

    #[test]
    fn point_to_segment_on_start_endpoint_is_zero() {
        // Query point exactly on the segment's start endpoint.
        let proj = point_to_segment_distance_m(10.0, 20.0, 10.0, 20.0, 10.0, 21.0);
        assert!(proj.distance_m < 1e-6, "distance_m={}", proj.distance_m);
        assert!((proj.t - 0.0).abs() < 1e-9, "t={}", proj.t);
    }

    #[test]
    fn point_to_segment_on_end_endpoint_is_zero() {
        let proj = point_to_segment_distance_m(10.0, 21.0, 10.0, 20.0, 10.0, 21.0);
        assert!(proj.distance_m < 1e-6, "distance_m={}", proj.distance_m);
        assert!((proj.t - 1.0).abs() < 1e-9, "t={}", proj.t);
    }

    #[test]
    fn point_exactly_on_segment_midpoint_is_zero() {
        // Point sitting on the segment (its midpoint) -> zero distance, t=0.5.
        let proj = point_to_segment_distance_m(0.0, 0.5, 0.0, 0.0, 0.0, 1.0);
        assert!(proj.distance_m < 1e-6, "distance_m={}", proj.distance_m);
        assert!((proj.t - 0.5).abs() < 1e-6, "t={}", proj.t);
    }

    #[test]
    fn point_to_segment_perpendicular_distance_known() {
        // Segment along the equator from lon 0 to lon 1; query point 0.001 deg
        // north of the midpoint. Perpendicular distance == 0.001 deg of
        // latitude ~= 111.19 m.
        let proj = point_to_segment_distance_m(0.001, 0.5, 0.0, 0.0, 0.0, 1.0);
        let expected = 111_194.9 * 0.001; // ~111.19 m
        assert!(
            (proj.distance_m - expected).abs() < 0.5,
            "distance_m={}",
            proj.distance_m
        );
    }

    // --- antimeridian / poles ---

    #[test]
    fn antimeridian_segment_distance_is_small() {
        // A short segment straddling +/-180 lon, near the equator. Physically
        // ~0.2 deg (~22 km) wide; a naive (lon-lon) delta would blow this up
        // to ~360 deg. Query point sits between the endpoints.
        let proj = point_to_segment_distance_m(0.0, 179.95, 0.0, 179.9, 0.0, -179.9);
        // Snapped somewhere on the segment; perpendicular distance ~0.
        assert!(proj.distance_m < 10_000.0, "distance_m={}", proj.distance_m);
    }

    #[test]
    fn antimeridian_projection_matches_haversine_scale() {
        // Two points 0.2 deg apart straddling the antimeridian at the equator.
        let hav = haversine_distance_m(0.0, 179.9, 0.0, -179.9);
        // Degenerate "segment" == single point at 179.9, measure to -179.9.
        let proj = point_to_segment_distance_m(0.0, -179.9, 0.0, 179.9, 0.0, 179.9);
        assert!(
            (proj.distance_m - hav).abs() < 50.0,
            "proj={} hav={hav}",
            proj.distance_m
        );
    }

    #[test]
    fn near_pole_haversine_finite_and_small() {
        // Two points near the north pole, 1 deg of longitude apart. Longitude
        // spacing collapses toward the pole, so the distance is tiny.
        let d = haversine_distance_m(89.999, 0.0, 89.999, 1.0);
        assert!(d.is_finite(), "d={d}");
        assert!(d < 5.0, "d={d}");
    }

    // --- NaN guards ---

    #[test]
    fn nan_coordinate_segment_is_skipped() {
        // First segment has a NaN coordinate (would yield NaN distance); the
        // second is a real, close segment. The NaN must not shadow it.
        let coords = [
            [f64::NAN, 0.0],
            [0.0002, 0.0],
            [0.0002, 0.001], // finite, close segment (idx 1)
        ];
        let (idx, proj) = point_to_linestring_distance_m(0.0, 0.0, &coords).unwrap();
        assert_eq!(idx, 1);
        assert!(proj.distance_m.is_finite());
    }

    // --- nearest_road ---

    #[test]
    fn nearest_road_empty_roads_is_none() {
        assert!(nearest_road(36.16, -86.78, &[], DEFAULT_THRESHOLD_M).is_none());
    }

    #[test]
    fn nearest_road_picks_geometrically_closest_of_three() {
        let roads = vec![
            road(vec![[-86.7810, 36.16], [-86.7809, 36.16]]), // ~90m W
            road(vec![[-86.78005, 36.16], [-86.78003, 36.16]]), // ~4m W (closest)
            road(vec![[-86.7799, 36.16], [-86.7798, 36.16]]), // ~9-18m E
        ];
        let result = nearest_road(36.16, -86.78, &roads, DEFAULT_THRESHOLD_M).unwrap();
        assert_eq!(result.road_index, 1, "result={result:?}");
        // Cross-check it really is the minimum over all roads.
        let mut min_d = f64::MAX;
        for r in &roads {
            if let Some((_, p)) = point_to_linestring_distance_m(36.16, -86.78, &r.coords) {
                if p.distance_m < min_d {
                    min_d = p.distance_m;
                }
            }
        }
        assert!((result.distance_m - min_d).abs() < 1e-9);
    }

    #[test]
    fn nearest_road_respects_threshold() {
        let roads = vec![road(vec![[-86.7803, 36.16], [-86.7802, 36.16]])]; // ~18-27m W
        // Loose threshold: found. Tight threshold: rejected.
        assert!(nearest_road(36.16, -86.78, &roads, 50.0).is_some());
        assert!(nearest_road(36.16, -86.78, &roads, 5.0).is_none());
    }

    #[test]
    fn normalize_lon_delta_wraps() {
        assert!((normalize_lon_delta(359.8) - (-0.2)).abs() < 1e-9);
        assert!((normalize_lon_delta(-359.8) - 0.2).abs() < 1e-9);
        assert!((normalize_lon_delta(10.0) - 10.0).abs() < 1e-9);
        assert!(normalize_lon_delta(f64::NAN).is_nan());
    }

    fn ix(lon_micro: i32, lat_micro: i32, intersection_type: u8) -> Intersection {
        Intersection {
            lon_micro,
            lat_micro,
            intersection_type,
        }
    }

    #[test]
    fn intersection_coords_scale_is_1e5() {
        // Golden first-intersection value from roads.rs decodes to Nashville.
        assert_eq!(ix(-8_679_367, 3_616_076, 1).coords(), [-86.79367, 36.16076]);
    }

    #[test]
    fn nearest_intersection_exact_hit_is_zero_distance() {
        let ints = vec![ix(-8_679_367, 3_616_076, 1)];
        let r = nearest_intersection(36.16076, -86.79367, &ints, DEFAULT_THRESHOLD_M).unwrap();
        assert_eq!(r.index, 0);
        assert_eq!(r.intersection_type, 1);
        assert!(r.distance_m < 1e-6, "distance={}", r.distance_m);
    }

    #[test]
    fn nearest_intersection_picks_closest_of_three() {
        // At lat 36.16, 1e-5 deg lon ~= 0.9 m; space them clearly apart.
        let ints = vec![
            ix(-8_678_000, 3_616_076, 2), // ~far W
            ix(-8_679_364, 3_616_076, 1), // ~3m from query (closest)
            ix(-8_681_000, 3_616_076, 3), // ~far E
        ];
        let r = nearest_intersection(36.16076, -86.79367, &ints, DEFAULT_THRESHOLD_M).unwrap();
        assert_eq!(r.index, 1);
        assert_eq!(r.intersection_type, 1);
        // Cross-check it is truly the minimum over all entries.
        let min_d = ints
            .iter()
            .map(|i| {
                let [ilon, ilat] = i.coords();
                haversine_distance_m(36.16076, -86.79367, ilat, ilon)
            })
            .fold(f64::MAX, f64::min);
        assert!((r.distance_m - min_d).abs() < 1e-9);
    }

    #[test]
    fn nearest_intersection_respects_threshold() {
        let ints = vec![ix(-8_679_000, 3_616_076, 1)]; // ~330m E of query
        assert!(nearest_intersection(36.16076, -86.79367, &ints, 1000.0).is_some());
        assert!(nearest_intersection(36.16076, -86.79367, &ints, 50.0).is_none());
    }

    #[test]
    fn nearest_intersection_empty_is_none() {
        assert!(nearest_intersection(36.16, -86.78, &[], DEFAULT_THRESHOLD_M).is_none());
    }

    #[test]
    fn nearest_intersection_skips_non_finite_query() {
        let ints = vec![ix(-8_679_367, 3_616_076, 1)];
        // A NaN query produces NaN distances, which must never be selected.
        assert!(nearest_intersection(f64::NAN, -86.79367, &ints, DEFAULT_THRESHOLD_M).is_none());
    }

    #[test]
    fn nearest_intersection_from_real_roads_block() {
        let data = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../test-fixtures/golden/roads.block.bin"
        ))
        .unwrap();
        let (_roads, ints) = crate::decode_road_block(&data, 2).expect("decode real roads block");
        assert!(!ints.is_empty());
        // Query exactly at the first intersection: it must be found at ~0 m.
        let [q_lon, q_lat] = ints[0].coords();
        let r = nearest_intersection(q_lat, q_lon, &ints, DEFAULT_THRESHOLD_M).unwrap();
        assert_eq!(r.index, 0);
        assert_eq!(r.intersection_type, ints[0].intersection_type);
        assert!(r.distance_m < 1.0, "distance={}", r.distance_m);
    }
}
