//! EV charging station decoder (`{ST}.ev_v1.ptiles`, PTILESE v1).
//!
//! Framing follows `scripts/build_ev.py::enc`: zigzag-delta osm_id, lon/lat as
//! i32 microdegrees, an indexed access byte, peak power as a u16 of tenths of
//! a kW, capacity, a u16 connector bitmask, then flags and their optional
//! name / network / ref strings. Merged blocks with a 38-byte index, like
//! trails and rail, so `PtilesFile::read_cell` slices a cell out before these
//! records are read.
//!
//! Two fields decide whether a given car can charge at a site -- power and
//! connector -- and both are the ones OSM most often lacks. They therefore
//! decode to an explicit unknown (`None`, and an empty connector set) rather
//! than to a plausible default. A caller that reads unknown as "fine" strands
//! people; one that reads it as "unusable" hides most of the rural network.
//! Only the caller can choose which of those to be wrong about.

use alloc::string::String;
use alloc::vec::Vec;

use crate::codec::{
    DecodeError, coord_from_micro, decode_string_u8, decode_string_u16, decode_varint, read_i32,
    read_u8, read_u16, zigzag_decode,
};

/// One charging site. A site, not a plug: `capacity` is how many vehicles it
/// takes at once, and `connectors` is which kinds it offers, not how many.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Charger {
    pub osm_id: i64,
    pub lon: f64,
    pub lat: f64,
    /// `yes`, `customers`, `permissive`, `private`, `no`, or `unknown`.
    pub access: String,
    /// Peak power the site advertises. `None` when nothing is tagged --
    /// roughly two thirds of them.
    pub power_kw: Option<f64>,
    /// Vehicles servable at once, `None` when untagged.
    pub capacity: Option<u8>,
    /// Connector kinds present, decoded from the bitmask. Empty means
    /// untagged, not "no connectors".
    pub connectors: Vec<String>,
    /// The raw bitmask, for a caller that wants to test membership without a
    /// string compare. Bit order is [`CONNECTORS`].
    pub connector_bits: u16,
    pub name: Option<String>,
    /// Charging network or operator (`Tesla`, `Electrify America`, ...).
    pub network: Option<String>,
    pub ref_tag: Option<String>,
    /// `name:en` (u16-prefixed) and `brand` (u8-prefixed), from v2. The widths
    /// differ because the builder caps brand at 255 bytes and name:en not --
    /// read one as the other and the record desyncs.
    pub name_en: Option<String>,
    pub brand: Option<String>,
}

/// Access vocabulary, in on-disk index order.
const ACCESS: &[&str] = &["unknown", "yes", "customers", "permissive", "private", "no"];

/// Connector kinds, one per bit, lowest bit first. Fixed forever: the bit
/// position *is* the meaning on every published file, so a kind may be
/// appended but never inserted.
pub const CONNECTORS: &[&str] = &[
    "type1",
    "type1_combo",
    "type2",
    "type2_combo",
    "type2_cable",
    "chademo",
    "tesla_supercharger",
    "tesla_destination",
    "tesla_supercharger_ccs",
    "nema_5_15",
    "nema_5_20",
    "nema_14_50",
    "schuko",
];

/// Connectors a car can physically use at DC speed in North America. Useful
/// for the "can I actually charge here" filter a router wants, and named here
/// rather than in each caller so the answer does not drift between them.
pub fn is_fast_connector(connector: &str) -> bool {
    matches!(
        connector,
        "type1_combo" | "type2_combo" | "chademo" | "tesla_supercharger" | "tesla_supercharger_ccs"
    )
}

fn access_name(idx: u8) -> String {
    ACCESS
        .get(idx as usize)
        .map(|s| String::from(*s))
        .unwrap_or_else(|| alloc::format!("unknown({idx})"))
}

fn connectors_from_bits(bits: u16) -> Vec<String> {
    CONNECTORS
        .iter()
        .enumerate()
        .filter(|(i, _)| bits & (1 << i) != 0)
        .map(|(_, name)| String::from(*name))
        .collect()
}

fn decode_charger_record(
    data: &[u8],
    pos: usize,
    prev_osm_id: i64,
) -> Result<(Charger, usize, i64), DecodeError> {
    let start = pos;
    let mut p = pos;

    let (delta_raw, consumed) = decode_varint(data, p)?;
    p += consumed;
    let osm_id = prev_osm_id.wrapping_add(zigzag_decode(delta_raw));

    let lon_micro = read_i32(data, p)?;
    let lat_micro = read_i32(data, p + 4)?;
    let (lon, lat) = coord_from_micro(lon_micro, lat_micro, p)?;
    p += 8;

    let access = access_name(read_u8(data, p)?);
    p += 1;

    // Tenths of a kW. Zero is the encoder's "untagged", not a 0 kW charger.
    let power_dkw = read_u16(data, p)?;
    p += 2;
    let capacity_raw = read_u8(data, p)?;
    p += 1;
    let connector_bits = read_u16(data, p)?;
    p += 2;

    let flags = read_u8(data, p)?;
    p += 1;

    let mut name = None;
    if flags & 0x01 != 0 {
        let (s, c) = decode_string_u16(data, p)?;
        name = Some(s);
        p += c;
    }
    let mut network = None;
    if flags & 0x02 != 0 {
        let (s, c) = decode_string_u8(data, p)?;
        network = Some(s);
        p += c;
    }
    let mut ref_tag = None;
    if flags & 0x04 != 0 {
        let (s, c) = decode_string_u8(data, p)?;
        ref_tag = Some(s);
        p += c;
    }

    // v2 fields, written after every v1 field and flag-guarded, so a v1 file
    // reads unchanged. The version bump exists to stop a v1 *reader* meeting a
    // v2 file: these records carry no length prefix, so an unread trailing
    // field desyncs the rest of the cell.
    let mut name_en = None;
    if flags & 0x08 != 0 {
        let (s, c) = decode_string_u16(data, p)?;
        name_en = Some(s);
        p += c;
    }
    let mut brand = None;
    if flags & 0x10 != 0 {
        let (s, c) = decode_string_u8(data, p)?;
        brand = Some(s);
        p += c;
    }

    Ok((
        Charger {
            osm_id,
            lon,
            lat,
            access,
            power_kw: (power_dkw > 0).then(|| f64::from(power_dkw) / 10.0),
            capacity: (capacity_raw > 0).then_some(capacity_raw),
            connectors: connectors_from_bits(connector_bits),
            connector_bits,
            name,
            network,
            ref_tag,
            name_en,
            brand,
        },
        p - start,
        osm_id,
    ))
}

/// Decode a decompressed EV block into its stations. Sequential records, no
/// length prefix -- a record that fails to decode stops the scan, same as
/// every other point layer.
pub fn decode_chargers(data: &[u8]) -> Result<Vec<Charger>, DecodeError> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    let mut prev_osm_id = 0i64;

    while pos < data.len() {
        match decode_charger_record(data, pos, prev_osm_id) {
            Ok((c, consumed, new_prev)) => {
                prev_osm_id = new_prev;
                pos += consumed.max(1);
                out.push(c);
            }
            Err(_) => break,
        }
    }

    Ok(out)
}

/// Whether a station is one the public can actually plug into.
///
/// `unknown` counts as usable: two thirds of stations carry no access tag,
/// and excluding them would empty the map. `private` and `no` are the only
/// outright exclusions -- a fleet depot behind a gate is not a stop.
pub fn is_public(charger: &Charger) -> bool {
    !matches!(charger.access.as_str(), "private" | "no")
}

// --- Planning charge stops along a route -----------------------------------

/// Fraction of range held back rather than driven. A driver who arrives at a
/// charger on 0% has already been stranded once by a queue, a broken unit or
/// a headwind, and the number the car reports is optimistic about all three.
pub const CHARGE_RESERVE: f64 = 0.2;

/// How far off the route a charger may sit and still count as on the way.
pub const DEFAULT_MAX_DETOUR_M: f64 = 5_000.0;

/// A stop the plan says to make.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ChargeStop {
    /// Index into the `chargers` slice that was searched.
    pub index: usize,
    /// Metres along the route where the charger is nearest.
    pub along_m: f64,
    /// Metres from the route to the charger itself, one way.
    pub detour_m: f64,
    /// Metres driven since the start or the previous stop.
    pub leg_m: f64,
}

/// The outcome of planning a drive that cannot be done on one charge.
#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ChargePlan {
    pub stops: Vec<ChargeStop>,
    /// True when every leg, including the last one to the destination, fits
    /// inside `usable_range_m`.
    pub reachable: bool,
    /// Metres of the final leg that cannot be covered, when `reachable` is
    /// false. Zero otherwise.
    pub shortfall_m: f64,
    /// The range actually planned against: `range_m` less [`CHARGE_RESERVE`].
    pub usable_range_m: f64,
    /// Total route length, for a caller reporting "x of y km".
    pub route_m: f64,
}

/// Where a charger sits relative to a route.
fn project_onto(path: &[[f64; 2]], lat: f64, lon: f64) -> Option<(f64, f64)> {
    let (seg, proj) = crate::proximity::point_to_linestring_distance_m(lat, lon, path)?;
    let mut along = 0.0;
    for i in 0..seg {
        let [alon, alat] = path[i];
        let [blon, blat] = path[i + 1];
        along += crate::proximity::haversine_distance_m(alat, alon, blat, blon);
    }
    let [alon, alat] = path[seg];
    along += crate::proximity::haversine_distance_m(alat, alon, proj.snapped.0, proj.snapped.1);
    Some((along, proj.distance_m))
}

/// Plan the charging stops a drive along `path` needs.
///
/// `path` is `[lon, lat]` pairs -- the decoders' order, and what
/// `RouteResult::path` gives after the caller flips it back. `range_m` is what
/// the car says it has *now*; the plan plans against
/// `range_m * (1 - CHARGE_RESERVE)`, so a driver arrives at each stop with
/// something left. Every leg after the first assumes a full charge of the
/// same range, which is the optimistic reading -- a fast charger is usually
/// left at 80%, not 100% -- and is stated here rather than buried, because a
/// caller wanting the pessimistic version can simply pass a smaller range.
///
/// Selection prefers a charger in the *far half* of each leg's reach, so the
/// plan does not stop 20 km in when it could stop 200 km in, and among those
/// prefers the highest advertised power, since a 150 kW stop costs twenty
/// minutes where a 7 kW one costs the afternoon. Untagged power sorts last
/// but is not excluded: two thirds of stations have none, and excluding them
/// empties the map.
///
/// Chargers behind a gate (`private`, `no`) and those further than
/// `max_detour_m` from the route are not considered.
pub fn plan_charge_stops(
    path: &[[f64; 2]],
    chargers: &[Charger],
    range_m: f64,
    max_detour_m: f64,
) -> ChargePlan {
    let usable_range_m = range_m * (1.0 - CHARGE_RESERVE);
    let mut route_m = 0.0;
    for w in path.windows(2) {
        route_m += crate::proximity::haversine_distance_m(w[0][1], w[0][0], w[1][1], w[1][0]);
    }
    let mut plan = ChargePlan {
        usable_range_m,
        route_m,
        ..Default::default()
    };
    if path.len() < 2 || usable_range_m <= 0.0 {
        plan.shortfall_m = route_m;
        return plan;
    }
    if route_m <= usable_range_m {
        plan.reachable = true;
        return plan;
    }

    let mut candidates: Vec<Candidate> = chargers
        .iter()
        .enumerate()
        .filter(|(_, c)| is_public(c))
        .filter_map(|(index, c)| {
            let (along_m, detour_m) = project_onto(path, c.lat, c.lon)?;
            (detour_m <= max_detour_m).then_some(Candidate {
                along_m,
                detour_m,
                index,
                // Untagged power sorts last but is not excluded.
                power_kw: c.power_kw.unwrap_or(0.0),
            })
        })
        .collect();
    candidates.sort_by(|a, b| a.along_m.total_cmp(&b.along_m));

    let mut cursor = 0.0_f64;
    loop {
        let reach = cursor + usable_range_m;
        if route_m <= reach {
            plan.reachable = true;
            return plan;
        }
        // Only stops ahead of the cursor, and far enough ahead to be worth
        // making: a stop 500 m after the last one is a loop, not a leg.
        let in_reach: Vec<&Candidate> = candidates
            .iter()
            .filter(|c| c.along_m > cursor + 1_000.0 && c.along_m <= reach)
            .collect();
        let Some(&Candidate { along_m: along, detour_m: detour, index, .. }) =
            pick_stop(&in_reach, cursor, usable_range_m)
        else {
            // Nothing in range: the drive stops where the charge does.
            plan.shortfall_m = route_m - reach;
            return plan;
        };
        plan.stops.push(ChargeStop {
            index,
            along_m: along,
            detour_m: detour,
            leg_m: along - cursor,
        });
        cursor = along;
        // A plan with a stop per kilometre is a bug, not an itinerary.
        if plan.stops.len() > 64 {
            plan.shortfall_m = route_m - cursor;
            return plan;
        }
    }
}

/// A charger reduced to what the planner needs: where it is on the route, how
/// far off it, and how fast it charges.
#[derive(Clone, Copy, Debug)]
struct Candidate {
    along_m: f64,
    detour_m: f64,
    index: usize,
    /// 0.0 when untagged, which sorts it last without excluding it.
    power_kw: f64,
}

/// The stop to make from those in reach.
///
/// Prefer the far half of the leg -- stopping 20 km in when 200 km was
/// available turns one stop into three -- and within it the most powerful
/// charger, since that is the difference between twenty minutes and an
/// afternoon. When the far half is empty, take the farthest thing reachable
/// and accept the short leg: the alternative is running out.
fn pick_stop<'a>(
    in_reach: &[&'a Candidate],
    cursor: f64,
    usable_range_m: f64,
) -> Option<&'a Candidate> {
    if in_reach.is_empty() {
        return None;
    }
    let far_half_starts = cursor + usable_range_m / 2.0;
    let far: Vec<&&Candidate> = in_reach
        .iter()
        .filter(|c| c.along_m >= far_half_starts)
        .collect();
    if far.is_empty() {
        return in_reach.iter().max_by(|a, b| a.along_m.total_cmp(&b.along_m)).copied();
    }
    far.into_iter()
        .max_by(|a, b| {
            a.power_kw
                .total_cmp(&b.power_kw)
                .then(a.along_m.total_cmp(&b.along_m))
        })
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn enc(
        osm_delta: i64,
        lon: f64,
        lat: f64,
        access: u8,
        power_dkw: u16,
        capacity: u8,
        bits: u16,
        name: Option<&str>,
        network: Option<&str>,
    ) -> Vec<u8> {
        let mut d = Vec::new();
        let zz = ((osm_delta << 1) ^ (osm_delta >> 63)) as u64;
        let mut v = zz;
        loop {
            let mut byte = (v & 0x7f) as u8;
            v >>= 7;
            if v != 0 {
                byte |= 0x80;
            }
            d.push(byte);
            if v == 0 {
                break;
            }
        }
        d.extend_from_slice(&((lon * 100_000.0).round() as i32).to_le_bytes());
        d.extend_from_slice(&((lat * 100_000.0).round() as i32).to_le_bytes());
        d.push(access);
        d.extend_from_slice(&power_dkw.to_le_bytes());
        d.push(capacity);
        d.extend_from_slice(&bits.to_le_bytes());
        let flags = (if name.is_some() { 0x01 } else { 0 }) | (if network.is_some() { 0x02 } else { 0 });
        d.push(flags);
        if let Some(n) = name {
            d.extend_from_slice(&(n.len() as u16).to_le_bytes());
            d.extend_from_slice(n.as_bytes());
        }
        if let Some(o) = network {
            d.push(o.len() as u8);
            d.extend_from_slice(o.as_bytes());
        }
        d
    }

    #[test]
    fn empty_block_decodes_to_empty_vec() {
        assert_eq!(decode_chargers(&[]).unwrap(), Vec::new());
    }

    #[test]
    fn a_tagged_station_decodes_exactly() {
        // CCS1 + CHAdeMO, 150 kW, 4 bays.
        let bits = (1 << 1) | (1 << 5);
        let data = enc(42, -86.78, 36.16, 1, 1500, 4, bits, Some("Broadway DC"), Some("Electrify America"));
        let got = decode_chargers(&data).unwrap();
        assert_eq!(got.len(), 1);
        let c = &got[0];
        assert_eq!(c.osm_id, 42);
        assert_eq!(c.access, "yes");
        assert_eq!(c.power_kw, Some(150.0));
        assert_eq!(c.capacity, Some(4));
        assert_eq!(c.connectors, vec!["type1_combo", "chademo"]);
        assert_eq!(c.name.as_deref(), Some("Broadway DC"));
        assert_eq!(c.network.as_deref(), Some("Electrify America"));
        assert!(is_public(c));
        assert!(c.connectors.iter().all(|k| is_fast_connector(k)));
    }

    #[test]
    fn untagged_power_and_connectors_read_as_unknown_not_as_zero() {
        let data = enc(1, -86.78, 36.16, 0, 0, 0, 0, None, None);
        let c = &decode_chargers(&data).unwrap()[0];
        assert_eq!(c.power_kw, None, "0 kW would mean a charger that cannot charge");
        assert_eq!(c.capacity, None);
        assert!(c.connectors.is_empty(), "empty means untagged, not 'no connectors'");
        assert_eq!(c.access, "unknown");
        assert!(is_public(c), "an untagged station is not a private one");
    }

    #[test]
    fn a_private_station_is_not_a_stop() {
        let private = enc(1, -86.78, 36.16, 4, 220, 2, 1 << 2, None, None);
        assert!(!is_public(&decode_chargers(&private).unwrap()[0]));
        let refused = enc(1, -86.78, 36.16, 5, 220, 2, 1 << 2, None, None);
        assert!(!is_public(&decode_chargers(&refused).unwrap()[0]));
    }

    #[test]
    fn delta_osm_ids_accumulate_across_records() {
        let mut data = enc(100, -86.78, 36.16, 1, 500, 1, 4, None, None);
        data.extend(enc(50, -86.77, 36.17, 1, 500, 1, 4, None, None));
        let got = decode_chargers(&data).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].osm_id, 100);
        assert_eq!(got[1].osm_id, 150);
    }

    #[test]
    fn an_impossible_coordinate_is_not_a_record() {
        // Same guard as signals and camera: another layer's block parses
        // cleanly here, and only the coordinates say otherwise.
        let mut data = Vec::new();
        data.push(0x02);
        data.extend_from_slice(&251_624_336i32.to_le_bytes());
        data.extend_from_slice(&16_791_342i32.to_le_bytes());
        data.extend_from_slice(&[0u8; 7]);
        assert!(matches!(
            decode_charger_record(&data, 0, 0),
            Err(DecodeError::CoordOutOfRange { .. })
        ));
        assert_eq!(decode_chargers(&data).unwrap(), Vec::new());
    }

    #[test]
    fn truncation_stops_the_scan_without_panicking() {
        let full = enc(1, -86.78, 36.16, 1, 500, 2, 4, Some("Name"), Some("Net"));
        for cut in 0..full.len() {
            assert!(decode_chargers(&full[..cut]).is_ok(), "cut at {cut}");
        }
    }

    fn charger_at(lat: f64, lon: f64, power_kw: Option<f64>, access: &str) -> Charger {
        Charger {
            osm_id: 1,
            lon,
            lat,
            access: String::from(access),
            power_kw,
            capacity: None,
            connectors: Vec::new(),
            connector_bits: 0,
            name: None,
            network: None,
            ref_tag: None,
            name_en: None,
            brand: None,
        }
    }

    /// A straight run east along lat 36.0, `km` long. One degree of longitude
    /// here is 90.06 km, which is what puts the landmarks below where they are.
    fn route(km: f64) -> Vec<[f64; 2]> {
        vec![[-86.0, 36.0], [lon_at(km), 36.0]]
    }

    /// Longitude of a point `km` east of the route's start.
    fn lon_at(km: f64) -> f64 {
        -86.0 + (km * 1000.0) / 90_060.0
    }

    #[test]
    fn a_drive_inside_usable_range_needs_no_stop() {
        let path = route(90.0);
        let plan = plan_charge_stops(&path, &[], 200_000.0, DEFAULT_MAX_DETOUR_M);
        assert!(plan.reachable);
        assert!(plan.stops.is_empty());
        assert_eq!(plan.usable_range_m, 160_000.0, "80% of what the car reports");
    }

    #[test]
    fn the_reserve_is_what_forces_the_stop() {
        // 90 km route, 100 km of range: reachable on paper, not on 80%.
        let path = route(90.0);
        let chargers = vec![charger_at(36.0, lon_at(60.0), Some(150.0), "yes")];
        let plan = plan_charge_stops(&path, &chargers, 100_000.0, DEFAULT_MAX_DETOUR_M);
        assert_eq!(plan.usable_range_m, 80_000.0);
        assert_eq!(plan.stops.len(), 1, "80 km of usable range cannot cover 90 km");
        assert!(plan.reachable);
    }

    #[test]
    fn a_long_drive_stops_as_late_as_it_can_and_prefers_power() {
        // 360 km on 160 km of usable range: two stops, and the second must
        // land past 200 km or the last leg cannot be covered.
        let path = route(360.0);
        let chargers = vec![
            // Near half of leg 1, and slow: should be passed by.
            charger_at(36.0, lon_at(40.0), Some(7.0), "yes"),
            // Far half of leg 1, slow.
            charger_at(36.0, lon_at(145.0), Some(11.0), "yes"),
            // Far half of leg 1, fast: the one to take.
            charger_at(36.0, lon_at(150.0), Some(150.0), "yes"),
            // Reachable from there, and gets us home.
            charger_at(36.0, lon_at(300.0), Some(50.0), "yes"),
        ];
        let plan = plan_charge_stops(&path, &chargers, 200_000.0, DEFAULT_MAX_DETOUR_M);
        assert!(plan.reachable, "{plan:?}");
        assert_eq!(plan.stops.len(), 2, "{:?}", plan.stops);
        assert_eq!(plan.stops[0].index, 2, "the 150 kW stop in the far half wins");
        assert!(plan.stops[0].leg_m > 130_000.0, "stopped too early: {:?}", plan.stops[0]);
    }

    #[test]
    fn a_gap_with_no_charger_is_reported_as_a_shortfall_not_a_plan() {
        let path = route(360.0);
        // Only one charger, and it is behind a gate.
        let chargers = vec![charger_at(36.0, lon_at(150.0), Some(150.0), "private")];
        let plan = plan_charge_stops(&path, &chargers, 200_000.0, DEFAULT_MAX_DETOUR_M);
        assert!(!plan.reachable);
        assert!(plan.stops.is_empty());
        assert!(plan.shortfall_m > 0.0);
        assert!(
            (plan.shortfall_m - (plan.route_m - plan.usable_range_m)).abs() < 1.0,
            "shortfall is what the charge cannot cover"
        );
    }

    #[test]
    fn a_charger_far_off_the_route_is_not_on_the_way() {
        // 180 km route, one stop needed, and the only charger is 22 km north.
        let path = route(180.0);
        let chargers = vec![charger_at(36.2, lon_at(108.0), Some(150.0), "yes")];
        let plan = plan_charge_stops(&path, &chargers, 200_000.0, DEFAULT_MAX_DETOUR_M);
        assert!(!plan.reachable, "a 22 km detour is not a stop on the way");

        // Widen what counts as on the way and it becomes usable.
        let generous = plan_charge_stops(&path, &chargers, 200_000.0, 30_000.0);
        assert!(generous.reachable);
        assert_eq!(generous.stops.len(), 1);
        assert!(generous.stops[0].detour_m > 20_000.0);
    }

    #[test]
    fn several_legs_chain_and_each_reports_its_own_length() {
        let path = route(900.0);
        // One every 90 km.
        let chargers: Vec<Charger> = (1..10)
            .map(|i| charger_at(36.0, lon_at(90.0 * i as f64), Some(50.0), "yes"))
            .collect();
        let plan = plan_charge_stops(&path, &chargers, 250_000.0, DEFAULT_MAX_DETOUR_M);
        assert!(plan.reachable, "{plan:?}");
        assert!(plan.stops.len() >= 4, "900 km on 200 km legs: {:?}", plan.stops);
        for stop in &plan.stops {
            assert!(
                stop.leg_m <= plan.usable_range_m + 1.0,
                "leg longer than the charge allows: {stop:?}"
            );
        }
        // Legs are consecutive: each starts where the last one stopped.
        let mut prev = 0.0;
        for stop in &plan.stops {
            assert!((stop.along_m - prev - stop.leg_m).abs() < 1.0);
            prev = stop.along_m;
        }
    }

    #[test]
    fn an_untagged_charger_is_still_a_stop() {
        let path = route(180.0);
        let chargers = vec![charger_at(36.0, lon_at(108.0), None, "unknown")];
        let plan = plan_charge_stops(&path, &chargers, 200_000.0, DEFAULT_MAX_DETOUR_M);
        assert!(plan.reachable, "excluding untagged power would empty the map");
        assert_eq!(plan.stops.len(), 1);
    }

    #[test]
    fn a_zero_range_car_plans_nothing_rather_than_looping() {
        let path = route(360.0);
        let chargers = vec![charger_at(36.0, lon_at(150.0), Some(150.0), "yes")];
        let plan = plan_charge_stops(&path, &chargers, 0.0, DEFAULT_MAX_DETOUR_M);
        assert!(!plan.reachable);
        assert!(plan.stops.is_empty());
        assert!(plan.shortfall_m > 0.0);
    }

    #[test]
    fn unknown_bits_do_not_invent_connector_names() {
        // Bit 15 is beyond the table: a file from a newer builder must not
        // make this reader name something it does not know.
        let data = enc(1, -86.78, 36.16, 1, 500, 1, 1 << 15, None, None);
        let c = &decode_chargers(&data).unwrap()[0];
        assert!(c.connectors.is_empty());
        assert_eq!(c.connector_bits, 1 << 15, "the raw bits are still reported");
    }
}
