//! Signals block decoder (`{ST}.signals.ptiles`, PTILESS v1).
//!
//! Point records: osm_id (zigzag delta), lon/i32, lat/i32, signal_type/u8,
//! flags/u8, [direction/u16].

use alloc::string::String;
use alloc::vec::Vec;

use crate::codec::{DecodeError, decode_varint, read_i32, read_u8, read_u16, zigzag_decode};

/// A signal point decoded from a `.signals.ptiles` block.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Signal {
    pub osm_id: i64,
    pub lon: f64,
    pub lat: f64,
    /// Signal type string ("traffic_signals", "stop", etc.).
    pub signal_type: String,
    /// Direction in degrees, if known.
    pub direction: Option<u16>,
}

/// Signal type table: byte index -> type name. Matches the Python builder's
/// `SIGNAL_TYPES` order.
const SIGNAL_TYPES: &[&str] = &[
    "traffic_signals",
    "crossing_signals",
    "stop",
    "give_way",
    "railway_signals",
];

fn decode_signal_record(
    data: &[u8],
    pos: usize,
    prev_osm_id: i64,
) -> Result<(Signal, usize, i64), DecodeError> {
    let start = pos;
    let mut p = pos;

    let (delta_raw, consumed) = decode_varint(data, p)?;
    p += consumed;
    let osm_id = prev_osm_id.wrapping_add(zigzag_decode(delta_raw));

    let lon_micro = read_i32(data, p)?;
    let lat_micro = read_i32(data, p + 4)?;
    p += 8;

    let st_idx = read_u8(data, p)? as usize;
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

    let signal_type = SIGNAL_TYPES
        .get(st_idx)
        .map(|s| String::from(*s))
        .unwrap_or_else(|| alloc::format!("unknown({st_idx})"));

    Ok((
        Signal {
            osm_id,
            lon: lon_micro as f64 / 100_000.0,
            lat: lat_micro as f64 / 100_000.0,
            signal_type,
            direction,
        },
        p - start,
        osm_id,
    ))
}

/// Decode a decompressed signals block into individual records.
pub fn decode_signals(data: &[u8]) -> Result<Vec<Signal>, DecodeError> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    let mut prev_osm_id = 0i64;

    while pos < data.len() {
        match decode_signal_record(data, pos, prev_osm_id) {
            Ok((sig, consumed, new_prev)) => {
                prev_osm_id = new_prev;
                pos += consumed.max(1);
                out.push(sig);
            }
            Err(_) => break,
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_node(osm_delta: i64, st_idx: u8, direction: Option<u16>) -> Vec<u8> {
        let mut d = Vec::new();
        // OSM delta as zigzag varint
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
        d.extend_from_slice(&(-8_677_373i32).to_le_bytes()); // lon ~ -86.77373
        d.extend_from_slice(&3_616_206i32.to_le_bytes()); // lat ~ 36.16206
        d.push(st_idx);
        let mut flags = 0u8;
        if direction.is_some() {
            flags |= 0x01;
        }
        d.push(flags);
        if let Some(dir) = direction {
            d.extend_from_slice(&dir.to_le_bytes());
        }
        d
    }

    #[test]
    fn empty_block_decodes_to_empty() {
        assert_eq!(decode_signals(&[]).unwrap(), Vec::new());
    }

    #[test]
    fn traffic_signal_no_direction() {
        let data = synthetic_node(100, 0, None);
        let sigs = decode_signals(&data).unwrap();
        assert_eq!(sigs.len(), 1);
        assert_eq!(sigs[0].osm_id, 100);
        assert!((sigs[0].lon - (-86.77373)).abs() < 1e-9);
        assert!((sigs[0].lat - 36.16206).abs() < 1e-9);
        assert_eq!(sigs[0].signal_type, "traffic_signals");
        assert_eq!(sigs[0].direction, None);
    }

    #[test]
    fn stop_with_direction() {
        let data = synthetic_node(200, 2, Some(90));
        let sigs = decode_signals(&data).unwrap();
        assert_eq!(sigs[0].osm_id, 200);
        assert_eq!(sigs[0].signal_type, "stop");
        assert_eq!(sigs[0].direction, Some(90));
    }

    #[test]
    fn unknown_signal_type_falls_back() {
        let data = synthetic_node(1, 255, None);
        let sigs = decode_signals(&data).unwrap();
        assert!(sigs[0].signal_type.starts_with("unknown"));
    }

    #[test]
    fn osmid_delta_accumulates() {
        let mut data = synthetic_node(100, 0, None);
        data.extend(synthetic_node(100, 1, None)); // delta +100 from prev=100
        let sigs = decode_signals(&data).unwrap();
        assert_eq!(sigs.len(), 2);
        assert_eq!(sigs[0].osm_id, 100);
        assert_eq!(sigs[1].osm_id, 200);
    }

    #[test]
    fn truncated_block_stops_gracefully() {
        let full = synthetic_node(100, 0, None);
        for cut in [1usize, 3, 10, full.len().saturating_sub(1)] {
            let sigs = decode_signals(&full[..cut]).unwrap();
            assert!(sigs.len() <= 1);
        }
    }
}
