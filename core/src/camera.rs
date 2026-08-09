//! Camera block decoder (`{ST}.camera.ptiles`, PTILESC v1).
//!
//! Point records: osm_id (zigzag delta), lon/i32, lat/i32, device_type/u8,
//! placement/u8, camera_type/u8, flags/u8, [direction/u16], [operator/u8str],
//! [name/u16str], [ref/u8str].

use alloc::string::String;
use alloc::vec::Vec;

use crate::codec::{
    DecodeError, coord_from_micro, decode_string_u8, decode_string_u16, decode_varint, read_i32,
    read_u8, read_u16, zigzag_decode,
};

/// A camera / ALPR point decoded from a `.camera.ptiles` block.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Camera {
    pub osm_id: i64,
    pub lon: f64,
    pub lat: f64,
    pub device_type: String,
    pub placement: String,
    pub camera_type: String,
    pub direction: Option<u16>,
    pub angle: Option<u8>,
    pub operator: Option<String>,
    pub name: Option<String>,
    pub ref_tag: Option<String>,
}

// Lookup tables matching the Python builder's order.
const DEVICE_TYPES: &[&str] = &["camera", "ALPR", "guard", "unknown"];
const PLACEMENTS: &[&str] = &["public", "outdoor", "indoor", "unknown"];
const CAMERA_TYPES: &[&str] = &["fixed", "panning", "dome", "unknown"];

fn lookup(idx: u8, table: &[&str]) -> String {
    table
        .get(idx as usize)
        .map(|s| String::from(*s))
        .unwrap_or_else(|| alloc::format!("unknown({idx})"))
}

fn decode_camera_record(
    data: &[u8],
    pos: usize,
    prev_osm_id: i64,
) -> Result<(Camera, usize, i64), DecodeError> {
    let start = pos;
    let mut p = pos;

    let (delta_raw, consumed) = decode_varint(data, p)?;
    p += consumed;
    let osm_id = prev_osm_id.wrapping_add(zigzag_decode(delta_raw));

    let lon_micro = read_i32(data, p)?;
    let lat_micro = read_i32(data, p + 4)?;
    let (lon, lat) = coord_from_micro(lon_micro, lat_micro, p)?;
    p += 8;

    let device_type = lookup(read_u8(data, p)?, DEVICE_TYPES);
    p += 1;
    let placement = lookup(read_u8(data, p)?, PLACEMENTS);
    p += 1;
    let camera_type = lookup(read_u8(data, p)?, CAMERA_TYPES);
    p += 1;

    let flags = read_u8(data, p)?;
    p += 1;

    let direction = if flags & 0x01 != 0 {
        let d = read_u16(data, p)?;
        p += 2;
        Some(d)
    } else {
        None
    };

    let operator = if flags & 0x02 != 0 {
        let (s, c) = decode_string_u8(data, p)?;
        p += c;
        Some(s)
    } else {
        None
    };

    let name = if flags & 0x04 != 0 {
        let (s, c) = decode_string_u16(data, p)?;
        p += c;
        Some(s)
    } else {
        None
    };

    let ref_tag = if flags & 0x08 != 0 {
        let (s, c) = decode_string_u8(data, p)?;
        p += c;
        Some(s)
    } else {
        None
    };

    let angle = if flags & 0x10 != 0 {
        let a = read_u8(data, p)?;
        p += 1;
        Some(a)
    } else {
        None
    };

    Ok((
        Camera {
            osm_id,
            lon,
            lat,
            device_type,
            placement,
            camera_type,
            direction,
            angle,
            operator,
            name,
            ref_tag,
        },
        p - start,
        osm_id,
    ))
}

/// Decode a decompressed camera block into individual records.
pub fn decode_cameras(data: &[u8]) -> Result<Vec<Camera>, DecodeError> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    let mut prev_osm_id = 0i64;

    while pos < data.len() {
        match decode_camera_record(data, pos, prev_osm_id) {
            Ok((cam, consumed, new_prev)) => {
                prev_osm_id = new_prev;
                pos += consumed.max(1);
                out.push(cam);
            }
            Err(_) => break,
        }
    }

    Ok(out)
}

/// Filter cameras to those within `radius_m` of any point on the given
/// road's linestring. Road coords are `[lon, lat]` pairs matching
/// [`RoadSegment::coords`][crate::RoadSegment].
///
/// This is a spatial-without-index filter: walk all cameras, measure
/// point-to-linestring distance against the road. Cameras per H3 cell are
/// few (typically <100), so O(cameras * segments) is cheap. Move to a
/// spatial partition (grid each camera to its road's cell) when per-cell
/// camera counts exceed ~500.
///
/// `radius_m` default: 30m — street width + sidewalk.
pub fn cameras_near_road(
    cameras: &[Camera],
    road_coords: &[[f64; 2]],
    radius_m: f64,
) -> Vec<Camera> {
    if road_coords.len() < 2 {
        return Vec::new();
    }

    // Pre-decompose road segments for fast distance checks
    let segments: Vec<_> = road_coords.windows(2).map(|w| (w[0], w[1])).collect();

    cameras
        .iter()
        .filter(|cam| {
            segments.iter().any(|(a, b)| {
                // Quick bounding-box cull before the trig
                let min_lat = a[1].min(b[1]) - 0.001; // ~100m buffer
                let max_lat = a[1].max(b[1]) + 0.001;
                let min_lon = a[0].min(b[0]) - 0.001;
                let max_lon = a[0].max(b[0]) + 0.001;
                if cam.lat < min_lat || cam.lat > max_lat || cam.lon < min_lon || cam.lon > max_lon
                {
                    return false;
                }
                let proj = crate::proximity::point_to_segment_distance_m(
                    cam.lat, cam.lon, a[1], a[0], b[1], b[0],
                );
                proj.distance_m <= radius_m
            })
        })
        .cloned()
        .collect()
}

// --- "Can a camera see me?" -------------------------------------------------

/// How far a camera is assumed to see a person. Nothing in the file says, and
/// the honest range depends on the lens: a fixed dome over a shop door reads a
/// face at 10 m, a purpose-built ALPR reads a plate at 50 m. 50 m is the far
/// end, chosen because the question is asked by someone deciding whether they
/// are being watched, and the wrong answer to give them is a confident "no".
pub const CAMERA_RANGE_M: f64 = 50.0;

/// Assumed mount height. Cameras go above head height so people cannot reach
/// them and so heads do not occlude each other -- a shopfront camera sits at
/// roughly one storey.
pub const CAMERA_MOUNT_M: f64 = 4.0;

/// Assumed height of the person being seen.
pub const SUBJECT_M: f64 = 1.7;

/// Field of view assumed for a camera tagged with a direction but no angle.
/// Wide-ish on purpose: a narrower guess would rule cameras out, and ruling a
/// camera out wrongly is the answer that matters here.
pub const DEFAULT_FOV_DEG: f64 = 90.0;

/// What one camera can see of a point.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CameraView {
    /// Index into the `cameras` slice that was searched.
    pub index: usize,
    pub osm_id: i64,
    /// Metres from the camera to the point.
    pub distance_m: f64,
    /// Bearing from the camera to the point, degrees clockwise from north --
    /// the same convention as the camera's own `direction`.
    pub bearing_deg: f64,
    /// False only when the camera is tagged with a direction and the point
    /// falls outside the resulting cone. An untagged or rotating camera is
    /// assumed to be able to point at you.
    pub aimed_at_you: bool,
    /// True when `aimed_at_you` rests on an assumption rather than on tags:
    /// no direction, or a direction with no angle, or a type that rotates.
    pub aim_assumed: bool,
    /// False when a building stands between the camera and the point.
    pub line_of_sight: bool,
    /// Index into `buildings` of what blocks it, when something does.
    pub blocked_by: Option<usize>,
    /// `aimed_at_you && line_of_sight`, within `range_m`.
    pub sees: bool,
}

/// Smallest angle between two bearings, in degrees.
fn bearing_delta(a: f64, b: f64) -> f64 {
    let d = (a - b).abs() % 360.0;
    if d > 180.0 { 360.0 - d } else { d }
}

/// The height of the sight line a fraction `t` of the way from the camera to
/// the subject.
fn sight_line_height(t: f64) -> f64 {
    CAMERA_MOUNT_M + (SUBJECT_M - CAMERA_MOUNT_M) * t
}

/// The height to credit a building with when it stands between a camera and a
/// person.
///
/// This is [`viewshed`](crate::viewshed)'s occluder rule *inverted*, and the
/// inversion is the whole point. There, an unmeasured building is assumed
/// tall, so an uncertain sight line comes out blocked and the mode
/// under-reports what can be seen. Here, "blocked" is the reassuring answer,
/// so the same caution would tell someone they are unobserved on the strength
/// of a guessed height. Uncertainty therefore costs cover instead: an
/// unmeasured building is credited with the low end of its type's range.
fn occluder_height(b: &crate::viewshed::ViewBuilding) -> f64 {
    match b.height_m {
        Some(h) if h > 0.0 => h,
        _ => crate::viewshed::estimate_height_range(&b.building_type).low,
    }
}

/// Which of `cameras` can see `(lat, lon)`, nearest first.
///
/// Answers the question a person asks standing on a street: *is anything
/// pointed at me right now.* Three things have to be true, and each is
/// reported separately so a caller can show why rather than only what:
/// the camera is within `range_m`, it is or could be aimed at the point, and
/// no building stands in the way.
///
/// Every assumption leans the same way — toward reporting a camera rather
/// than omitting one. A camera with no direction tag is assumed to be able to
/// point at you; a dome or panning camera rotates, so it always can; an
/// unmeasured building is credited with the *low* end of its height range.
/// The result is a query that will sometimes name a camera that cannot in
/// fact see you, and should not miss one that can. `aim_assumed` and
/// `blocked_by` are there so a caller can say which parts are inference.
///
/// Buildings are the same `ViewBuilding` list [`viewshed`](crate::viewshed)
/// takes, so a caller already holding one for the view can pass it here
/// unchanged. Pass an empty slice for the no-occlusion answer.
pub fn cameras_seeing(
    lat: f64,
    lon: f64,
    cameras: &[Camera],
    buildings: &[crate::viewshed::ViewBuilding],
    range_m: f64,
) -> Vec<CameraView> {
    // Standing inside a footprint that the camera is outside of means walls
    // between the two, whatever the sight line does. Computed once rather
    // than per camera.
    let subject_indoors: Vec<usize> = buildings
        .iter()
        .enumerate()
        .filter(|(_, b)| crate::proximity::point_in_polygon(lat, lon, &b.coords))
        .map(|(i, _)| i)
        .collect();

    let mut out: Vec<CameraView> = cameras
        .iter()
        .enumerate()
        .filter_map(|(index, cam)| {
            let distance_m = crate::proximity::haversine_distance_m(cam.lat, cam.lon, lat, lon);
            if distance_m > range_m {
                return None;
            }
            let bearing_deg = bearing_to(cam.lat, cam.lon, lat, lon);

            // A dome or a panning camera sweeps; its `direction`, when it has
            // one, is where it happens to be looking, not where it can look.
            let rotates = matches!(cam.camera_type.as_str(), "dome" | "panning");
            let (aimed_at_you, aim_assumed) = match (rotates, cam.direction) {
                (true, _) => (true, true),
                (false, None) => (true, true),
                (false, Some(dir)) => {
                    let fov = cam.angle.map(f64::from).unwrap_or(DEFAULT_FOV_DEG);
                    let inside = bearing_delta(bearing_deg, f64::from(dir)) <= fov / 2.0;
                    (inside, cam.angle.is_none())
                }
            };

            let blocked_by = subject_indoors
                .iter()
                .copied()
                .find(|&i| !crate::proximity::point_in_polygon(cam.lat, cam.lon, &buildings[i].coords))
                .or_else(|| first_occluder(cam, lat, lon, buildings));

            Some(CameraView {
                index,
                osm_id: cam.osm_id,
                distance_m,
                bearing_deg,
                aimed_at_you,
                aim_assumed,
                line_of_sight: blocked_by.is_none(),
                blocked_by,
                sees: aimed_at_you && blocked_by.is_none(),
            })
        })
        .collect();

    out.sort_by(|a, b| a.distance_m.total_cmp(&b.distance_m));
    out
}

/// The first building whose walls rise above the camera-to-subject sight line
/// where that line crosses them.
fn first_occluder(
    cam: &Camera,
    lat: f64,
    lon: f64,
    buildings: &[crate::viewshed::ViewBuilding],
) -> Option<usize> {
    let from = [cam.lon, cam.lat];
    let to = [lon, lat];
    buildings.iter().position(|b| {
        if b.coords.len() < 3 {
            return false;
        }
        let height = occluder_height(b);
        // The ring as stored may or may not repeat its first vertex, so walk
        // the closing edge explicitly rather than trusting it to be there.
        let n = b.coords.len();
        (0..n).any(|i| {
            let edge_a = b.coords[i];
            let edge_b = b.coords[(i + 1) % n];
            match crate::proximity::segment_crossing(from, to, edge_a, edge_b) {
                Some(t) => height > sight_line_height(t),
                None => false,
            }
        })
    })
}

/// Bearing from one point to another, degrees clockwise from north.
///
/// Great-circle, not planar: over the tens of metres this query works at the
/// two agree to well under a degree, but the planar version quietly breaks
/// near the poles and there is no reason to ship the version with a latitude
/// range.
pub fn bearing_to(from_lat: f64, from_lon: f64, to_lat: f64, to_lon: f64) -> f64 {
    let (f_lat, t_lat) = (from_lat.to_radians(), to_lat.to_radians());
    let d_lon = (to_lon - from_lon).to_radians();
    let y = crate::math::sin(d_lon) * crate::math::cos(t_lat);
    let x = crate::math::cos(f_lat) * crate::math::sin(t_lat)
        - crate::math::sin(f_lat) * crate::math::cos(t_lat) * crate::math::cos(d_lon);
    let deg = crate::math::atan2(y, x).to_degrees();
    (deg + 360.0) % 360.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synth_cam(osm_delta: i64, lon_micro: i32, lat_micro: i32) -> Vec<u8> {
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
        d.extend_from_slice(&lon_micro.to_le_bytes());
        d.extend_from_slice(&lat_micro.to_le_bytes());
        d.push(0); // camera
        d.push(0); // public
        d.push(0); // fixed
        d.push(0); // no flags
        d
    }

    #[test]
    fn empty_block_is_empty() {
        assert_eq!(decode_cameras(&[]).unwrap(), Vec::new());
    }

    #[test]
    fn basic_camera_decodes() {
        let data = synth_cam(42, -86_77373, 3_616_206);
        let cams = decode_cameras(&data).unwrap();
        assert_eq!(cams.len(), 1);
        assert_eq!(cams[0].osm_id, 42);
        assert!((cams[0].lon - (-86.77373)).abs() < 1e-9);
        assert!((cams[0].lat - 36.16206).abs() < 1e-9);
        assert_eq!(cams[0].device_type, "camera");
        assert_eq!(cams[0].placement, "public");
        assert_eq!(cams[0].camera_type, "fixed");
        assert_eq!(cams[0].direction, None);
        assert_eq!(cams[0].operator, None);
    }

    #[test]
    fn delta_osm_id_accumulates() {
        let mut data = synth_cam(100, 0, 0);
        data.extend(synth_cam(50, 1_000, 1_000));
        let cams = decode_cameras(&data).unwrap();
        assert_eq!(cams.len(), 2);
        assert_eq!(cams[0].osm_id, 100);
        assert_eq!(cams[1].osm_id, 150);
    }

    /// Same class of bug as `signals::impossible_coordinate_is_not_a_record`:
    /// a block from another layer parses cleanly here, and nothing but the
    /// coordinate values says so.
    #[test]
    fn impossible_coordinate_is_not_a_record() {
        let data = synth_cam(2, 251_624_336, 16_791_342);
        assert!(matches!(
            decode_camera_record(&data, 0, 0),
            Err(DecodeError::CoordOutOfRange { .. })
        ));
        assert_eq!(decode_cameras(&data).unwrap(), Vec::new());
    }

    #[test]
    fn truncated_block_graceful() {
        let full = synth_cam(1, 0, 0);
        for cut in [1usize, 3, 6, full.len().saturating_sub(1)] {
            assert!(decode_cameras(&full[..cut]).is_ok());
        }
    }

    #[test]
    fn cameras_near_road_filters() {
        // A short road along the equator
        let road = [[-86.78, 36.16], [-86.77, 36.16]];
        let on_road = Camera {
            osm_id: 1,
            lon: -86.775,
            lat: 36.16001,
            device_type: String::from("camera"),
            placement: String::from("public"),
            camera_type: String::from("fixed"),
            direction: None,
            angle: None,
            operator: None,
            name: None,
            ref_tag: None,
        };
        let far = Camera {
            osm_id: 2,
            lon: -86.78,
            lat: 36.18, // ~2.2 km north
            device_type: String::from("camera"),
            placement: String::from("public"),
            camera_type: String::from("fixed"),
            direction: None,
            angle: None,
            operator: None,
            name: None,
            ref_tag: None,
        };
        let result = cameras_near_road(&[on_road.clone(), far], &road, 30.0);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].osm_id, 1);
    }

    fn cam_at(lat: f64, lon: f64, camera_type: &str, direction: Option<u16>, angle: Option<u8>) -> Camera {
        Camera {
            osm_id: 1,
            lon,
            lat,
            device_type: String::from("camera"),
            placement: String::from("public"),
            camera_type: String::from(camera_type),
            direction,
            angle,
            operator: None,
            name: None,
            ref_tag: None,
        }
    }

    /// A square footprint centred on `(lat, lon)`, `half` degrees to a side.
    fn block(lat: f64, lon: f64, half: f64, height_m: Option<f64>, building_type: &str) -> crate::viewshed::ViewBuilding {
        crate::viewshed::ViewBuilding {
            coords: alloc::vec![
                [lon - half, lat - half],
                [lon + half, lat - half],
                [lon + half, lat + half],
                [lon - half, lat + half],
            ],
            height_m,
            building_type: String::from(building_type),
        }
    }

    // A camera ~22 m due south of the subject, so the bearing camera->subject
    // is 0 degrees (north).
    const SUBJECT_LAT: f64 = 36.0002;
    const SUBJECT_LON: f64 = -86.78;
    const CAM_LAT: f64 = 36.0;
    const CAM_LON: f64 = -86.78;

    #[test]
    fn an_untagged_camera_in_range_sees_you() {
        let cams = alloc::vec![cam_at(CAM_LAT, CAM_LON, "fixed", None, None)];
        let seen = cameras_seeing(SUBJECT_LAT, SUBJECT_LON, &cams, &[], CAMERA_RANGE_M);
        assert_eq!(seen.len(), 1);
        assert!(seen[0].sees);
        assert!(seen[0].aim_assumed, "no direction tag means the aim is an assumption");
        assert!(seen[0].line_of_sight);
        assert!((seen[0].bearing_deg).abs() < 1.0, "due north, got {}", seen[0].bearing_deg);
        assert!(seen[0].distance_m > 15.0 && seen[0].distance_m < 30.0);
    }

    #[test]
    fn out_of_range_cameras_are_not_reported_at_all() {
        let cams = alloc::vec![cam_at(CAM_LAT, CAM_LON, "fixed", None, None)];
        assert!(cameras_seeing(SUBJECT_LAT, SUBJECT_LON, &cams, &[], 5.0).is_empty());
    }

    #[test]
    fn a_camera_aimed_away_does_not_see_you_but_a_dome_does() {
        // Facing due south (180) with a 60-degree cone; the subject is north.
        let facing_away = cam_at(CAM_LAT, CAM_LON, "fixed", Some(180), Some(60));
        let seen = cameras_seeing(SUBJECT_LAT, SUBJECT_LON, &[facing_away], &[], CAMERA_RANGE_M);
        assert!(!seen[0].aimed_at_you);
        assert!(!seen[0].sees);
        assert!(!seen[0].aim_assumed, "direction and angle were both tagged");

        // Same aim, but a dome sweeps -- where it points now says nothing
        // about where it can point.
        let dome = cam_at(CAM_LAT, CAM_LON, "dome", Some(180), Some(60));
        let seen = cameras_seeing(SUBJECT_LAT, SUBJECT_LON, &[dome], &[], CAMERA_RANGE_M);
        assert!(seen[0].sees);
        assert!(seen[0].aim_assumed);
    }

    #[test]
    fn a_camera_aimed_at_you_sees_you() {
        // Facing due north (0), narrow cone, subject dead ahead.
        let cams = alloc::vec![cam_at(CAM_LAT, CAM_LON, "fixed", Some(0), Some(30))];
        let seen = cameras_seeing(SUBJECT_LAT, SUBJECT_LON, &cams, &[], CAMERA_RANGE_M);
        assert!(seen[0].aimed_at_you);
        assert!(seen[0].sees);
    }

    #[test]
    fn a_building_in_the_way_blocks_the_sight_line() {
        let cams = alloc::vec![cam_at(CAM_LAT, CAM_LON, "fixed", None, None)];
        // A tall block halfway between, straddling the line.
        let wall = block(36.0001, -86.78, 0.00003, Some(20.0), "office");
        let seen = cameras_seeing(SUBJECT_LAT, SUBJECT_LON, &cams, &[wall], CAMERA_RANGE_M);
        assert!(!seen[0].line_of_sight);
        assert_eq!(seen[0].blocked_by, Some(0));
        assert!(!seen[0].sees, "aimed at you, but there is a building in between");
    }

    #[test]
    fn a_low_canopy_does_not_block_a_sight_line_that_passes_over_it() {
        let cams = alloc::vec![cam_at(CAM_LAT, CAM_LON, "fixed", None, None)];
        // 1 m tall: below the line, which runs from 4 m down to 1.7 m.
        let canopy = block(36.0001, -86.78, 0.00003, Some(1.0), "canopy");
        let seen = cameras_seeing(SUBJECT_LAT, SUBJECT_LON, &cams, &[canopy], CAMERA_RANGE_M);
        assert!(seen[0].line_of_sight, "the line clears a 1 m obstacle");
        assert!(seen[0].sees);
    }

    #[test]
    fn an_unmeasured_building_is_credited_with_its_low_height_not_its_typical() {
        // A shed: low 2.5 m, typical 3.0. The line at the crossing is ~2.9 m,
        // so the low end does not block and the typical would. Uncertainty
        // must cost cover, not grant it.
        let cams = alloc::vec![cam_at(CAM_LAT, CAM_LON, "fixed", None, None)];
        let shed = block(36.00005, -86.78, 0.00002, None, "shed");
        let seen = cameras_seeing(SUBJECT_LAT, SUBJECT_LON, &cams, &[shed], CAMERA_RANGE_M);
        assert!(
            seen[0].line_of_sight,
            "a guessed height must not be what tells someone they are unobserved"
        );
    }

    #[test]
    fn standing_inside_a_building_blocks_a_camera_outside_it() {
        let cams = alloc::vec![cam_at(CAM_LAT, CAM_LON, "fixed", None, None)];
        // A footprint around the subject that the camera is outside of.
        let indoors = alloc::vec![block(SUBJECT_LAT, SUBJECT_LON, 0.00005, Some(10.0), "office")];
        let seen = cameras_seeing(SUBJECT_LAT, SUBJECT_LON, &cams, &indoors, CAMERA_RANGE_M);
        assert!(!seen[0].sees, "walls are between you and it");
        assert_eq!(seen[0].blocked_by, Some(0));

        // Both inside the same building: the walls are no longer between them.
        let inside_too = alloc::vec![cam_at(SUBJECT_LAT + 0.00001, SUBJECT_LON, "fixed", None, None)];
        let seen = cameras_seeing(SUBJECT_LAT, SUBJECT_LON, &inside_too, &indoors, CAMERA_RANGE_M);
        assert!(seen[0].sees);
    }

    #[test]
    fn the_nearest_camera_is_reported_first() {
        let near = cam_at(36.0001, -86.78, "fixed", None, None);
        let mut far = cam_at(36.0004, -86.78, "fixed", None, None);
        far.osm_id = 2;
        let seen = cameras_seeing(SUBJECT_LAT, SUBJECT_LON, &[far, near], &[], CAMERA_RANGE_M);
        assert_eq!(seen.len(), 2);
        assert!(seen[0].distance_m <= seen[1].distance_m);
        assert_eq!(seen[0].index, 1, "index still points into the slice as given");
    }

    #[test]
    fn bearing_is_clockwise_from_north() {
        // North, east, south, west of the same origin.
        assert!(bearing_to(36.0, -86.78, 36.001, -86.78).abs() < 0.5);
        assert!((bearing_to(36.0, -86.78, 36.0, -86.779) - 90.0).abs() < 0.5);
        assert!((bearing_to(36.0, -86.78, 35.999, -86.78) - 180.0).abs() < 0.5);
        assert!((bearing_to(36.0, -86.78, 36.0, -86.781) - 270.0).abs() < 0.5);
    }

    #[test]
    fn cameras_near_road_degenerate_segment_returns_none() {
        let road = [[-86.78, 36.16]]; // single point, not a segment
        let cam = Camera {
            osm_id: 1,
            lon: -86.78,
            lat: 36.16,
            device_type: String::from("camera"),
            placement: String::from("public"),
            camera_type: String::from("fixed"),
            direction: None,
            angle: None,
            operator: None,
            name: None,
            ref_tag: None,
        };
        assert!(cameras_near_road(&[cam], &road, 30.0).is_empty());
    }
}
