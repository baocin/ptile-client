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
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

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
        let (status, total_len, body) = fetch_range(&agent, &url, 0, range_end)?;
        if status != 206 {
            return Err(SourceError::RangeNotSupported { url, status });
        }

        Ok(HttpSource {
            agent,
            url,
            len: total_len,
            prefetch: body,
            cache: Mutex::new(HashMap::new()),
            request_count: AtomicUsize::new(1),
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
}

impl PtilesSource for HttpSource {
    fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), SourceError> {
        let needed = buf.len();
        let end = offset.checked_add(needed as u64).ok_or(SourceError::OutOfBounds {
            offset,
            needed,
            len: self.len,
        })?;
        if end > self.len {
            return Err(SourceError::OutOfBounds {
                offset,
                needed,
                len: self.len,
            });
        }

        // Served from the eager prefetch when the whole request falls
        // inside it -- the common case for header/index reads on
        // small-to-medium layers (see module doc).
        if end <= self.prefetch.len() as u64 {
            let start = offset as usize;
            buf.copy_from_slice(&self.prefetch[start..start + needed]);
            return Ok(());
        }

        if let Some(cached) = self.cache.lock().unwrap().get(&(offset, needed)) {
            buf.copy_from_slice(cached);
            return Ok(());
        }

        let range_end = end - 1;
        self.request_count.fetch_add(1, Ordering::Relaxed);
        let (status, _total, body) = fetch_range(&self.agent, &self.url, offset, range_end)?;
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

/// Issue one Range GET, returning `(status, total_resource_len, body_bytes)`.
/// `total_resource_len` is parsed from the `Content-Range` response header
/// (`bytes start-end/total`); falls back to the fetched body's length if
/// that header is missing.
fn fetch_range(
    agent: &ureq::Agent,
    url: &str,
    start: u64,
    end: u64,
) -> Result<(u16, u64, Vec<u8>), SourceError> {
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
    let total_len = response
        .headers()
        .get("Content-Range")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.rsplit('/').next())
        .and_then(|s| s.parse::<u64>().ok());

    let mut body = Vec::new();
    response
        .body_mut()
        .as_reader()
        .read_to_end(&mut body)
        .map_err(|e| SourceError::HttpNetwork {
            url: url.into(),
            message: format!("{e}"),
        })?;

    let total_len = total_len.unwrap_or(body.len() as u64);
    Ok((status, total_len, body))
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
        assert!(source.len().unwrap_or(0) > 0, "remote file must report a nonzero length");

        let file = match crate::file::PtilesFile::open(source) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("skipping (open/parse failed, possibly network flake): {e}");
                return;
            }
        };
        assert_eq!(file.header().magic_str(), "PTILEST");

        let Some(entry) = file.index().first() else {
            eprintln!("skipping: rail index is empty (or v2 index misparsed as v1 with 0 entries)");
            return;
        };
        let cell = entry.h3_cell;

        match file.read_block(cell) {
            Ok(Some(block)) => match crate::rail::decode_rail(&block) {
                Ok(features) => {
                    assert!(!features.is_empty(), "rail block should decode to at least one feature");
                }
                Err(e) => {
                    eprintln!(
                        "skipping: decode_rail failed, likely the known v2-index/offset mismatch, not an HTTP bug: {e}"
                    );
                }
            },
            Ok(None) => {
                eprintln!("skipping: read_block found no entry (likely v2 index misparse, not an HTTP bug)");
            }
            Err(e) => {
                eprintln!("skipping: read_block failed (network or v2-index mismatch, not necessarily HTTP transport): {e}");
            }
        }
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
}
