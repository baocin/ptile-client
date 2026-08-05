//! GPS candidate scoring: given a fix (lat/lon + accuracy + optional speed)
//! and nearby decoded features (roads/buildings/businesses), rank them by
//! an emission-probability score. Lives in core (not rookery TS) because
//! the primary targets are CoreLocation/macOS/iOS/Android — see plan
//! addendum item 2 (~/.hermes/plans/ptiles-client-extraction-plan.md).
//!
//! This is NOT a gravity well / position filter: it never mutates the fix,
//! it only returns ranked candidates. Caller (or a later HMM/transition
//! layer) decides what to do with the ranking.
//!
//! Model: emission score `exp(-d^2 / (2*sigma^2))`, `sigma` = the fix's
//! `horizontal_accuracy_m` (CoreLocation semantics), clamped to a floor so
//! a suspiciously-precise fix doesn't collapse the Gaussian to a spike.
//! `d` is:
//! - roads: point-to-segment distance (reuses `proximity::point_to_linestring_distance_m`)
//! - buildings: 0 if the fix falls inside the footprint polygon, else
//!   distance to the nearest polygon edge
//! - businesses: point distance (haversine)
//!
//! Speed gating multiplies each kind's weight based on `Fix::speed_mps`:
//! above `road_speed_gate_mps` the road weight is used, at/below it (or
//! when speed is unknown) the stationary weights are used. Weights are
//! parameters (`ScoringParams`), not hardcoded constants.

use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::Ordering;

use crate::buildings::Building;
use crate::business::Business;
use crate::proximity::{haversine_distance_m, point_to_linestring_distance_m};
use crate::roads::RoadSegment;

/// `exp` isn't in libcore; mirrors the `std`/`libm` split used elsewhere in
/// this crate (`proximity::math`, h3o's own no_std strategy).
#[cfg(feature = "std")]
#[inline]
fn exp(x: f64) -> f64 {
    x.exp()
}
#[cfg(not(feature = "std"))]
#[inline]
fn exp(x: f64) -> f64 {
    libm::exp(x)
}

/// A single GPS fix to score candidates against.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fix {
    pub lat: f64,
    pub lon: f64,
    /// CoreLocation `horizontalAccuracy`-style 1-sigma radius, in meters.
    pub horizontal_accuracy_m: f64,
    /// Instantaneous speed, m/s, if the platform provides it.
    pub speed_mps: Option<f64>,
}

/// What kind of feature a [`Candidate`] came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum CandidateKind {
    Road,
    Building,
    Business,
}

/// A ranked candidate location for a [`Fix`]. Never mutates the fix's
/// position — this is a ranking, not a snap.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Candidate {
    pub kind: CandidateKind,
    /// OSM id of the underlying feature.
    pub osm_id: i64,
    pub name: Option<String>,
    /// Raw geometric distance from the fix to the feature, in meters
    /// (0.0 for a fix inside a building footprint).
    pub distance_m: f64,
    /// Final ranking score: emission probability * kind weight (see
    /// [`ScoringParams`]). Higher is better; candidates are sorted desc.
    pub score: f64,
}

/// Tunable weights and thresholds for [`score_candidates`]. All weights
/// are plain multipliers on the Gaussian emission score, so callers can
/// retune per-platform without touching the scoring math.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScoringParams {
    /// Speed (m/s) above which a fix is considered "moving" for weighting
    /// purposes. ~3 m/s is brisk-walk/slow-bike; comfortably above normal
    /// GPS jitter while a person is standing still.
    pub road_speed_gate_mps: f64,
    /// Road weight applied when speed > `road_speed_gate_mps`.
    pub moving_road_weight: f64,
    pub moving_building_weight: f64,
    pub moving_business_weight: f64,
    /// Weights applied when speed <= `road_speed_gate_mps` (or unknown).
    pub stationary_road_weight: f64,
    pub stationary_building_weight: f64,
    pub stationary_business_weight: f64,
    /// Floor on `sigma` (== `Fix::horizontal_accuracy_m`) in meters, so an
    /// implausibly tight accuracy reading doesn't collapse the Gaussian to
    /// a near-zero-width spike that makes every candidate score ~0.
    pub sigma_floor_m: f64,
}

impl Default for ScoringParams {
    fn default() -> Self {
        ScoringParams {
            road_speed_gate_mps: 3.0,
            moving_road_weight: 1.0,
            moving_building_weight: 0.3,
            moving_business_weight: 0.3,
            stationary_road_weight: 0.3,
            stationary_building_weight: 1.0,
            stationary_business_weight: 0.8,
            sigma_floor_m: 3.0,
        }
    }
}

/// Gaussian emission score for distance `d` given std-dev `sigma`
/// (both meters). `sigma` should already be floored by the caller.
fn emission_score(d_m: f64, sigma_m: f64) -> f64 {
    let z = d_m / sigma_m;
    exp(-0.5 * z * z)
}

/// Ray-casting point-in-polygon test. `coords` are `(lon, lat)` pairs
/// (matches every decoder's `[lon, lat]` coordinate order); the ring need
/// not be explicitly closed (first point repeated as last) -- the wrap-
/// around edge `coords[n-1]-coords[0]` is included automatically.
fn point_in_polygon(lat: f64, lon: f64, coords: &[[f64; 2]]) -> bool {
    if coords.len() < 3 {
        return false;
    }
    let mut inside = false;
    let n = coords.len();
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (coords[i][0], coords[i][1]);
        let (xj, yj) = (coords[j][0], coords[j][1]);
        let intersects =
            ((yi > lat) != (yj > lat)) && (lon < (xj - xi) * (lat - yi) / (yj - yi) + xi);
        if intersects {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Distance from `(lat, lon)` to a building footprint: 0.0 if inside the
/// polygon, else the distance to the nearest edge of the ring (closing the
/// ring across `coords[n-1]-coords[0]` if it isn't already closed).
fn distance_to_polygon_m(lat: f64, lon: f64, coords: &[[f64; 2]]) -> f64 {
    if coords.len() < 2 {
        return f64::MAX;
    }
    if point_in_polygon(lat, lon, coords) {
        return 0.0;
    }
    let mut best = point_to_linestring_distance_m(lat, lon, coords)
        .map(|(_, proj)| proj.distance_m)
        .unwrap_or(f64::MAX);
    // Closing edge, if the ring wasn't already closed by the decoder.
    let first = coords[0];
    let last = coords[coords.len() - 1];
    if first != last {
        let closing = [[last[0], last[1]], [first[0], first[1]]];
        if let Some((_, proj)) = point_to_linestring_distance_m(lat, lon, &closing) {
            if proj.distance_m < best {
                best = proj.distance_m;
            }
        }
    }
    best
}

/// Score and rank road/building/business candidates against a GPS fix.
/// Returns candidates sorted descending by `score` (best match first).
/// Positions are never mutated -- this only ranks what's already decoded.
pub fn score_candidates(
    fix: &Fix,
    roads: &[RoadSegment],
    buildings: &[Building],
    businesses: &[Business],
    params: &ScoringParams,
) -> Vec<Candidate> {
    let sigma = fix.horizontal_accuracy_m.max(params.sigma_floor_m);
    let moving = fix.speed_mps.unwrap_or(0.0) > params.road_speed_gate_mps;

    let (road_weight, building_weight, business_weight) = if moving {
        (
            params.moving_road_weight,
            params.moving_building_weight,
            params.moving_business_weight,
        )
    } else {
        (
            params.stationary_road_weight,
            params.stationary_building_weight,
            params.stationary_business_weight,
        )
    };

    let mut out = Vec::with_capacity(roads.len() + buildings.len() + businesses.len());

    for road in roads {
        if let Some((_, proj)) = point_to_linestring_distance_m(fix.lat, fix.lon, &road.coords) {
            let score = emission_score(proj.distance_m, sigma) * road_weight;
            out.push(Candidate {
                kind: CandidateKind::Road,
                osm_id: road.osm_id as i64,
                name: road.name.clone(),
                distance_m: proj.distance_m,
                score,
            });
        }
    }

    for building in buildings {
        let d = distance_to_polygon_m(fix.lat, fix.lon, &building.coords);
        if d == f64::MAX {
            continue;
        }
        let score = emission_score(d, sigma) * building_weight;
        out.push(Candidate {
            kind: CandidateKind::Building,
            osm_id: building.osm_id,
            name: building.name.clone(),
            distance_m: d,
            score,
        });
    }

    for business in businesses {
        let d = haversine_distance_m(fix.lat, fix.lon, business.lat, business.lon);
        let score = emission_score(d, sigma) * business_weight;
        out.push(Candidate {
            kind: CandidateKind::Business,
            osm_id: business.osm_id,
            name: Some(business.name.clone()),
            distance_m: d,
            score,
        });
    }

    // Rank by score descending. Ties (equal scores, common when two features
    // sit at the same distance, or when both scores underflow to 0.0) are
    // broken deterministically so the ordering is stable across runs and
    // platforms: closer feature first, then a fixed kind order, then osm_id.
    // A NaN score (shouldn't happen post-guarding, but be defensive) sorts to
    // the end rather than corrupting the comparison.
    out.sort_by(|a, b| {
        let by_score = match (a.score.is_nan(), b.score.is_nan()) {
            (false, false) => b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal),
            (true, false) => Ordering::Greater, // NaN after real scores
            (false, true) => Ordering::Less,
            (true, true) => Ordering::Equal,
        };
        by_score
            .then_with(|| {
                a.distance_m
                    .partial_cmp(&b.distance_m)
                    .unwrap_or(Ordering::Equal)
            })
            .then_with(|| kind_rank(a.kind).cmp(&kind_rank(b.kind)))
            .then_with(|| a.osm_id.cmp(&b.osm_id))
    });
    out
}

/// Stable ordering key for tie-breaking equal-scored candidates of different
/// kinds. Arbitrary but fixed.
fn kind_rank(kind: CandidateKind) -> u8 {
    match kind {
        CandidateKind::Road => 0,
        CandidateKind::Building => 1,
        CandidateKind::Business => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;
    use alloc::vec;

    fn road(osm_id: u64, coords: Vec<[f64; 2]>) -> RoadSegment {
        RoadSegment {
            osm_id,
            road_class: String::from("residential"),
            coords,
            name: Some(String::from("Test Road")),
            ref_tag: None,
            oneway: None,
            speed_limit_kmh: None,
            lanes: None,
            surface: None,
            bridge_tunnel: None,
        }
    }

    fn building(osm_id: i64, coords: Vec<[f64; 2]>) -> Building {
        Building {
            osm_id,
            building_type: String::from("residential"),
            centroid_lat: 0.0,
            centroid_lon: 0.0,
            coords,
            name: Some(String::from("Test Building")),
            category: None,
            name_source: None,
            poi_osm_id: None,
            height_m: None,
        }
    }

    fn business(osm_id: i64, lat: f64, lon: f64) -> Business {
        Business {
            osm_id,
            lat,
            lon,
            name: String::from("Test Business"),
            category_idx: 0,
            phone: None,
            website: None,
            address: None,
            brand: None,
            operating_status: String::from("open"),
            emails: Vec::new(),
            socials: Vec::new(),
        }
    }

    /// Small square footprint (lon, lat) centered near `(clat, clon)`,
    /// ~`half_deg` degrees on a side.
    fn square(clat: f64, clon: f64, half_deg: f64) -> Vec<[f64; 2]> {
        vec![
            [clon - half_deg, clat - half_deg],
            [clon + half_deg, clat - half_deg],
            [clon + half_deg, clat + half_deg],
            [clon - half_deg, clat + half_deg],
        ]
    }

    #[test]
    fn moving_fix_ranks_road_first() {
        // Fix near the equator (meridian degrees ~111km/deg at the equator,
        // small offsets keep both candidates within the same neighborhood).
        let fix = Fix {
            lat: 0.0,
            lon: 0.0,
            horizontal_accuracy_m: 10.0,
            speed_mps: Some(8.0),
        };
        // Road ~10m away: a segment running north-south about 0.00009 deg
        // east of the fix (~10m at the equator).
        let d_offset = 10.0 / 111_320.0;
        let roads = vec![road(1, vec![[d_offset, -0.01], [d_offset, 0.01]])];
        // Building footprint ~15m away.
        let b_offset = 15.0 / 111_320.0;
        let buildings = vec![building(2, square(0.0, -b_offset, 0.0001))];

        let candidates = score_candidates(&fix, &roads, &buildings, &[], &ScoringParams::default());
        assert_eq!(
            candidates[0].kind,
            CandidateKind::Road,
            "candidates={candidates:?}"
        );
        assert_eq!(candidates[0].osm_id, 1);
    }

    #[test]
    fn stationary_fix_inside_building_ranks_building_first() {
        let fix = Fix {
            lat: 0.0,
            lon: 0.0,
            horizontal_accuracy_m: 10.0,
            speed_mps: Some(0.0),
        };
        // Building footprint containing the fix.
        let buildings = vec![building(1, square(0.0, 0.0, 0.0005))];
        // Road ~5m away.
        let d_offset = 5.0 / 111_320.0;
        let roads = vec![road(2, vec![[d_offset, -0.01], [d_offset, 0.01]])];

        let candidates = score_candidates(&fix, &roads, &buildings, &[], &ScoringParams::default());
        assert_eq!(
            candidates[0].kind,
            CandidateKind::Building,
            "candidates={candidates:?}"
        );
        assert_eq!(candidates[0].osm_id, 1);
        assert_eq!(candidates[0].distance_m, 0.0);
    }

    #[test]
    fn widening_sigma_flattens_ranking() {
        let roads = vec![road(1, vec![[0.0, -0.01], [0.0, 0.01]])];
        // Building ~30m further away than the road.
        let d_offset = 30.0 / 111_320.0;
        let buildings = vec![building(2, square(0.0, d_offset, 0.0001))];

        let fix_tight = Fix {
            lat: 0.0,
            lon: 0.0001,
            horizontal_accuracy_m: 5.0,
            speed_mps: Some(0.0),
        };
        let fix_wide = Fix {
            horizontal_accuracy_m: 50.0,
            ..fix_tight
        };

        let params = ScoringParams::default();
        let tight = score_candidates(&fix_tight, &roads, &buildings, &[], &params);
        let wide = score_candidates(&fix_wide, &roads, &buildings, &[], &params);

        let ratio_tight = tight[1].score / tight[0].score;
        let ratio_wide = wide[1].score / wide[0].score;
        assert!(
            ratio_wide > ratio_tight,
            "expected wider sigma to shrink the score gap (ratio closer to 1): \
             ratio_tight={ratio_tight} ratio_wide={ratio_wide}"
        );
        assert!(ratio_wide <= 1.0 + 1e-9);
    }

    // --- empty / degenerate input ---

    #[test]
    fn empty_input_yields_no_candidates() {
        let fix = Fix {
            lat: 0.0,
            lon: 0.0,
            horizontal_accuracy_m: 10.0,
            speed_mps: None,
        };
        let out = score_candidates(&fix, &[], &[], &[], &ScoringParams::default());
        assert!(out.is_empty());
    }

    #[test]
    fn degenerate_building_footprint_is_skipped() {
        let fix = Fix {
            lat: 0.0,
            lon: 0.0,
            horizontal_accuracy_m: 10.0,
            speed_mps: None,
        };
        // Building with <2 coords -> distance_to_polygon_m returns MAX -> skip.
        let buildings = vec![building(1, vec![[0.0, 0.0]])];
        let out = score_candidates(&fix, &[], &buildings, &[], &ScoringParams::default());
        assert!(out.is_empty(), "out={out:?}");
    }

    // --- scoring ordering / ranking correctness ---

    #[test]
    fn businesses_ranked_by_distance_ascending() {
        // Three businesses at increasing distance east of the fix. Same kind,
        // so score is monotone in distance -> closest ranks first.
        let fix = Fix {
            lat: 0.0,
            lon: 0.0,
            horizontal_accuracy_m: 20.0,
            speed_mps: None,
        };
        let d = 1.0 / 111_320.0; // ~1m in lon at equator
        let businesses = vec![
            business(30, 0.0, 30.0 * d),
            business(10, 0.0, 10.0 * d),
            business(20, 0.0, 20.0 * d),
        ];
        let out = score_candidates(&fix, &[], &[], &businesses, &ScoringParams::default());
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].osm_id, 10);
        assert_eq!(out[1].osm_id, 20);
        assert_eq!(out[2].osm_id, 30);
        // Scores strictly descending.
        assert!(out[0].score > out[1].score && out[1].score > out[2].score);
    }

    #[test]
    fn map_matching_picks_geometrically_closest_road() {
        // Among several roads, the ranking's top candidate must be the nearest.
        let fix = Fix {
            lat: 0.0,
            lon: 0.0,
            horizontal_accuracy_m: 15.0,
            speed_mps: Some(10.0), // moving -> road weighting
        };
        let d = 1.0 / 111_320.0;
        let roads = vec![
            road(1, vec![[40.0 * d, -0.001], [40.0 * d, 0.001]]),
            road(2, vec![[5.0 * d, -0.001], [5.0 * d, 0.001]]), // closest
            road(3, vec![[20.0 * d, -0.001], [20.0 * d, 0.001]]),
        ];
        let out = score_candidates(&fix, &roads, &[], &[], &ScoringParams::default());
        assert_eq!(out[0].osm_id, 2, "out={out:?}");
        // Its distance is the minimum of all candidates' distances.
        let min_d = out.iter().map(|c| c.distance_m).fold(f64::MAX, f64::min);
        assert!((out[0].distance_m - min_d).abs() < 1e-9);
    }

    // --- tie-breaking determinism ---

    #[test]
    fn equal_score_ties_broken_deterministically() {
        // Two businesses at the *same* distance (mirror images E/W of the fix)
        // -> identical scores. Tie-break must give a stable, repeatable order
        // (by our rule: equal distance, equal kind -> lower osm_id first).
        let fix = Fix {
            lat: 0.0,
            lon: 0.0,
            horizontal_accuracy_m: 10.0,
            speed_mps: None,
        };
        let d = 5.0 / 111_320.0;
        let businesses = vec![business(99, 0.0, d), business(7, 0.0, -d)];
        let params = ScoringParams::default();
        let a = score_candidates(&fix, &[], &[], &businesses, &params);
        // Reversed input order must produce the identical ranking.
        let businesses_rev = vec![business(7, 0.0, -d), business(99, 0.0, d)];
        let b = score_candidates(&fix, &[], &[], &businesses_rev, &params);
        assert!((a[0].score - a[1].score).abs() < 1e-12, "scores should tie");
        assert_eq!(a[0].osm_id, 7, "lower osm_id first on tie");
        assert_eq!(a[1].osm_id, 99);
        assert_eq!(
            a.iter().map(|c| c.osm_id).collect::<Vec<_>>(),
            b.iter().map(|c| c.osm_id).collect::<Vec<_>>(),
            "ranking must be independent of input order"
        );
    }

    #[test]
    fn all_scores_zero_still_sorts_by_distance() {
        // Very tight sigma + far candidates -> all emission scores underflow
        // to 0.0. Tie-break by distance keeps the closest on top.
        let fix = Fix {
            lat: 0.0,
            lon: 0.0,
            horizontal_accuracy_m: 3.0,
            speed_mps: None,
        };
        let d = 1.0 / 111_320.0;
        let businesses = vec![business(2, 0.0, 500.0 * d), business(1, 0.0, 300.0 * d)];
        let out = score_candidates(&fix, &[], &[], &businesses, &ScoringParams::default());
        assert_eq!(out[0].score, 0.0);
        assert_eq!(out[1].score, 0.0);
        assert_eq!(
            out[0].osm_id, 1,
            "closer candidate first even when scores tie at 0"
        );
    }

    // --- speed gating ---

    #[test]
    fn stationary_unknown_speed_uses_stationary_weights() {
        // Same geometry, speed None -> stationary weighting favors building.
        let fix = Fix {
            lat: 0.0,
            lon: 0.0,
            horizontal_accuracy_m: 10.0,
            speed_mps: None,
        };
        let buildings = vec![building(1, square(0.0, 0.0, 0.0005))]; // fix inside
        let d_offset = 5.0 / 111_320.0;
        let roads = vec![road(2, vec![[d_offset, -0.01], [d_offset, 0.01]])];
        let out = score_candidates(&fix, &roads, &buildings, &[], &ScoringParams::default());
        assert_eq!(out[0].kind, CandidateKind::Building, "out={out:?}");
    }

    // --- out-of-range GPS coords ---

    #[test]
    fn out_of_range_fix_does_not_panic_and_ranks() {
        // Absurd lat/lon (beyond valid range) must not panic; math stays
        // finite and the closer feature still wins.
        let fix = Fix {
            lat: 91.0,
            lon: 200.0,
            horizontal_accuracy_m: 50.0,
            speed_mps: None,
        };
        let businesses = vec![business(1, 91.0, 200.0), business(2, 88.0, 150.0)];
        let out = score_candidates(&fix, &[], &[], &businesses, &ScoringParams::default());
        assert_eq!(out.len(), 2);
        for c in &out {
            assert!(c.score.is_finite(), "score not finite: {c:?}");
            assert!(c.distance_m.is_finite(), "distance not finite: {c:?}");
        }
        // The business colocated with the fix is nearer -> ranks first.
        assert_eq!(out[0].osm_id, 1, "out={out:?}");
    }
}
