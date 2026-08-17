//! "Where am I?" — the road or trail under a point, and the address nearest it.
//!
//! The pieces existed and had to be assembled by every caller: `nearest_road`
//! for roads, nothing for trails, and address records that until now carried no
//! position at all. Each caller then invented its own rule for "am I *on* this
//! or merely near it", and those rules disagreed.
//!
//! Like the router, this takes already-decoded features. I/O and cell selection
//! stay with the caller; this module only measures.

use alloc::string::String;
use alloc::vec::Vec;

use crate::address::AddressRecord;
use crate::parks::ParkFeature;
use crate::proximity::{
    haversine_distance_m, point_in_polygon, point_to_linestring_distance_m,
    point_to_ring_distance_m,
};
use crate::rail::RailFeature;
use crate::roads::RoadSegment;
use crate::trails::TrailFeature;
use crate::water::WaterFeature;

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

/// How much further a road may be and still win on being the one a person
/// would name. A click or a GPS fix is good to roughly this much, so within
/// the band "closest" is not evidence of anything.
pub const ROAD_TIE_BREAK_M: f64 = 15.0;

/// Lower is better. An unnamed alley, driveway or parking aisle is almost
/// never the answer to "what road am I on" when a named street is the same
/// distance away, and pure nearest-centreline picked the alley every time.
fn road_rank(w: &NearbyWay) -> u8 {
    let minor = matches!(
        w.class.as_str(),
        "service" | "track" | "footway" | "cycleway" | "path" | "pedestrian"
    );
    let unnamed = w.name.as_deref().map_or(true, |n| n.trim().is_empty());
    (unnamed as u8) * 2 + (minor as u8)
}

/// The road a point is most likely *on*: nearest, but a better-ranked road
/// within [`ROAD_TIE_BREAK_M`] of the nearest wins. Distances reported are
/// still the true ones.
pub fn best_road_way(lat: f64, lon: f64, roads: &[RoadSegment]) -> Option<NearbyWay> {
    let ways: Vec<NearbyWay> = roads
        .iter()
        .enumerate()
        .filter_map(|(i, r)| way_from_road(i, r, lat, lon))
        .collect();
    let best = ways
        .iter()
        .map(|w| w.distance_m)
        .fold(f64::INFINITY, f64::min);
    ways.into_iter()
        .filter(|w| w.distance_m <= best + ROAD_TIE_BREAK_M)
        .min_by(|a, b| {
            road_rank(a)
                .cmp(&road_rank(b))
                .then(a.distance_m.total_cmp(&b.distance_m))
        })
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

/// An area feature (a park polygon, a lake, a river) the point is in or near.
///
/// Separate from [`NearbyWay`] because "on it" and "in it" are different
/// questions: a way answers with a snapped point on a centreline, an area
/// answers with containment, and collapsing the two would make `snapped`
/// meaningless for a polygon.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct NearbyArea {
    /// Index into the slice that was searched.
    pub index: usize,
    /// `park` or `water` — which slice `index` refers to.
    pub kind: String,
    /// Park or water body name, when the feature carries one.
    pub name: Option<String>,
    /// Park type (`park`, `nature_reserve`) or water type (`lake`, `river`).
    pub class: String,
    /// Distance to the feature's boundary in metres, `0.0` when inside it.
    pub distance_m: f64,
    /// True when the point falls inside the polygon.
    pub inside: bool,
}

/// A point feature (a trailhead, a station) near the query point.
///
/// The linear lookups skip these deliberately — a point has no centreline to
/// be on — so they need their own answer rather than being dropped.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct NearbyPoint {
    /// Index into the slice that was searched.
    pub index: usize,
    /// `trailhead` or `station`.
    pub kind: String,
    pub name: Option<String>,
    /// Trail type (`trailhead`) or rail type (`station`, `halt`).
    pub class: String,
    pub lat: f64,
    pub lon: f64,
    pub distance_m: f64,
}

fn way_from_rail(index: usize, rail: &RailFeature, lat: f64, lon: f64) -> Option<NearbyWay> {
    // Stations are points, same as trailheads; `nearest_station` answers those.
    if rail.geom_type != 0 || rail.coords.len() < 2 {
        return None;
    }
    let (_, proj) = point_to_linestring_distance_m(lat, lon, &rail.coords)?;
    Some(NearbyWay {
        index,
        kind: String::from("rail"),
        name: rail.name.clone(),
        class: rail.rail_type.clone(),
        distance_m: proj.distance_m,
        snapped: proj.snapped,
        on_it: proj.distance_m <= ON_WAY_THRESHOLD_M,
    })
}

/// The nearest rail line to a point. Station points are skipped: this answers
/// "which track is this", not "which platform".
pub fn nearest_rail(lat: f64, lon: f64, rail: &[RailFeature]) -> Option<NearbyWay> {
    rail.iter()
        .enumerate()
        .filter_map(|(i, r)| way_from_rail(i, r, lat, lon))
        .min_by(|a, b| a.distance_m.total_cmp(&b.distance_m))
}

fn nearest_point_feature<'a, T>(
    lat: f64,
    lon: f64,
    features: &'a [T],
    kind: &str,
    is_point: impl Fn(&T) -> bool,
    parts: impl Fn(&'a T) -> (Option<String>, String, &'a [[f64; 2]]),
) -> Option<NearbyPoint> {
    features
        .iter()
        .enumerate()
        .filter(|(_, f)| is_point(f))
        .filter_map(|(i, f)| {
            let (name, class, coords) = parts(f);
            let [flon, flat] = *coords.first()?;
            Some(NearbyPoint {
                index: i,
                kind: String::from(kind),
                name,
                class,
                lat: flat,
                lon: flon,
                distance_m: haversine_distance_m(lat, lon, flat, flon),
            })
        })
        .min_by(|a, b| a.distance_m.total_cmp(&b.distance_m))
}

/// The nearest trailhead — the point feature a trail network is entered at,
/// which is what a caller planning to *start* a walk wants, as opposed to
/// [`nearest_trail`]'s "which path am I on".
pub fn nearest_trailhead(lat: f64, lon: f64, trails: &[TrailFeature]) -> Option<NearbyPoint> {
    nearest_point_feature(
        lat,
        lon,
        trails,
        "trailhead",
        |t| t.geom_type == 1,
        |t| (t.name.clone(), t.trail_type.clone(), &t.coords),
    )
}

/// The nearest rail station/halt point.
pub fn nearest_station(lat: f64, lon: f64, rail: &[RailFeature]) -> Option<NearbyPoint> {
    nearest_point_feature(
        lat,
        lon,
        rail,
        "station",
        |r| r.geom_type == 1,
        |r| (r.name.clone(), r.rail_type.clone(), &r.coords),
    )
}

/// Rank areas the way a caller reads them: the one you are inside wins, and
/// among equals the closer boundary wins. Without the `inside` tiebreak a
/// large park you are standing in the middle of loses to a small one whose
/// edge is nearer.
fn best_area(mut areas: Vec<NearbyArea>) -> Option<NearbyArea> {
    areas.sort_by(|a, b| {
        b.inside
            .cmp(&a.inside)
            .then(a.distance_m.total_cmp(&b.distance_m))
    });
    areas.into_iter().next()
}

fn area_of(
    index: usize,
    kind: &str,
    name: Option<String>,
    class: String,
    coords: &[[f64; 2]],
    lat: f64,
    lon: f64,
) -> Option<NearbyArea> {
    let inside = point_in_polygon(lat, lon, coords);
    let distance_m = if inside {
        0.0
    } else {
        point_to_ring_distance_m(lat, lon, coords)?
    };
    Some(NearbyArea {
        index,
        kind: String::from(kind),
        name,
        class,
        distance_m,
        inside,
    })
}

/// The park at a point: the polygon containing it, else the nearest park
/// boundary. `None` when no park has usable geometry.
pub fn park_at(lat: f64, lon: f64, parks: &[ParkFeature]) -> Option<NearbyArea> {
    best_area(
        parks
            .iter()
            .enumerate()
            .filter_map(|(i, p)| {
                area_of(i, "park", p.name.clone(), p.park_type.clone(), &p.coords, lat, lon)
            })
            .collect(),
    )
}

/// The water at a point: the polygon containing it, else the nearest water
/// feature. Linestring water (a river centreline, `geom_type == 1`) is never
/// "inside" — it has no area — so it competes on distance alone, and
/// reference geometries (`geom_type == 2`, coordinates held elsewhere in the
/// file) are skipped rather than reported at a position they do not carry.
pub fn water_at(lat: f64, lon: f64, water: &[WaterFeature]) -> Option<NearbyArea> {
    best_area(
        water
            .iter()
            .enumerate()
            .filter_map(|(i, w)| match w.geom_type {
                0 => area_of(i, "water", w.name.clone(), w.water_type.clone(), &w.coords, lat, lon),
                1 => {
                    let (_, proj) = point_to_linestring_distance_m(lat, lon, &w.coords)?;
                    Some(NearbyArea {
                        index: i,
                        kind: String::from("water"),
                        name: w.name.clone(),
                        class: w.water_type.clone(),
                        distance_m: proj.distance_m,
                        inside: false,
                    })
                }
                _ => None,
            })
            .collect(),
    )
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
    let road = best_road_way(lat, lon, roads);
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
    let street_q = crate::address::fold_street_for_match(q[digits.len()..].trim());

    let mut hits: Vec<NearbyAddress> = addresses
        .iter()
        .filter_map(|a| {
            if !digits.is_empty() && a.housenumber.to_lowercase() != digits {
                return None;
            }
            // Folded on both sides so "Beale Street" and "Beale St" are the
            // same street -- see address::fold_street_for_match.
            let street = crate::address::fold_street_for_match(&a.street);
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
        let ap = crate::address::fold_street_for_match(&a.street).starts_with(&street_q);
        let bp = crate::address::fold_street_for_match(&b.street).starts_with(&street_q);
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
            source: Default::default(),
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
    fn a_named_street_beats_a_nearer_unnamed_alley() {
        // Alley 5 m north of the click, named street 12 m south of it: within
        // the tie-break band, so the street wins even though the alley is nearer.
        let mut alley = road("x", "service", vec![[-86.80, 36.00011], [-86.79, 36.00011]]);
        alley.name = None;
        let street = road("Broadway", "residential", vec![[-86.80, 35.99989], [-86.79, 35.99989]]);
        let got = locate(36.0, -86.795, &[alley, street], &[], &[]);
        let on = got.on_way.expect("on something");
        assert_eq!(on.name.as_deref(), Some("Broadway"));
        assert!(on.distance_m > 10.0, "true distance kept: {}", on.distance_m);
    }

    #[test]
    fn a_far_enough_named_street_does_not_steal_the_alley() {
        let mut alley = road("x", "service", vec![[-86.80, 36.00001], [-86.79, 36.00001]]);
        alley.name = None;
        // 40 m south -- outside the band, so nearest still wins.
        let street = road("Broadway", "residential", vec![[-86.80, 35.99964], [-86.79, 35.99964]]);
        let got = locate(36.0, -86.795, &[alley, street], &[], &[]);
        assert_eq!(got.on_way.expect("on something").name, None);
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

    fn rail_line(name: &str, rtype: &str, coords: Vec<[f64; 2]>) -> RailFeature {
        RailFeature {
            osm_id: 3,
            rail_type: rtype.to_string(),
            geom_type: if coords.len() < 2 { 1 } else { 0 },
            coords,
            name: Some(name.to_string()),
        }
    }

    fn square(center_lon: f64, center_lat: f64, half: f64) -> Vec<[f64; 2]> {
        vec![
            [center_lon - half, center_lat - half],
            [center_lon + half, center_lat - half],
            [center_lon + half, center_lat + half],
            [center_lon - half, center_lat + half],
        ]
    }

    #[test]
    fn nearest_rail_reports_track_and_skips_stations() {
        let rail = vec![
            rail_line("Main Line", "rail", vec![[-86.80, 36.0], [-86.79, 36.0]]),
            rail_line("Union", "station", vec![[-86.795, 36.0001]]),
        ];
        let track = nearest_rail(36.00005, -86.795, &rail).expect("track");
        assert_eq!(track.kind, "rail");
        assert_eq!(track.index, 0, "the station point is not a way");

        let station = nearest_station(36.0, -86.795, &rail).expect("station");
        assert_eq!(station.index, 1);
        assert!(station.distance_m < 20.0, "distance {}", station.distance_m);
    }

    #[test]
    fn nearest_trailhead_answers_the_point_that_nearest_trail_skips() {
        let mut th = trail("North Gate", "trailhead", vec![[-86.795, 36.0]]);
        th.geom_type = 1;
        let trails = vec![
            trail("Greenway", "path", vec![[-86.80, 36.01], [-86.79, 36.01]]),
            th,
        ];
        assert_eq!(nearest_trail(36.0, -86.795, &trails).unwrap().index, 0);
        let head = nearest_trailhead(36.0, -86.795, &trails).expect("trailhead");
        assert_eq!(head.index, 1);
        assert_eq!(head.class, "trailhead");
        assert!(head.distance_m < 1.0);
    }

    #[test]
    fn park_at_prefers_the_park_you_are_standing_in() {
        let parks = vec![
            ParkFeature {
                osm_id: 1,
                park_type: "park".to_string(),
                // A small park a couple of hundred metres east: near, but not
                // the park the query point is standing in.
                coords: square(-86.792, 36.0, 0.0005),
                name: Some("Small".to_string()),
            },
            ParkFeature {
                osm_id: 2,
                park_type: "nature_reserve".to_string(),
                coords: square(-86.795, 36.0, 0.01),
                name: Some("Big".to_string()),
            },
        ];
        let got = park_at(36.0, -86.795, &parks).expect("a park");
        assert_eq!(got.name.as_deref(), Some("Big"), "containment beats a nearer edge");
        assert!(got.inside);
        assert_eq!(got.distance_m, 0.0);

        // Outside both: the nearer boundary wins, and nothing claims containment.
        let outside = park_at(36.05, -86.795, &parks).expect("still answers");
        assert!(!outside.inside);
        assert!(outside.distance_m > 0.0);
    }

    #[test]
    fn water_at_never_calls_a_river_centreline_containment() {
        let water = vec![WaterFeature {
            osm_id: 1,
            geom_type: 1,
            water_type: "river".to_string(),
            coords: vec![[-86.80, 36.0], [-86.79, 36.0]],
            ref_feature_id: None,
            name: Some("Cumberland".to_string()),
            width: None,
        }];
        let got = water_at(36.0, -86.795, &water).expect("the river");
        assert!(!got.inside, "a linestring has no interior");
        assert!(got.distance_m < 1.0);

        // Reference geometry carries no coordinates; reporting it would place
        // it wherever the reader guessed.
        let reference = vec![WaterFeature {
            osm_id: 2,
            geom_type: 2,
            water_type: "lake".to_string(),
            coords: Vec::new(),
            ref_feature_id: Some(7),
            name: None,
            width: None,
        }];
        assert!(water_at(36.0, -86.795, &reference).is_none());
    }

    #[test]
    fn ring_distance_uses_the_closing_edge() {
        // Just outside the middle of the closing edge (last vertex back to
        // first). Left open, the nearest thing is a corner ~110 m away.
        let ring = square(-86.795, 36.0, 0.001);
        let open = point_to_linestring_distance_m(36.0, -86.7961, &ring)
            .map(|(_, p)| p.distance_m)
            .unwrap();
        let closed = point_to_ring_distance_m(36.0, -86.7961, &ring).unwrap();
        assert!(closed < open, "closed {closed} should beat open {open}");
        assert!(closed < 20.0, "closing edge is ~9 m away, got {closed}");
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
            source: Default::default(),
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
