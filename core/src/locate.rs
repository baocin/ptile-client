//! "Where am I?" — the road or trail under a point, and the address nearest it.
//!
//! The pieces existed and had to be assembled by every caller: `nearest_road`
//! for roads, nothing for trails, and address records that until now carried no
//! position at all. Each caller then invented its own rule for "am I *on* this
//! or merely near it", and those rules disagreed.
//!
//! Like the router, this takes already-decoded features. I/O and cell selection
//! stay with the caller; this module only measures.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::address::AddressRecord;
use crate::proximity::{haversine_distance_m, point_to_linestring_distance_m};
use crate::roads::RoadSegment;
use crate::trails::TrailFeature;

/// How close a linear feature must be before it counts as the one you are on.
///
/// GPS on a phone is good to about 5 m in the open and much worse among tall
/// buildings, and a two-lane road with verges is ~12 m kerb to kerb, so a point
/// genuinely on a road can measure 15 m off its centreline. Beyond 25 m the
/// answer is "near", not "on" — the caller still gets the distance and can
/// decide for itself.
pub const ON_WAY_THRESHOLD_M: f64 = 25.0;

/// Search radius for the nearest address. A rural driveway can sit 80 m from
/// the road, so a tighter bound would report "no address" on exactly the roads
/// where the address is most useful.
pub const ADDRESS_THRESHOLD_M: f64 = 150.0;

/// A linear feature the query point is on or near.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct NearbyWay {
    /// Index into the slice that was searched.
    pub index: usize,
    /// `road` or `trail` — which slice `index` refers to.
    pub kind: String,
    /// Street or trail name, when the feature carries one.
    pub name: Option<String>,
    /// Road class (`residential`, `motorway`) or trail type (`path`, `track`).
    pub class: String,
    /// Perpendicular distance from the query point, in metres.
    pub distance_m: f64,
    /// Closest point on the feature, `(lat, lon)`.
    pub snapped: (f64, f64),
    /// True when `distance_m <= ON_WAY_THRESHOLD_M`.
    pub on_it: bool,
}

/// An address near the query point.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct NearbyAddress {
    pub osm_id: i64,
    pub housenumber: String,
    pub street: String,
    pub lat: f64,
    pub lon: f64,
    pub distance_m: f64,
}

/// Everything known about a point from the layers the caller supplied.
#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Located {
    /// The nearest way of either kind, whether or not it is within the
    /// on-way threshold. `None` only when no ways were supplied or none had
    /// usable geometry.
    pub nearest_way: Option<NearbyWay>,
    /// The way the point is actually on, if any is within the threshold.
    /// A subset of `nearest_way`: when set, they are the same feature.
    pub on_way: Option<NearbyWay>,
    /// Closest address within `ADDRESS_THRESHOLD_M`.
    pub address: Option<NearbyAddress>,
}

fn way_from_road(index: usize, road: &RoadSegment, lat: f64, lon: f64) -> Option<NearbyWay> {
    let (_, proj) = point_to_linestring_distance_m(lat, lon, &road.coords)?;
    Some(NearbyWay {
        index,
        kind: String::from("road"),
        name: road.name.clone(),
        class: road.road_class.clone(),
        distance_m: proj.distance_m,
        snapped: proj.snapped,
        on_it: proj.distance_m <= ON_WAY_THRESHOLD_M,
    })
}

fn way_from_trail(index: usize, trail: &TrailFeature, lat: f64, lon: f64) -> Option<NearbyWay> {
    // Trailheads are points, not ways; a point has no centreline to be "on".
    if trail.geom_type != 0 || trail.coords.len() < 2 {
        return None;
    }
    let (_, proj) = point_to_linestring_distance_m(lat, lon, &trail.coords)?;
    Some(NearbyWay {
        index,
        kind: String::from("trail"),
        name: trail.name.clone(),
        class: trail.trail_type.clone(),
        distance_m: proj.distance_m,
        snapped: proj.snapped,
        on_it: proj.distance_m <= ON_WAY_THRESHOLD_M,
    })
}

/// The nearest road to a point, as a [`NearbyWay`].
pub fn nearest_road_way(lat: f64, lon: f64, roads: &[RoadSegment]) -> Option<NearbyWay> {
    roads
        .iter()
        .enumerate()
        .filter_map(|(i, r)| way_from_road(i, r, lat, lon))
        .min_by(|a, b| a.distance_m.total_cmp(&b.distance_m))
}

/// The nearest trail to a point. Trailhead points are skipped: this answers
/// "which trail am I walking on".
pub fn nearest_trail(lat: f64, lon: f64, trails: &[TrailFeature]) -> Option<NearbyWay> {
    trails
        .iter()
        .enumerate()
        .filter_map(|(i, t)| way_from_trail(i, t, lat, lon))
        .min_by(|a, b| a.distance_m.total_cmp(&b.distance_m))
}

/// The nearest address within `threshold_m`.
///
/// Records without a position are skipped rather than placed at their cell
/// centre: a res-7 cell is roughly 5 km across, so a cell-centre "nearest
/// address" would beat a real one 2 km away and be wrong every time. v1 files
/// therefore return `None` here, which is the honest answer.
pub fn nearest_address(
    lat: f64,
    lon: f64,
    addresses: &[AddressRecord],
    threshold_m: f64,
) -> Option<NearbyAddress> {
    addresses
        .iter()
        .filter_map(|a| {
            let (alat, alon) = (a.lat?, a.lon?);
            let d = haversine_distance_m(lat, lon, alat, alon);
            (d <= threshold_m).then(|| NearbyAddress {
                osm_id: a.osm_id,
                housenumber: a.housenumber.clone(),
                street: a.street.clone(),
                lat: alat,
                lon: alon,
                distance_m: d,
            })
        })
        .min_by(|a, b| a.distance_m.total_cmp(&b.distance_m))
}

/// Reverse geocode: what is at this point.
///
/// Roads and trails compete on distance alone. A rail trail often runs beside
/// the road it replaced, and a park path can parallel a street within a few
/// metres, so preferring one kind outright would answer confidently and wrongly
/// depending on which way the user is actually travelling. Closest wins; the
/// caller gets both distances and can apply its own bias (a hiking app should
/// prefer the trail, a car app the road).
pub fn locate(
    lat: f64,
    lon: f64,
    roads: &[RoadSegment],
    trails: &[TrailFeature],
    addresses: &[AddressRecord],
) -> Located {
    let road = nearest_road_way(lat, lon, roads);
    let trail = nearest_trail(lat, lon, trails);

    let nearest_way = match (road, trail) {
        (Some(r), Some(t)) => Some(if t.distance_m < r.distance_m { t } else { r }),
        (Some(r), None) => Some(r),
        (None, Some(t)) => Some(t),
        (None, None) => None,
    };
    let on_way = nearest_way.clone().filter(|w| w.on_it);

    Located {
        nearest_way,
        on_way,
        address: nearest_address(lat, lon, addresses, ADDRESS_THRESHOLD_M),
    }
}

/// Forward geocode: address records matching a typed query, best first.
///
/// Deliberately not a full geocoder. It matches over the records the caller has
/// already loaded — a viewport, a cell ring — because the address layer has no
/// name index to search a whole state through. `"400 Broadway"` and `"broadway"`
/// both work; the number, when given, must match exactly, since a house number
/// is an identifier rather than a word to fuzzy-match.
pub fn match_addresses(
    query: &str,
    addresses: &[AddressRecord],
    limit: usize,
) -> Vec<NearbyAddress> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    // A leading run of digits is a house number; the rest is the street.
    let digits: String = q.chars().take_while(|c| c.is_ascii_digit()).collect();
    let street_q = q[digits.len()..].trim().to_string();

    let mut hits: Vec<NearbyAddress> = addresses
        .iter()
        .filter_map(|a| {
            if !digits.is_empty() && a.housenumber.to_lowercase() != digits {
                return None;
            }
            let street = a.street.to_lowercase();
            if !street_q.is_empty() && !street.contains(&street_q) {
                return None;
            }
            // With neither part supplied there is nothing to match on.
            if digits.is_empty() && street_q.is_empty() {
                return None;
            }
            Some(NearbyAddress {
                osm_id: a.osm_id,
                housenumber: a.housenumber.clone(),
                street: a.street.clone(),
                lat: a.lat?,
                lon: a.lon?,
                distance_m: 0.0,
            })
        })
        .collect();

    // Prefix matches on the street first, then shorter names: "Broadway"
    // should outrank "West Broadway Circle" for the query "broadway".
    hits.sort_by(|a, b| {
        let ap = a.street.to_lowercase().starts_with(&street_q);
        let bp = b.street.to_lowercase().starts_with(&street_q);
        bp.cmp(&ap).then(a.street.len().cmp(&b.street.len()))
    });
    hits.truncate(limit);
    hits
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;

    fn road(name: &str, class: &str, coords: Vec<[f64; 2]>) -> RoadSegment {
        RoadSegment {
            osm_id: 1,
            road_class: class.to_string(),
            coords,
            name: Some(name.to_string()),
            ref_tag: None,
            oneway: None,
            speed_limit_kmh: None,
            lanes: None,
            surface: None,
            bridge_tunnel: None,
        }
    }

    fn trail(name: &str, ttype: &str, coords: Vec<[f64; 2]>) -> TrailFeature {
        TrailFeature {
            osm_id: 2,
            trail_type: ttype.to_string(),
            geom_type: 0,
            coords,
            surface: String::new(),
            sac_scale: String::new(),
            name: Some(name.to_string()),
        }
    }

    fn addr(id: i64, num: &str, street: &str, lat: f64, lon: f64) -> AddressRecord {
        AddressRecord {
            osm_id: id,
            housenumber: num.to_string(),
            street: street.to_string(),
            lat: Some(lat),
            lon: Some(lon),
        }
    }

    // A road running east along lat 36.0, and a trail 100 m north of it.
    fn scene() -> (Vec<RoadSegment>, Vec<TrailFeature>) {
        (
            vec![road("Broadway", "residential", vec![[-86.80, 36.0], [-86.79, 36.0]])],
            vec![trail("Greenway", "path", vec![[-86.80, 36.0009], [-86.79, 36.0009]])],
        )
    }

    #[test]
    fn a_point_on_the_road_reports_the_road() {
        let (roads, trails) = scene();
        let got = locate(36.0, -86.795, &roads, &trails, &[]);
        let on = got.on_way.expect("should be on something");
        assert_eq!(on.kind, "road");
        assert_eq!(on.name.as_deref(), Some("Broadway"));
        assert!(on.distance_m < 1.0, "distance {}", on.distance_m);
    }

    #[test]
    fn a_point_on_the_trail_reports_the_trail_not_the_road() {
        let (roads, trails) = scene();
        let got = locate(36.0009, -86.795, &roads, &trails, &[]);
        let on = got.on_way.expect("should be on something");
        assert_eq!(on.kind, "trail", "closest wins; the road is 100 m away");
        assert_eq!(on.name.as_deref(), Some("Greenway"));
    }

    #[test]
    fn between_the_two_nothing_is_on_but_nearest_still_answers() {
        let (roads, trails) = scene();
        // ~50 m north of the road, ~50 m south of the trail: near both, on neither.
        let got = locate(36.00045, -86.795, &roads, &trails, &[]);
        assert!(got.on_way.is_none(), "50 m off is not 'on'");
        let near = got.nearest_way.expect("still reports the nearest");
        assert!(near.distance_m > ON_WAY_THRESHOLD_M);
        assert!(!near.on_it);
    }

    #[test]
    fn trailheads_are_not_offered_as_ways() {
        let mut th = trail("Trailhead", "trailhead", vec![[-86.795, 36.0]]);
        th.geom_type = 1;
        assert!(nearest_trail(36.0, -86.795, &[th]).is_none());
    }

    #[test]
    fn nearest_address_picks_the_closest_and_respects_the_threshold() {
        let a = vec![
            addr(1, "400", "Broadway", 36.0001, -86.795),
            addr(2, "402", "Broadway", 36.0020, -86.795),
        ];
        let got = nearest_address(36.0, -86.795, &a, ADDRESS_THRESHOLD_M).unwrap();
        assert_eq!(got.housenumber, "400");
        // Both are outside a 5 m threshold.
        assert!(nearest_address(36.0, -86.795, &a, 5.0).is_none());
    }

    #[test]
    fn addresses_without_a_position_are_skipped_not_guessed() {
        // A v1 record: no coordinates. Placing it at the cell centre would
        // make it "nearest" from anywhere in a 5 km cell.
        let v1 = AddressRecord {
            osm_id: 9,
            housenumber: "1".into(),
            street: "Nowhere".into(),
            lat: None,
            lon: None,
        };
        assert!(nearest_address(36.0, -86.795, &[v1], ADDRESS_THRESHOLD_M).is_none());
    }

    #[test]
    fn forward_geocode_matches_number_and_street() {
        let a = vec![
            addr(1, "400", "Broadway", 36.0, -86.79),
            addr(2, "402", "Broadway", 36.0, -86.79),
            addr(3, "400", "West Broadway Circle", 36.0, -86.79),
        ];
        let hits = match_addresses("400 broadway", &a, 10);
        assert_eq!(hits.len(), 2, "both 400s match the street substring");
        assert_eq!(hits[0].street, "Broadway", "prefix match ranks first");

        let street_only = match_addresses("broadway", &a, 10);
        assert_eq!(street_only.len(), 3);

        assert_eq!(match_addresses("999 broadway", &a, 10).len(), 0);
        assert_eq!(match_addresses("", &a, 10).len(), 0);
        assert_eq!(match_addresses("   ", &a, 10).len(), 0);
    }

    #[test]
    fn forward_geocode_respects_the_limit() {
        let a: Vec<AddressRecord> = (0..50)
            .map(|i| addr(i, "1", "Broadway", 36.0, -86.79))
            .collect();
        assert_eq!(match_addresses("broadway", &a, 7).len(), 7);
    }
}
