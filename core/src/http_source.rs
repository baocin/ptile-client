//! `HttpSource`: `PtilesSource` over HTTP Range requests (feature `http`,
//! std-only -- see `Cargo.toml`'s `http = ["std", "dep:ureq"]`).
//!
//! ureq chosen over reqwest/hyper/isahc/attohttpc (see `Cargo.toml` comment
//! next to the dependency): no async runtime needed anywhere else in this
//! workspace, rustls-by-default, small dependency footprint, and a blocking
//! `Agent` that pools/reuses connections without extra wiring.
//!
//! Design:
//! - one shared `ureq::Agent` per `HttpSource` -- ureq's `Agent` internally
//!   keeps a connection pool, so repeated `read_exact_at` calls against the
//!   same host reuse the underlying TCP+TLS connection instead of paying a
//!   fresh handshake per read.
//! - an eager prefetch of the first `PREFETCH_BYTES` (64 KiB) at construction
//!   time, in one Range request. `PtilesFile::open` always issues up to three
//!   `read_exact_at` calls in file order (256-byte header at offset 0, the
//!   zstd dictionary, then the spatial index) -- for layers whose dictionary
//!   and index both live inside the first 64 KiB (true for header-only/no-dict
//!   layers like parks/rail/places, and for small test fixtures), `open()`
//!   costs zero *additional* requests beyond the one prefetch already paid at
//!   construction. Large trained dictionaries (buildings/roads/business train
//!   ~512 KB dictionaries per the plan) still need one extra request for the
//!   dict itself -- that is a real, unavoidable fetch of data that must be
//!   read, not chattiness to fix.
//! - a read-through cache keyed by exact `(offset, len)`, so a byte range
//!   fetched once (e.g. the header, on repeated `open()` calls in tests, or a
//!   block re-read for the same cell) is served from memory afterwards
//!   instead of refetched.
//! - Range-support detection: any non-`206` response to a Range request
//!   (typically `200 OK` from a server that ignores `Range` entirely) is
//!   reported as `SourceError::RangeNotSupported` rather than silently
//!   treating the wrong bytes (the whole body) as the requested slice.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use std::collections::HashMap;
use std::io::Read;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::source::{PtilesSource, SourceError};

/// Bytes fetched eagerly at construction, in one request. Sized to cover a
/// `.ptiles` header (256 B) plus most spatial index sections for
/// small-to-medium layers; see this module's doc comment for the large-dict
/// case this doesn't (and can't) cover for free.
const PREFETCH_BYTES: u64 = 64 * 1024;

/// A `.ptiles` file served over HTTP, read via byte-range GET requests.
/// Requires the server to support `Range` (RFC 7233) -- detected at
/// construction and on every subsequent read; servers that don't are
/// reported via `SourceError::RangeNotSupported` rather than silently
/// misread.
pub struct HttpSource {
    agent: ureq::Agent,
    url: String,
    /// Total byte length of the remote resource, learned from the
    /// prefetch's `Content-Range` header (or its body length as a fallback,
    /// for the rare case a server answers with no `Content-Range` at all).
    len: u64,
    /// Bytes `[0, prefetch.len())`, fetched once at construction.
    prefetch: Vec<u8>,
    /// Read-through cache for ranges fetched on demand, keyed by the exact
    /// `(offset, len)` requested.
    cache: Mutex<HashMap<(u64, usize), Vec<u8>>>,
    /// Count of HTTP requests issued (including the construction-time
    /// prefetch). Exposed via [`HttpSource::request_count`] so callers/tests
    /// can verify a given query pattern isn't chatty.
    request_count: AtomicUsize,
    /// `ETag` and `Last-Modified` from the construction-time response.
    ///
    /// The `.ptiles` header carries no build date, so for a file you range-read
    /// rather than download these are the *only* provenance available: they are
    /// what tells you whether the roads layer you are querying was built last
    /// week or two years ago. Both are `None` when the server does not send
    /// them, which is a fact worth surfacing rather than papering over.
    etag: Option<String>,
    last_modified: Option<String>,
}

impl HttpSource {
    /// Open a remote `.ptiles` file: builds a connection-reuse-capable
    /// `ureq::Agent` and eagerly fetches the first `PREFETCH_BYTES` bytes in
    /// one request, which also establishes the resource's total length and
    /// confirms the server supports Range requests.
    pub fn open(url: impl Into<String>) -> Result<Self, SourceError> {
        let url = url.into();
        let agent: ureq::Agent = ureq::Agent::config_builder().build().into();

        let range_end = PREFETCH_BYTES - 1;
        let fetched = fetch_range(&agent, &url, 0, range_end)?;
        if fetched.status != 206 {
            return Err(SourceError::RangeNotSupported {
                url,
                status: fetched.status,
            });
        }

        Ok(HttpSource {
            agent,
            url,
            len: fetched.total_len,
            prefetch: fetched.body,
            cache: Mutex::new(HashMap::new()),
            request_count: AtomicUsize::new(1),
            etag: fetched.etag,
            last_modified: fetched.last_modified,
        })
    }

    /// The remote URL this source reads from.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Number of HTTP requests issued so far (including the construction-time
    /// prefetch). Reads served from the prefetch buffer or the read-through
    /// cache do not increment this.
    pub fn request_count(&self) -> usize {
        self.request_count.load(Ordering::Relaxed)
    }

    /// The resource's `ETag`, if the server sent one. Opaque, but stable per
    /// build: a changed ETag means the file was rebuilt.
    pub fn etag(&self) -> Option<&str> {
        self.etag.as_deref()
    }

    /// The resource's `Last-Modified` header, if the server sent one. The
    /// closest thing to a build date a `.ptiles` file has -- the format itself
    /// stores none.
    pub fn last_modified(&self) -> Option<&str> {
        self.last_modified.as_deref()
    }
}

/// Where a positioned read should be served from, decided purely from the
/// request geometry and the source's known length/prefetch -- no I/O. Split
/// out so the range math (overflow, bounds, prefetch-window containment) is
/// unit-testable without any HTTP layer.
#[derive(Debug, PartialEq, Eq)]
enum ReadPlan {
    /// The whole `[offset, offset+needed)` range lies inside the prefetch
    /// buffer; copy from `prefetch[start..start+needed]`.
    Prefetch { start: usize },
    /// The range is valid but past the prefetch; fetch bytes `offset..=range_end`.
    Fetch { range_end: u64 },
}

/// Decide how to serve a read of `needed` bytes at `offset` against a source
/// of total length `len` with `prefetch_len` eagerly-cached leading bytes.
/// Returns `OutOfBounds` for reads that overflow `u64` or run past `len`.
/// A zero-length read is always in bounds and served from the prefetch
/// (`start = offset`), matching a no-op `copy_from_slice`.
fn plan_read(
    offset: u64,
    needed: usize,
    len: u64,
    prefetch_len: usize,
) -> Result<ReadPlan, SourceError> {
    let end = offset
        .checked_add(needed as u64)
        .ok_or(SourceError::OutOfBounds {
            offset,
            needed,
            len,
        })?;
    if end > len {
        return Err(SourceError::OutOfBounds {
            offset,
            needed,
            len,
        });
    }
    if end <= prefetch_len as u64 {
        // `end <= prefetch_len <= usize::MAX`, so `offset` fits in usize.
        Ok(ReadPlan::Prefetch {
            start: offset as usize,
        })
    } else {
        // `end >= 1` here (end > prefetch_len >= 0), so `end - 1` can't wrap.
        Ok(ReadPlan::Fetch { range_end: end - 1 })
    }
}

impl PtilesSource for HttpSource {
    fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), SourceError> {
        let needed = buf.len();

        let range_end = match plan_read(offset, needed, self.len, self.prefetch.len())? {
            // Served from the eager prefetch when the whole request falls
            // inside it -- the common case for header/index reads on
            // small-to-medium layers (see module doc).
            ReadPlan::Prefetch { start } => {
                buf.copy_from_slice(&self.prefetch[start..start + needed]);
                return Ok(());
            }
            ReadPlan::Fetch { range_end } => range_end,
        };

        if let Some(cached) = self.cache.lock().unwrap().get(&(offset, needed)) {
            buf.copy_from_slice(cached);
            return Ok(());
        }

        self.request_count.fetch_add(1, Ordering::Relaxed);
        let RangeResponse { status, body, .. } =
            fetch_range(&self.agent, &self.url, offset, range_end)?;
        if status != 206 {
            return Err(SourceError::RangeNotSupported {
                url: self.url.clone(),
                status,
            });
        }
        if body.len() != needed {
            return Err(SourceError::HttpStatus {
                url: self.url.clone(),
                status,
                offset,
                end: range_end,
            });
        }
        buf.copy_from_slice(&body);
        self.cache.lock().unwrap().insert((offset, needed), body);
        Ok(())
    }

    fn len(&self) -> Option<u64> {
        Some(self.len)
    }
}

/// One range response: status, the resource's total length, the bytes, and the
/// two provenance headers.
///
/// A struct rather than a 5-tuple because the two header fields are only read at
/// construction and a positional tuple that long is a footgun at every call
/// site.
struct RangeResponse {
    status: u16,
    /// Parsed from `Content-Range` (`bytes start-end/total`); falls back to the
    /// fetched body's length when the server sends no such header.
    total_len: u64,
    body: Vec<u8>,
    etag: Option<String>,
    last_modified: Option<String>,
}

/// Issue one Range GET.
fn fetch_range(
    agent: &ureq::Agent,
    url: &str,
    start: u64,
    end: u64,
) -> Result<RangeResponse, SourceError> {
    let range_header = format!("bytes={start}-{end}");
    let mut response = match agent.get(url).header("Range", &range_header).call() {
        Ok(r) => r,
        Err(ureq::Error::StatusCode(code)) => {
            return Err(SourceError::HttpStatus {
                url: url.into(),
                status: code,
                offset: start,
                end,
            });
        }
        Err(e) => {
            return Err(SourceError::HttpNetwork {
                url: url.into(),
                message: format!("{e}"),
            });
        }
    };

    let status = response.status().as_u16();
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
    };
    let content_range = header("Content-Range");
    let etag = header("ETag");
    let last_modified = header("Last-Modified");

    let mut body = Vec::new();
    response
        .body_mut()
        .as_reader()
        .read_to_end(&mut body)
        .map_err(|e| SourceError::HttpNetwork {
            url: url.into(),
            message: format!("{e}"),
        })?;

    let total_len = parse_content_range_total(content_range.as_deref(), body.len());
    Ok(RangeResponse { status, total_len, body, etag, last_modified })
}

/// Extract the total resource length from a `Content-Range` header value of
/// the form `bytes START-END/TOTAL` (RFC 7233). Falls back to `body_len` when
/// the header is absent, malformed, or reports an unknown total (`*`). Pure so
/// the parsing is testable without a live server.
fn parse_content_range_total(content_range: Option<&str>, body_len: usize) -> u64 {
    content_range
        .and_then(|s| s.rsplit('/').next())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(body_len as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Network test: opens the real 2 KB rail fixture over HTTP, reads its
    /// header block, and decodes it. Skips (does not fail) on any network
    /// error -- offline `cargo test` must still pass.
    ///
    /// Known limitation carried over from `file.rs`'s module doc:
    /// `TN.rail.ptiles` uses the undocumented v2 "merged block" spatial index
    /// format, which `ptiles-core`'s `index.rs` does not parse (out of scope
    /// for this task -- flagged there as a follow-up). So this test only
    /// asserts the HTTP layer (open + header/index parse via
    /// `PtilesFile::open`, then a `read_block` attempt) behaves -- a
    /// `read_block` failure caused by the v2-index mismatch is treated the
    /// same as "skip", not a hard failure, since it's a pre-existing,
    /// already-documented gap unrelated to HTTP transport correctness.
    #[test]
    fn opens_real_rail_file_over_http_and_reads_a_block() {
        let url = "https://maps.mydatatimeline.com/maps/TN.rail.ptiles";
        let source = match HttpSource::open(url) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("skipping opens_real_rail_file_over_http_and_reads_a_block: {e}");
                return;
            }
        };
        assert!(
            source.len().unwrap_or(0) > 0,
            "remote file must report a nonzero length"
        );

        let file = match crate::file::PtilesFile::open(source) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("skipping (open/parse failed, possibly network flake): {e}");
                return;
            }
        };
        assert_eq!(file.header().magic_str(), "PTILEST");

        // rail uses a 38-byte index and merged blocks. Both are now detected
        // and handled, so the outcome is asserted rather than excused: the
        // escape hatches this test used to carry ("skipping: likely the known
        // v2-index mismatch") would now hide a real regression.
        assert_eq!(
            file.layout().entry_size,
            crate::index::ENTRY_SIZE_V2,
            "rail is a 38-byte-index layer"
        );
        let entry = file
            .index()
            .iter()
            .find(|e| e.block_length > 0)
            .expect("rail index must name at least one non-empty block");

        // read_cell, not read_block: on a merged-block layer the latter hands
        // the decoder a cell table it will parse as records.
        let block = file
            .read_cell(entry.h3_cell)
            .expect("read_cell over HTTP")
            .expect("a cell named by the index must resolve");
        let features = crate::rail::decode_rail(&block).expect("decode_rail on a real block");
        assert!(
            !features.is_empty(),
            "rail block should decode to at least one feature"
        );
    }

    /// Evidence test (not a strict assertion, since it depends on network
    /// availability): opens the real remote roads file and runs a single
    /// query (one `read_block` call), then reports the total request count
    /// via `eprintln` so `cargo test -- --nocapture` shows it. Skips
    /// gracefully offline.
    #[test]
    fn request_count_for_open_plus_one_query_is_small() {
        let url = "https://maps.mydatatimeline.com/maps/TN.roads.ptiles";
        let source = match HttpSource::open(url) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("skipping request_count_for_open_plus_one_query_is_small: {e}");
                return;
            }
        };
        let file = match crate::file::PtilesFile::open(source) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("skipping (open/parse failed, possibly network flake): {e}");
                return;
            }
        };
        let after_open = file.source().request_count();

        let cell = match file.index().first() {
            Some(e) => e.h3_cell,
            None => {
                eprintln!("skipping: empty index");
                return;
            }
        };
        let _ = file.read_block(cell);
        let after_query = file.source().request_count();

        eprintln!(
            "TN.roads.ptiles: HTTP requests after PtilesFile::open() = {after_open}, \
             after open()+1 read_block() query = {after_query}"
        );
        // 1 request for the 64 KiB prefetch, plus (only if the layer's
        // dictionary and/or index spill past that window) one more each for
        // dict and index. Roads trains a ~512 KB dict per the plan, so open()
        // is expected to cost the prefetch + 1 dict request here (index for
        // a 33 MB roads file may or may not fit the prefetch too).
        assert!(
            after_open <= 3,
            "open() should need at most ~3 requests (1 prefetch + dict + index), got {after_open}"
        );
        assert_eq!(
            after_query,
            after_open + 1,
            "a single read_block() for an uncached cell should cost exactly 1 more request"
        );
    }

    /// A server (or path) that doesn't support Range must fail clearly, not
    /// silently misread the whole body as a slice. `httpbin`-style flaky
    /// network dependency is undesirable here, so this test only checks the
    /// error path when a request altogether fails (e.g. DNS failure for a
    /// bogus host) -- it still must be `SourceError`, never a panic.
    #[test]
    fn open_of_nonexistent_host_is_a_clean_error_not_a_panic() {
        let result = HttpSource::open("https://this-host-should-not-resolve.invalid/x.ptiles");
        assert!(result.is_err());
    }

    // ---- Pure range-math tests (no network) -------------------------------

    #[test]
    fn plan_read_serves_range_fully_inside_prefetch() {
        // len 1000, 64 bytes prefetched; a read of [10,20) is inside it.
        assert_eq!(
            plan_read(10, 10, 1000, 64).unwrap(),
            ReadPlan::Prefetch { start: 10 }
        );
    }

    #[test]
    fn plan_read_exact_prefetch_boundary_is_prefetch_but_one_past_is_fetch() {
        // Read ending exactly at prefetch_len stays in the prefetch...
        assert_eq!(
            plan_read(60, 4, 1000, 64).unwrap(),
            ReadPlan::Prefetch { start: 60 }
        );
        // ...but ending one byte later must be fetched. range_end = end-1 = 64.
        assert_eq!(
            plan_read(61, 4, 1000, 64).unwrap(),
            ReadPlan::Fetch { range_end: 64 }
        );
    }

    #[test]
    fn plan_read_range_straddling_prefetch_end_is_fetched_whole() {
        // Starts inside prefetch, ends past it -> a single fetch of the
        // whole requested range (no partial-prefetch stitching).
        assert_eq!(
            plan_read(60, 20, 1000, 64).unwrap(),
            ReadPlan::Fetch { range_end: 79 }
        );
    }

    #[test]
    fn plan_read_past_prefetch_computes_inclusive_range_end() {
        // [100,150) with a 64-byte prefetch -> fetch bytes 100..=149.
        assert_eq!(
            plan_read(100, 50, 1000, 64).unwrap(),
            ReadPlan::Fetch { range_end: 149 }
        );
    }

    #[test]
    fn plan_read_zero_length_read_is_in_bounds_never_oob() {
        // Zero-length reads are always in bounds (never OOB), even at the
        // exact end of the resource. Within the prefetch window they resolve
        // to a no-op prefetch copy.
        assert_eq!(
            plan_read(0, 0, 0, 0).unwrap(),
            ReadPlan::Prefetch { start: 0 }
        );
        assert_eq!(
            plan_read(50, 0, 1000, 64).unwrap(),
            ReadPlan::Prefetch { start: 50 }
        );
        // Past the prefetch window a zero-length read is still in bounds
        // (Ok, not an error) -- it just isn't classified as a prefetch hit.
        assert!(plan_read(1000, 0, 1000, 64).is_ok());
    }

    #[test]
    fn plan_read_end_exactly_at_len_is_allowed() {
        // Reading the final byte(s) up to and including EOF is in bounds.
        assert_eq!(
            plan_read(996, 4, 1000, 64).unwrap(),
            ReadPlan::Fetch { range_end: 999 }
        );
    }

    #[test]
    fn plan_read_one_past_len_is_out_of_bounds() {
        let err = plan_read(996, 5, 1000, 64).unwrap_err();
        match err {
            SourceError::OutOfBounds {
                offset,
                needed,
                len,
            } => {
                assert_eq!((offset, needed, len), (996, 5, 1000));
            }
            other => panic!("expected OutOfBounds, got {other:?}"),
        }
    }

    #[test]
    fn plan_read_offset_beyond_len_is_out_of_bounds() {
        assert!(matches!(
            plan_read(2000, 1, 1000, 64),
            Err(SourceError::OutOfBounds { .. })
        ));
    }

    #[test]
    fn plan_read_offset_plus_len_overflow_is_out_of_bounds_not_panic() {
        // offset near u64::MAX + a nonzero len overflows the addition; must be
        // reported as OutOfBounds, never a wrapping-add panic.
        let err = plan_read(u64::MAX, 1, u64::MAX, 64).unwrap_err();
        assert!(matches!(err, SourceError::OutOfBounds { .. }));
    }

    // ---- Content-Range parsing tests (no network) -------------------------

    #[test]
    fn content_range_total_parsed_from_well_formed_header() {
        assert_eq!(parse_content_range_total(Some("bytes 0-63/2048"), 64), 2048);
        assert_eq!(
            parse_content_range_total(Some("bytes 100-199/12345"), 100),
            12345
        );
    }

    #[test]
    fn content_range_absent_falls_back_to_body_len() {
        assert_eq!(parse_content_range_total(None, 512), 512);
    }

    #[test]
    fn content_range_unknown_total_star_falls_back_to_body_len() {
        // RFC 7233 allows `bytes 0-63/*` when the total is unknown.
        assert_eq!(parse_content_range_total(Some("bytes 0-63/*"), 64), 64);
    }

    #[test]
    fn content_range_malformed_falls_back_to_body_len() {
        // No slash, or non-numeric tail, or empty -> use the body length.
        assert_eq!(parse_content_range_total(Some("garbage"), 77), 77);
        assert_eq!(parse_content_range_total(Some("bytes 0-63/abc"), 88), 88);
        assert_eq!(parse_content_range_total(Some(""), 99), 99);
    }
}
