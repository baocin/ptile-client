//! Places (`{SCOPE}.places_v2.ptiles`, magic `PTILESP`): named settlements.
//!
//! The layer was in the supported-versions table but had no decoder at all, so
//! a client could open a places file and never read a record out of it. It is
//! the layer that answers "what is this place called" -- 千代田区 for a point
//! in central Tokyo -- and the one with the second-highest `name:en` coverage
//! (75.5% of named features, measured on Shikoku), which is what makes a
//! cross-language search possible.
//!
//! ```text
//! varint zigzag osm_id delta
//! i32 lon_micro, i32 lat_micro
//! u8  place_type index
//! varint population
//! u16-prefixed name
//! u8  flags   0x01 alt_name, 0x02 admin_level, 0x04 name:en, 0x08 brand
//! ```
//!
//! Records field-walk: there is no length prefix, so an unknown trailing field
//! desyncs every record after it in the cell. That is why v2 bumped the
//! version even though every new field is flag-guarded.

use alloc::string::String;
use alloc::vec::Vec;

use crate::codec::{decode_string_u16, decode_varint, read_i32, read_u8, zigzag_decode, DecodeError};

/// Place type index, matching `PLACE_TYPE_REVERSE` in `ptiles/places.py`.
pub const PLACE_TYPE_REVERSE: &[(u8, &str)] = &[
    (0, "city"),
    (1, "town"),
    (2, "village"),
    (3, "hamlet"),
    (4, "neighborhood"),
    (5, "suburb"),
    (6, "borough"),
    (7, "quarter"),
    (8, "isolated_dwelling"),
];

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Place {
    pub osm_id: i64,
    pub lat: f64,
    pub lon: f64,
    pub place_type: String,
    pub population: u64,
    pub name: String,
    pub alt_name: Option<String>,
    pub admin_level: Option<u8>,
    /// From v2. The valuable one outside the US.
    pub name_en: Option<String>,
    pub brand: Option<String>,
}

fn place_type_name(idx: u8) -> String {
    PLACE_TYPE_REVERSE
        .iter()
        .find(|(i, _)| *i == idx)
        .map(|(_, s)| String::from(*s))
        .unwrap_or_else(|| alloc::format!("unknown({idx})"))
}

/// Decode one record. Returns `(place, bytes_consumed, osm_id)`.
pub fn decode_place(
    data: &[u8],
    offset: usize,
    prev_osm_id: i64,
) -> Result<(Place, usize, i64), DecodeError> {
    let start = offset;
    let mut p = offset;

    let (delta_raw, consumed) = decode_varint(data, p)?;
    p += consumed;
    let osm_id = prev_osm_id + zigzag_decode(delta_raw);

    let lon_micro = read_i32(data, p)?;
    let lat_micro = read_i32(data, p + 4)?;
    p += 8;

    let place_type = place_type_name(read_u8(data, p)?);
    p += 1;

    let (population, consumed) = decode_varint(data, p)?;
    p += consumed;

    let (name, consumed) = decode_string_u16(data, p)?;
    p += consumed;

    let flags = read_u8(data, p)?;
    p += 1;

    let mut alt_name = None;
    if flags & 0x01 != 0 {
        let (s, consumed) = decode_string_u16(data, p)?;
        alt_name = Some(s);
        p += consumed;
    }
    let mut admin_level = None;
    if flags & 0x02 != 0 {
        admin_level = Some(read_u8(data, p)?);
        p += 1;
    }
    let mut name_en = None;
    if flags & 0x04 != 0 {
        let (s, consumed) = decode_string_u16(data, p)?;
        name_en = Some(s);
        p += consumed;
    }
    let mut brand = None;
    if flags & 0x08 != 0 {
        let (s, consumed) = decode_string_u16(data, p)?;
        brand = Some(s);
        p += consumed;
    }

    Ok((
        Place {
            osm_id,
            lat: f64::from(lat_micro) / 100_000.0,
            lon: f64::from(lon_micro) / 100_000.0,
            place_type,
            population,
            name,
            alt_name,
            admin_level,
            name_en,
            brand,
        },
        p - start,
        osm_id,
    ))
}

/// Decode a decompressed places block. A record that fails to decode ends the
/// scan rather than being skipped: without a length prefix there is no way to
/// find where the next one starts.
pub fn decode_places(data: &[u8]) -> Result<Vec<Place>, DecodeError> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    let mut prev = 0i64;
    while pos < data.len() {
        match decode_place(data, pos, prev) {
            Ok((place, consumed, osm_id)) => {
                if consumed == 0 {
                    break;
                }
                pos += consumed;
                prev = osm_id;
                out.push(place);
            }
            Err(_) => break,
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn varint(v: u64, out: &mut Vec<u8>) {
        let mut v = v;
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(b);
                break;
            }
            out.push(b | 0x80);
        }
    }

    fn record(name: &str, name_en: Option<&str>, brand: Option<&str>) -> Vec<u8> {
        let mut out = Vec::new();
        varint(2, &mut out); // zigzag(1) -> osm_id 1
        out.extend_from_slice(&13_976_710i32.to_le_bytes());
        out.extend_from_slice(&3_568_120i32.to_le_bytes());
        out.push(0); // city
        varint(1_000, &mut out);
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        let mut flags = 0u8;
        if name_en.is_some() {
            flags |= 0x04;
        }
        if brand.is_some() {
            flags |= 0x08;
        }
        out.push(flags);
        for s in [name_en, brand].into_iter().flatten() {
            out.extend_from_slice(&(s.len() as u16).to_le_bytes());
            out.extend_from_slice(s.as_bytes());
        }
        out
    }

    #[test]
    fn decodes_a_v1_shaped_record() {
        let places = decode_places(&record("Nashville", None, None)).unwrap();
        assert_eq!(places.len(), 1);
        assert_eq!(places[0].name, "Nashville");
        assert_eq!(places[0].place_type, "city");
        assert_eq!(places[0].population, 1000);
        assert!(places[0].name_en.is_none());
    }

    #[test]
    fn decodes_v2_alternative_names() {
        let places = decode_places(&record("千代田区", Some("Chiyoda"), None)).unwrap();
        assert_eq!(places[0].name, "千代田区");
        assert_eq!(places[0].name_en.as_deref(), Some("Chiyoda"));
    }

    #[test]
    fn walks_several_records_in_one_block() {
        let mut block = record("A", None, None);
        block.extend(record("B", Some("Bee"), None));
        let places = decode_places(&block).unwrap();
        assert_eq!(places.len(), 2);
        assert_eq!(places[1].name_en.as_deref(), Some("Bee"));
        // Ids are deltas: the second record's zigzag(1) advances from the first.
        assert_eq!(places[1].osm_id, places[0].osm_id + 1);
    }

    #[test]
    fn a_truncated_tail_ends_the_scan_without_panicking() {
        let mut block = record("A", None, None);
        block.extend_from_slice(&[0xff, 0xff]);
        let places = decode_places(&block).unwrap();
        assert_eq!(places.len(), 1);
    }
}
