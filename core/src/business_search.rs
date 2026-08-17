//! Business name/category search.
//!
//! Two paths, one API surface:
//!
//! - **Index-accelerated** (`search_business_indexed`): reads a sidecar
//!   `{STATE}.business_name_index.ptiles` file (magic `PTILESX\0`, version 1,
//!   built by `~/kino/projects/ptiles/scripts/build_business_name_index.py`).
//!   That builder groups every business record from the state's
//!   `{STATE}.business.ptiles` by the **first character of its name**
//!   (`a`-`z` -> keys 0-25, digit/other/non-letter start -> key 26, empty
//!   name -> key 27 — `name_to_key` in the reference script) into 28 blocks,
//!   each plain zstd (no dict) at compression level 12, and reuses the
//!   spatial index's `h3_cell` field to hold that 0-27 key instead of an H3
//!   cell. So the "index" is a **first-letter bucket index, not a full
//!   inverted/substring index**: looking up a query fetches and decompresses
//!   exactly one ~1 MB block (out of 28) instead of scanning the ~54 MB,
//!   18k-block main business file. Within that one block we do a
//!   case-insensitive substring match — but because the bucket is keyed by
//!   the *business name's* first letter, only queries whose own first
//!   character equals the matched name's first character are guaranteed to
//!   be found this way (e.g. searching "affle" will not surface "Waffle
//!   House", which lives in the `w` bucket, not the `a` bucket). In
//!   practice this makes the index a **prefix-search accelerator**:
//!   case-insensitive prefix queries are always correct; case-insensitive
//!   substring queries are only complete when the substring starts at the
//!   name's first character. This is reported explicitly rather than
//!   silently promising full substring search.
//!
//! - **Brute-force fallback** (`search_business_brute_force`): iterates the
//!   main `{STATE}.business.ptiles` file's spatial index, decompressing one
//!   block at a time via [`crate::business::decode_business`] and matching
//!   full case-insensitive substrings across the whole name, with an
//!   early-exit once `limit` hits are found. This is the correct-but-slow
//!   path for when no name-index sidecar is available, or as a genuine
//!   substring search that the index path can't fully provide.
//!
//! Both need positioned file I/O (`PtilesFile<S>`/`PtilesSource`), so this
//! module is not `no_std`-only: the record decoders (`decode_name_index_block`,
//! reused `decode_business`) are pure `&[u8] -> Vec<_>` and work under
//! `no_std` + `alloc`, but the two search entry points that walk a
//! `PtilesFile` pull in the same `alloc`-only bound as `file.rs` itself
//! (no additional `std` requirement beyond what `PtilesFile` already needs).

use alloc::string::String;
use alloc::vec::Vec;

use crate::codec::{DecodeError, decode_string_u8, decode_string_u16, read_i32, read_u8, read_u32};
use crate::file::{FileError, PtilesFile};
use crate::source::PtilesSource;

/// A single matched business record, from either search path.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BusinessHit {
    pub name: String,
    /// Raw category table index (0 = missing); resolve against the
    /// categories sidecar at a higher layer, same as [`crate::business::Business`].
    pub category_idx: u8,
    pub lat: f64,
    pub lon: f64,
    /// H3 res-7 cell for the brute-force path (derived from the record's
    /// lat/lon so callers can re-query the spatial index); `None` for the
    /// name-index path, which has no cell information (its "index" reuses
    /// the field for the letter-bucket key, not a real H3 cell).
    pub cell: Option<u64>,
    /// Higher is a better match: 2 = exact (case-insensitive) name match,
    /// 1 = prefix match, 0 = substring-only match.
    pub score: u8,
}

/// Accent-insensitive, case-insensitive normalization applied to both queries
/// and stored names before matching/bucketing. Steps: NFD-decompose, drop
/// combining diacritical marks (`U+0300..=U+036F` — the block that covers Latin
/// accents; a pragmatic range, not the full Unicode `Mn` category, so
/// non-Latin combining marks pass through unchanged), lowercase, then fold
/// `ß`→`ss` (which `to_lowercase` alone does not do). So `Café`, `CAFE`, and a
/// decomposed `Cafe\u{0301}` all fold to `cafe`, and `Éclair`→`eclair`.
///
/// Does NOT trim — callers trim the query first where they want to.
pub fn fold_name(s: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    let stripped: String = s
        .nfd()
        .filter(|c| !('\u{0300}'..='\u{036F}').contains(c))
        .collect();
    stripped.to_lowercase().replace('ß', "ss")
}

/// [`fold_name`], plus punctuation dropped and runs of space collapsed, so
/// `St. Mary's` and `St Marys` fold alike.
///
/// Deliberately NOT used by either business path. The name-index sidecar's
/// buckets were built with [`fold_name`]'s rule, and a fold that can change a
/// name's first character (`'Round the Mountain` -> `round the mountain`) would
/// look in the wrong bucket of a pack already on the user's phone. This is for
/// searches that read a layer whole and consult no bucket at all --
/// [`crate::search_trails`].
pub(crate) fn fold_loose(s: &str) -> String {
    let folded = fold_name(s);
    let mut out = String::with_capacity(folded.len());
    let mut pending_space = false;
    for c in folded.chars() {
        if c.is_alphanumeric() {
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            out.push(c);
        } else if c != '\'' && c != '\u{2019}' {
            pending_space = true;
        }
        // Apostrophes vanish rather than separate: `Mary's` is one word, and
        // splitting it made `marys` stop matching.
    }
    out
}

/// Score a match between two already-[`fold_name`]-normalized strings.
pub(crate) fn score_match(name_folded: &str, query_folded: &str) -> Option<u8> {
    if query_folded.is_empty() {
        return None;
    }
    if name_folded == query_folded {
        Some(2)
    } else if name_folded.starts_with(query_folded) {
        Some(1)
    } else if name_folded.contains(query_folded) {
        Some(0)
    } else {
        None
    }
}

/// Map a query's first character to the same 0-27 bucket key the reference
/// builder (`build_business_name_index.py::name_to_key`) uses to group
/// business names. Mirrors: `a`-`z` (case-insensitive) -> 0-25, any other
/// non-empty first character -> 26, empty string -> 27.
///
/// Public (not just an internal helper of [`search_business_indexed`]) so a
/// caller that already holds the sidecar's parsed index -- e.g. the wasm
/// boundary, where JS owns file I/O and index lookup, see `wasm/src/lib.rs`
/// -- can compute which block key to fetch without re-deriving this table.
pub fn name_to_key(query: &str) -> u8 {
    // Fold first so accented first letters bucket to their base letter
    // (`Éclair` -> `eclair` -> bucket 4), not the catch-all 26.
    let folded = fold_name(query.trim());
    match folded.chars().next() {
        None => 27,
        // `folded` is already lowercase.
        Some(c) if c.is_ascii_lowercase() => (c as u8) - b'a',
        Some(_) => 26,
    }
}

/// Bucket keys an indexed search should fetch for `query`: the folded-letter
/// bucket, plus bucket 26 as a fallback. Sidecars built by the pre-folding
/// reference builder bucket accented-first-letter names (e.g. `Éclair`) into 26
/// by their raw first char, so a folded query (`eclair` -> bucket 4) would miss
/// them without also scanning 26. Buckets are disjoint, so callers need no
/// dedup. Returns just `[26]` when the query already targets 26.
fn probe_bucket_keys(query: &str) -> Vec<u64> {
    let key = name_to_key(query) as u64;
    if key == 26 {
        alloc::vec![26]
    } else {
        alloc::vec![key, 26]
    }
}

/// One decoded record from a name-index block: the subset of business
/// fields `build_business_name_index.py::encode_name_record` preserves.
struct NameIndexRecord {
    name: String,
    lat: f64,
    lon: f64,
    #[allow(dead_code)] // cross-reference id from the builder; not otherwise consumed yet
    uid: u32,
    category_idx: u8,
}

/// Decode one record from a decompressed name-index block. Format (matches
/// `encode_name_record`): `u32 record_len`, then within the record body:
/// `name` (u16-length string), `lat_micro: i32`, `lon_micro: i32`,
/// `uid: u32`, `category_idx: u8`, `flags: u8`, then optional
/// `phone`/`website`/`brand` (u8-length strings) gated on `flags` bits
/// 0x01/0x02/0x08 respectively (the only flags the builder keeps).
fn decode_name_index_record(
    data: &[u8],
    pos: usize,
) -> Result<(NameIndexRecord, usize), DecodeError> {
    let start = pos;
    let mut p = pos;

    let (name, consumed) = decode_string_u16(data, p)?;
    p += consumed;

    let lat_micro = read_i32(data, p)?;
    p += 4;
    let lon_micro = read_i32(data, p)?;
    p += 4;

    let uid = read_u32(data, p)?;
    p += 4;

    let category_idx = read_u8(data, p)?;
    p += 1;

    let flags = read_u8(data, p)?;
    p += 1;

    if flags & 0x01 != 0 {
        let (_, consumed) = decode_string_u8(data, p)?;
        p += consumed;
    }
    if flags & 0x02 != 0 {
        let (_, consumed) = decode_string_u8(data, p)?;
        p += consumed;
    }
    if flags & 0x08 != 0 {
        let (_, consumed) = decode_string_u8(data, p)?;
        p += consumed;
    }

    Ok((
        NameIndexRecord {
            name,
            lat: lat_micro as f64 / 100_000.0,
            lon: lon_micro as f64 / 100_000.0,
            uid,
            category_idx,
        },
        p - start,
    ))
}

/// Decode a whole decompressed name-index block into its records. Same
/// `{ u32 len, body }*` framing as [`crate::business::decode_business`]; a
/// record that fails to decode is skipped rather than aborting the block.
fn decode_name_index_block(data: &[u8]) -> Result<Vec<NameIndexRecord>, DecodeError> {
    let mut records = Vec::new();
    let mut p = 0usize;

    while p + 4 <= data.len() {
        let record_len = read_u32(data, p)? as usize;
        p += 4;
        if record_len == 0 {
            break;
        }
        // `p + record_len` can wrap on 32-bit targets (wasm) when a corrupt
        // `record_len` is near `u32::MAX`, silently bypassing the overrun
        // guard. Compare against remaining bytes instead — `p <= data.len()`
        // here (loop invariant), so `data.len() - p` cannot underflow.
        if record_len > data.len() - p {
            return Err(DecodeError::RecordOverrun {
                offset: p,
                len: record_len,
                block_len: data.len(),
            });
        }
        if let Ok((rec, _consumed)) = decode_name_index_record(data, p) {
            records.push(rec);
        }
        p += record_len;
    }

    Ok(records)
}

/// Search a `{STATE}.business_name_index.ptiles` sidecar file for `query`.
///
/// Fetches exactly one block (the first-letter bucket for `query`'s first
/// character) via [`PtilesFile::read_block`] and matches case-insensitively
/// within it. See this module's doc comment for the prefix-vs-substring
/// completeness caveat. Returns at most `limit` hits, highest score first
/// (ties broken by name). Empty query returns no hits (there is nothing to
/// rank a match against).
pub fn search_business_indexed<S: PtilesSource>(
    name_index: &PtilesFile<S>,
    query: &str,
    limit: usize,
) -> Result<Vec<BusinessHit>, FileError> {
    search_buckets(name_index, query, limit, probe_bucket_keys(query.trim()))
}

/// The same search over *every* bucket, so a substring is found wherever it
/// sits in a name.
///
/// The index is keyed by the first letter of the name, so the fast path reads
/// two buckets of 28 and `affle` cannot find `Waffle House`. This reads all of
/// them, which is complete and costs about ten times as much: measured against
/// the published Tennessee sidecar, 53-122 ms becomes 1050-1480 ms.
///
/// Meant to run *after* the fast search has already answered, never instead of
/// it: the first results should appear at typing speed, and this fills in what
/// the buckets hid.
pub fn search_business_everywhere<S: PtilesSource>(
    name_index: &PtilesFile<S>,
    query: &str,
    limit: usize,
) -> Result<Vec<BusinessHit>, FileError> {
    search_buckets(name_index, query, limit, (0..=27u64).collect())
}

fn search_buckets<S: PtilesSource>(
    name_index: &PtilesFile<S>,
    query: &str,
    limit: usize,
    buckets: Vec<u64>,
) -> Result<Vec<BusinessHit>, FileError> {
    let query_trimmed = query.trim();
    let query_folded = fold_name(query_trimmed);
    if query_folded.is_empty() || limit == 0 {
        return Ok(Vec::new());
    }

    let mut records = Vec::new();
    for k in buckets {
        if let Some(block) = name_index.read_block(k)? {
            records.extend(decode_name_index_block(&block)?);
        }
    }

    Ok(match_records(records, &query_folded, limit))
}

/// Score, rank, and truncate already-decoded name-index records against
/// `query_lower` (expected pre-lowercased/trimmed by the caller). Shared by
/// [`search_business_indexed`] and [`match_business_name_block`] so the two
/// entry points (I/O-backed vs. pure-block) can't drift in ranking behavior.
fn match_records(
    records: Vec<NameIndexRecord>,
    query_folded: &str,
    limit: usize,
) -> Vec<BusinessHit> {
    let mut hits: Vec<BusinessHit> = records
        .into_iter()
        .filter_map(|rec| {
            // The sidecar is built from the same source as the layer, so it
            // carries the departure board too: before this, typing "DL"
            // returned 38 Delta flights out of 40 hits.
            if crate::flight_nodes::is_flight_node(&rec.name) {
                return None;
            }
            let name_folded = fold_name(&rec.name);
            score_match(&name_folded, query_folded).map(|score| BusinessHit {
                name: rec.name,
                category_idx: rec.category_idx,
                lat: rec.lat,
                lon: rec.lon,
                cell: None,
                score,
            })
        })
        .collect();

    hits.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.name.cmp(&b.name)));
    hits.truncate(limit);
    hits
}

/// Pure (no I/O) name-index search: decode an already-fetched-and-decompressed
/// name-index block and match/rank/truncate against `query`, without touching
/// a [`PtilesFile`]. This is the wasm boundary's shape -- JS there owns
/// fetch + Range requests + zstd decompression (see `wasm/src/lib.rs`'s
/// module doc and [`crate::query::cells_for_bounds`]'s sibling split), so it
/// needs a decode-and-match step that takes bytes in and hits out, not
/// something that reaches back into a source. [`name_to_key`] tells the
/// caller which block (index entry) to fetch in the first place; this
/// function does the rest once that block's bytes are in hand.
pub fn match_business_name_block(
    block: &[u8],
    query: &str,
    limit: usize,
) -> Result<Vec<BusinessHit>, DecodeError> {
    let query_folded = fold_name(query.trim());
    if query_folded.is_empty() || limit == 0 {
        return Ok(Vec::new());
    }
    let records = decode_name_index_block(block)?;
    Ok(match_records(records, &query_folded, limit))
}

/// Brute-force fallback: scan every block in the main `.business.ptiles`
/// file's spatial index, decompressing and matching one block at a time,
/// stopping as soon as `limit` hits have been found (streaming early-exit —
/// does not decompress the whole file up front). Use when no name-index
/// sidecar exists, or when a query needs true "substring anywhere" matching
/// that the letter-bucket index can't guarantee (see module doc).
pub fn search_business_brute_force<S: PtilesSource>(
    business_file: &PtilesFile<S>,
    query: &str,
    limit: usize,
) -> Result<Vec<BusinessHit>, FileError> {
    let query_folded = fold_name(query.trim());
    let mut hits: Vec<BusinessHit> = Vec::new();
    if query_folded.is_empty() || limit == 0 {
        return Ok(hits);
    }

    // Snapshot cells up front: `read_block` re-does a binary search per
    // call, so we index by h3_cell directly rather than holding a borrow
    // across the loop.
    let cells: Vec<u64> = business_file.index().iter().map(|e| e.h3_cell).collect();

    'outer: for cell in cells {
        let Some(block) = business_file.read_block(cell)? else {
            continue;
        };
        let records = crate::business::decode_business(&block)?;
        for biz in records {
            // A departure board is not a set of destinations. Typing "DL"
            // otherwise returned a screenful of Delta flights.
            if crate::flight_nodes::is_flight_node(&biz.name) {
                continue;
            }
            let name_folded = fold_name(&biz.name);
            if let Some(score) = score_match(&name_folded, &query_folded) {
                hits.push(BusinessHit {
                    name: biz.name,
                    category_idx: biz.category_idx,
                    lat: biz.lat,
                    lon: biz.lon,
                    cell: Some(cell),
                    score,
                });
                if hits.len() >= limit {
                    // Streaming early-exit: stop decompressing further
                    // blocks the moment we have enough hits. Not globally
                    // rank-optimal (a later block could contain a
                    // higher-scoring match), but matches the "streaming
                    // with early-exit at limit" requirement for the slow
                    // path; index path is the one expected to be used when
                    // ranked completeness matters.
                    break 'outer;
                }
            }
        }
    }

    hits.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.name.cmp(&b.name)));
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_to_key_matches_reference_builder() {
        assert_eq!(name_to_key("Waffle House"), 22); // 'w' - 'a' = 22
        assert_eq!(name_to_key("waffle house"), 22);
        assert_eq!(name_to_key("WAFFLE"), 22);
        assert_eq!(name_to_key("7-Eleven"), 26);
        assert_eq!(name_to_key(""), 27);
        assert_eq!(name_to_key("   "), 27);
    }

    #[test]
    fn score_match_ranks_exact_over_prefix_over_substring() {
        assert_eq!(score_match("waffle house", "waffle house"), Some(2));
        assert_eq!(score_match("waffle house", "waffle"), Some(1));
        assert_eq!(score_match("waffle house", "house"), Some(0));
        assert_eq!(score_match("waffle house", "pancake"), None);
        assert_eq!(score_match("waffle house", ""), None);
    }

    #[test]
    fn match_business_name_block_matches_search_business_indexed_shape() {
        // Same hand-built single-record block as
        // decode_name_index_record_roundtrip, exercised through the pure
        // wasm-facing entry point instead of the I/O-backed one.
        let mut body = Vec::new();
        let name = b"Waffle House";
        body.extend_from_slice(&(name.len() as u16).to_le_bytes());
        body.extend_from_slice(name);
        body.extend_from_slice(&3595000i32.to_le_bytes());
        body.extend_from_slice(&(-8680000i32).to_le_bytes());
        body.extend_from_slice(&42u32.to_le_bytes());
        body.push(5);
        body.push(0);
        let mut block = Vec::new();
        block.extend_from_slice(&(body.len() as u32).to_le_bytes());
        block.extend_from_slice(&body);

        let hits = match_business_name_block(&block, "waffle", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "Waffle House");
        assert_eq!(hits[0].score, 1); // prefix match
        assert_eq!(hits[0].cell, None);

        assert!(
            match_business_name_block(&block, "", 10)
                .unwrap()
                .is_empty()
        );
        assert!(
            match_business_name_block(&block, "pancake", 10)
                .unwrap()
                .is_empty()
        );
        assert!(
            match_business_name_block(&block, "waffle", 0)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn name_to_key_is_public_and_stable_for_wasm_callers() {
        // Locks in the exact values a JS caller would rely on to pick the
        // right index entry before fetching -- see match_business_name_block.
        assert_eq!(name_to_key("waffle"), 22);
        assert_eq!(name_to_key(""), 27);
    }

    #[test]
    fn decode_name_index_block_empty_is_empty() {
        assert_eq!(decode_name_index_block(&[]).unwrap().len(), 0);
    }

    #[test]
    fn decode_name_index_block_truncated_errors_not_panics() {
        let block = [10u8, 0, 0, 0, 1, 2];
        assert!(decode_name_index_block(&block).is_err());
    }

    /// Round-trips a single hand-built name-index record through the decoder
    /// (mirrors `encode_name_record` in
    /// `~/kino/projects/ptiles/scripts/build_business_name_index.py`), so
    /// the block-level tests below don't depend solely on the real fixture.
    #[test]
    fn decode_name_index_record_roundtrip() {
        let mut body = Vec::new();
        // name (u16_str)
        let name = b"Waffle House";
        body.extend_from_slice(&(name.len() as u16).to_le_bytes());
        body.extend_from_slice(name);
        // lat_micro, lon_micro
        body.extend_from_slice(&3595000i32.to_le_bytes());
        body.extend_from_slice(&(-8680000i32).to_le_bytes());
        // uid
        body.extend_from_slice(&42u32.to_le_bytes());
        // category_idx
        body.push(5);
        // flags = 0 (no optional fields)
        body.push(0);

        let mut block = Vec::new();
        block.extend_from_slice(&(body.len() as u32).to_le_bytes());
        block.extend_from_slice(&body);

        let records = decode_name_index_block(&block).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].name, "Waffle House");
        assert_eq!(records[0].category_idx, 5);
        assert_eq!(records[0].uid, 42);
        assert!((records[0].lat - 35.95).abs() < 1e-9);
        assert!((records[0].lon - (-86.80)).abs() < 1e-9);
    }

    // --- helpers + additional pure-logic coverage --------------------------

    /// Build one name-index record body (no length prefix); no optional fields.
    fn name_record_body(name: &str, lat_micro: i32, lon_micro: i32, uid: u32, cat: u8) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&(name.len() as u16).to_le_bytes());
        b.extend_from_slice(name.as_bytes());
        b.extend_from_slice(&lat_micro.to_le_bytes());
        b.extend_from_slice(&lon_micro.to_le_bytes());
        b.extend_from_slice(&uid.to_le_bytes());
        b.push(cat);
        b.push(0); // flags
        b
    }

    /// Frame a set of record bodies into a name-index block.
    fn name_block(bodies: &[Vec<u8>]) -> Vec<u8> {
        let mut out = Vec::new();
        for body in bodies {
            out.extend_from_slice(&(body.len() as u32).to_le_bytes());
            out.extend_from_slice(body);
        }
        out
    }

    #[test]
    fn score_match_is_case_insensitive() {
        // Callers pass pre-lowercased query; the name is lowercased in
        // match_records. Verify score_match itself on lowercased inputs and
        // the end-to-end lowercasing via match_business_name_block.
        assert_eq!(
            score_match("waffle house", "WAFFLE HOUSE".to_lowercase().as_str()),
            Some(2)
        );

        let block = name_block(&[name_record_body("Waffle House", 0, 0, 1, 0)]);
        for q in ["WAFFLE", "waffle", "WaFfLe"] {
            let hits = match_business_name_block(&block, q, 10).unwrap();
            assert_eq!(hits.len(), 1, "query {q:?} should match case-insensitively");
            assert_eq!(hits[0].score, 1);
        }
    }

    #[test]
    fn unicode_names_lowercase_and_match() {
        let block = name_block(&[name_record_body("Café Roma", 0, 0, 1, 0)]);
        // Uppercase accented query matches via Unicode lowercasing.
        let hits = match_business_name_block(&block, "CAFÉ", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "Café Roma");
        assert_eq!(hits[0].score, 1); // prefix
        // Substring on the accented segment.
        let sub = match_business_name_block(&block, "é ro", 10).unwrap();
        assert_eq!(sub.len(), 1);
        assert_eq!(sub[0].score, 0);
        assert_eq!(name_to_key("Café"), 2); // 'c' - 'a'
    }

    #[test]
    fn accent_folding_makes_search_accent_insensitive() {
        let block = name_block(&[name_record_body("Café Roma", 0, 0, 1, 0)]);
        // Un-accented query now matches an accented stored name (exact fold).
        let hits = match_business_name_block(&block, "cafe roma", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(
            hits[0].score, 2,
            "cafe roma should exactly fold-match Café Roma"
        );
        // And prefix.
        assert_eq!(
            match_business_name_block(&block, "cafe", 10).unwrap()[0].score,
            1
        );
    }

    #[test]
    fn accented_first_letter_buckets_to_base_letter() {
        // The bug this fix targets: `Éclair` must bucket to 'e' (4), so an
        // `eclair` query can find it, not the catch-all 26.
        assert_eq!(name_to_key("Éclair"), 4);
        assert_eq!(name_to_key("éclair"), 4);
        assert_eq!(name_to_key("eclair"), 4);
        // Digits/punctuation still bucket to 26; empty to 27.
        assert_eq!(name_to_key("7-Eleven"), 26);
        assert_eq!(name_to_key(""), 27);
    }

    #[test]
    fn fold_name_folds_accents_case_and_eszett() {
        assert_eq!(fold_name("Café"), "cafe");
        assert_eq!(fold_name("CAFÉ"), "cafe");
        assert_eq!(fold_name("naïve"), "naive");
        assert_eq!(fold_name("José"), "jose");
        // ß / ẞ -> ss (to_lowercase alone leaves ß).
        assert_eq!(fold_name("Straße"), "strasse");
        assert_eq!(fold_name("STRASSE"), "strasse");
        // Precomposed vs decomposed é fold identically (NFD normalizes both).
        assert_eq!(fold_name("caf\u{00e9}"), fold_name("cafe\u{0301}"));
        // Idempotent.
        let once = fold_name("Éclair");
        assert_eq!(fold_name(&once), once);
    }

    #[test]
    fn indexed_search_probes_folded_bucket_plus_legacy_26() {
        // The dual-bucket probe that keeps accented names findable against
        // pre-folding sidecars (where `Éclair` sits in bucket 26 while a
        // folded `eclair` query targets bucket 4).
        assert_eq!(probe_bucket_keys("eclair"), std::vec![4, 26]);
        assert_eq!(probe_bucket_keys("Éclair"), std::vec![4, 26]);
        assert_eq!(probe_bucket_keys("waffle"), std::vec![22, 26]);
        // A query already in bucket 26 is fetched once, not twice.
        assert_eq!(probe_bucket_keys("7-Eleven"), std::vec![26]);
    }

    #[test]
    fn empty_and_whitespace_query_yield_no_hits() {
        let block = name_block(&[name_record_body("Anything", 0, 0, 1, 0)]);
        for q in ["", "   ", "\t\n"] {
            assert!(
                match_business_name_block(&block, q, 10).unwrap().is_empty(),
                "query {q:?} must produce no hits"
            );
        }
        // score_match guards empty directly too.
        assert_eq!(score_match("anything", ""), None);
    }

    #[test]
    fn query_is_trimmed_before_matching() {
        let block = name_block(&[name_record_body("Waffle House", 0, 0, 1, 0)]);
        let hits = match_business_name_block(&block, "  Waffle House  ", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].score, 2, "trimmed query should be an exact match");
    }

    #[test]
    fn no_match_returns_empty() {
        let block = name_block(&[name_record_body("Waffle House", 0, 0, 1, 0)]);
        assert!(
            match_business_name_block(&block, "pancake", 10)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn ranking_orders_exact_then_prefix_then_substring() {
        let block = name_block(&[
            name_record_body("Sunny Waffle", 0, 0, 1, 0), // substring -> 0
            name_record_body("Waffle House", 0, 0, 2, 0), // prefix -> 1
            name_record_body("Waffle", 0, 0, 3, 0),       // exact -> 2
        ]);
        let hits = match_business_name_block(&block, "waffle", 10).unwrap();
        assert_eq!(hits.len(), 3);
        assert_eq!(hits[0].name, "Waffle");
        assert_eq!(hits[0].score, 2);
        assert_eq!(hits[1].name, "Waffle House");
        assert_eq!(hits[1].score, 1);
        assert_eq!(hits[2].name, "Sunny Waffle");
        assert_eq!(hits[2].score, 0);
    }

    #[test]
    fn equal_score_ties_break_by_name_ascending() {
        let block = name_block(&[
            name_record_body("Walmart", 0, 0, 1, 0),      // prefix "wa"
            name_record_body("Waffle House", 0, 0, 2, 0), // prefix "wa"
            name_record_body("Wawa", 0, 0, 3, 0),         // prefix "wa"
        ]);
        let hits = match_business_name_block(&block, "wa", 10).unwrap();
        let names: Vec<&str> = hits.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(names, ["Waffle House", "Walmart", "Wawa"]);
        assert!(hits.iter().all(|h| h.score == 1));
    }

    #[test]
    fn limit_truncates_after_ranking() {
        let block = name_block(&[
            name_record_body("Sunny Waffle", 0, 0, 1, 0), // substring
            name_record_body("Waffle", 0, 0, 2, 0),       // exact -> should survive limit=1
        ]);
        let hits = match_business_name_block(&block, "waffle", 1).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "Waffle"); // highest score kept
    }

    #[test]
    fn overflowing_record_len_errors_not_panics() {
        let block = [0xFFu8, 0xFF, 0xFF, 0xFF, 1, 2, 3];
        assert!(matches!(
            decode_name_index_block(&block),
            Err(DecodeError::RecordOverrun { .. })
        ));
    }

    // --- Integration tests against real fixtures ---------------------------
    //
    // Skip (pass trivially) when the fixture isn't present, matching the
    // convention in file.rs's real-file tests.

    #[cfg(feature = "std")]
    fn open_real(path: &str) -> Option<PtilesFile<crate::source::FileSource>> {
        let p = std::path::Path::new(path);
        if !p.exists() {
            eprintln!("skipping: fixture not present at {p:?}");
            return None;
        }
        let src = crate::source::FileSource::open(p).expect("open fixture");
        Some(PtilesFile::open(src).expect("parse header/dict/index"))
    }

    #[cfg(feature = "std")]
    #[test]
    fn golden_block_search_ranking_on_real_data() {
        // The main-business golden block is always present. Decode it and run
        // the shared score/rank logic (the same code path both search entry
        // points use) over real records, confirming case-insensitive matching
        // and exact > prefix > substring ordering hold on real data.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("test-fixtures")
            .join("golden")
            .join("business.block.bin");
        let block = std::fs::read(&path).expect("read golden business block");
        let businesses = crate::business::decode_business(&block).expect("decode golden block");
        assert!(!businesses.is_empty());

        // Case-insensitive: mixed-case query finds "Drug Store Coffee".
        let query = "drug store coffee";
        let mut scored: Vec<(u8, &str)> = businesses
            .iter()
            .filter_map(|b| {
                score_match(&b.name.to_lowercase(), query).map(|s| (s, b.name.as_str()))
            })
            .collect();
        assert!(
            scored
                .iter()
                .any(|(s, n)| *s == 2 && *n == "Drug Store Coffee"),
            "exact case-insensitive match expected on real data"
        );

        // Ranking invariant: same sort as match_records; scores must be
        // non-increasing once sorted highest-first.
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
        assert!(scored.windows(2).all(|w| w[0].0 >= w[1].0));

        // A prefix query returns matches all starting with it (score >= 1).
        let prefix = "coffee";
        let prefix_hits: Vec<&str> = businesses
            .iter()
            .filter(|b| score_match(&b.name.to_lowercase(), prefix) == Some(1))
            .map(|b| b.name.as_str())
            .collect();
        assert!(
            prefix_hits
                .iter()
                .all(|n| n.to_lowercase().starts_with(prefix)),
            "prefix-scored hits must all start with the query"
        );
    }

    #[cfg(feature = "std")]
    #[test]
    fn indexed_search_finds_a_known_chain_in_tn() {
        let Some(name_index) =
            open_real("/home/aoi/kino/data/ptiles/TN.business_name_index.ptiles")
        else {
            return;
        };
        assert_eq!(name_index.header().magic_str(), "PTILESX");

        let start = std::time::Instant::now();
        let hits = search_business_indexed(&name_index, "Waffle House", 20).unwrap();
        let elapsed = start.elapsed();
        eprintln!(
            "indexed search for 'Waffle House' took {elapsed:?}, {} hits",
            hits.len()
        );

        assert!(!hits.is_empty(), "expected at least one Waffle House in TN");
        for hit in &hits {
            assert!(hit.name.to_lowercase().contains("waffle"));
            // Tennessee's bounding box, roughly.
            assert!(
                (34.9..=36.7).contains(&hit.lat),
                "lat {} out of TN range",
                hit.lat
            );
            assert!(
                (-90.4..=-81.6).contains(&hit.lon),
                "lon {} out of TN range",
                hit.lon
            );
        }
        assert!(
            elapsed.as_secs_f64() < 1.0,
            "indexed search should be well under 1s, took {elapsed:?}"
        );
    }

    #[cfg(feature = "std")]
    #[test]
    fn indexed_search_respects_limit() {
        let Some(name_index) =
            open_real("/home/aoi/kino/data/ptiles/TN.business_name_index.ptiles")
        else {
            return;
        };
        let hits = search_business_indexed(&name_index, "s", 5).unwrap();
        assert!(hits.len() <= 5);
    }

    #[cfg(feature = "std")]
    #[test]
    fn indexed_search_empty_query_returns_no_hits() {
        let Some(name_index) =
            open_real("/home/aoi/kino/data/ptiles/TN.business_name_index.ptiles")
        else {
            return;
        };
        assert!(
            search_business_indexed(&name_index, "", 10)
                .unwrap()
                .is_empty()
        );
    }

    #[cfg(feature = "std")]
    #[test]
    fn indexed_search_no_match_returns_no_hits() {
        let Some(name_index) =
            open_real("/home/aoi/kino/data/ptiles/TN.business_name_index.ptiles")
        else {
            return;
        };
        let hits =
            search_business_indexed(&name_index, "zzzzzznonexistentbusinessxyz", 10).unwrap();
        assert!(hits.is_empty());
    }

    #[cfg(feature = "std")]
    #[test]
    fn brute_force_search_finds_same_chain_and_respects_limit() {
        let Some(business_file) = open_real("/home/aoi/kino/data/ptiles/TN.business.ptiles") else {
            return;
        };

        let hits = search_business_brute_force(&business_file, "Waffle House", 3).unwrap();
        assert!(!hits.is_empty(), "expected at least one Waffle House in TN");
        assert!(hits.len() <= 3);
        for hit in &hits {
            assert!(hit.name.to_lowercase().contains("waffle"));
            assert!(hit.cell.is_some());
        }

        assert!(
            search_business_brute_force(&business_file, "", 10)
                .unwrap()
                .is_empty()
        );
        assert!(
            search_business_brute_force(&business_file, "zzzzzznonexistentxyz", 10)
                .unwrap()
                .is_empty()
        );
    }
}
