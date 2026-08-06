//! What can be seen from a point on the ground.
//!
//! Given an observer and a set of building footprints, decide which buildings
//! are visible and which are hidden behind something nearer. This is a 2.5D
//! viewshed: a short building does not hide a tall one behind it, so height
//! is what makes the answer interesting rather than just a shadow polygon.
//!
//! # The occlusion test
//!
//! For a target of height `H` at distance `D`, the sight line from the eye
//! (height `e` at distance 0) has height `e + (H - e) * x / D` at distance
//! `x`. That is monotonic in `x`, so an occluder of height `h` spanning
//! `[d_near, d_far]` binds hardest at its *near* edge: the line clears it iff
//!
//! ```text
//!     (H - e) / D  >  (h - e) / d_near
//! ```
//!
//! Both sides are slopes, and slope is monotonic in elevation angle, so the
//! whole algorithm compares slopes and never needs `atan`. Nearest-edge is
//! also why occluders are processed in near-to-far order: the horizon only
//! ever rises, so one pass suffices.
//!
//! Bearings are binned (see [`BINS`]) and each bin keeps the highest slope
//! blocked so far. Per bin, the distance comes from an actual ray/segment
//! intersection against the footprint edges rather than from the building's
//! overall nearest corner -- a long building seen obliquely would otherwise
//! block its entire angular span using its closest point, hiding far more
//! than it really does.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::math::{atan2, ceil, cos, sqrt};

/// Bearing bins around the observer. 1440 is 0.25 degrees, which at a 300 m
/// radius is about 1.3 m of arc -- finer than a building edge matters.
const BINS: usize = 1440;

const M_PER_DEG_LAT: f64 = 111_320.0;
const PI: f64 = core::f64::consts::PI;
const TWO_PI: f64 = 2.0 * PI;

/// Observer inside (or all but touching) a footprint. Distances below this
/// are clamped so the slope stays finite, and the building is reported
/// visible -- you are standing in it.
const MIN_DIST_M: f64 = 1.0;

/// A footprint to test, as decoded from a buildings block.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ViewBuilding {
    /// (lon, lat) pairs in degrees, the ring as `Building::coords` gives it.
    pub coords: Vec<[f64; 2]>,
    /// Published height, when there is one.
    pub height_m: Option<f64>,
    /// Used to estimate a height when `height_m` is `None`.
    pub building_type: String,
}

/// What the viewshed concluded about one building.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Visibility {
    pub visible: bool,
    /// The height actually used, published or estimated.
    pub height_m: f64,
    /// True when `height_m` came from [`estimate_height`] rather than the file.
    pub estimated: bool,
    /// Metres from the observer to the nearest point of the footprint.
    pub distance_m: f64,
}

/// The height an unmeasured building of this type plausibly has: 25th
/// percentile, median, 75th.
///
/// These are **measured from the published data itself** -- every building that
/// does carry a `height_m`, across dense cells in NY, CA, FL and PA (16552
/// samples), with the 127.5 m encoder clamp excluded. Better than invented
/// storey counts, with one bias worth stating: the sample is downtown cells,
/// so these run tall for the same type in a suburb. "yes" is by far the most
/// common type (13075 of the sample) and really means "untyped", so it gets
/// the overall distribution rather than anything cleverer.
///
/// The spread is the useful part. Office ran 13 m at the 25th percentile and
/// 55.5 m at the 75th, commercial 8 to 44 -- no single number represents
/// those, and [`viewshed`] uses the two ends rather than the middle so that
/// uncertainty costs visibility instead of inventing it.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct HeightGuess {
    pub low: f64,
    pub typical: f64,
    pub high: f64,
}

pub fn estimate_height_range(building_type: &str) -> HeightGuess {
    let (low, typical, high) = match building_type {
        "garage" | "garages" | "shed" | "carport" | "roof" | "canopy" => (2.5, 3.0, 4.0),
        "detached" | "bungalow" | "cabin" | "hut" | "static_caravan" => (4.5, 5.0, 5.5),
        "warehouse" | "factory" => (6.0, 7.0, 9.5),
        "industrial" | "kiosk" | "service" => (6.5, 8.0, 10.5),
        "retail" => (6.5, 8.5, 12.0),
        "school" | "kindergarten" => (6.5, 9.0, 15.0),
        "terrace" | "house" | "semidetached_house" => (10.0, 10.5, 13.0),
        "residential" | "dormitory" => (12.0, 14.0, 18.0),
        "apartments" => (11.0, 15.0, 32.5),
        "commercial" | "civic" | "public" | "hospital" | "university" => (8.0, 18.5, 44.0),
        "church" | "cathedral" | "chapel" | "mosque" | "synagogue" | "temple" => (12.0, 20.0, 30.0),
        "theatre" | "stadium" | "sports_hall" => (20.0, 21.0, 22.5),
        "hotel" => (16.0, 28.5, 49.5),
        "office" => (13.0, 32.5, 55.5),
        // "yes" and everything unrecognised.
        _ => (10.0, 12.0, 15.0),
    };
    HeightGuess { low, typical, high }
}

/// The single number to *show* for an unmeasured building of this type.
/// [`viewshed`] deliberately does not use this; see [`estimate_height_range`].
pub fn estimate_height(building_type: &str) -> f64 {
    estimate_height_range(building_type).typical
}

/// The encoder stores height as a `u8` of half-metre steps, so 127.5 m is a
/// ceiling rather than a measurement -- a 300 m tower is written as 127.5 too.
/// Treated as "at least this" when occluding, because assuming a skyscraper is
/// 127.5 m tall is exactly the error that reports things visible past it.
const CLAMP_M: f64 = 127.5;
const CLAMP_ASSUMED_M: f64 = 250.0;

/// Nothing in the data is flat ground. A published 0.5 m "building" is a bad
/// measurement, not a kerbstone, and from a street-level eye (1.7 m) it would
/// otherwise occlude nothing at all and never itself be worth seeing. Every
/// footprint is treated as at least one low storey in both roles.
const MIN_BUILDING_M: f64 = 3.0;

/// Local east/north metres relative to the observer. Equirectangular is exact
/// enough here: the radius is a few hundred metres, where the error against a
/// proper geodesic is millimetres.
#[derive(Clone, Copy)]
struct Pt {
    x: f64,
    y: f64,
}

#[inline]
fn cross(a: Pt, b: Pt) -> f64 {
    a.x * b.y - a.y * b.x
}

#[inline]
fn bin_of(theta: f64) -> usize {
    // theta is (-PI, PI] measured from north; shift to [0, TWO_PI).
    let t = if theta < 0.0 { theta + TWO_PI } else { theta };
    let b = (t / TWO_PI * BINS as f64) as usize;
    if b >= BINS { BINS - 1 } else { b }
}

/// Distance along a bearing to a segment, if the ray hits it at all.
///
/// Ray is the unit vector for `theta` from the origin (the observer). Solving
/// `t*d - s*e = p` by crossing with each of `e` and `d` gives both parameters;
/// the hit counts only ahead of the observer (`t > 0`) and within the segment
/// (`0 <= s <= 1`).
fn ray_segment_distance(theta: f64, p: Pt, q: Pt) -> Option<f64> {
    let d = Pt {
        x: crate::math::sin(theta),
        y: cos(theta),
    };
    let e = Pt {
        x: q.x - p.x,
        y: q.y - p.y,
    };
    let denom = cross(d, e);
    if denom.abs() < 1e-12 {
        return None; // parallel
    }
    let t = cross(p, e) / denom;
    let s = cross(p, d) / denom;
    if t > 0.0 && (0.0..=1.0).contains(&s) {
        Some(t)
    } else {
        None
    }
}

/// How far clear of a footprint the observer is placed when the given point
/// landed inside one. Two metres is a pavement width: far enough that the
/// building becomes a normal occluder, near enough that the answer is still
/// about the spot that was asked for.
const STREET_OFFSET_M: f64 = 2.0;

/// Is the observer (the origin) inside this ring? Even-odd crossing count.
fn origin_inside(ring: &[Pt]) -> bool {
    let mut inside = false;
    let n = ring.len();
    if n < 3 {
        return false;
    }
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (ring[i], ring[j]);
        // Does the edge straddle y = 0, and is the crossing on the +x side?
        if (a.y > 0.0) != (b.y > 0.0) {
            let x = a.x + (b.x - a.x) * (0.0 - a.y) / (b.y - a.y);
            if x > 0.0 {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

/// Closest point on the ring's boundary to the observer.
fn nearest_boundary_point(ring: &[Pt]) -> Option<(Pt, f64)> {
    let mut best: Option<(Pt, f64)> = None;
    for w in ring.windows(2) {
        let (a, b) = (w[0], w[1]);
        let (ex, ey) = (b.x - a.x, b.y - a.y);
        let len2 = ex * ex + ey * ey;
        let t = if len2 <= 0.0 {
            0.0
        } else {
            (((0.0 - a.x) * ex + (0.0 - a.y) * ey) / len2).clamp(0.0, 1.0)
        };
        let p = Pt {
            x: a.x + ex * t,
            y: a.y + ey * t,
        };
        let d = sqrt(p.x * p.x + p.y * p.y);
        if best.map_or(true, |(_, bd)| d < bd) {
            best = Some((p, d));
        }
    }
    best
}

/// Street level, enforced in geometry: if the observer landed inside a
/// footprint they are standing on a roof, not on the ground, and the building
/// they are inside stops occluding anything -- which reports sight lines
/// straight through it. Move the observer out to the nearest pavement instead,
/// by translating the whole scene (the observer is the origin).
///
/// Overlapping footprints can need more than one step, so it repeats, but only
/// a few times: a point buried in a pile of rings is not a street and further
/// nudging just walks it somewhere arbitrary.
fn step_outside(projected: &mut [Vec<Pt>]) {
    for _ in 0..3 {
        let mut shift: Option<Pt> = None;
        for ring in projected.iter() {
            if !origin_inside(ring) {
                continue;
            }
            if let Some((p, d)) = nearest_boundary_point(ring) {
                if d > 1e-6 {
                    // Out through the nearest wall, plus a pavement.
                    let scale = (d + STREET_OFFSET_M) / d;
                    shift = Some(Pt {
                        x: p.x * scale,
                        y: p.y * scale,
                    });
                    break;
                }
            }
        }
        let Some(s) = shift else { return };
        for ring in projected.iter_mut() {
            for p in ring.iter_mut() {
                p.x -= s.x;
                p.y -= s.y;
            }
        }
    }
}

/// Which buildings an observer at `lat`/`lon` can see.
///
/// The observer stands on the ground: a point that falls inside a footprint is
/// taken as the pavement beside that building rather than its roof, and is
/// moved out accordingly (see [`step_outside`]).
///
/// `eye_m` is the observer's eye height above ground and `radius_m` bounds the
/// search; buildings whose nearest point is beyond it are reported as not
/// visible without being tested. Results are returned in the order the
/// buildings were given, so callers can zip them straight back onto their own
/// list.
pub fn viewshed(
    lat: f64,
    lon: f64,
    eye_m: f64,
    radius_m: f64,
    buildings: &[ViewBuilding],
) -> Vec<Visibility> {
    let lat_rad = lat * PI / 180.0;
    let m_per_deg_lon = M_PER_DEG_LAT * cos(lat_rad);

    // Project once; every later step works in metres.
    let mut projected: Vec<Vec<Pt>> = Vec::with_capacity(buildings.len());
    let mut out: Vec<Visibility> = Vec::with_capacity(buildings.len());
    let mut target_h: Vec<f64> = Vec::with_capacity(buildings.len());
    let mut occluder_h: Vec<f64> = Vec::with_capacity(buildings.len());
    for b in buildings {
        let ring: Vec<Pt> = b
            .coords
            .iter()
            .map(|c| Pt {
                x: (c[0] - lon) * m_per_deg_lon,
                y: (c[1] - lat) * M_PER_DEG_LAT,
            })
            .collect();
        // Heights are uncertain, and the two roles a building plays want the
        // error pointing opposite ways. As an occluder, assume it is tall; as
        // a target, assume it is short. Anything marginal therefore comes out
        // hidden, so the mode under-reports what can be seen rather than
        // promising sight lines that do not exist.
        //
        // A measured height is used as-is for both, except at the encoder's
        // 127.5 m ceiling, which is a floor on the truth rather than a value.
        let (height_m, estimated, as_target, as_occluder) = match b.height_m {
            Some(h) if h >= CLAMP_M => (h, false, h, CLAMP_ASSUMED_M),
            Some(h) if h > 0.0 => (h, false, h, h),
            _ => {
                let g = estimate_height_range(&b.building_type);
                (g.typical, true, g.low, g.high)
            }
        };
        out.push(Visibility {
            visible: false,
            height_m,
            estimated,
            distance_m: f64::INFINITY,
        });
        // Report what the file says, but never let the geometry see a
        // building shorter than one storey. See MIN_BUILDING_M.
        target_h.push(as_target.max(MIN_BUILDING_M));
        occluder_h.push(as_occluder.max(MIN_BUILDING_M));
        projected.push(ring);
    }

    // A caller's point is a point on a map, not a rooftop: put the eye on the
    // street. Done here, after projection, so every caller gets it.
    step_outside(&mut projected);

    for (i, ring) in projected.iter().enumerate() {
        let mut nearest = f64::INFINITY;
        for p in ring {
            let d = sqrt(p.x * p.x + p.y * p.y);
            if d < nearest {
                nearest = d;
            }
        }
        out[i].distance_m = nearest;
    }

    // Near to far: the horizon only rises, so one pass settles every building.
    let mut order: Vec<usize> = (0..buildings.len())
        .filter(|&i| out[i].distance_m <= radius_m)
        .collect();
    order.sort_by(|&a, &b| {
        out[a]
            .distance_m
            .partial_cmp(&out[b].distance_m)
            .unwrap_or(core::cmp::Ordering::Equal)
    });

    let mut horizon = vec![f64::NEG_INFINITY; BINS];
    // Reused across buildings so a dense scene does not allocate per footprint.
    let mut hits: Vec<(usize, f64)> = Vec::new();

    for &i in &order {
        let ring = &projected[i];
        if ring.len() < 2 {
            continue;
        }

        // Standing inside it: visible, and not an occluder of anything.
        if out[i].distance_m < MIN_DIST_M {
            out[i].visible = true;
            continue;
        }

        hits.clear();
        for w in ring.windows(2) {
            let (p, q) = (w[0], w[1]);
            let t1 = atan2(p.x, p.y);
            let t2 = atan2(q.x, q.y);
            // Walk the short way round; an edge subtending more than PI would
            // mean the observer is inside the footprint, handled above.
            let mut delta = t2 - t1;
            if delta > PI {
                delta -= TWO_PI;
            } else if delta < -PI {
                delta += TWO_PI;
            }
            let steps = ceil(delta.abs() / (TWO_PI / BINS as f64)) as usize + 1;
            for s in 0..=steps {
                let theta = t1 + delta * (s as f64 / steps as f64);
                if let Some(d) = ray_segment_distance(theta, p, q) {
                    hits.push((bin_of(theta), d.max(MIN_DIST_M)));
                }
            }
        }
        if hits.is_empty() {
            continue;
        }

        // Visible if the top clears the horizon anywhere in its own span, at
        // the *pessimistic* height. Test before committing, or a building
        // occludes itself.
        let mut visible = false;
        for &(bin, d) in &hits {
            if (target_h[i] - eye_m) / d > horizon[bin] {
                visible = true;
                break;
            }
        }
        out[i].visible = visible;

        // Block using the optimistic height, so an uncertain building hides
        // more than it strictly proves it does.
        for &(bin, d) in &hits {
            let slope = (occluder_h[i] - eye_m) / d;
            if slope > horizon[bin] {
                horizon[bin] = slope;
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    /// A square footprint `size` metres across, centred `north` metres north
    /// and `east` metres east of (0, 0).
    fn square(north: f64, east: f64, size: f64, height: Option<f64>) -> ViewBuilding {
        let dlat = |m: f64| m / M_PER_DEG_LAT;
        let dlon = |m: f64| m / M_PER_DEG_LAT; // cos(0) == 1 at the equator
        let (n, e, s) = (north, east, size / 2.0);
        let ring = vec![
            [dlon(e - s), dlat(n - s)],
            [dlon(e + s), dlat(n - s)],
            [dlon(e + s), dlat(n + s)],
            [dlon(e - s), dlat(n + s)],
            [dlon(e - s), dlat(n - s)],
        ];
        ViewBuilding {
            coords: ring,
            height_m: height,
            building_type: "yes".to_string(),
        }
    }

    #[test]
    fn a_lone_building_is_visible() {
        let out = viewshed(0.0, 0.0, 1.7, 500.0, &[square(100.0, 0.0, 20.0, Some(10.0))]);
        assert!(out[0].visible);
        assert!((out[0].distance_m - 90.0).abs() < 2.0, "{:?}", out[0].distance_m);
    }

    #[test]
    fn a_taller_building_hides_a_shorter_one_directly_behind_it() {
        let near = square(100.0, 0.0, 20.0, Some(40.0));
        let far = square(200.0, 0.0, 20.0, Some(10.0));
        let out = viewshed(0.0, 0.0, 1.7, 500.0, &[near, far]);
        assert!(out[0].visible, "the near one must be visible");
        assert!(!out[1].visible, "a low building behind a tall one is hidden");
    }

    #[test]
    fn a_short_building_does_not_hide_a_tower_behind_it() {
        // The whole point of carrying height: in plan view the near footprint
        // covers the far one, and a 2D shadow test would wrongly hide it.
        let near = square(100.0, 0.0, 20.0, Some(5.0));
        let far = square(200.0, 0.0, 20.0, Some(80.0));
        let out = viewshed(0.0, 0.0, 1.7, 500.0, &[near, far]);
        assert!(out[0].visible);
        assert!(out[1].visible, "the tower's top clears the low building");
    }

    /// The web demo answers "which cameras can see me" by appending a 1 m,
    /// 4 m-tall square at each camera's position to the observer's own
    /// viewshed and reading back `visible` -- line of sight is reciprocal, so
    /// no separate point-to-point test exists or is needed. That only holds if
    /// a footprint this small is still a first-class target here, which is
    /// exactly what a bin-based horizon could quietly stop doing: at 100 m a
    /// 1 m object spans under three of the 1440 bearing bins.
    #[test]
    fn a_camera_sized_marker_is_a_target_in_its_own_right() {
        let pole = || square(50.0, 0.0, 1.0, Some(4.0));
        assert!(
            viewshed(0.0, 0.0, 1.7, 400.0, &[pole()])[0].visible,
            "a 1 m marker with nothing in front of it must be visible"
        );

        // Same marker at 100 m, behind a 30 m wall spanning the sight line.
        let wall = square(60.0, 0.0, 80.0, Some(30.0));
        let far = square(100.0, 0.0, 1.0, Some(4.0));
        let out = viewshed(0.0, 0.0, 1.7, 400.0, &[wall, far]);
        assert!(out[0].visible, "the wall itself is visible");
        assert!(!out[1].visible, "the marker behind the wall is not");
    }

    #[test]
    fn a_building_off_to_the_side_is_not_occluded() {
        let near = square(100.0, 0.0, 20.0, Some(60.0));
        let beside = square(100.0, 120.0, 20.0, Some(8.0));
        let out = viewshed(0.0, 0.0, 1.7, 500.0, &[near, beside]);
        assert!(out[0].visible);
        assert!(out[1].visible, "different bearing, nothing in the way");
    }

    #[test]
    fn beyond_the_radius_is_not_reported_visible() {
        let out = viewshed(0.0, 0.0, 1.7, 150.0, &[square(400.0, 0.0, 20.0, Some(30.0))]);
        assert!(!out[0].visible);
        assert!(out[0].distance_m > 150.0);
    }

    #[test]
    fn missing_height_is_estimated_and_flagged() {
        let mut b = square(100.0, 0.0, 20.0, None);
        b.building_type = "office".to_string();
        let out = viewshed(0.0, 0.0, 1.7, 500.0, &[b]);
        assert!(out[0].estimated);
        assert_eq!(out[0].height_m, 32.5);

        let mut known = square(100.0, 0.0, 20.0, Some(9.0));
        known.building_type = "office".to_string();
        let out = viewshed(0.0, 0.0, 1.7, 500.0, &[known]);
        assert!(!out[0].estimated, "a published height must win over the guess");
        assert_eq!(out[0].height_m, 9.0);
    }

    #[test]
    fn standing_inside_a_footprint_does_not_divide_by_zero() {
        let out = viewshed(0.0, 0.0, 1.7, 500.0, &[square(0.0, 0.0, 40.0, Some(20.0))]);
        assert!(out[0].visible);
        assert!(out[0].height_m.is_finite());
    }

    #[test]
    fn a_point_inside_a_footprint_is_moved_out_to_the_street() {
        // The observer is on the ground, so a point that lands on a footprint
        // means the pavement beside it -- not the roof. On the roof the block
        // occludes nothing and the street behind it reads as visible.
        let stood_on = square(0.0, 0.0, 40.0, Some(30.0));
        let behind = square(100.0, 0.0, 20.0, Some(10.0));
        let out = viewshed(0.0, 0.0, 1.7, 500.0, &[stood_on, behind]);
        assert!(out[0].visible, "the building being stood beside is in sight");
        assert!(!out[1].visible, "the block stood on must still occlude");
        // Only as far as the kerb: the 20 m half-width plus a pavement, not a
        // walk to somewhere else. (distance_m is to the nearest corner.)
        assert!(
            out[1].distance_m < 120.0,
            "the scene moved metres, not blocks: {:?}",
            out[1].distance_m
        );
    }

    #[test]
    fn eye_height_changes_what_is_hidden() {
        // From a rooftop the same low wall stops blocking the street behind it.
        let wall = square(50.0, 0.0, 10.0, Some(12.0));
        let behind = square(150.0, 0.0, 20.0, Some(6.0));
        let ground = viewshed(0.0, 0.0, 1.7, 500.0, &[wall.clone(), behind.clone()]);
        assert!(!ground[1].visible, "hidden from the pavement");
        let roof = viewshed(0.0, 0.0, 60.0, 500.0, &[wall, behind]);
        assert!(roof[1].visible, "visible from 60 m up");
    }

    #[test]
    fn an_uncertain_occluder_is_assumed_tall() {
        // An untyped "yes" guessed at its median 12 m would let the 14 m block
        // behind it show. Occluding at the 75th percentile (15 m) hides it.
        // Uncertainty must cost visibility, not manufacture it.
        let mut guesser = square(60.0, 0.0, 20.0, None);
        guesser.building_type = "yes".to_string();
        let behind = square(160.0, 0.0, 20.0, Some(14.0));
        let out = viewshed(0.0, 0.0, 1.7, 500.0, &[guesser, behind]);
        assert!(!out[1].visible, "a guessed occluder should block generously");
    }

    #[test]
    fn an_uncertain_target_is_assumed_short() {
        // Mirror image: the guessed building is now the thing being looked at,
        // behind a known 20 m wall. At its median it would peek over; at the
        // 25th percentile it does not, so it is reported hidden.
        let wall = square(60.0, 0.0, 20.0, Some(20.0));
        let mut target = square(160.0, 0.0, 20.0, None);
        target.building_type = "yes".to_string();
        let out = viewshed(0.0, 0.0, 1.7, 500.0, &[wall, target]);
        assert!(out[1].estimated);
        assert!(!out[1].visible, "a guessed target should have to prove itself");
    }

    #[test]
    fn a_measured_height_is_symmetric() {
        // The pessimism is about *uncertainty*. A published height is used
        // as-is in both roles, or the mode would start hiding things it has
        // actual measurements for.
        let wall = square(60.0, 0.0, 20.0, Some(10.0));
        let target = square(160.0, 0.0, 20.0, Some(30.0));
        let out = viewshed(0.0, 0.0, 1.7, 500.0, &[wall, target]);
        assert!(!out[0].estimated && !out[1].estimated);
        assert!(out[1].visible, "measured heights must not be second-guessed");
    }

    #[test]
    fn a_clamped_height_occludes_as_a_skyscraper() {
        // 127.5 m is the u8 ceiling, so a tower stored there could be any
        // height at all. Occluding with the literal value is what reports
        // buildings visible straight through midtown.
        let clamped = square(100.0, 0.0, 40.0, Some(127.5));
        let behind = square(300.0, 0.0, 40.0, Some(120.0));
        let out = viewshed(0.0, 0.0, 1.7, 800.0, &[clamped, behind]);
        assert!(out[0].visible);
        assert!(!out[1].visible, "assume a clamped height is a floor, not a value");
        assert_eq!(out[0].height_m, 127.5, "but still report what the file says");
    }

    #[test]
    fn a_near_zero_height_still_occludes_at_street_level() {
        // A 0.5 m published height is a bad measurement. Taken literally it
        // sits below a 1.7 m eye and blocks nothing; floored at one storey it
        // hides the low block behind it.
        let flat = square(20.0, 0.0, 20.0, Some(0.5));
        let behind = square(60.0, 0.0, 20.0, Some(2.0));
        let out = viewshed(0.0, 0.0, 1.7, 500.0, &[flat, behind]);
        assert!(out[0].visible);
        assert!(!out[1].visible, "a floored occluder must block");
        assert_eq!(out[0].height_m, 0.5, "still report the published value");
    }

    #[test]
    fn a_wide_building_does_not_block_its_whole_bearing_span() {
        // Regression for using one nearest-corner distance across the entire
        // angular span: a long slab seen obliquely would then hide things well
        // outside its actual silhouette.
        let slab = square(60.0, 60.0, 80.0, Some(15.0));
        let far_side = square(300.0, -200.0, 20.0, Some(10.0));
        let out = viewshed(0.0, 0.0, 1.7, 600.0, &[slab, far_side]);
        assert!(out[1].visible, "nothing is actually in this line of sight");
    }
}
