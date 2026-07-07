//! ptiles-core: no_std-optional decoder library for the PTiles geospatial format.
//!
//! Cross-target design (see ~/.hermes/plans/ptiles-client-extraction-plan.md):
//! - decoders operate on `&[u8]` only
//! - `PtilesSource` is the only file-I/O abstraction; concrete sources
//!   (`MemorySource`, `FileSource`) implement it
//! - `ruzstd` for dict-aware zstd decompress everywhere
//! - `h3o` for H3 cell resolution (no_std + alloc, `std` feature enables extras)

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod codec;
pub mod header;
pub mod index;
pub mod source;
pub mod file;
#[cfg(feature = "http")]
pub mod http_source;
pub mod versions;

pub mod buildings;
pub mod roads;
pub mod water;
pub mod parks;
pub mod rail;
pub mod business;

pub mod proximity;
pub mod query;
pub mod scoring;

pub use codec::DecodeError;

pub use buildings::{decode_buildings, Building};
pub use business::{decode_business, Business};
pub use parks::{decode_parks, ParkFeature};
pub use rail::{decode_rail, RailFeature};
pub use roads::{decode_road_block, decode_roads, Intersection, RoadSegment};
pub use water::{decode_water, WaterFeature};

pub use proximity::{
    haversine_distance_m, nearest_road, point_to_linestring_distance_m,
    point_to_segment_distance_m, NearestRoad, SegmentProjection, DEFAULT_THRESHOLD_M,
};
pub use query::{cell_center, cell_for_coord, neighbor_cells};
pub use scoring::{score_candidates, Candidate, CandidateKind, Fix, ScoringParams};

pub use file::{FileError, PtilesFile};
pub use header::Header;
pub use index::{binary_search as index_binary_search, parse_index, IndexEntry};
#[cfg(feature = "std")]
pub use source::FileSource;
#[cfg(feature = "http")]
pub use http_source::HttpSource;
pub use source::{MemorySource, PtilesSource, SourceError};
pub use versions::{
    check_supported, format_table as supported_formats_table, versions_for, FormatEntry,
    UnsupportedVersion, SUPPORTED_FORMATS,
};

/// Human-readable table of format versions this client supports, generated
/// from [`SUPPORTED_FORMATS`]. Exposed for FFI/wasm/CLI to surface directly
/// (e.g. a `--supported-formats` CLI flag) without duplicating the table.
pub fn supported_formats() -> alloc::string::String {
    versions::format_table()
}
