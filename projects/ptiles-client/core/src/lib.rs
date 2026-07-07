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

pub mod buildings;
pub mod roads;
pub mod water;
pub mod parks;
pub mod rail;
pub mod business;

pub mod proximity;
pub mod query;

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

pub use file::{FileError, PtilesFile};
pub use header::Header;
pub use index::{binary_search as index_binary_search, parse_index, IndexEntry};
#[cfg(feature = "std")]
pub use source::FileSource;
pub use source::{MemorySource, PtilesSource, SourceError};
