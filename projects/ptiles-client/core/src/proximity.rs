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

use crate::roads::RoadSegment;

/// `sin`/`cos`/`sqrt`/`atan2` aren't in libcore -- they need either `std` or
/// a software implementation. Mirrors h3o's own no_std strategy
/// (`h3o::math::functions-libm.rs`): native `f64` methods under `std`,
/// `libm` under `no_std`.
#[cfg(feature = "std")]
mod math {
    #[inline]
    pub fn sin(x: f64) -> f64 {
        x.sin()
    }
    #[inline]
    pub fn cos(x: f64) -> f64 {
        x.cos()
    }
    #[inline]
    pub fn sqrt(x: f64) -> f64 {
        x.sqrt()
    }
    #[inline]
    pub fn atan2(y: f64, x: f64) -> f64 {
        y.atan2(x)
    }
}
#[cfg(not(feature = "std"))]
mod math {
    #[inline]
    pub fn sin(x: f64) -> f64 {
        libm::sin(x)
    }
    #[inline]
    pub fn cos(x: f64) -> f64 {
        libm::cos(x)
    }
    #[inline]
    pub fn sqrt(x: f64) -> f64 {
        libm::sqrt(x)
    }
    #[inline]
    pub fn atan2(y: f64, x: f64) -> f64 {
        libm::atan2(y, x)
    }
}

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

/// Project a lat/lon point to meters on a local equirectangular tangent
/// plane centered at `(origin_lat, origin_lon)`. Accurate for the
/// sub-kilometer distances this module is used at; degrades over long
/// ranges (that's what `haversine_distance_m` is for).
fn project_m(origin_lat: f64, origin_lon: f64, lat: f64, lon: f64) -> (f64, f64) {
    let x = (lon - origin_lon).to_radians() * math::cos(origin_lat.to_radians()) * EARTH_RADIUS_M;
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
        if best.is_none_or(|(_, b)| proj.distance_m < b.distance_m) {
            best = Some((i, proj));
        }
    }
    best
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
pub fn nearest_road(lat: f64, lon: f64, roads: &[RoadSegment], threshold_m: f64) -> Option<NearestRoad> {
    let mut best: Option<NearestRoad> = None;
    for (road_index, road) in roads.iter().enumerate() {
        if let Some((segment_index, proj)) = point_to_linestring_distance_m(lat, lon, &road.coords) {
            if proj.distance_m <= threshold_m
                && best.is_none_or(|b| proj.distance_m < b.distance_m)
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
}
