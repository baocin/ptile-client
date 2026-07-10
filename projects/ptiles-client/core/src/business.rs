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

use crate::codec::{
    DecodeError, decode_string_u8, decode_string_u16, decode_varint, read_i32, read_u8, read_u32,
    zigzag_decode,
};

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
        emails = s
            .split(';')
            .map(|e| e.trim())
            .filter(|e| !e.is_empty())
            .map(String::from)
            .collect();
    }
    if flags & 0x40 != 0 {
        let (s, consumed) = decode_string_u8(data, p)?;
        p += consumed;
        socials = s
            .split(';')
            .map(|e| e.trim())
            .filter(|e| !e.is_empty())
            .map(String::from)
            .collect();
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
        // `p + record_len` can wrap on 32-bit targets (wasm is 32-bit, and is
        // this crate's deployment target) when a corrupt `record_len` is near
        // `u32::MAX`, silently bypassing the overrun guard. Compare against the
        // remaining bytes instead — `p <= data.len()` here (loop invariant), so
        // `data.len() - p` cannot underflow.
        if record_len > data.len() - p {
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

    // --- helpers for hand-building records/blocks --------------------------

    fn zigzag_encode(n: i64) -> u64 {
        ((n << 1) ^ (n >> 63)) as u64
    }

    fn encode_varint(mut v: u64, out: &mut Vec<u8>) {
        loop {
            let mut byte = (v & 0x7f) as u8;
            v >>= 7;
            if v != 0 {
                byte |= 0x80;
            }
            out.push(byte);
            if v == 0 {
                break;
            }
        }
    }

    fn push_str_u8(s: &str, out: &mut Vec<u8>) {
        out.push(s.len() as u8);
        out.extend_from_slice(s.as_bytes());
    }

    fn push_str_u16(s: &str, out: &mut Vec<u8>) {
        out.extend_from_slice(&(s.len() as u16).to_le_bytes());
        out.extend_from_slice(s.as_bytes());
    }

    /// Build one record body (no length prefix) with the given fields.
    #[allow(clippy::too_many_arguments)]
    fn record_body(
        osm_id: i64,
        lon_micro: i32,
        lat_micro: i32,
        name: &str,
        category_idx: u8,
        flags: u8,
        phone: Option<&str>,
        website: Option<&str>,
        address: Option<&str>,
        brand: Option<&str>,
        emails: Option<&str>,
        socials: Option<&str>,
    ) -> Vec<u8> {
        let mut b = Vec::new();
        encode_varint(zigzag_encode(osm_id), &mut b);
        b.extend_from_slice(&lon_micro.to_le_bytes());
        b.extend_from_slice(&lat_micro.to_le_bytes());
        push_str_u16(name, &mut b);
        b.push(category_idx);
        b.push(flags);
        if let Some(p) = phone {
            push_str_u8(p, &mut b);
        }
        if let Some(w) = website {
            push_str_u8(w, &mut b);
        }
        if let Some(a) = address {
            push_str_u16(a, &mut b);
        }
        if let Some(br) = brand {
            push_str_u8(br, &mut b);
        }
        if let Some(e) = emails {
            push_str_u8(e, &mut b);
        }
        if let Some(s) = socials {
            push_str_u8(s, &mut b);
        }
        b
    }

    fn frame(body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(body);
        out
    }

    #[test]
    fn decodes_full_record_all_optional_fields() {
        // flags: phone(0x01) + address(0x04) + brand(0x08) + emails(0x20)
        //        + socials(0x40) = 0x6D. Deliberately no website bit (0x02)
        //        so operating_status stays "open".
        let body = record_body(
            123,
            -8_680_000,
            3_595_000,
            "Waffle House",
            5,
            0x01 | 0x04 | 0x08 | 0x20 | 0x40,
            Some("+1-615-555-0100"),
            None,
            Some("123 Main St"),
            Some("Waffle House Inc"),
            Some("a@x.com ; b@y.com ;"),
            Some("tw ; ; ig"),
        );
        let block = frame(&body);
        let recs = decode_business(&block).unwrap();
        assert_eq!(recs.len(), 1);
        let r = &recs[0];
        assert_eq!(r.osm_id, 123);
        assert_eq!(r.name, "Waffle House");
        assert_eq!(r.category_idx, 5);
        assert_eq!(r.phone.as_deref(), Some("+1-615-555-0100"));
        assert_eq!(r.website, None);
        assert_eq!(r.address.as_deref(), Some("123 Main St"));
        assert_eq!(r.brand.as_deref(), Some("Waffle House Inc"));
        assert_eq!(r.operating_status, "open");
        // trimmed + empty entries dropped
        assert_eq!(r.emails, ["a@x.com", "b@y.com"]);
        assert_eq!(r.socials, ["tw", "ig"]);
        assert!((r.lat - 35.95).abs() < 1e-9);
        assert!((r.lon - (-86.80)).abs() < 1e-9);
    }

    #[test]
    fn operating_status_variants() {
        // open: neither 0x10 nor 0x02
        let open = decode_business(&frame(&record_body(
            1, 0, 0, "A", 0, 0x00, None, None, None, None, None, None,
        )))
        .unwrap();
        assert_eq!(open[0].operating_status, "open");

        // closed: 0x10 set, 0x02 (website) clear
        let closed = decode_business(&frame(&record_body(
            1, 0, 0, "A", 0, 0x10, None, None, None, None, None, None,
        )))
        .unwrap();
        assert_eq!(closed[0].operating_status, "closed");

        // temporarily_closed: 0x10 AND 0x02 set (website field present)
        let temp = decode_business(&frame(&record_body(
            1,
            0,
            0,
            "A",
            0,
            0x10 | 0x02,
            None,
            Some("http://x"),
            None,
            None,
            None,
            None,
        )))
        .unwrap();
        assert_eq!(temp[0].operating_status, "temporarily_closed");
        assert_eq!(temp[0].website.as_deref(), Some("http://x"));
    }

    #[test]
    fn negative_osm_id_roundtrips_via_zigzag() {
        let recs = decode_business(&frame(&record_body(
            -42, 0, 0, "X", 0, 0, None, None, None, None, None, None,
        )))
        .unwrap();
        assert_eq!(recs[0].osm_id, -42);
    }

    #[test]
    fn multiple_records_then_zero_terminator() {
        let mut block = frame(&record_body(
            1, 0, 0, "First", 0, 0, None, None, None, None, None, None,
        ));
        block.extend(frame(&record_body(
            2, 0, 0, "Second", 0, 0, None, None, None, None, None, None,
        )));
        block.extend_from_slice(&0u32.to_le_bytes()); // zero-length terminator
        block.extend_from_slice(&[0xAA, 0xBB]); // garbage after terminator, must be ignored
        let recs = decode_business(&block).unwrap();
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].name, "First");
        assert_eq!(recs[1].name, "Second");
    }

    #[test]
    fn record_that_fails_to_decode_is_skipped_not_fatal() {
        // A valid record, then a record whose framed length fits the block but
        // whose declared name length overruns the record body -> the inner
        // decode fails and that record is skipped, not fatal.
        let good = record_body(1, 0, 0, "Good", 0, 0, None, None, None, None, None, None);
        // Bad body: osm(1) + 8 coord bytes + name len says 200 but no bytes.
        let mut bad = Vec::new();
        encode_varint(zigzag_encode(2), &mut bad);
        bad.extend_from_slice(&0i32.to_le_bytes());
        bad.extend_from_slice(&0i32.to_le_bytes());
        bad.extend_from_slice(&200u16.to_le_bytes()); // claims 200-byte name
        let mut block = frame(&good);
        block.extend(frame(&bad));
        let recs = decode_business(&block).unwrap();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].name, "Good");
    }

    #[test]
    fn overflowing_record_len_errors_not_panics() {
        // record_len = u32::MAX. On 32-bit targets `p + record_len` would wrap;
        // the fixed guard must still reject it as an overrun rather than panic
        // or (worse) accept it.
        let block = [0xFFu8, 0xFF, 0xFF, 0xFF, 1, 2, 3];
        assert!(matches!(
            decode_business(&block),
            Err(DecodeError::RecordOverrun { .. })
        ));
    }

    #[test]
    fn trailing_partial_length_prefix_is_ignored() {
        // Fewer than 4 bytes left cannot form a length prefix; loop stops
        // cleanly rather than reading out of bounds.
        let mut block = frame(&record_body(
            1, 0, 0, "Only", 0, 0, None, None, None, None, None, None,
        ));
        block.extend_from_slice(&[0x01, 0x02]); // 2 dangling bytes
        let recs = decode_business(&block).unwrap();
        assert_eq!(recs.len(), 1);
    }

    #[test]
    fn lossy_utf8_name_does_not_panic() {
        // Invalid UTF-8 in the name is replaced, not fatal.
        let mut body = Vec::new();
        encode_varint(zigzag_encode(1), &mut body);
        body.extend_from_slice(&0i32.to_le_bytes());
        body.extend_from_slice(&0i32.to_le_bytes());
        body.extend_from_slice(&2u16.to_le_bytes());
        body.extend_from_slice(&[0xFF, 0xFE]); // invalid UTF-8
        body.push(0); // category
        body.push(0); // flags
        let recs = decode_business(&frame(&body)).unwrap();
        assert_eq!(recs.len(), 1);
        assert!(recs[0].name.contains('\u{FFFD}'));
    }

    #[cfg(feature = "std")]
    #[test]
    fn golden_block_decodes_and_spot_checks_known_business() {
        // Decode the real golden business block and confirm a known record is
        // present with the expected fields (mirrors the reference decoder).
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("test-fixtures")
            .join("golden")
            .join("business.block.bin");
        let block = std::fs::read(&path).expect("read golden business block");
        let recs = decode_business(&block).expect("decode golden block");
        assert!(
            recs.len() > 1000,
            "golden block should hold many businesses"
        );
        let coffee = recs
            .iter()
            .find(|b| b.name == "Drug Store Coffee")
            .expect("known business present in golden fixture");
        assert_eq!(coffee.category_idx, 91);
        assert_eq!(coffee.osm_id, 323742190609548);
        assert_eq!(coffee.operating_status, "open");
        assert!((coffee.lat - 36.16377).abs() < 1e-5);
    }
}
