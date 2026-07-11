//! PtilesSource trait: positioned reads (read_exact_at) without std::io::Read.
//! Concrete sources: MemorySource (no_std-friendly, tests/wasm/MCU) and
//! FileSource (std-gated, pread on unix).

use alloc::vec::Vec;

/// Error type for `PtilesSource` reads. `no_std`-compatible — does not wrap
/// `std::io::Error` so the trait can exist without `std`.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum SourceError {
    #[error("read of {needed} bytes at offset {offset} exceeds source length {len}")]
    OutOfBounds {
        offset: u64,
        needed: usize,
        len: u64,
    },
    #[error("I/O error reading {needed} bytes at offset {offset}: {message}")]
    Io {
        offset: u64,
        needed: usize,
        message: alloc::string::String,
    },
    /// Server answered a Range request with `200 OK` instead of `206 Partial
    /// Content` -- it does not support Range requests, so `HttpSource`'s
    /// positioned reads cannot work against it (it would otherwise silently
    /// fetch the whole file body for every "range").
    #[cfg(feature = "http")]
    #[error(
        "{url} does not support HTTP Range requests (got status {status}, expected 206 Partial Content)"
    )]
    RangeNotSupported {
        url: alloc::string::String,
        status: u16,
    },
    /// Any other unsuccessful HTTP status for a range fetch.
    #[cfg(feature = "http")]
    #[error("HTTP {status} fetching {url} (range {offset}..{end})")]
    HttpStatus {
        url: alloc::string::String,
        status: u16,
        offset: u64,
        end: u64,
    },
    /// Transport-level failure: DNS, TLS, connection refused/reset, timeout, etc.
    #[cfg(feature = "http")]
    #[error("network error fetching {url}: {message}")]
    HttpNetwork {
        url: alloc::string::String,
        message: alloc::string::String,
    },
}

/// Positioned-read abstraction over a `.ptiles` file's bytes. Implementations
/// must not require `std::io::Read`/`Seek` so this works in `no_std` contexts
/// (MCU targets reading from flash/SPI, wasm reading from an in-memory buffer).
#[allow(clippy::len_without_is_empty)] // `len()` is the source's total byte size, not
// container length in the collection sense — an `is_empty()` companion doesn't apply.
pub trait PtilesSource {
    /// Read exactly `buf.len()` bytes starting at `offset` into `buf`.
    fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), SourceError>;

    /// Total length of the underlying data, if known. Used for bounds-checking
    /// before attempting a read. Default implementation reports unknown (`None`),
    /// which skips the pre-check and relies on the read itself to fail.
    fn len(&self) -> Option<u64> {
        None
    }
}

/// An owned in-memory `.ptiles` file. Works everywhere `alloc` works —
/// no_std, wasm, tests, MCU-with-PSRAM.
#[derive(Clone, Debug)]
pub struct MemorySource(pub Vec<u8>);

impl MemorySource {
    pub fn new(data: Vec<u8>) -> Self {
        MemorySource(data)
    }
}

impl PtilesSource for MemorySource {
    fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), SourceError> {
        let start = usize::try_from(offset).map_err(|_| SourceError::OutOfBounds {
            offset,
            needed: buf.len(),
            len: self.0.len() as u64,
        })?;
        let end = start
            .checked_add(buf.len())
            .filter(|&end| end <= self.0.len())
            .ok_or(SourceError::OutOfBounds {
                offset,
                needed: buf.len(),
                len: self.0.len() as u64,
            })?;
        buf.copy_from_slice(&self.0[start..end]);
        Ok(())
    }

    fn len(&self) -> Option<u64> {
        Some(self.0.len() as u64)
    }
}

/// A `.ptiles` file backed by an open `std::fs::File`, read via positioned
/// reads (`pread` on unix) so no seek state is shared across concurrent reads.
#[cfg(feature = "std")]
pub struct FileSource(std::fs::File);

#[cfg(feature = "std")]
impl FileSource {
    pub fn open(path: impl AsRef<std::path::Path>) -> std::io::Result<Self> {
        Ok(FileSource(std::fs::File::open(path)?))
    }

    pub fn from_file(file: std::fs::File) -> Self {
        FileSource(file)
    }
}

#[cfg(all(feature = "std", unix))]
impl PtilesSource for FileSource {
    fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), SourceError> {
        use std::os::unix::fs::FileExt;
        self.0
            .read_exact_at(buf, offset)
            .map_err(|e| SourceError::Io {
                offset,
                needed: buf.len(),
                message: alloc::format!("{e}"),
            })
    }

    fn len(&self) -> Option<u64> {
        self.0.metadata().ok().map(|m| m.len())
    }
}

// Non-unix std targets (e.g. wasm32-unknown-unknown with std, or Windows):
// fall back to a Seek+Read based implementation guarded by interior
// mutability, since positioned reads without a shared cursor need platform
// support we don't have without extra deps. Not expected to be exercised in
// this workspace (native targets are unix; wasm uses MemorySource), but kept
// so `--features std` compiles on non-unix hosts too.
#[cfg(all(feature = "std", not(unix)))]
impl PtilesSource for FileSource {
    fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), SourceError> {
        use std::io::{Read, Seek, SeekFrom};
        // SAFETY/NOTE: this takes `&self` per the trait, so we reopen a cursor
        // via try_clone rather than requiring `&mut self`.
        let mut f = self.0.try_clone().map_err(|e| SourceError::Io {
            offset,
            needed: buf.len(),
            message: alloc::format!("{e}"),
        })?;
        f.seek(SeekFrom::Start(offset))
            .map_err(|e| SourceError::Io {
                offset,
                needed: buf.len(),
                message: alloc::format!("{e}"),
            })?;
        f.read_exact(buf).map_err(|e| SourceError::Io {
            offset,
            needed: buf.len(),
            message: alloc::format!("{e}"),
        })
    }

    fn len(&self) -> Option<u64> {
        self.0.metadata().ok().map(|m| m.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_source_reads_in_bounds() {
        let src = MemorySource::new(alloc::vec![1, 2, 3, 4, 5]);
        let mut buf = [0u8; 3];
        src.read_exact_at(1, &mut buf).unwrap();
        assert_eq!(buf, [2, 3, 4]);
    }

    #[test]
    fn memory_source_out_of_bounds_errors_not_panics() {
        let src = MemorySource::new(alloc::vec![1, 2, 3]);
        let mut buf = [0u8; 4];
        assert!(src.read_exact_at(0, &mut buf).is_err());
        let mut buf2 = [0u8; 1];
        assert!(src.read_exact_at(10, &mut buf2).is_err());
    }

    #[test]
    fn memory_source_full_range_read() {
        let src = MemorySource::new(alloc::vec![10, 20, 30, 40]);
        let mut buf = [0u8; 4];
        src.read_exact_at(0, &mut buf).unwrap();
        assert_eq!(buf, [10, 20, 30, 40]);
        assert_eq!(src.len(), Some(4));
    }

    #[test]
    fn memory_source_partial_read_at_tail() {
        // Read the final byte exactly at the boundary — no off-by-one error.
        let src = MemorySource::new(alloc::vec![10, 20, 30, 40]);
        let mut buf = [0u8; 1];
        src.read_exact_at(3, &mut buf).unwrap();
        assert_eq!(buf, [40]);
    }

    #[test]
    fn memory_source_read_straddling_eof_errors() {
        // Starts in-bounds but the requested length runs one byte past the end.
        let src = MemorySource::new(alloc::vec![1, 2, 3, 4]);
        let mut buf = [0u8; 3];
        let err = src.read_exact_at(2, &mut buf).unwrap_err();
        assert_eq!(
            err,
            SourceError::OutOfBounds {
                offset: 2,
                needed: 3,
                len: 4,
            }
        );
    }

    #[test]
    fn memory_source_zero_length_reads() {
        let src = MemorySource::new(alloc::vec![1, 2, 3]);
        let mut empty: [u8; 0] = [];
        // Zero-length read is a no-op success anywhere within [0, len].
        src.read_exact_at(0, &mut empty).unwrap();
        src.read_exact_at(3, &mut empty).unwrap(); // exactly at EOF is fine
        // But a zero-length read starting past the end is still out of bounds.
        assert!(src.read_exact_at(4, &mut empty).is_err());
    }

    #[test]
    fn memory_source_offset_overflows_usize_errors_not_panics() {
        // On 64-bit an offset > isize::MAX can't index a real buffer; must be a
        // clean OutOfBounds error, never a panic.
        let src = MemorySource::new(alloc::vec![1, 2, 3]);
        let mut buf = [0u8; 1];
        assert!(src.read_exact_at(u64::MAX, &mut buf).is_err());
    }
}
