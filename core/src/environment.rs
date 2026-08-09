//! Explainable indoor/outdoor estimation from a GPS fix and building footprints.
//!
//! This is deliberately a **map heuristic**, not an indoor-positioning system.
//! A coordinate inside a mapped, enclosed footprint is evidence for being
//! indoors; a sufficiently accurate coordinate whose uncertainty circle is
//! clear of every nearby footprint is evidence for being outdoors. Edge hits,
//! poor fixes, open-sided structures, and incomplete building coverage remain
//! uncertain instead of being forced into a binary answer.
//!
//! Callers must pass all building footprints which could intersect the fix's
//! accuracy circle. With PTiles that normally means the containing H3 cell plus
//! ring 1. `coverage_complete` must be false when those cells were not loaded or
//! the building layer does not cover the coordinate; absence of map data is not
//! evidence for being outdoors.

use crate::buildings::Building;
use crate::proximity::{point_in_polygon, point_to_ring_distance_m};
use crate::scoring::Fix;

/// The map-supported environment estimate for a fix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum IndoorOutdoor {
    Indoor,
    Outdoor,
    Uncertain,
}

/// The strongest piece of evidence behind an [`IndoorOutdoorEstimate`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum IndoorOutdoorReason {
    /// The fix falls inside an apparently enclosed building footprint.
    InsideBuilding,
    /// The fix is inside a footprint tagged as open-sided cover.
    InsideOpenStructure,
    /// The accuracy circle reaches a nearby footprint or its boundary.
    AccuracyOverlapsBuilding,
    /// The accuracy circle plus the configured margin is clear of footprints.
    ClearOfBuildings,
    /// No usable building geometry was supplied, but coverage is complete.
    NoBuildingsNearby,
    /// The caller could not assert complete building coverage around the fix.
    IncompleteCoverage,
    /// The coordinate or horizontal accuracy is invalid.
    InvalidFix,
    /// The fix is valid but too imprecise for a footprint-level decision.
    PoorAccuracy,
}

/// Tunables for [`estimate_indoor_outdoor`].
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct IndoorOutdoorParams {
    /// Fixes worse than this cannot settle a footprint-level answer.
    pub max_accuracy_m: f64,
    /// Extra clearance beyond the reported accuracy circle required before an
    /// outside point is called `Outdoor`.
    pub outdoor_clearance_m: f64,
}

impl Default for IndoorOutdoorParams {
    fn default() -> Self {
        IndoorOutdoorParams {
            max_accuracy_m: 50.0,
            outdoor_clearance_m: 3.0,
        }
    }
}

/// An explainable indoor/outdoor answer.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct IndoorOutdoorEstimate {
    pub classification: IndoorOutdoor,
    /// Heuristic confidence in `0.0..=1.0`. `Uncertain` answers intentionally
    /// stay at or below 0.5.
    pub confidence: f64,
    pub reason: IndoorOutdoorReason,
    /// Index into the `buildings` slice for the footprint that drove the
    /// answer, when there was one.
    pub building_index: Option<usize>,
    pub building_osm_id: Option<i64>,
    /// Distance to that footprint's boundary. For an inside point this is the
    /// depth inside the footprint; for an outside point it is the clearance.
    pub distance_to_boundary_m: Option<f64>,
}

impl IndoorOutdoorEstimate {
    fn uncertain(reason: IndoorOutdoorReason) -> Self {
        IndoorOutdoorEstimate {
            classification: IndoorOutdoor::Uncertain,
            confidence: 0.0,
            reason,
            building_index: None,
            building_osm_id: None,
            distance_to_boundary_m: None,
        }
    }

    fn with_building(
        mut self,
        index: usize,
        building: &Building,
        distance_to_boundary_m: f64,
    ) -> Self {
        self.building_index = Some(index);
        self.building_osm_id = Some(building.osm_id);
        self.distance_to_boundary_m = Some(distance_to_boundary_m);
        self
    }
}

/// Whether a building tag describes cover which is normally open to outside
/// air. Being inside one of these polygons is intentionally not enough to call
/// a person indoors.
pub fn building_type_is_open_air(building_type: &str) -> bool {
    matches!(building_type, "roof" | "carport" | "canopy")
}

/// Estimate whether `fix` is indoors or outdoors from nearby buildings.
///
/// `coverage_complete` is part of the evidence, not a performance hint. Pass
/// false if the building layer is missing, does not cover the coordinate, or
/// not all cells intersecting the accuracy circle were loaded. Incomplete
/// coverage can still support an indoor answer when the fix is inside a known
/// footprint, but it can never support an outdoor answer.
pub fn estimate_indoor_outdoor(
    fix: &Fix,
    buildings: &[Building],
    coverage_complete: bool,
    params: &IndoorOutdoorParams,
) -> IndoorOutdoorEstimate {
    if !valid_fix(fix)
        || !params.max_accuracy_m.is_finite()
        || params.max_accuracy_m < 0.0
        || !params.outdoor_clearance_m.is_finite()
        || params.outdoor_clearance_m < 0.0
    {
        return IndoorOutdoorEstimate::uncertain(IndoorOutdoorReason::InvalidFix);
    }

    // Deepest containing footprint wins when buildings overlap. Otherwise
    // retain the nearest boundary for outside/edge evidence.
    let mut inside: Option<(usize, &Building, f64)> = None;
    let mut nearest: Option<(usize, &Building, f64)> = None;
    for (index, building) in buildings.iter().enumerate() {
        let Some(distance) = point_to_ring_distance_m(fix.lat, fix.lon, &building.coords) else {
            continue;
        };
        if !distance.is_finite() {
            continue;
        }
        if point_in_polygon(fix.lat, fix.lon, &building.coords) {
            if inside.is_none_or(|(_, _, best)| distance > best) {
                inside = Some((index, building, distance));
            }
        } else if nearest.is_none_or(|(_, _, best)| distance < best) {
            nearest = Some((index, building, distance));
        }
    }

    if fix.horizontal_accuracy_m > params.max_accuracy_m {
        let mut answer = IndoorOutdoorEstimate::uncertain(IndoorOutdoorReason::PoorAccuracy);
        answer.confidence = 0.1;
        if let Some((index, building, depth)) = inside.or(nearest) {
            return answer.with_building(index, building, depth);
        }
        return answer;
    }

    if let Some((index, building, depth)) = inside {
        let accuracy = fix.horizontal_accuracy_m.max(1.0);
        // Inside a footprint starts as weak positive evidence. Depth relative
        // to the accuracy radius strengthens it, but never claims certainty:
        // footprints, source data, and GNSS can all be wrong.
        let confidence = (0.5 + 0.45 * depth / (depth + accuracy)).min(0.95);
        if building_type_is_open_air(&building.building_type) {
            return IndoorOutdoorEstimate {
                classification: IndoorOutdoor::Uncertain,
                confidence: confidence.min(0.5),
                reason: IndoorOutdoorReason::InsideOpenStructure,
                building_index: None,
                building_osm_id: None,
                distance_to_boundary_m: None,
            }
            .with_building(index, building, depth);
        }
        return IndoorOutdoorEstimate {
            classification: IndoorOutdoor::Indoor,
            confidence,
            reason: IndoorOutdoorReason::InsideBuilding,
            building_index: None,
            building_osm_id: None,
            distance_to_boundary_m: None,
        }
        .with_building(index, building, depth);
    }

    if !coverage_complete {
        let mut answer = IndoorOutdoorEstimate::uncertain(IndoorOutdoorReason::IncompleteCoverage);
        answer.confidence = 0.1;
        if let Some((index, building, distance)) = nearest {
            return answer.with_building(index, building, distance);
        }
        return answer;
    }

    let Some((index, building, distance)) = nearest else {
        return IndoorOutdoorEstimate {
            classification: IndoorOutdoor::Outdoor,
            confidence: 0.8,
            reason: IndoorOutdoorReason::NoBuildingsNearby,
            building_index: None,
            building_osm_id: None,
            distance_to_boundary_m: None,
        };
    };

    let required = fix.horizontal_accuracy_m + params.outdoor_clearance_m;
    if distance >= required {
        let excess = distance - required;
        let confidence = (0.55 + 0.4 * excess / (distance + required + 1.0)).min(0.95);
        IndoorOutdoorEstimate {
            classification: IndoorOutdoor::Outdoor,
            confidence,
            reason: IndoorOutdoorReason::ClearOfBuildings,
            building_index: None,
            building_osm_id: None,
            distance_to_boundary_m: None,
        }
        .with_building(index, building, distance)
    } else {
        let overlap = (required - distance) / required.max(1.0);
        IndoorOutdoorEstimate {
            classification: IndoorOutdoor::Uncertain,
            confidence: (0.5 * (1.0 - overlap)).clamp(0.0, 0.5),
            reason: IndoorOutdoorReason::AccuracyOverlapsBuilding,
            building_index: None,
            building_osm_id: None,
            distance_to_boundary_m: None,
        }
        .with_building(index, building, distance)
    }
}

fn valid_fix(fix: &Fix) -> bool {
    fix.lat.is_finite()
        && (-90.0..=90.0).contains(&fix.lat)
        && fix.lon.is_finite()
        && (-180.0..=180.0).contains(&fix.lon)
        && fix.horizontal_accuracy_m.is_finite()
        && fix.horizontal_accuracy_m >= 0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn building(osm_id: i64, lat: f64, lon: f64, half: f64, kind: &str) -> Building {
        Building {
            osm_id,
            building_type: kind.into(),
            centroid_lat: lat,
            centroid_lon: lon,
            coords: alloc::vec![
                [lon - half, lat - half],
                [lon + half, lat - half],
                [lon + half, lat + half],
                [lon - half, lat + half],
            ],
            name: None,
            category: None,
            name_source: None,
            poi_osm_id: None,
            height_m: None,
        }
    }

    fn fix(lat: f64, lon: f64, accuracy: f64) -> Fix {
        Fix {
            lat,
            lon,
            horizontal_accuracy_m: accuracy,
            speed_mps: None,
        }
    }

    #[test]
    fn point_inside_enclosed_footprint_is_indoor() {
        let b = building(42, 36.0, -86.8, 0.0001, "house");
        let got = estimate_indoor_outdoor(
            &fix(36.0, -86.8, 5.0),
            &[b],
            true,
            &IndoorOutdoorParams::default(),
        );
        assert_eq!(got.classification, IndoorOutdoor::Indoor);
        assert_eq!(got.reason, IndoorOutdoorReason::InsideBuilding);
        assert_eq!(got.building_osm_id, Some(42));
        assert!(got.distance_to_boundary_m.unwrap() > 8.0);
        assert!(got.confidence > 0.7);
    }

    #[test]
    fn open_sided_cover_is_not_called_indoor() {
        let b = building(7, 36.0, -86.8, 0.0001, "canopy");
        let got = estimate_indoor_outdoor(
            &fix(36.0, -86.8, 3.0),
            &[b],
            true,
            &IndoorOutdoorParams::default(),
        );
        assert_eq!(got.classification, IndoorOutdoor::Uncertain);
        assert_eq!(got.reason, IndoorOutdoorReason::InsideOpenStructure);
    }

    #[test]
    fn accurate_fix_clear_of_buildings_is_outdoor() {
        let b = building(9, 36.0, -86.8, 0.00005, "house");
        let got = estimate_indoor_outdoor(
            &fix(36.0, -86.7995, 4.0),
            &[b],
            true,
            &IndoorOutdoorParams::default(),
        );
        assert_eq!(got.classification, IndoorOutdoor::Outdoor);
        assert_eq!(got.reason, IndoorOutdoorReason::ClearOfBuildings);
        assert!(got.distance_to_boundary_m.unwrap() > 30.0);
    }

    #[test]
    fn edge_uncertainty_is_not_forced_to_either_state() {
        let b = building(9, 36.0, -86.8, 0.00005, "house");
        let got = estimate_indoor_outdoor(
            &fix(36.0, -86.7999, 10.0),
            &[b],
            true,
            &IndoorOutdoorParams::default(),
        );
        assert_eq!(got.classification, IndoorOutdoor::Uncertain);
        assert_eq!(got.reason, IndoorOutdoorReason::AccuracyOverlapsBuilding);
    }

    #[test]
    fn missing_coverage_never_proves_outdoor() {
        let got = estimate_indoor_outdoor(
            &fix(36.0, -86.8, 4.0),
            &[],
            false,
            &IndoorOutdoorParams::default(),
        );
        assert_eq!(got.classification, IndoorOutdoor::Uncertain);
        assert_eq!(got.reason, IndoorOutdoorReason::IncompleteCoverage);
    }

    #[test]
    fn complete_empty_area_can_be_outdoor() {
        let got = estimate_indoor_outdoor(
            &fix(36.0, -86.8, 4.0),
            &[],
            true,
            &IndoorOutdoorParams::default(),
        );
        assert_eq!(got.classification, IndoorOutdoor::Outdoor);
        assert_eq!(got.reason, IndoorOutdoorReason::NoBuildingsNearby);
    }

    #[test]
    fn poor_or_invalid_fix_stays_uncertain() {
        let b = building(42, 36.0, -86.8, 0.0001, "house");
        let poor = estimate_indoor_outdoor(
            &fix(36.0, -86.8, 80.0),
            &[b.clone()],
            true,
            &IndoorOutdoorParams::default(),
        );
        assert_eq!(poor.classification, IndoorOutdoor::Uncertain);
        assert_eq!(poor.reason, IndoorOutdoorReason::PoorAccuracy);
        assert_eq!(poor.building_osm_id, Some(42));

        let invalid = estimate_indoor_outdoor(
            &fix(f64::NAN, -86.8, 5.0),
            &[b],
            true,
            &IndoorOutdoorParams::default(),
        );
        assert_eq!(invalid.reason, IndoorOutdoorReason::InvalidFix);
    }
}
