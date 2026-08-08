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
    DecodeError, decode_string_u8, decode_string_u16, decode_varint, read_i16, read_i32, read_u8,
    read_u16, read_u32, zigzag_decode,
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
    /// Which upstream dataset this record came from: 1 = Overture, 2 =
    /// Foursquare. From the extended-attributes trailer (see
    /// [`decode_ext_attrs`]); `None` when the record carries no trailer.
    pub source_type: Option<u8>,
    /// The upstream record id -- a GERS id for Overture, a venue id for
    /// Foursquare. The only stable handle back to the source dataset, which is
    /// why it is worth carrying rather than skipping.
    pub source_id: Option<String>,
    /// Upstream confidence, 0-100 as the builder writes it.
    pub confidence: Option<u8>,
}

/// Decode one v3 record body. `end` is the record's own end offset (from its
/// `u32` length prefix): the trailer must be read within the record, or on a
/// record that happens to lack one it would read the *next* record's length
/// prefix as `ext_flags`.
fn decode_business_record(
    data: &[u8],
    pos: usize,
    end: usize,
) -> Result<(Business, usize), DecodeError> {
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

    if flags & 0x80 != 0 {
        // chain_count: u8. Never read before, which put the trailer read one
        // byte early on the 54% of golden-fixture records that set this bit.
        p += 1;
    }

    // The same trailer v4 carries. v3's length prefix meant discarding it cost
    // nothing structurally, but it is real data: the upstream dataset and id.
    let (source_type, source_id, confidence, consumed) =
        decode_ext_attrs(&data[..end.min(data.len())], p)?;
    p += consumed;

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
            source_type,
            source_id,
            confidence,
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
pub fn decode_business_v3(data: &[u8]) -> Result<Vec<Business>, DecodeError> {
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
        if let Ok((biz, _consumed)) = decode_business_record(data, rec_start, rec_start + record_len)
        {
            records.push(biz);
        }
        p += record_len;
    }

    Ok(records)
}

/// `(source_type, source_id, confidence, bytes_consumed)` -- see
/// [`decode_ext_attrs`].
type ExtAttrs = (Option<u8>, Option<String>, Option<u8>, usize);

/// The extended-attributes trailer both v3 and v4 records end with.
///
/// ```text
/// ext_flags   u16   0x01 source_type, 0x02 source_id, 0x04 confidence
///   0x01 -> u8            source_type (1 = Overture, 2 = Foursquare)
///   0x02 -> u16 len + utf8 source_id
///   0x04 -> u8            confidence
/// ```
///
/// **Reading this is not optional.** The builder emits it whenever
/// `ext_flags != 0`, and `source_type` is always set, so every record has one.
/// v3 hid that: its `u32 record_len` prefix resynchronises the stream every
/// record, so 30-42 unread bytes per record were invisible. v4 has no prefix, so
/// skipping the trailer desynchronised the stream permanently after record #1 --
/// and because a v4 record has no structural check, the result was thousands of
/// well-formed garbage records and then an "unexpected end of input" once a
/// garbage `u16` length finally exceeded the bytes left. That is the bug this
/// function exists to close; see the tests at the bottom of this module.
///
/// Returns `(source_type, source_id, confidence, consumed)`. A record with no
/// bytes left is legal and yields all-`None` with `consumed == 0`.
fn decode_ext_attrs(data: &[u8], pos: usize) -> Result<ExtAttrs, DecodeError> {
    if pos + 2 > data.len() {
        return Ok((None, None, None, 0));
    }
    let mut p = pos;
    let ext_flags = read_u16(data, p)?;
    p += 2;
    if ext_flags == 0 {
        return Ok((None, None, None, p - pos));
    }
    let mut source_type = None;
    let mut source_id = None;
    let mut confidence = None;
    if ext_flags & 0x01 != 0 {
        source_type = Some(read_u8(data, p)?);
        p += 1;
    }
    if ext_flags & 0x02 != 0 {
        let (s, consumed) = decode_string_u16(data, p)?;
        source_id = Some(s);
        p += consumed;
    }
    if ext_flags & 0x04 != 0 {
        confidence = Some(read_u8(data, p)?);
        p += 1;
    }
    Ok((source_type, source_id, confidence, p - pos))
}

/// Decode one v4 business record body. v4 format: sequential uid (zigzag
/// varint from 0), i16 cell-relative coords, no u32 record_len prefix.
/// Returns `(Business, bytes_consumed)`.
fn decode_business_record_v4(
    data: &[u8],
    pos: usize,
    cell_center_lon_micro: i32,
    cell_center_lat_micro: i32,
) -> Result<(Business, usize), DecodeError> {
    let mut p = pos;

    // Sequential uid (zigzag varint from 0)
    let (uid_raw, consumed) = decode_varint(data, p)?;
    p += consumed;
    let uid = zigzag_decode(uid_raw);

    // Cell-relative i16 coords (may be i32 on overflow — detect by checking
    // whether remaining bytes support i16 or i32, but the Python builder
    // always i16-packs. Ponytail: read i16, the rare edge-of-cell POI
    // that overflows is small enough not to matter for the demo/geo lookup.
    let offset_lon = read_i16(data, p)? as i32;
    let offset_lat = read_i16(data, p + 2)? as i32;
    p += 4;
    let lon_micro = cell_center_lon_micro.wrapping_add(offset_lon);
    let lat_micro = cell_center_lat_micro.wrapping_add(offset_lat);

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
    // v4 bits 0x10, 0x20, 0x40 are unused (reserved for future amenities)
    if flags & 0x80 != 0 {
        // chain_count: u8, consumed for position tracking
        p += 1;
    }

    // The trailer. Without this the next record starts 30-42 bytes early and
    // every record after this one is garbage -- see `decode_ext_attrs`.
    let (source_type, source_id, confidence, consumed) = decode_ext_attrs(data, p)?;
    p += consumed;

    Ok((
        Business {
            osm_id: uid,
            lat: lat_micro as f64 / 100_000.0,
            lon: lon_micro as f64 / 100_000.0,
            name,
            category_idx,
            phone,
            website,
            address,
            brand,
            operating_status: String::from("open"),
            emails: Vec::new(),
            socials: Vec::new(),
            source_type,
            source_id,
            confidence,
        },
        p - pos,
    ))
}

/// Decode a decompressed v4 business block into its records.
///
/// Format: sequentially concatenated records with no length prefix and no
/// terminator. Each record is self-delimiting. The feature_count from the
/// spatial index determines the exact count in production; here we parse
/// until end of input (which catches truncated data as errors via codec).
pub fn decode_business_v4(data: &[u8]) -> Result<Vec<Business>, DecodeError> {
    decode_business_v4_at(data, 0, 0)
}

/// Decode a v4 block against a known cell centre, in microdegrees.
///
/// v4 stores coordinates as `i16` offsets from the centre of the H3 cell the
/// block belongs to, so without the centre the coordinates are meaningless —
/// `decode_business_v4` passing `(0, 0)` put every record within a few hundred
/// metres of Null Island. Prefer [`decode_business_for_cell`], which derives
/// the centre from the cell id and cannot be handed the wrong one.
pub fn decode_business_v4_at(
    data: &[u8],
    center_lon_micro: i32,
    center_lat_micro: i32,
) -> Result<Vec<Business>, DecodeError> {
    let mut records = Vec::new();
    let mut p = 0usize;

    while p < data.len() {
        let (biz, consumed) = decode_business_record_v4(data, p, center_lon_micro, center_lat_micro)?;
        records.push(biz);
        p += consumed;
    }

    Ok(records)
}

/// Decode a v4 business block belonging to `cell`, deriving the coordinate
/// origin from the cell id itself.
///
/// The counterpart to `decode_buildings_for_cell`, and for the same reason: a
/// wrong origin yields well-formed records in the wrong place with no error, so
/// the origin should never be a caller-supplied number.
///
/// Callers that know the index's `feature_count` should compare it against
/// `records.len()`: v4 has no per-record framing, so a count mismatch is the
/// only cheap signal that the stream desynchronised.
pub fn decode_business_for_cell(data: &[u8], cell: u64) -> Result<Vec<Business>, DecodeError> {
    let (lat, lon) = crate::query::try_cell_center(cell).ok_or(DecodeError::InvalidCell { cell })?;
    decode_business_v4_at(
        data,
        (lon * 100_000.0).round() as i32,
        (lat * 100_000.0).round() as i32,
    )
}

/// Decode a business block whose format version is known from the file header.
///
/// Prefer this over [`decode_business`], whose version sniff is a heuristic.
pub fn decode_business_versioned(
    data: &[u8],
    version: u8,
    cell: u64,
) -> Result<Vec<Business>, DecodeError> {
    if version >= 4 {
        decode_business_for_cell(data, cell)
    } else {
        decode_business_v3(data)
    }
}

/// Auto-detect and decode a decompressed business block (v3 or v4).
///
/// v3 blocks use `{ u32 record_len, record_body }` framing terminated by a
/// zero-length record. v4 blocks have no framing — records are concatenated
/// sequentially with the count known from the spatial index.
///
/// Detection: if the first 4 bytes as a little-endian u32 are a plausible
/// v3 record length (≥ 4), try v3 first. Otherwise try v4. Falls back to
/// the other format on parse failure.
pub fn decode_business(data: &[u8]) -> Result<Vec<Business>, DecodeError> {
    if data.is_empty() || data.len() < 4 {
        return Ok(Vec::new());
    }
    let first_u32 = u32::from_le_bytes(data[..4].try_into().unwrap());
    // v3 record_len is always ≥ 4 (minimum record: 1 varint + 4 coords +
    // 2 name-len + 1 name-byte + 1 cat + 1 flags = 10+). v4 starts with a
    // tiny zigzag uid (0→0x00, 1→0x02, 2→0x04…), so first_u32 is very
    // small (≤ 0x04040404 for uid 0..=3 spread across 4 bytes).
    if (4..=0x0001_0000).contains(&first_u32) {
        // Try v3 (length-prefixed framing)
        let result = decode_business_v3(data);
        if result.is_ok() {
            return result;
        }
    }
    // Try v4 (sequential records, no framing)
    decode_business_v4(data)
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
        // Auto-detect: first_u32=10 → try v3, fails RecordOverrun → try v4, fails
        // UnexpectedEof → returns v4 error. Both are Err — any error is fine.
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
        // v3-only: direct call bypasses auto-detect (heuristic rejects 0xFFFFFFFF)
        assert!(matches!(
            decode_business_v3(&block),
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
    #[test]
    fn golden_v3_records_all_carry_the_trailer() {
        // Every real record has one: the builder always writes `source_type`,
        // so `ext_flags != 0` always. This is the check that proves the v3
        // decoder now lands flush on `record_len` rather than 1-42 bytes short.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("test-fixtures")
            .join("golden")
            .join("business.block.bin");
        let block = std::fs::read(&path).expect("read golden business block");
        let recs = decode_business(&block).expect("decode golden block");
        assert_eq!(recs.len(), 9388, "every record must survive the trailer read");
        for b in &recs {
            assert!(
                matches!(b.source_type, Some(1) | Some(2)),
                "{}: source_type {:?}",
                b.name,
                b.source_type
            );
            let id = b.source_id.as_deref().unwrap_or("");
            assert!(
                id.len() >= 20 && id.chars().all(|c| c.is_ascii_graphic()),
                "{}: source_id {:?}",
                b.name,
                id
            );
            assert!(
                b.confidence.map_or(true, |c| c <= 100),
                "{}: confidence {:?}",
                b.name,
                b.confidence
            );
        }
    }

    // --- v4 ---------------------------------------------------------------
    //
    // v4 has no per-record length prefix, so every byte a record leaves unread
    // shifts the start of the next one. The extended-attributes trailer is
    // 30-42 bytes on real data, and skipping it desynchronised the whole block
    // after record #1: thousands of well-formed garbage records, then an
    // "unexpected end of input" once a garbage u16 length ran past the block.
    // These tests pin the framing so that cannot come back.

    /// Build one v4 record body. `ext` is `(source_type, source_id, confidence)`;
    /// `None` writes `ext_flags = 0`, and each inner `None` clears its bit.
    fn v4_body(
        uid: i64,
        lon_off: i16,
        lat_off: i16,
        name: &str,
        category_idx: u8,
        ext: Option<(Option<u8>, Option<&str>, Option<u8>)>,
    ) -> Vec<u8> {
        let mut b = Vec::new();
        encode_varint(zigzag_encode(uid), &mut b);
        b.extend_from_slice(&lon_off.to_le_bytes());
        b.extend_from_slice(&lat_off.to_le_bytes());
        push_str_u16(name, &mut b);
        b.push(category_idx);
        b.push(0); // flags: no optional fields
        match ext {
            None => b.extend_from_slice(&0u16.to_le_bytes()),
            Some((st, sid, conf)) => {
                let mut flags = 0u16;
                if st.is_some() {
                    flags |= 0x01;
                }
                if sid.is_some() {
                    flags |= 0x02;
                }
                if conf.is_some() {
                    flags |= 0x04;
                }
                b.extend_from_slice(&flags.to_le_bytes());
                if let Some(v) = st {
                    b.push(v);
                }
                if let Some(v) = sid {
                    push_str_u16(v, &mut b);
                }
                if let Some(v) = conf {
                    b.push(v);
                }
            }
        }
        b
    }

    #[test]
    fn v4_reads_the_full_trailer() {
        let block = v4_body(
            0,
            120,
            -250,
            "Dollar General",
            42,
            Some((Some(1), Some("08f2a9b4c1d0e5f60123456789abcdef"), Some(93))),
        );
        let out = decode_business_v4_at(&block, 3_600_000, -8_600_000).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "Dollar General");
        assert_eq!(out[0].category_idx, 42);
        assert_eq!(out[0].source_type, Some(1));
        assert_eq!(
            out[0].source_id.as_deref(),
            Some("08f2a9b4c1d0e5f60123456789abcdef")
        );
        assert_eq!(out[0].confidence, Some(93));
        // Offsets are microdegrees relative to the supplied centre.
        assert!((out[0].lon - 36.0012).abs() < 1e-6, "lon {}", out[0].lon);
        assert!((out[0].lat + 86.0025).abs() < 1e-6, "lat {}", out[0].lat);
    }

    #[test]
    fn v4_trailer_with_only_source_type() {
        let block = v4_body(0, 0, 0, "Shell", 7, Some((Some(2), None, None)));
        let out = decode_business_v4_at(&block, 0, 0).unwrap();
        assert_eq!(out[0].source_type, Some(2));
        assert_eq!(out[0].source_id, None);
        assert_eq!(out[0].confidence, None);
    }

    #[test]
    fn v4_zero_ext_flags_is_a_two_byte_trailer() {
        let block = v4_body(0, 0, 0, "Shell", 7, None);
        let out = decode_business_v4_at(&block, 0, 0).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].source_type, None);
    }

    /// The regression: with the trailer unread, record #2 started 30-odd bytes
    /// early and decoded as garbage.
    #[test]
    fn v4_second_record_starts_after_the_first_trailer() {
        let mut block = v4_body(
            0,
            100,
            100,
            "First",
            1,
            Some((Some(1), Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"), Some(80))),
        );
        block.extend_from_slice(&v4_body(
            1,
            -100,
            -100,
            "Second",
            2,
            Some((Some(2), Some("bbbbbbbbbbbbbbbbbbbb"), Some(50))),
        ));
        let out = decode_business_v4_at(&block, 0, 0).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].name, "First");
        assert_eq!(out[1].name, "Second");
        assert_eq!(out[1].osm_id, 1);
        assert_eq!(out[1].category_idx, 2);
        assert_eq!(out[1].confidence, Some(50));
    }

    #[test]
    fn v4_truncated_trailer_errors_not_panics() {
        let full = v4_body(0, 0, 0, "Cafe", 3, Some((Some(1), Some("abcdef"), Some(70))));
        for cut in 1..full.len() {
            // Every truncation must either decode cleanly or error. Never panic,
            // and never loop forever on a zero-length record.
            let _ = decode_business_v4_at(&full[..cut], 0, 0);
        }
        // Cutting inside the source_id string is a hard error, not a silent short read.
        assert!(decode_business_v4_at(&full[..full.len() - 3], 0, 0).is_err());
    }

    #[test]
    fn v4_for_cell_rejects_an_unresolvable_cell() {
        let block = v4_body(0, 0, 0, "Cafe", 3, None);
        assert!(matches!(
            decode_business_for_cell(&block, 0),
            Err(DecodeError::InvalidCell { cell: 0 })
        ));
    }

    #[test]
    fn v4_for_cell_puts_records_inside_the_cell() {
        // Res-7 cell containing 36.35605, -86.07246 -- the point that produced
        // the original "unexpected end of input" report.
        let cell = crate::query::cell_for_coord(36.35605, -86.07246);
        let (clat, clon) = crate::query::try_cell_center(cell).unwrap();
        let block = v4_body(0, 250, -400, "Rural Store", 5, Some((Some(1), None, None)));
        let out = decode_business_for_cell(&block, cell).unwrap();
        assert!(
            (out[0].lat - (clat - 0.004)).abs() < 1e-4,
            "lat {} vs centre {}",
            out[0].lat,
            clat
        );
        assert!(
            (out[0].lon - (clon + 0.0025)).abs() < 1e-4,
            "lon {} vs centre {}",
            out[0].lon,
            clon
        );
        // The check that would have caught Null Island.
        assert!(out[0].lat > 36.0 && out[0].lat < 36.7, "lat {}", out[0].lat);
        assert!(out[0].lon > -86.5 && out[0].lon < -85.6, "lon {}", out[0].lon);
    }

    #[test]
    fn golden_v4_block_decodes_flush_and_in_place() {
        // The real block for the cell that produced the original report,
        // captured by test-fixtures/extract_business_v4.py.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("test-fixtures")
            .join("golden");
        let block = std::fs::read(dir.join("business_v4.block.bin")).expect("read v4 block");
        let meta = std::fs::read_to_string(dir.join("business_v4.meta.json")).expect("read v4 meta");
        // Two fields out of a tiny flat JSON object; a serde dependency for
        // this would be the tail wagging the dog.
        // Parsed as text, not through f64: a res-7 cell id needs 59 bits and
        // f64 has 53, so `609196074095083519` came back as `...520`.
        let field = |key: &str| -> &str {
            let at = meta.find(key).unwrap_or_else(|| panic!("{key} in meta"));
            let rest = &meta[at + key.len() + 2..];
            let end = rest
                .find(|c: char| !(c.is_ascii_digit() || c == '-' || c == '.' || c == 'e'))
                .unwrap_or(rest.len());
            rest[..end].trim()
        };
        let cell: u64 = field("\"cell_id_int\"").parse().unwrap();
        let feature_count: usize = field("\"feature_count_in_index\"").parse().unwrap();
        let clat: f64 = field("\"cell_center_lat\"").parse().unwrap();
        let clon: f64 = field("\"cell_center_lon\"").parse().unwrap();

        let recs = decode_business_for_cell(&block, cell).expect("v4 block decodes");
        // Exactly the index's count: v4 has no per-record framing, so a count
        // mismatch is the only cheap signal that the stream desynchronised.
        // Before the trailer fix this produced garbage records and then
        // "unexpected end of input at offset 42".
        assert_eq!(recs.len(), feature_count, "records vs index feature_count");
        for b in &recs {
            assert!(!b.name.is_empty(), "unnamed record: {b:?}");
            assert!(
                matches!(b.source_type, Some(1) | Some(2)),
                "{}: source_type {:?}",
                b.name,
                b.source_type
            );
            // A res-7 cell is roughly 5 km across; anything further out means
            // the coordinate origin was wrong. This is the Null Island check.
            assert!(
                (b.lat - clat).abs() < 0.05 && (b.lon - clon).abs() < 0.05,
                "{} at {},{} is outside cell centred {},{}",
                b.name,
                b.lat,
                b.lon,
                clat,
                clon
            );
        }
    }

    #[test]
    fn versioned_dispatch_picks_the_right_framing() {
        let cell = crate::query::cell_for_coord(36.35605, -86.07246);
        let v4 = v4_body(0, 0, 0, "V4 Store", 1, Some((Some(1), None, None)));
        assert_eq!(
            decode_business_versioned(&v4, 4, cell).unwrap()[0].name,
            "V4 Store"
        );
        let v3 = frame(&record_body(
            1, 3_600_000, -8_600_000, "V3 Store", 1, 0, None, None, None, None, None, None,
        ));
        assert_eq!(
            decode_business_versioned(&v3, 3, cell).unwrap()[0].name,
            "V3 Store"
        );
    }

}
