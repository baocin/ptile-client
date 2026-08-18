//! The polygon a file was cut to, and the containment test that uses it.
//!
//! A file's bounding box cannot say which file owns a point: Tennessee's box
//! holds parts of six neighbours, New Jersey's holds all of Manhattan, and two
//! overlapping regional extracts (Kanto's box and Chubu's both contain Tokyo)
//! cannot be told apart by box at all. So every file built since the format
//! change carries the polygon it was cut to, in a self-describing PTBD block
//! pointed at by header @84/@92.
//!
//! ```text
//! magic       4   b"PTBD"
//! version     1
//! ring_count  u16
//! per ring:   u32 vertex_count, i32 first_lon, i32 first_lat, delta pairs
//! ```
//!
//! Self-describing because the aux section is not: water stores a large-body
//! table there, admin a lookup grid, points a PTCI index, all with no magic to
//! tell them apart.
//!
//! Note the stored polygon is deliberately a little larger than the region --
//! the builder dilates it before simplifying, because a simplification that
//! cuts inside the line leaves border towns owned by no file at all. Two
//! neighbours may therefore both claim a point within a couple of hundred
//! metres of their shared border. A client that merges answers is unaffected;
//! one that must pick a single file breaks the tie elsewhere (the res-9 state
//! cell table), not here.

use alloc::vec::Vec;

use crate::codec::{decode_coordinates, DecodeError};

pub const BOUNDARY_MAGIC: &[u8; 4] = b"PTBD";
pub const BOUNDARY_VERSION: u8 = 1;

/// One closed ring of (lon, lat) degrees.
pub type Ring = Vec<[f64; 2]>;

/// Decode a PTBD block. Unrecognised or empty input yields an empty list --
/// the same "no polygon here, fall back to the bbox" answer a file written
/// before the field existed gives.
pub fn decode_boundary(data: &[u8]) -> Result<Vec<Ring>, DecodeError> {
    if data.len() < 7 || &data[0..4] != BOUNDARY_MAGIC || data[4] != BOUNDARY_VERSION {
        return Ok(Vec::new());
    }
    let ring_count = u16::from_le_bytes([data[5], data[6]]) as usize;
    let mut rings = Vec::with_capacity(ring_count.min(1 << 10));
    let mut pos = 7usize;
    for _ in 0..ring_count {
        if pos + 12 > data.len() {
            break; // truncated tail: keep the rings that did decode
        }
        let count = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
        pos += 4;
        let first_lon = i32::from_le_bytes(data[pos..pos + 4].try_into().unwrap());
        let first_lat = i32::from_le_bytes(data[pos + 4..pos + 8].try_into().unwrap());
        pos += 8;
        match decode_coordinates(data, pos, first_lon, first_lat, count) {
            Ok((coords, consumed)) => {
                pos += consumed;
                if coords.len() >= 3 {
                    rings.push(coords);
                }
            }
            Err(_) => break,
        }
    }
    Ok(rings)
}

/// Whether a point is inside a ring, by ray casting.
///
/// Longitude first, matching the stored (lon, lat) order -- getting this the
/// wrong way round produces a test that is right near the equator and wrong
/// everywhere else.
pub fn point_in_ring(lon: f64, lat: f64, ring: &[[f64; 2]]) -> bool {
    if ring.len() < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = ring.len() - 1;
    for i in 0..ring.len() {
        let (xi, yi) = (ring[i][0], ring[i][1]);
        let (xj, yj) = (ring[j][0], ring[j][1]);
        if (yi > lat) != (yj > lat) {
            let t = (lat - yi) / (yj - yi);
            if lon < xi + t * (xj - xi) {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

/// Whether any ring contains the point.
pub fn point_in_rings(lon: f64, lat: f64, rings: &[Ring]) -> bool {
    rings.iter().any(|r| point_in_ring(lon, lat, r))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn encode(rings: &[&[[f64; 2]]]) -> Vec<u8> {
        // Mirrors scripts/encoding.py::encode_boundary closely enough to test
        // the decoder: first vertex absolute, the rest zigzag varint deltas.
        let mut out = Vec::new();
        out.extend_from_slice(BOUNDARY_MAGIC);
        out.push(BOUNDARY_VERSION);
        out.extend_from_slice(&(rings.len() as u16).to_le_bytes());
        for ring in rings {
            out.extend_from_slice(&(ring.len() as u32).to_le_bytes());
            let first_lon = (ring[0][0] * 100_000.0).round() as i32;
            let first_lat = (ring[0][1] * 100_000.0).round() as i32;
            out.extend_from_slice(&first_lon.to_le_bytes());
            out.extend_from_slice(&first_lat.to_le_bytes());
            let (mut plon, mut plat) = (first_lon, first_lat);
            for v in &ring[1..] {
                let lon = (v[0] * 100_000.0).round() as i32;
                let lat = (v[1] * 100_000.0).round() as i32;
                push_varint(&mut out, zigzag(lon - plon));
                push_varint(&mut out, zigzag(lat - plat));
                plon = lon;
                plat = lat;
            }
        }
        out
    }

    fn zigzag(v: i32) -> u64 {
        ((v << 1) ^ (v >> 31)) as u32 as u64
    }

    fn push_varint(out: &mut Vec<u8>, mut v: u64) {
        loop {
            let byte = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(byte);
                break;
            }
            out.push(byte | 0x80);
        }
    }

    const SQUARE: [[f64; 2]; 5] = [
        [-86.0, 35.0],
        [-85.0, 35.0],
        [-85.0, 36.0],
        [-86.0, 36.0],
        [-86.0, 35.0],
    ];

    #[test]
    fn round_trips_a_ring() {
        let blob = encode(&[&SQUARE]);
        let rings = decode_boundary(&blob).unwrap();
        assert_eq!(rings.len(), 1);
        assert_eq!(rings[0].len(), 5);
        assert!((rings[0][1][0] - -85.0).abs() < 1e-6);
    }

    #[test]
    fn contains_inside_and_rejects_outside() {
        let rings = decode_boundary(&encode(&[&SQUARE])).unwrap();
        assert!(point_in_rings(-85.5, 35.5, &rings));
        assert!(!point_in_rings(-84.5, 35.5, &rings)); // east of the square
        assert!(!point_in_rings(-85.5, 34.5, &rings)); // south of it
    }

    #[test]
    fn absent_or_foreign_block_is_no_boundary_not_an_error() {
        assert!(decode_boundary(&[]).unwrap().is_empty());
        assert!(decode_boundary(b"PTCI\x01\x00\x00").unwrap().is_empty());
        // A future version is unreadable, not misread.
        let mut wrong = encode(&[&SQUARE]);
        wrong[4] = 99;
        assert!(decode_boundary(&wrong).unwrap().is_empty());
    }

    #[test]
    fn truncated_tail_keeps_the_rings_that_decoded() {
        let mut blob = encode(&[&SQUARE, &SQUARE]);
        blob.truncate(blob.len() - 4);
        let rings = decode_boundary(&blob).unwrap();
        assert_eq!(rings.len(), 1, "the intact ring survives a cut second one");
    }
}
