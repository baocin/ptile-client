//! Camera block decoder (`{ST}.camera.ptiles`, PTILESC v1).
//!
//! Point records: osm_id (zigzag delta), lon/i32, lat/i32, device_type/u8,
//! placement/u8, camera_type/u8, flags/u8, [direction/u16], [operator/u8str],
//! [name/u16str], [ref/u8str].

use alloc::string::String;
use alloc::vec::Vec;

use crate::codec::{
    DecodeError, decode_string_u8, decode_string_u16, decode_varint, read_i32, read_u8, read_u16,
    zigzag_decode,
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
            lon: lon_micro as f64 / 100_000.0,
            lat: lat_micro as f64 / 100_000.0,
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
