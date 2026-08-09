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

pub mod coarse;
pub mod codec;
#[cfg(any(test, feature = "fixtures"))]
pub mod fixtures;
pub mod file;
pub mod header;
#[cfg(feature = "http")]
pub mod http_source;
pub mod index;
pub mod source;
pub mod versions;

pub mod address;
pub mod admin;
pub mod buildings;
pub mod business;
pub mod business_search;
pub mod camera;
pub mod ev;
pub mod environment;
pub mod math;
pub mod locate;
pub mod merged;
pub mod nav;
pub mod parks;
pub mod rail;
pub mod roads;
pub mod signals;
pub mod trails;
pub mod water;

pub mod proximity;
pub mod query;
pub mod route_graph;
pub mod scoring;
pub mod viewshed;

pub use codec::DecodeError;

pub use address::{
    AddressFile, AddressIndexEntry, AddressRecord, decode_address_cell, parse_v2_index,
};
pub use admin::{
    AdminFile, AdminGridEntry, AdminInfo, AdminLookup, AdminPolygon, AdminStringTables,
};
pub use buildings::{Building, decode_buildings, decode_buildings_for_cell};
pub use business::{
    Business, decode_business, decode_business_for_cell, decode_business_v3, decode_business_v4,
    decode_business_v4_at, decode_business_versioned,
};
pub use business_search::{
    BusinessHit, match_business_name_block, name_to_key, search_business_brute_force,
    search_business_indexed,
};
pub use ev::{
    CHARGE_RESERVE, CONNECTORS, ChargePlan, ChargeStop, Charger, DEFAULT_MAX_DETOUR_M,
    decode_chargers, is_fast_connector, is_public, plan_charge_stops,
};
pub use environment::{
    IndoorOutdoor, IndoorOutdoorEstimate, IndoorOutdoorParams, IndoorOutdoorReason,
    building_type_is_open_air, estimate_indoor_outdoor,
};
pub use camera::{
    CAMERA_MOUNT_M, CAMERA_RANGE_M, Camera, CameraView, DEFAULT_FOV_DEG, SUBJECT_M, bearing_to,
    cameras_near_road, cameras_seeing, decode_cameras,
};
pub use merged::{cell_ids as merged_cell_ids, cell_slice as merged_cell_slice};
pub use parks::{ParkFeature, decode_parks};
pub use rail::{RailFeature, decode_rail};
pub use roads::{
    Intersection, RoadSegment, decode_highways, decode_road_block, decode_roads,
    intersection_type_name,
};
pub use signals::{Signal, decode_signals};
pub use trails::{TrailFeature, decode_trails, trail_is_developed};
pub use nav::{
    LOOKAHEAD_M, MIN_TURN_DEG, Maneuver, NavState, OFF_ROUTE_M, Turn, bearing_delta,
    cumulative_m, name_turn, navigate, turn_queue,
};
pub use locate::{
    ADDRESS_THRESHOLD_M, Located, NearbyAddress, NearbyArea, NearbyPoint, NearbyWay,
    ON_WAY_THRESHOLD_M, locate, match_addresses, nearest_address, nearest_rail, nearest_road_way,
    nearest_station, nearest_trail, nearest_trailhead, park_at, water_at,
};
pub use water::{WaterFeature, decode_water};

pub use proximity::{
    DEFAULT_THRESHOLD_M, NearestIntersection, NearestRoad, SegmentProjection, haversine_distance_m,
    nearest_intersection, nearest_road, point_in_polygon, point_to_linestring_distance_m,
    point_to_ring_distance_m, point_to_segment_distance_m, segment_crossing,
};
pub use query::{
    BoundsError, MAX_BOUNDS_CELLS, cell_center, cell_for_coord, cells_for_bounds, neighbor_cells,
    try_cell_center,
};
pub use route_graph::{
    RouteFailure, RoutePrefs, RouteProfile, RouteResult, keep_road_class, profile_allows,
    profile_allows_driving, profile_allows_foot, route_roads, route_roads_diagnostic,
    route_roads_with, trail_segments,
};
pub use scoring::{Candidate, CandidateKind, Fix, ScoringParams, score_candidates};
pub use viewshed::{ViewBuilding, Visibility, estimate_height, height_or_estimate, viewshed};

pub use coarse::{
    CELL_FILLER_BITS, CoarseBracket, CoarseIndex, CoarseSample, normalize_cell,
    parse as parse_coarse_index,
};
pub use file::{BlockOffsetBase, FileError, IndexLayout, PtilesFile, index_layout};
pub use header::Header;
#[cfg(feature = "http")]
pub use http_source::HttpSource;
pub use index::{
    ENTRY_SIZE_V1, ENTRY_SIZE_V2, EntrySizeSource, IndexEntry, ParsedIndex,
    binary_search as index_binary_search, parse_entry_run, parse_index, parse_index_detected,
};
#[cfg(feature = "std")]
pub use source::FileSource;
pub use source::{MemorySource, PtilesSource, SourceError};
pub use versions::{
    FormatEntry, SUPPORTED_FORMATS, UnsupportedVersion, check_supported,
    format_table as supported_formats_table, versions_for,
};

/// Human-readable table of format versions this client supports, generated
/// from [`SUPPORTED_FORMATS`]. Exposed for FFI/wasm/CLI to surface directly
/// (e.g. a `--supported-formats` CLI flag) without duplicating the table.
pub fn supported_formats() -> alloc::string::String {
    versions::format_table()
}
