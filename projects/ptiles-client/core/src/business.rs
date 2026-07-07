//! Business/POI block decoder (`.business.ptiles`).
//!
//! SPEC.md lists business schema version 2, whose merged-block v2 framing
//! (`ptiles/business.py::decode_business_record_v2`/`decode_merged_block_*`)
//! needs the H3 cell center and a `cell_index` from the spatial index — it
//! is not a pure `&[u8] -> records` function and belongs with the
//! file/index layer (out of scope for this task, see report). This decoder
//! instead ports the self-contained v1 record format
//! (`decode_business_record`/`decode_block`), which matches the plan's
//! `decode_business(data: &[u8]) -> Result<Vec<Business>, DecodeError>`
//! signature and is what the seed crate's `decode_business` approximated.
//! Differences from the seed, cross-checked against the Python reference:
//! `osm_id` is a single zigzag varint (NOT delta-from-previous, unlike every
//! other layer), `name` is required (u16-length), category is carried as a
//! raw table index (resolution against the categories sidecar is a
//! file/query-layer concern), and `operating_status`/`emails`/`socials` are
//! decoded from flag bits the seed never modeled — while the seed's
//! `chain_count` field does not exist in the reference format and is
//! dropped here.

use alloc::string::String;
use alloc::vec::Vec;

use crate::codec::{decode_string_u16, decode_string_u8, decode_varint, read_i32, read_u32, read_u8, zigzag_decode, DecodeError};

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Business {
    pub osm_id: i64,
    pub lat: f64,
    pub lon: f64,
    pub name: String,
    /// Raw category table index (0 = missing); resolve against the
    /// categories sidecar at the file/query layer.
    pub category_idx: u8,
    pub phone: Option<String>,
    pub website: Option<String>,
    pub address: Option<String>,
    pub brand: Option<String>,
    pub operating_status: String, // "open" | "closed" | "temporarily_closed"
    pub emails: Vec<String>,
    pub socials: Vec<String>,
}

fn decode_business_record(data: &[u8], pos: usize) -> Result<(Business, usize), DecodeError> {
    let start = pos;
    let mut p = pos;

    // osm_id: single zigzag varint, NOT delta from previous (differs from
    // every other layer — confirmed against ptiles/business.py).
    let (osm_raw, consumed) = decode_varint(data, p)?;
    p += consumed;
    let osm_id = zigzag_decode(osm_raw);

    let lon_micro = read_i32(data, p)?;
    let lat_micro = read_i32(data, p + 4)?;
    p += 8;

    let (name, consumed) = decode_string_u16(data, p)?;
    p += consumed;

    let category_idx = read_u8(data, p)?;
    p += 1;

    let flags = read_u8(data, p)?;
    p += 1;

    let mut phone = None;
    let mut website = None;
    let mut address = None;
    let mut brand = None;
    let mut emails = Vec::new();
    let mut socials = Vec::new();

    if flags & 0x01 != 0 {
        let (s, consumed) = decode_string_u8(data, p)?;
        phone = Some(s);
        p += consumed;
    }
    if flags & 0x02 != 0 {
        let (s, consumed) = decode_string_u8(data, p)?;
        website = Some(s);
        p += consumed;
    }
    if flags & 0x04 != 0 {
        let (s, consumed) = decode_string_u16(data, p)?;
        address = Some(s);
        p += consumed;
    }
    if flags & 0x08 != 0 {
        let (s, consumed) = decode_string_u8(data, p)?;
        brand = Some(s);
        p += consumed;
    }
    if flags & 0x20 != 0 {
        let (s, consumed) = decode_string_u8(data, p)?;
        p += consumed;
        emails = s.split(';').map(|e| e.trim()).filter(|e| !e.is_empty()).map(String::from).collect();
    }
    if flags & 0x40 != 0 {
        let (s, consumed) = decode_string_u8(data, p)?;
        p += consumed;
        socials = s.split(';').map(|e| e.trim()).filter(|e| !e.is_empty()).map(String::from).collect();
    }

    // operating_status: 0x10 combined with 0x02 (website flag reused as a
    // second bit here) per ptiles.business's encoding.
    let operating_status = if flags & 0x10 != 0 && flags & 0x02 == 0 {
        String::from("closed")
    } else if flags & 0x10 != 0 && flags & 0x02 != 0 {
        String::from("temporarily_closed")
    } else {
        String::from("open")
    };

    Ok((
        Business {
            osm_id,
            lat: lat_micro as f64 / 100_000.0,
            lon: lon_micro as f64 / 100_000.0,
            name,
            category_idx,
            phone,
            website,
            address,
            brand,
            operating_status,
            emails,
            socials,
        },
        p - start,
    ))
}

/// Decode a decompressed v1 business block into its records.
///
/// Format: repeated `{ u32 record_len, record_body }`, terminated by a
/// zero-length record or end of input. A record that fails to decode is
/// skipped, matching `ptiles.business.decode_block`'s log-and-continue
/// behavior.
pub fn decode_business(data: &[u8]) -> Result<Vec<Business>, DecodeError> {
    let mut records = Vec::new();
    let mut p = 0usize;

    while p + 4 <= data.len() {
        let record_len = read_u32(data, p)? as usize;
        p += 4;
        if record_len == 0 {
            break;
        }
        if p + record_len > data.len() {
            return Err(DecodeError::RecordOverrun {
                offset: p,
                len: record_len,
                block_len: data.len(),
            });
        }
        let rec_start = p;
        if let Ok((biz, _consumed)) = decode_business_record(data, rec_start) {
            records.push(biz);
        }
        p += record_len;
    }

    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_block_decodes_to_empty_vec() {
        assert_eq!(decode_business(&[]).unwrap(), Vec::new());
    }

    #[test]
    fn truncated_block_errors_not_panics() {
        let block = [10u8, 0, 0, 0, 1, 2]; // claims 10-byte record, only 2 present
        assert!(decode_business(&block).is_err());
    }
}
