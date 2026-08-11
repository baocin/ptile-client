//! ptiles-ffi: UniFFI mobile bindings (Swift + Kotlin) over ptiles-core.
//!
//! Concrete, non-generic surface per the extraction plan (Phase 5 +
//! "Addendum: decisions 2026-07-07" item 3, `~/.hermes/plans/
//! ptiles-client-extraction-plan.md`): `core::PtilesFile<S: PtilesSource>` is
//! generic over its source, but UniFFI cannot export a generic type, so this
//! crate fixes `S = FileSource` and wraps it in one opaque object,
//! [`PtilesLayer`]. A second opaque object, [`PtilesStack`], groups up to one
//! roads/buildings/business `PtilesLayer` each for a state and exposes
//! [`PtilesStack::score`] — this is the "small stack object" shape from the
//! addendum, chosen because a CoreLocation caller naturally has one state's
//! three files open at once and one `CLLocation` (lat/lon/horizontalAccuracy/
//! speed) to score against all of them together, matching the CLI's
//! `--serve` cross-layer scoring path (`cli/src/main.rs::handle_serve_line`)
//! rather than the one-shot single-file path.
//!
//! UniFFI setup: proc-macro-only mode (`uniffi::setup_scaffolding!()` below),
//! no `.udl` file. Justification: UDL duplicates every signature in a
//! separate interface-definition file that must be kept in sync by hand;
//! proc-macro attributes (`#[uniffi::export]` etc.) sit directly on the Rust
//! item they describe, so the compiler enforces the one true signature and
//! `cargo test`/normal `cargo build` already exercise the binding-relevant
//! code paths. UDL remains useful for exposing a *foreign*-defined interface
//! or for teams that want the language-neutral IDL as documentation, neither
//! of which applies here — this is a Rust-authored library binding into
//! Swift/Kotlin callers, so proc-macros are the more direct, less
//! duplicative path (this is also UniFFI's own recommended default as of the
//! 0.28+ line).

use std::sync::Arc;

use ptiles_core::{
    cell_center, cell_for_coord, cells_for_bounds, decode_buildings, decode_business_versioned, decode_road_block,
    decode_roads, haversine_distance_m, nearest_intersection as core_nearest_intersection,
    nearest_road as core_nearest_road, neighbor_cells, point_in_polygon, score_candidates,
    search_business_indexed, trail_is_developed as core_trail_is_developed,
    route_roads_diagnostic, trail_segments as core_trail_segments, RoutePrefs, RouteProfile,
    Building, Business, Candidate as CoreCandidate, CandidateKind as CoreCandidateKind, FileSource,
    Fix as CoreFix, HttpSource, Intersection, PtilesFile, RoadSegment, ScoringParams,
};
use ptiles_core::file::FileError;
use ptiles_core::source::SourceError;
use ptiles_core::{AdminFile as CoreAdminFile, AdminInfo as CoreAdminInfo};
use ptiles_core::{AddressFile as CoreAddressFile, AddressRecord as CoreAddressRecord};

pub mod motion;

uniffi::setup_scaffolding!();

// --- Errors -----------------------------------------------------------------

/// Flat UniFFI error enum. Every error path anywhere in this crate collapses
/// into one of these variants; wrapped source errors are stringified
/// (`{0}`/`{message}`) rather than exposed as nested UniFFI error types,
/// which keeps the generated Swift/Kotlin error surface small and stable.
#[derive(Debug, thiserror::Error, uniffi::Error)]
#[uniffi(flat_error)]
pub enum PtilesError {
    /// Local or otherwise unclassified open failure: a missing local file, a
    /// bad magic prefix, an unsupported version.
    #[error("failed to open {path}: {message}")]
    Open { path: String, message: String },
    /// The host could not be reached at all: DNS, TLS, connection refused or
    /// reset, timeout.
    ///
    /// This is deliberately separate from [`PtilesError::NotFound`]. "You are
    /// offline" and "this coordinate is outside coverage" are opposite
    /// situations -- one should be retried later and the other never will
    /// succeed -- and a caller that cannot tell them apart has to guess. That
    /// guess is what an offline fallback ends up encoding: `core`'s
    /// `SourceError` has always distinguished them, this layer was flattening
    /// the distinction away.
    #[error("network error reaching {path}: {message}")]
    Network { path: String, message: String },
    /// The server answered, and said no: the file is not there (404), or not
    /// permitted (403), or any other non-success status. The layer genuinely
    /// does not exist at that URL -- retrying will not change that.
    #[error("HTTP {status} for {path}")]
    NotFound { path: String, status: u16 },
    /// The server ignored the `Range` header (answered 200 instead of 206), so
    /// positioned reads cannot work against it. A server/CDN configuration
    /// problem, not a data problem, and it fails loudly rather than reading the
    /// whole body and treating it as a slice.
    #[error("{path} does not support HTTP range requests (status {status})")]
    RangeUnsupported { path: String, status: u16 },
    #[error("could not infer layer from filename {path:?} (expected <state>.<layer>.ptiles)")]
    UnknownLayer { path: String },
    #[error("block decode failed: {message}")]
    Decode { message: String },
    #[error("this operation is not supported on a {layer} layer")]
    UnsupportedForLayer { layer: String },
    #[error("ring {ring} not supported (only 0 or 1)")]
    InvalidRing { ring: u8 },
    /// A bounding box that is malformed, or larger than
    /// `ptiles_core::MAX_BOUNDS_CELLS` (512 H3 res-7 cells, roughly a
    /// metropolitan area). Reported rather than truncated: a prefetch that
    /// silently covered part of the region would leave the caller trusting data
    /// it does not have.
    #[error("bad bounding box: {message}")]
    InvalidBounds { message: String },
    #[error("offline route failed: {message}")]
    Routing { message: String },
}

impl PtilesError {
    /// Classify a `SourceError` into the transport-vs-server-vs-local split.
    ///
    /// `core` already knows which of these happened; the job here is only to
    /// carry it across the FFI instead of stringifying everything into `Open`.
    fn from_source(path: &str, e: &SourceError) -> PtilesError {
        match e {
            SourceError::HttpNetwork { message, .. } => PtilesError::Network {
                path: path.to_string(),
                message: message.clone(),
            },
            SourceError::HttpStatus { status, .. } => PtilesError::NotFound {
                path: path.to_string(),
                status: *status,
            },
            SourceError::RangeNotSupported { status, .. } => PtilesError::RangeUnsupported {
                path: path.to_string(),
                status: *status,
            },
            // OutOfBounds and Io are local/structural: a truncated file, a
            // read past the end, a filesystem error.
            other => PtilesError::Open {
                path: path.to_string(),
                message: other.to_string(),
            },
        }
    }

    /// Same, for the `FileError` that wraps it. A `FileError::Source` carries
    /// the network detail; everything else (bad magic, unsupported version,
    /// failed decompress) is a property of the bytes, not the transport.
    fn from_file(path: &str, e: &FileError) -> PtilesError {
        match e {
            FileError::Source(src) => PtilesError::from_source(path, src),
            other => PtilesError::Open {
                path: path.to_string(),
                message: other.to_string(),
            },
        }
    }
}

// --- Plain data records -------------------------------------------------------

#[derive(Debug, Clone, uniffi::Record)]
pub struct LatLon {
    pub lat: f64,
    pub lon: f64,
}

/// Which local network an offline route may use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum OfflineRouteMode {
    Driving,
    Trail,
}

/// A complete route computed from installed PTiles blocks only.
#[derive(Debug, Clone, uniffi::Record)]
pub struct OfflineRoute {
    pub distance_m: f64,
    pub duration_s: f64,
    pub path: Vec<LatLon>,
    pub decoded_segments: u32,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct NearestRoad {
    pub osm_id: u64,
    pub name: Option<String>,
    pub road_class: String,
    pub snapped_lat: f64,
    pub snapped_lon: f64,
    pub distance_m: f64,
    pub geometry: Vec<LatLon>,
}

/// One trail feature, as stored. Unlike the `nearest_trail` answer this
/// includes trailhead points, which carry a single coordinate.
#[derive(Debug, Clone, uniffi::Record)]
pub struct TrailInfo {
    pub osm_id: i64,
    pub name: Option<String>,
    pub trail_type: String,
    pub surface: String,
    pub sac_scale: String,
    pub developed: bool,
    /// True for a trailhead marker rather than a length of trail. The same
    /// fact as `geom_type == 1`, named for callers that read rather than
    /// decode.
    pub is_trailhead: bool,
    /// 0 = linestring (a way you walk), 1 = point (a trailhead).
    pub geom_type: u8,
    pub geometry: Vec<LatLon>,
}

/// Nearest labeled intersection to a query point (the "am I at an
/// intersection?" answer). `intersection_type`: 1 = traffic_signals,
/// 2 = stop, 3 = give_way, 4 = roundabout (0/other = untyped). Reports a
/// mapped intersection *point*, not junction degree — the format stores no
/// road-to-node topology.
#[derive(Debug, Clone, uniffi::Record)]
pub struct NearestIntersection {
    pub lat: f64,
    pub lon: f64,
    pub distance_m: f64,
    pub intersection_type: u8,
}

/// Resolved jurisdiction for a point, from the admin layer.
#[derive(Debug, Clone, uniffi::Record)]
pub struct AdminInfo {
    pub country: String,
    pub state: String,
    pub county: String,
    pub zip: String,
    pub timezone: String,
    pub boundary_flags: u8,
}

impl From<CoreAdminInfo> for AdminInfo {
    fn from(a: CoreAdminInfo) -> Self {
        AdminInfo {
            country: a.country,
            state: a.state,
            county: a.county,
            zip: a.zip,
            timezone: a.timezone,
            boundary_flags: a.boundary_flags,
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct RoadInfo {
    pub osm_id: u64,
    pub name: Option<String>,
    pub road_class: String,
    pub geometry: Vec<LatLon>,
}

/// One decoded surveillance camera. `direction` is degrees clockwise from
/// north when tagged; `angle` is the field of view in degrees when tagged.
#[derive(Debug, Clone, uniffi::Record)]
pub struct CameraInfo {
    pub osm_id: i64,
    pub location: LatLon,
    /// `camera`, `ALPR`, `guard`, or `unknown`.
    pub device_type: String,
    /// `public`, `outdoor`, `indoor`, or `unknown`.
    pub placement: String,
    /// `fixed`, `panning`, `dome`, or `unknown`. The last two rotate.
    pub camera_type: String,
    pub direction: Option<u16>,
    pub angle: Option<u8>,
    pub operator: Option<String>,
    pub name: Option<String>,
    pub ref_tag: Option<String>,
}

/// What one camera can see of a point -- `ptiles_core::CameraView`, plus
/// enough of the camera itself that a caller need not join back to the
/// listing.
///
/// `sees` is the answer; the other flags are why. Every assumption behind
/// them leans toward reporting a camera rather than omitting one, so a `true`
/// may be inference (check `aim_assumed`) while a `false` is comparatively
/// solid.
#[derive(Debug, Clone, uniffi::Record)]
pub struct CameraViewInfo {
    pub osm_id: i64,
    pub name: Option<String>,
    pub operator: Option<String>,
    pub camera_type: String,
    pub location: LatLon,
    pub distance_m: f64,
    /// Bearing from the camera to you, degrees clockwise from north.
    pub bearing_deg: f64,
    /// False only when the camera is tagged with a direction and you fall
    /// outside the resulting cone.
    pub aimed_at_you: bool,
    /// True when `aimed_at_you` rests on an assumption rather than on tags.
    pub aim_assumed: bool,
    /// False when a building stands between you and it.
    pub line_of_sight: bool,
    pub sees: bool,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ParkInfo {
    pub osm_id: i64,
    pub name: Option<String>,
    pub park_type: String,
    pub geometry: Vec<LatLon>,
}

/// One decoded water feature. `geom_type`: 0 = polygon, 1 = linestring,
/// 2 = reference (geometry lives elsewhere in the file, so `geometry` is
/// empty and `ref_feature_id` is the handle).
#[derive(Debug, Clone, uniffi::Record)]
pub struct WaterInfo {
    pub osm_id: i64,
    pub name: Option<String>,
    pub water_type: String,
    pub geom_type: u8,
    pub width: Option<u16>,
    pub ref_feature_id: Option<u32>,
    pub geometry: Vec<LatLon>,
}

/// One decoded rail feature. `geom_type`: 0 = track, 1 = station/halt point.
#[derive(Debug, Clone, uniffi::Record)]
pub struct RailInfo {
    pub osm_id: i64,
    pub name: Option<String>,
    pub rail_type: String,
    pub geom_type: u8,
    pub geometry: Vec<LatLon>,
}

/// A linear feature the query point is on or near — the shape of
/// `ptiles_core::NearbyWay`. `on_it` is true within
/// `ptiles_core::ON_WAY_THRESHOLD_M` (25 m); outside that the answer is
/// "near", and the caller decides what to do with the distance.
#[derive(Debug, Clone, uniffi::Record)]
pub struct WayInfo {
    /// `road`, `trail`, or `rail`.
    pub kind: String,
    /// OSM id of the feature, or `None` when the caller did not supply the
    /// slice `core` indexed into (as in `PtilesStack::locate`, which merges
    /// several layers and keeps no single index).
    ///
    /// Signed, not unsigned like `NearestRoad::osm_id`: the trails and rail
    /// decoders carry the id as a signed delta and OSM ids for derived ways
    /// can be negative, so widening it would corrupt exactly those records.
    pub osm_id: Option<i64>,
    pub name: Option<String>,
    /// Road class, trail type, or rail type. Pass it to
    /// [`trail_is_developed`] for the made-trail-vs-desire-path split.
    pub class: String,
    pub distance_m: f64,
    pub snapped: LatLon,
    pub on_it: bool,
}

impl From<ptiles_core::NearbyWay> for WayInfo {
    fn from(w: ptiles_core::NearbyWay) -> Self {
        WayInfo {
            kind: w.kind,
            osm_id: None,
            name: w.name,
            class: w.class,
            distance_m: w.distance_m,
            snapped: LatLon { lat: w.snapped.0, lon: w.snapped.1 },
            on_it: w.on_it,
        }
    }
}

/// An area the query point is in or near (`ptiles_core::NearbyArea`).
/// `distance_m` is 0 when `inside`, else the distance to the boundary.
#[derive(Debug, Clone, uniffi::Record)]
pub struct AreaInfo {
    /// `park` or `water`.
    pub kind: String,
    pub name: Option<String>,
    pub class: String,
    pub distance_m: f64,
    pub inside: bool,
}

impl From<ptiles_core::NearbyArea> for AreaInfo {
    fn from(a: ptiles_core::NearbyArea) -> Self {
        AreaInfo {
            kind: a.kind,
            name: a.name,
            class: a.class,
            distance_m: a.distance_m,
            inside: a.inside,
        }
    }
}

/// A point feature near the query point — a trailhead or a station
/// (`ptiles_core::NearbyPoint`). These are exactly what the linear lookups
/// skip, since a point has no centreline to be on.
#[derive(Debug, Clone, uniffi::Record)]
pub struct PointInfo {
    /// `trailhead` or `station`.
    pub kind: String,
    pub name: Option<String>,
    pub class: String,
    pub location: LatLon,
    pub distance_m: f64,
}

impl From<ptiles_core::NearbyPoint> for PointInfo {
    fn from(p: ptiles_core::NearbyPoint) -> Self {
        PointInfo {
            kind: p.kind,
            name: p.name,
            class: p.class,
            location: LatLon { lat: p.lat, lon: p.lon },
            distance_m: p.distance_m,
        }
    }
}

/// An address near the query point, with where it is and how far.
///
/// Distinct from [`AddressRecord`], which carries no position: v1 address
/// files store none, and only v2 records can be measured against a point at
/// all (see `core::locate::nearest_address`).
#[derive(Debug, Clone, uniffi::Record)]
pub struct NearbyAddressInfo {
    pub osm_id: i64,
    pub housenumber: String,
    pub street: String,
    pub location: LatLon,
    pub distance_m: f64,
}

impl From<ptiles_core::NearbyAddress> for NearbyAddressInfo {
    fn from(a: ptiles_core::NearbyAddress) -> Self {
        NearbyAddressInfo {
            osm_id: a.osm_id,
            housenumber: a.housenumber,
            street: a.street,
            location: LatLon { lat: a.lat, lon: a.lon },
            distance_m: a.distance_m,
        }
    }
}

/// What is at a point, across whichever layers a [`PtilesStack`] holds —
/// `ptiles_core::Located` plus the area and point answers the trail/park/
/// water/rail layers contribute.
#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct LocatedInfo {
    /// Nearest road/trail/rail way, whether or not you are on it.
    pub nearest_way: Option<WayInfo>,
    /// The way you are actually on, within 25 m. When set, the same feature
    /// as `nearest_way`.
    pub on_way: Option<WayInfo>,
    /// Nearest address within `ptiles_core::ADDRESS_THRESHOLD_M` (150 m).
    pub address: Option<NearbyAddressInfo>,
    /// The park you are in, else the nearest one.
    pub park: Option<AreaInfo>,
    /// The water body you are in, else the nearest.
    pub water: Option<AreaInfo>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct BuildingInfo {
    pub osm_id: i64,
    pub name: Option<String>,
    pub building_type: String,
    pub category: Option<String>,
    pub centroid: LatLon,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct BusinessInfo {
    pub osm_id: i64,
    pub name: String,
    pub location: LatLon,
    pub category_idx: u8,
    pub phone: Option<String>,
    pub website: Option<String>,
    pub operating_status: String,
    /// Upstream dataset: 1 = Overture, 2 = Foursquare. `None` on records with
    /// no extended-attributes trailer.
    pub source_type: Option<u8>,
    /// Upstream record id (a GERS id for Overture, a venue id for Foursquare) --
    /// the only stable handle back to the source dataset.
    pub source_id: Option<String>,
    /// Upstream confidence, 0-100.
    pub confidence: Option<u8>,
}

/// One hit from [`PtilesLayer::search_business`], the shape of
/// `ptiles_core::business_search::BusinessHit` translated to a UniFFI
/// record. No `osm_id`/`phone`/`website`/`operating_status`: the
/// `business_name_index.ptiles` sidecar this searches doesn't carry them
/// (see `core::business_search`'s module doc) -- only the spatial
/// `.ptiles` file (`PtilesLayer::businesses_near`) has that detail.
#[derive(Debug, Clone, uniffi::Record)]
pub struct BusinessSearchHit {
    pub name: String,
    pub category_idx: u8,
    pub location: LatLon,
    /// 2 = exact (case-insensitive) name match, 1 = prefix, 0 = substring.
    pub score: u8,
}

/// A GPS fix to score candidates against. Field names/units mirror
/// `CoreLocation`: `horizontal_accuracy_m` is `CLLocation.horizontalAccuracy`,
/// `speed_mps` is `CLLocation.speed` (pass `nil`/`None` when unavailable, not
/// CoreLocation's `-1` sentinel — the caller translates that at the FFI
/// boundary).
#[derive(Debug, Clone, Copy, uniffi::Record)]
pub struct Fix {
    pub lat: f64,
    pub lon: f64,
    pub horizontal_accuracy_m: f64,
    pub speed_mps: Option<f64>,
}

/// Indoor/outdoor estimate exposed to Swift/Kotlin/Python. `Uncertain` is a
/// first-class result: a building-edge GPS fix or missing map coverage is not
/// forced into a binary answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum IndoorOutdoorState {
    Indoor,
    Outdoor,
    Uncertain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum IndoorOutdoorReason {
    InsideBuilding,
    InsideOpenStructure,
    AccuracyOverlapsBuilding,
    ClearOfBuildings,
    NoBuildingsNearby,
    IncompleteCoverage,
    InvalidFix,
    PoorAccuracy,
}

/// Explainable result from [`PtilesLayer::indoor_outdoor`].
#[derive(Debug, Clone, uniffi::Record)]
pub struct IndoorOutdoorEstimate {
    pub state: IndoorOutdoorState,
    pub confidence: f64,
    pub reason: IndoorOutdoorReason,
    pub building_osm_id: Option<i64>,
    /// Depth inside or clearance outside the relevant footprint.
    pub distance_to_boundary_m: Option<f64>,
}

impl From<ptiles_core::IndoorOutdoorEstimate> for IndoorOutdoorEstimate {
    fn from(e: ptiles_core::IndoorOutdoorEstimate) -> Self {
        let state = match e.classification {
            ptiles_core::IndoorOutdoor::Indoor => IndoorOutdoorState::Indoor,
            ptiles_core::IndoorOutdoor::Outdoor => IndoorOutdoorState::Outdoor,
            ptiles_core::IndoorOutdoor::Uncertain => IndoorOutdoorState::Uncertain,
        };
        let reason = match e.reason {
            ptiles_core::IndoorOutdoorReason::InsideBuilding => IndoorOutdoorReason::InsideBuilding,
            ptiles_core::IndoorOutdoorReason::InsideOpenStructure => IndoorOutdoorReason::InsideOpenStructure,
            ptiles_core::IndoorOutdoorReason::AccuracyOverlapsBuilding => IndoorOutdoorReason::AccuracyOverlapsBuilding,
            ptiles_core::IndoorOutdoorReason::ClearOfBuildings => IndoorOutdoorReason::ClearOfBuildings,
            ptiles_core::IndoorOutdoorReason::NoBuildingsNearby => IndoorOutdoorReason::NoBuildingsNearby,
            ptiles_core::IndoorOutdoorReason::IncompleteCoverage => IndoorOutdoorReason::IncompleteCoverage,
            ptiles_core::IndoorOutdoorReason::InvalidFix => IndoorOutdoorReason::InvalidFix,
            ptiles_core::IndoorOutdoorReason::PoorAccuracy => IndoorOutdoorReason::PoorAccuracy,
        };
        IndoorOutdoorEstimate {
            state,
            confidence: e.confidence,
            reason,
            building_osm_id: e.building_osm_id,
            distance_to_boundary_m: e.distance_to_boundary_m,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum CandidateKind {
    Road,
    Building,
    Business,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct Candidate {
    pub kind: CandidateKind,
    pub osm_id: i64,
    pub name: Option<String>,
    pub distance_m: f64,
    pub score: f64,
}

fn to_candidate(c: &CoreCandidate) -> Candidate {
    Candidate {
        kind: match c.kind {
            CoreCandidateKind::Road => CandidateKind::Road,
            CoreCandidateKind::Building => CandidateKind::Building,
            CoreCandidateKind::Business => CandidateKind::Business,
        },
        osm_id: c.osm_id,
        name: c.name.clone(),
        distance_m: c.distance_m,
        score: c.score,
    }
}

fn camera_info(c: &ptiles_core::Camera) -> CameraInfo {
    CameraInfo {
        osm_id: c.osm_id,
        location: LatLon { lat: c.lat, lon: c.lon },
        device_type: c.device_type.clone(),
        placement: c.placement.clone(),
        camera_type: c.camera_type.clone(),
        direction: c.direction,
        angle: c.angle,
        operator: c.operator.clone(),
        name: c.name.clone(),
        ref_tag: c.ref_tag.clone(),
    }
}

/// `core::cameras_seeing`, with each answer carrying its camera's own
/// details. Shared by the layer and stack entry points so the two cannot
/// drift on what "sees" means.
fn camera_views(
    lat: f64,
    lon: f64,
    cameras: &[ptiles_core::Camera],
    buildings: &[ptiles_core::ViewBuilding],
    range_m: f64,
) -> Vec<CameraViewInfo> {
    ptiles_core::cameras_seeing(lat, lon, cameras, buildings, range_m)
        .into_iter()
        .map(|v| {
            let cam = &cameras[v.index];
            CameraViewInfo {
                osm_id: v.osm_id,
                name: cam.name.clone(),
                operator: cam.operator.clone(),
                camera_type: cam.camera_type.clone(),
                location: LatLon { lat: cam.lat, lon: cam.lon },
                distance_m: v.distance_m,
                bearing_deg: v.bearing_deg,
                aimed_at_you: v.aimed_at_you,
                aim_assumed: v.aim_assumed,
                line_of_sight: v.line_of_sight,
                sees: v.sees,
            }
        })
        .collect()
}

fn geometry_of(coords: &[[f64; 2]]) -> Vec<LatLon> {
    // Decoders store `[lon, lat]` pairs (see roads.rs/buildings.rs doc
    // comments) -- flip to the lat/lon field order this FFI surface uses
    // everywhere else.
    coords.iter().map(|c| LatLon { lat: c[1], lon: c[0] }).collect()
}

fn validate_ring(ring: u8) -> Result<(), PtilesError> {
    if ring > 1 {
        Err(PtilesError::InvalidRing { ring })
    } else {
        Ok(())
    }
}

// --- Layer inference (mirrors cli/src/main.rs::Layer) ------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LayerKind {
    Roads,
    BuildingsV8,
    Business,
    /// `{STATE}.business_name_index.ptiles` -- the name-search sidecar, see
    /// `core::business_search`. Separate from `Business` since it's a
    /// different file with a different (first-letter-bucket) index shape,
    /// only usable via `PtilesLayer::search_business`.
    BusinessNameIndex,
    Trails,
    Parks,
    Water,
    Rail,
    Camera,
}

impl LayerKind {
    fn from_path(path: &str) -> Option<LayerKind> {
        let name = std::path::Path::new(path).file_name()?.to_str()?;
        let mut parts = name.split('.');
        let _state = parts.next()?;
        // Published snapshots version the stem (`TN.trails_v1.ptiles`,
        // `TN.roads_v2.ptiles`) while the local corpus does not; strip a
        // trailing `_v<N>` so both resolve, exactly as
        // `cli/src/main.rs::Layer::from_filename_token` does. The real schema
        // version always comes from the header, never the filename.
        let token = parts.next()?;
        let base = match token.rsplit_once("_v") {
            Some((stem, digits))
                if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) =>
            {
                stem
            }
            _ => token,
        };
        match base {
            "roads" => Some(LayerKind::Roads),
            // `trails_v1` and `buildings_v8` need no arm of their own: the
            // `_v<N>` strip above already reduces them to `trails`/`buildings`.
            "buildings" => Some(LayerKind::BuildingsV8),
            "business" => Some(LayerKind::Business),
            "business_name_index" => Some(LayerKind::BusinessNameIndex),
            "trails" => Some(LayerKind::Trails),
            "parks" => Some(LayerKind::Parks),
            "water" => Some(LayerKind::Water),
            "rail" => Some(LayerKind::Rail),
            "camera" => Some(LayerKind::Camera),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            LayerKind::Roads => "roads",
            LayerKind::BuildingsV8 => "buildings_v8",
            LayerKind::Business => "business",
            LayerKind::BusinessNameIndex => "business_name_index",
            LayerKind::Trails => "trails",
            LayerKind::Parks => "parks",
            LayerKind::Water => "water",
            LayerKind::Rail => "rail",
            LayerKind::Camera => "camera",
        }
    }
}

// --- PtilesLayer: one opened `.ptiles` file --------------------------------

/// True for `http://`/`https://` -- the scheme sniff `PtilesLayer::open`
/// uses to pick `HttpSource` vs. `FileSource`, matching `ptiles-cli`'s
/// `is_url` (`cli/src/main.rs`).
fn is_url(s: &str) -> bool {
    s.starts_with("http://") || s.starts_with("https://")
}

/// `PtilesFile` over either a local file or an HTTP(S) URL. `PtilesFile<S>`
/// is generic over its source, but UniFFI cannot export a generic type
/// (see this module's top doc comment), so `PtilesLayer` needs one concrete
/// field type covering both backends -- picked at `open()` time by scheme
/// sniff, mirroring `ptiles-cli`'s `AnyFile` (`cli/src/main.rs`).
enum AnyFile {
    File(PtilesFile<FileSource>),
    Http(PtilesFile<HttpSource>),
}

impl AnyFile {
    /// One cell's decompressed bytes, ready to hand to a decoder.
    ///
    /// `read_cell`, not `read_block`. In a merged-block layer several cells
    /// share one compressed block behind a small `(h3_cell, offset)` directory,
    /// and `read_block` returns that whole thing -- directory included. A
    /// decoder handed those bytes reads the directory as its first record and
    /// gets nothing.
    ///
    /// This was silent rather than loud: `decode_trails` breaks out of its loop
    /// on the first bad record and returns `Ok(vec![])`, so a real block holding
    /// 581 features decoded as zero, with no error, for every point in the
    /// state. `read_cell` slices the cell out first and returns the whole block
    /// unchanged when the layer is not merged, so this is correct for both.
    fn read_block(&self, cell: u64) -> Result<Option<Vec<u8>>, PtilesError> {
        let result = match self {
            AnyFile::File(f) => f.read_cell(cell).map_err(|e| e.to_string()),
            AnyFile::Http(f) => f.read_cell(cell).map_err(|e| e.to_string()),
        };
        result.map_err(|message| PtilesError::Decode { message })
    }

    /// Header fields, for [`PtilesLayer::metadata`].
    fn header(&self) -> &ptiles_core::Header {
        match self {
            AnyFile::File(f) => f.header(),
            AnyFile::Http(f) => f.header(),
        }
    }

    /// Total file size in bytes, if the source knows it.
    fn byte_length(&self) -> Option<u64> {
        use ptiles_core::source::PtilesSource;
        match self {
            AnyFile::File(f) => f.source().len(),
            AnyFile::Http(f) => f.source().len(),
        }
    }

    /// `(etag, last_modified)` from the construction-time response, for a
    /// remote file. A local file has neither.
    fn http_provenance(&self) -> (Option<String>, Option<String>) {
        match self {
            AnyFile::File(_) => (None, None),
            AnyFile::Http(f) => {
                let src = f.source();
                (
                    src.etag().map(|s| s.to_string()),
                    src.last_modified().map(|s| s.to_string()),
                )
            }
        }
    }

    /// Format/schema version from the file header — needed by
    /// `decode_road_block` to know whether a trailing intersection table is
    /// present (v2+).
    fn version(&self) -> u8 {
        match self {
            AnyFile::File(f) => f.header().version,
            AnyFile::Http(f) => f.header().version,
        }
    }
}

/// What an opened layer can say about itself.
///
/// For a file you range-read rather than download, this is the only way to know
/// what you are querying: coverage, size, schema version, and -- since the
/// format carries no build date -- the HTTP validators, which are the closest
/// thing to provenance a remote `.ptiles` has.
#[derive(Debug, Clone, uniffi::Record)]
pub struct LayerMetadata {
    /// Layer name inferred from the filename (`roads`, `buildings_v8`, ...).
    pub layer: String,
    /// Path or URL this layer was opened from.
    pub path: String,
    /// Format/schema version from the header.
    pub version: u8,
    /// Coverage bounding box, degrees. Everything outside it is guaranteed
    /// absent, so a caller can skip the query rather than pay a range read to
    /// learn there is nothing there.
    pub min_lat: f64,
    pub min_lon: f64,
    pub max_lat: f64,
    pub max_lon: f64,
    /// Features the header claims. **Not always true**: every published
    /// business layer reports 0 because of a builder bug (it compares a string
    /// to an int), while its records decode fine. Treat 0 as "unknown", not as
    /// "empty".
    pub feature_count: u64,
    /// Blocks in the file, i.e. how many populated H3 cells it has.
    pub block_count: u32,
    /// Total size of the remote/local file in bytes, if known.
    pub byte_length: Option<u64>,
    /// `Last-Modified` of the remote file. The format stores no build date, so
    /// this is the only answer to "is this layer from 2024 or last week?".
    /// `None` for a local file, or a server that does not send it.
    pub last_modified: Option<String>,
    /// `ETag` of the remote file: opaque, but a change means a rebuild. Pair it
    /// with a cached copy to detect that the layer moved on.
    pub etag: Option<String>,
}

/// One opened `.ptiles` file (local path or `http(s)://` URL), its layer
/// inferred from the filename (`<state>.<layer>.ptiles`), wrapping
/// `AnyFile` -- see that type's doc comment for why this isn't
/// `PtilesFile<FileSource>` directly anymore.
#[derive(uniffi::Object)]
pub struct PtilesLayer {
    kind: LayerKind,
    file: AnyFile,
    path: String,
    /// Decompressed blocks, keyed by H3 cell.
    ///
    /// `HttpSource` already caches byte *ranges*, so without this a second
    /// query in the same cell skipped the network but still re-ran zstd over
    /// half a megabyte. That was affordable for one-shot lookups and is not for
    /// the batch methods below, which is what made per-point enrichment of a
    /// day's trace infeasible: 12,300 points in a few dozen cells should
    /// decompress a few dozen blocks, not 12,300.
    ///
    /// ponytail: unbounded, so a caller that sweeps a whole state accumulates
    /// every block it touched. Bound it (LRU by cell, or a byte budget) if a
    /// long-lived process ever holds one layer across many regions; for the
    /// per-trace and per-region lifetimes this API is built for, the cache is
    /// smaller than the download it replaces.
    blocks: std::sync::Mutex<std::collections::HashMap<u64, Option<Arc<Vec<u8>>>>>,
}

#[uniffi::export]
impl PtilesLayer {
    /// Open a `.ptiles` file, local or remote. `path` must be
    /// `<state>.<layer>.ptiles` (optionally under an `http(s)://` URL) where
    /// `<layer>` is one of `roads`, `buildings_v8`, `business`.
    #[uniffi::constructor]
    pub fn open(path: String) -> Result<Arc<Self>, PtilesError> {
        let kind = LayerKind::from_path(&path).ok_or_else(|| PtilesError::UnknownLayer {
            path: path.clone(),
        })?;
        let opened_from = path.clone();
        let file = if is_url(&path) {
            let source =
                HttpSource::open(&path).map_err(|e| PtilesError::from_source(&path, &e))?;
            AnyFile::Http(
                PtilesFile::open(source).map_err(|e| PtilesError::from_file(&path, &e))?,
            )
        } else {
            // A local open failure is a `std::io::Error`, not a `SourceError`:
            // there is no transport to classify, so it stays `Open`.
            let source = FileSource::open(&path).map_err(|e| PtilesError::Open {
                path: path.clone(),
                message: e.to_string(),
            })?;
            AnyFile::File(
                PtilesFile::open(source).map_err(|e| PtilesError::from_file(&path, &e))?,
            )
        };
        Ok(Arc::new(PtilesLayer {
            kind,
            file,
            path: opened_from,
            blocks: std::sync::Mutex::new(std::collections::HashMap::new()),
        }))
    }
}

// Private decode helpers, deliberately in a plain (non-`#[uniffi::export]`)
// impl block: they return core types (`RoadSegment`/`Building`/`Business`)
// that don't implement UniFFI's `Lower`/`TypeId` traits and aren't meant to
// cross the FFI boundary -- only the exported methods below translate their
// output into this crate's `Record` types. Every method in an
// `#[uniffi::export] impl` block is exported, so these must live outside it.
impl PtilesLayer {
    fn cells_for(&self, lat: f64, lon: f64, ring: u8) -> Vec<u64> {
        let center = cell_for_coord(lat, lon);
        let mut cells = vec![center];
        if ring >= 1 {
            cells.extend(neighbor_cells(center));
        }
        cells
    }

    /// Whether the whole positional-uncertainty circle lies inside the layer's
    /// declared coverage. At a state edge the adjacent state's building layer
    /// is missing, so `covers(point)` alone is not enough evidence for an
    /// outdoor classification.
    fn covers_radius(&self, lat: f64, lon: f64, radius_m: f64) -> bool {
        if !(lat.is_finite()
            && lon.is_finite()
            && radius_m.is_finite()
            && radius_m >= 0.0
            && (-90.0..=90.0).contains(&lat)
            && (-180.0..=180.0).contains(&lon))
        {
            return false;
        }
        let lat_delta = radius_m / 111_320.0;
        let lon_scale = (lat.to_radians().cos().abs() * 111_320.0).max(1.0);
        let lon_delta = radius_m / lon_scale;
        self.covers(lat - lat_delta, lon - lon_delta)
            && self.covers(lat + lat_delta, lon + lon_delta)
    }

    /// One cell's decompressed block, memoized -- including the *absence* of a
    /// block, so an empty cell is not re-requested either.
    fn block(&self, cell: u64) -> Result<Option<Arc<Vec<u8>>>, PtilesError> {
        if let Some(hit) = self.blocks.lock().expect("block cache").get(&cell) {
            return Ok(hit.clone());
        }
        let got = self.file.read_block(cell)?.map(Arc::new);
        self.blocks
            .lock()
            .expect("block cache")
            .insert(cell, got.clone());
        Ok(got)
    }

    /// Points grouped by the cell that answers them, input order preserved
    /// inside each group.
    ///
    /// This is the whole trick behind the batch methods: a day of tracking is
    /// ~12,000 points but only a few dozen H3 res-7 cells, so grouping first
    /// turns "12,000 range-read-and-decompress sessions" into "one per cell".
    fn group_by_cell(points: &[LatLon]) -> Vec<(u64, Vec<usize>)> {
        let mut groups: Vec<(u64, Vec<usize>)> = Vec::new();
        for (i, p) in points.iter().enumerate() {
            let cell = cell_for_coord(p.lat, p.lon);
            match groups.iter_mut().find(|(c, _)| *c == cell) {
                Some((_, idx)) => idx.push(i),
                None => groups.push((cell, vec![i])),
            }
        }
        groups
    }

    fn decoded_roads(&self, lat: f64, lon: f64, ring: u8) -> Result<Vec<RoadSegment>, PtilesError> {
        let mut roads = Vec::new();
        for cell in self.cells_for(lat, lon, ring) {
            let Some(block) = self.block(cell)? else {
                continue;
            };
            let mut r = decode_roads(&block).map_err(|e| PtilesError::Decode {
                message: e.to_string(),
            })?;
            roads.append(&mut r);
        }
        Ok(roads)
    }

    fn decoded_roads_for_cells(&self, cells: &[u64]) -> Result<Vec<RoadSegment>, PtilesError> {
        let mut roads = Vec::new();
        for &cell in cells {
            let Some(block) = self.block(cell)? else { continue };
            let mut decoded = decode_roads(&block).map_err(|e| PtilesError::Decode {
                message: e.to_string(),
            })?;
            roads.append(&mut decoded);
        }
        Ok(roads)
    }

    fn decoded_trails_for_cells(
        &self,
        cells: &[u64],
    ) -> Result<Vec<ptiles_core::TrailFeature>, PtilesError> {
        let mut trails = Vec::new();
        for &cell in cells {
            let Some(block) = self.block(cell)? else { continue };
            let mut decoded = ptiles_core::decode_trails(&block).map_err(|e| PtilesError::Decode {
                message: e.to_string(),
            })?;
            trails.append(&mut decoded);
        }
        Ok(trails)
    }

    /// Intersections from the road blocks covering `(lat, lon)` (+ ring-1
    /// neighbors when `ring == 1`). Uses `decode_road_block` with the file's
    /// header version so the trailing v2 intersection table is read.
    fn decoded_intersections(
        &self,
        lat: f64,
        lon: f64,
        ring: u8,
    ) -> Result<Vec<Intersection>, PtilesError> {
        let version = self.file.version();
        let mut intersections = Vec::new();
        for cell in self.cells_for(lat, lon, ring) {
            let Some(block) = self.block(cell)? else {
                continue;
            };
            let (_roads, mut ix) = decode_road_block(&block, version).map_err(|e| {
                PtilesError::Decode { message: e.to_string() }
            })?;
            intersections.append(&mut ix);
        }
        Ok(intersections)
    }

    fn decoded_buildings(&self, lat: f64, lon: f64, ring: u8) -> Result<Vec<Building>, PtilesError> {
        let mut buildings = Vec::new();
        for cell in self.cells_for(lat, lon, ring) {
            let Some(block) = self.block(cell)? else {
                continue;
            };
            let (center_lat, center_lon) = cell_center(cell);
            let mut b = decode_buildings(&block, center_lat, center_lon).map_err(|e| {
                PtilesError::Decode { message: e.to_string() }
            })?;
            buildings.append(&mut b);
        }
        Ok(buildings)
    }

    fn decoded_business(&self, lat: f64, lon: f64, ring: u8) -> Result<Vec<Business>, PtilesError> {
        let version = self.file.version();
        let mut businesses = Vec::new();
        for cell in self.cells_for(lat, lon, ring) {
            let Some(block) = self.block(cell)? else {
                continue;
            };
            // Versioned, not sniffed: v4 stores coordinates relative to the
            // cell centre, so decoding it without the cell puts every record
            // near Null Island (see `core::business::decode_business_for_cell`).
            let mut b =
                decode_business_versioned(&block, version, cell).map_err(|e| PtilesError::Decode {
                    message: e.to_string(),
                })?;
            businesses.append(&mut b);
        }
        Ok(businesses)
    }

    fn decoded_trails(
        &self,
        lat: f64,
        lon: f64,
        ring: u8,
    ) -> Result<Vec<ptiles_core::TrailFeature>, PtilesError> {
        self.decoded_layer(lat, lon, ring, ptiles_core::decode_trails)
    }

    fn decoded_parks(
        &self,
        lat: f64,
        lon: f64,
        ring: u8,
    ) -> Result<Vec<ptiles_core::ParkFeature>, PtilesError> {
        self.decoded_layer(lat, lon, ring, ptiles_core::decode_parks)
    }

    fn decoded_water(
        &self,
        lat: f64,
        lon: f64,
        ring: u8,
    ) -> Result<Vec<ptiles_core::WaterFeature>, PtilesError> {
        self.decoded_layer(lat, lon, ring, ptiles_core::decode_water)
    }

    fn decoded_cameras(
        &self,
        lat: f64,
        lon: f64,
        ring: u8,
    ) -> Result<Vec<ptiles_core::Camera>, PtilesError> {
        self.decoded_layer(lat, lon, ring, ptiles_core::decode_cameras)
    }

    fn decoded_rail(
        &self,
        lat: f64,
        lon: f64,
        ring: u8,
    ) -> Result<Vec<ptiles_core::RailFeature>, PtilesError> {
        self.decoded_layer(lat, lon, ring, ptiles_core::decode_rail)
    }

    /// Decode every block covering the query cells with `decode`, concatenated.
    ///
    /// The trails/parks/water/rail decoders all take a bare `&[u8]` and need
    /// neither the header version nor the cell (unlike roads v2's trailing
    /// intersection table, or business v4's cell-relative coordinates), so one
    /// helper covers all four instead of four near-identical loops.
    fn decoded_layer<T>(
        &self,
        lat: f64,
        lon: f64,
        ring: u8,
        decode: fn(&[u8]) -> Result<Vec<T>, ptiles_core::DecodeError>,
    ) -> Result<Vec<T>, PtilesError> {
        let mut out = Vec::new();
        for cell in self.cells_for(lat, lon, ring) {
            let Some(block) = self.block(cell)? else {
                continue;
            };
            // No slicing here: `AnyFile::read_block` reads through
            // `PtilesFile::read_cell`, which already cuts the cell out of a
            // merged block (parks/rail/trails carry a 38-byte index and pack
            // several cells per block). Slicing again would treat records as
            // a cell table and fail on the first read.
            let mut decoded = decode(&block).map_err(|e| PtilesError::Decode {
                message: e.to_string(),
            })?;
            out.append(&mut decoded);
        }
        Ok(out)
    }

    /// Reject a query aimed at the wrong file. Every layer-specific method
    /// starts here, so a caller that opened `TN.parks.ptiles` and asked for
    /// trails gets told which layer it actually has rather than an empty list.
    fn require(&self, kind: LayerKind) -> Result<(), PtilesError> {
        if self.kind == kind {
            Ok(())
        } else {
            Err(PtilesError::UnsupportedForLayer {
                layer: self.kind.as_str().to_string(),
            })
        }
    }

    /// `core::search_business_indexed` needs a concrete `PtilesFile<S>`, not
    /// `AnyFile` -- dispatch to whichever backend `self.file` holds, same
    /// pattern as `AnyFile::read_block`.
    fn search_business_indexed_dispatch(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<ptiles_core::BusinessHit>, PtilesError> {
        let result = match &self.file {
            AnyFile::File(f) => search_business_indexed(f, query, limit),
            AnyFile::Http(f) => search_business_indexed(f, query, limit),
        };
        result.map_err(|e| PtilesError::Decode { message: e.to_string() })
    }
}

/// Name for an `intersection_type` byte: `traffic_signals`, `stop`, `give_way`,
/// `roundabout`, or `junction` for 0/unrecognised.
///
/// The vocabulary is a property of the format, so it comes from
/// `ptiles_core::intersection_type_name` rather than being re-spelled by each
/// caller. A caller holding only the integer would otherwise have to invent
/// names, which is fabrication with a plausible face.
#[uniffi::export]
pub fn intersection_type_name(intersection_type: u8) -> String {
    ptiles_core::intersection_type_name(intersection_type).to_string()
}

/// Whether an `intersection_type` is a node traffic *waits* at (signals, stop,
/// give-way) rather than one it flows through (roundabout, untyped junction).
///
/// This is the distinction the motion classifier uses to tell "stopped at a
/// light" from "arrived somewhere", and it is a fact about the vocabulary, so it
/// lives here rather than in every caller that needs it.
/// Whether a trail type is built infrastructure (`cycleway`, `footway`)
/// rather than a walked way (`path`, `track`, `bridleway`, `steps`).
///
/// [`TrailInfo`] carries this already; the free function is for a caller
/// holding only a [`WayInfo`], whose `class` is the trail type. The split is a
/// property of the layer's vocabulary, so it comes from
/// `ptiles_core::trail_is_developed` rather than being re-listed per caller.
#[uniffi::export]
pub fn trail_is_developed(trail_type: String) -> bool {
    ptiles_core::trail_is_developed(&trail_type)
}

#[uniffi::export]
pub fn intersection_holds_traffic(intersection_type: u8) -> bool {
    matches!(intersection_type, 1 | 2 | 3)
}

/// **Business `category_idx` has no vocabulary to expose.** The published
/// `business_v4` files carry the index and no category table, and no sidecar
/// mapping ships with them, so nothing in this library can turn `7` into a name
/// without inventing one. Log the integer; do not guess. If the POI builder
/// starts emitting a category table (in `aux`, or as a sidecar file), a
/// `business_category_name` accessor belongs right here next to these two.
///
/// Signals are unaffected: `.signals` records carry their type as a *string*
/// already (`traffic_signals`, `stop`, ...), decoded from the format's own
/// table.
/// The building a point belongs to: containing polygon first, else the nearest
/// centroid within 50 m. Shared by `building` and `buildings_at` so the single
/// and batch paths cannot drift apart on what "the building here" means.
fn pick_building(buildings: &[Building], lat: f64, lon: f64) -> Option<BuildingInfo> {
    buildings
        .iter()
        .find(|b| point_in_polygon(lat, lon, &b.coords))
        .or_else(|| {
            buildings
                .iter()
                .map(|b| (b, haversine_distance_m(lat, lon, b.centroid_lat, b.centroid_lon)))
                .filter(|(_, d)| *d <= 50.0)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(b, _)| b)
        })
        .map(|b| BuildingInfo {
            osm_id: b.osm_id,
            name: b.name.clone(),
            building_type: b.building_type.clone(),
            category: b.category.clone(),
            centroid: LatLon {
                lat: b.centroid_lat,
                lon: b.centroid_lon,
            },
        })
}

#[uniffi::export]
impl PtilesLayer {
    /// What this layer covers, how big it is, and when it was built -- as far as
    /// that can be known. See [`LayerMetadata`], especially the caveat on
    /// `feature_count`.
    ///
    /// Free: every field comes from the 256-byte header already read at
    /// `open()`, plus the HTTP validators from that same first response. No
    /// additional request.
    pub fn metadata(&self) -> LayerMetadata {
        let h = self.file.header();
        let (etag, last_modified) = self.file.http_provenance();
        LayerMetadata {
            layer: self.kind.as_str().to_string(),
            path: self.path.clone(),
            version: h.version,
            min_lat: h.min_lat as f64,
            min_lon: h.min_lon as f64,
            max_lat: h.max_lat as f64,
            max_lon: h.max_lon as f64,
            feature_count: h.feature_count,
            block_count: h.block_count,
            byte_length: self.file.byte_length(),
            last_modified,
            etag,
        }
    }

    /// Whether a coordinate is inside this layer's declared coverage.
    ///
    /// Cheap and local, so it is the right first question: outside the box the
    /// answer is definitively "nothing here", and no range read can improve on
    /// that. Being *inside* the box does not promise a block exists.
    pub fn covers(&self, lat: f64, lon: f64) -> bool {
        let h = self.file.header();
        lat >= h.min_lat as f64
            && lat <= h.max_lat as f64
            && lon >= h.min_lon as f64
            && lon <= h.max_lon as f64
    }

    /// Fetch and cache every block covering a bounding box, in one pass.
    ///
    /// The middle ground between range-reading forever and downloading a whole
    /// state (CA roads is 118 MB): name the region you are about to work in,
    /// pay for it once, and every later query against it is served from memory.
    /// Returns the number of blocks now cached (cells with no block are cached
    /// as absent and counted as 0).
    ///
    /// Bounded by `ptiles_core::MAX_BOUNDS_CELLS` (512 H3 res-7 cells, ~2,600
    /// km^2 -- a metropolitan area, not a state): a larger box is an
    /// `InvalidBounds` error rather than a silent truncation, because a partial
    /// prefetch that looks complete is worse than a refusal. Prefetch the region
    /// you are working in, or walk a larger area in tiles.
    pub fn prefetch_bbox(
        &self,
        min_lat: f64,
        min_lon: f64,
        max_lat: f64,
        max_lon: f64,
    ) -> Result<u32, PtilesError> {
        let cells = ptiles_core::cells_for_bounds(min_lat, min_lon, max_lat, max_lon).map_err(
            |e| PtilesError::InvalidBounds {
                message: e.to_string(),
            },
        )?;
        let mut warmed = 0u32;
        for cell in cells {
            if self.block(cell)?.is_some() {
                warmed += 1;
            }
        }
        Ok(warmed)
    }

    /// How many blocks are currently cached in memory. For a caller that wants
    /// to know what a prefetch actually bought, or when to drop the layer.
    pub fn cached_block_count(&self) -> u32 {
        self.blocks.lock().expect("block cache").len() as u32
    }

    /// Drop the block cache, keeping the layer open.
    pub fn clear_cache(&self) {
        self.blocks.lock().expect("block cache").clear();
    }

    /// The building at each of `points`, in input order.
    ///
    /// Grouped by H3 cell internally, so a run of points in the same cell costs
    /// one block read and one decompress rather than one each. This is the
    /// difference between enriching a day of tracking per-point (~12,000 points,
    /// a few dozen cells) and having to sample it.
    ///
    /// `None` for a point with no building within 50 m, per the single-point
    /// [`PtilesLayer::building`] rule.
    pub fn buildings_at(&self, points: Vec<LatLon>) -> Result<Vec<Option<BuildingInfo>>, PtilesError> {
        if self.kind != LayerKind::BuildingsV8 {
            return Err(PtilesError::UnsupportedForLayer {
                layer: self.kind.as_str().to_string(),
            });
        }
        let mut out: Vec<Option<BuildingInfo>> = vec![None; points.len()];
        for (cell, idx) in Self::group_by_cell(&points) {
            let Some(block) = self.block(cell)? else {
                continue;
            };
            let (center_lat, center_lon) = cell_center(cell);
            let buildings = decode_buildings(&block, center_lat, center_lon).map_err(|e| {
                PtilesError::Decode {
                    message: e.to_string(),
                }
            })?;
            for i in idx {
                let (lat, lon) = (points[i].lat, points[i].lon);
                out[i] = pick_building(&buildings, lat, lon);
            }
        }
        Ok(out)
    }

    /// The nearest road to each of `points`, in input order. Same cell grouping
    /// as [`PtilesLayer::buildings_at`].
    ///
    /// `threshold_m <= 0` uses the same default as the single-point
    /// [`PtilesLayer::nearest_road`].
    pub fn nearest_roads_at(
        &self,
        points: Vec<LatLon>,
        threshold_m: f64,
    ) -> Result<Vec<Option<NearestRoad>>, PtilesError> {
        if self.kind != LayerKind::Roads {
            return Err(PtilesError::UnsupportedForLayer {
                layer: self.kind.as_str().to_string(),
            });
        }
        let threshold = if threshold_m > 0.0 {
            threshold_m
        } else {
            ptiles_core::DEFAULT_THRESHOLD_M * 2.0
        };
        let mut out: Vec<Option<NearestRoad>> = vec![None; points.len()];
        for (cell, idx) in Self::group_by_cell(&points) {
            let Some(block) = self.block(cell)? else {
                continue;
            };
            let roads = decode_roads(&block).map_err(|e| PtilesError::Decode {
                message: e.to_string(),
            })?;
            for i in idx {
                let (lat, lon) = (points[i].lat, points[i].lon);
                out[i] = core_nearest_road(lat, lon, &roads, threshold).map(|nr| {
                    let road = &roads[nr.road_index];
                    NearestRoad {
                        osm_id: road.osm_id,
                        name: road.name.clone(),
                        road_class: road.road_class.clone(),
                        snapped_lat: nr.snapped.0,
                        snapped_lon: nr.snapped.1,
                        distance_m: nr.distance_m,
                        geometry: road
                            .coords
                            .iter()
                            .map(|c| LatLon { lat: c[1], lon: c[0] })
                            .collect(),
                    }
                });
            }
        }
        Ok(out)
    }

    /// The nearest mapped intersection to each of `points`, in input order.
    ///
    /// Unlike the other two batch methods this reads ring-1 neighbours per cell,
    /// matching single-point [`PtilesLayer::nearest_intersection`]: an
    /// intersection is a point feature and the nearest one to a fix near a cell
    /// edge frequently lives in the next cell over.
    pub fn nearest_intersections_at(
        &self,
        points: Vec<LatLon>,
        threshold_m: f64,
    ) -> Result<Vec<Option<NearestIntersection>>, PtilesError> {
        if self.kind != LayerKind::Roads {
            return Err(PtilesError::UnsupportedForLayer {
                layer: self.kind.as_str().to_string(),
            });
        }
        let threshold = if threshold_m > 0.0 {
            threshold_m
        } else {
            ptiles_core::DEFAULT_THRESHOLD_M
        };
        let version = self.file.version();
        let mut out: Vec<Option<NearestIntersection>> = vec![None; points.len()];
        for (cell, idx) in Self::group_by_cell(&points) {
            let mut intersections: Vec<Intersection> = Vec::new();
            let mut cells = vec![cell];
            cells.extend(neighbor_cells(cell));
            for c in cells {
                let Some(block) = self.block(c)? else { continue };
                let (_roads, mut ix) =
                    decode_road_block(&block, version).map_err(|e| PtilesError::Decode {
                        message: e.to_string(),
                    })?;
                intersections.append(&mut ix);
            }
            for i in idx {
                let (lat, lon) = (points[i].lat, points[i].lon);
                out[i] = core_nearest_intersection(lat, lon, &intersections, threshold).map(|ni| {
                    let [ix_lon, ix_lat] = intersections[ni.index].coords();
                    NearestIntersection {
                        lat: ix_lat,
                        lon: ix_lon,
                        distance_m: ni.distance_m,
                        intersection_type: ni.intersection_type,
                    }
                });
            }
        }
        Ok(out)
    }

    /// Nearest road segment to `(lat, lon)` within the CLI's default search
    /// threshold (`ptiles_core::DEFAULT_THRESHOLD_M * 2.0`, matching
    /// `cli/src/main.rs::OpenedLayer::query`'s roads branch). Roads-layer
    /// only.
    pub fn nearest_road(&self, lat: f64, lon: f64) -> Result<Option<NearestRoad>, PtilesError> {
        if self.kind != LayerKind::Roads {
            return Err(PtilesError::UnsupportedForLayer {
                layer: self.kind.as_str().to_string(),
            });
        }
        // Ring is opt-in everywhere else in this API but nearest_road here
        // mirrors the CLI's ring-0 one-shot default: callers that want a
        // wider search should call `roads(..., ring: 1)` and snap manually,
        // or this method can be revisited if mobile callers need ring-1.
        let roads = self.decoded_roads(lat, lon, 0)?;
        let Some(nr) = core_nearest_road(lat, lon, &roads, ptiles_core::DEFAULT_THRESHOLD_M * 2.0)
        else {
            return Ok(None);
        };
        let road = &roads[nr.road_index];
        Ok(Some(NearestRoad {
            osm_id: road.osm_id,
            name: road.name.clone(),
            road_class: road.road_class.clone(),
            snapped_lat: nr.snapped.0,
            snapped_lon: nr.snapped.1,
            distance_m: nr.distance_m,
            geometry: geometry_of(&road.coords),
        }))
    }

    /// Nearest labeled intersection to `(lat, lon)` within `threshold_m`
    /// (defaults to `ptiles_core::DEFAULT_THRESHOLD_M` when `threshold_m <= 0`).
    /// Answers "am I at an intersection?" — returns the nearest mapped
    /// intersection point and its traffic-control type, or `None`. Roads-layer
    /// only. Searches ring-1 neighbors so a point near a cell edge still finds
    /// an intersection in the adjacent cell.
    pub fn nearest_intersection(
        &self,
        lat: f64,
        lon: f64,
        threshold_m: f64,
    ) -> Result<Option<NearestIntersection>, PtilesError> {
        if self.kind != LayerKind::Roads {
            return Err(PtilesError::UnsupportedForLayer {
                layer: self.kind.as_str().to_string(),
            });
        }
        let threshold = if threshold_m > 0.0 {
            threshold_m
        } else {
            ptiles_core::DEFAULT_THRESHOLD_M
        };
        let intersections = self.decoded_intersections(lat, lon, 1)?;
        let Some(ni) = core_nearest_intersection(lat, lon, &intersections, threshold) else {
            return Ok(None);
        };
        let [ix_lon, ix_lat] = intersections[ni.index].coords();
        Ok(Some(NearestIntersection {
            lat: ix_lat,
            lon: ix_lon,
            distance_m: ni.distance_m,
            intersection_type: ni.intersection_type,
        }))
    }

    /// All decoded road segments in the cell containing `(lat, lon)`, plus
    /// ring-1 neighbors when `ring == 1`. `ring` must be 0 or 1 (matches the
    /// CLI's `--ring` semantics in `cli/src/main.rs::validate_ring`).
    /// Roads-layer only.
    pub fn roads(&self, lat: f64, lon: f64, ring: u8) -> Result<Vec<RoadInfo>, PtilesError> {
        if self.kind != LayerKind::Roads {
            return Err(PtilesError::UnsupportedForLayer {
                layer: self.kind.as_str().to_string(),
            });
        }
        validate_ring(ring)?;
        let roads = self.decoded_roads(lat, lon, ring)?;
        Ok(roads
            .iter()
            .map(|r| RoadInfo {
                osm_id: r.osm_id,
                name: r.name.clone(),
                road_class: r.road_class.clone(),
                geometry: geometry_of(&r.coords),
            })
            .collect())
    }

    /// Building containing `(lat, lon)`, falling back to the nearest
    /// centroid within 50m (mirrors `cli/src/main.rs::find_building`).
    /// Buildings-layer only.
    pub fn building(&self, lat: f64, lon: f64) -> Result<Option<BuildingInfo>, PtilesError> {
        if self.kind != LayerKind::BuildingsV8 {
            return Err(PtilesError::UnsupportedForLayer {
                layer: self.kind.as_str().to_string(),
            });
        }
        let buildings = self.decoded_buildings(lat, lon, 0)?;
        Ok(pick_building(&buildings, lat, lon))
    }

    /// Estimate whether a GPS fix is indoors or outdoors from building
    /// footprints, with an explicit uncertainty result and evidence.
    ///
    /// The containing H3 cell plus ring 1 are read so a footprint assigned to
    /// a neighboring cell is not missed. A fix worse than 50 m, a point whose
    /// accuracy circle overlaps a wall, an open-sided `roof`/`carport`/`canopy`,
    /// or incomplete layer coverage returns `Uncertain`. This is map inference,
    /// not room/floor positioning or proof of physical occupancy.
    pub fn indoor_outdoor(
        &self,
        lat: f64,
        lon: f64,
        horizontal_accuracy_m: f64,
    ) -> Result<IndoorOutdoorEstimate, PtilesError> {
        if self.kind != LayerKind::BuildingsV8 {
            return Err(PtilesError::UnsupportedForLayer {
                layer: self.kind.as_str().to_string(),
            });
        }
        let params = ptiles_core::IndoorOutdoorParams::default();
        let coverage_complete = self.covers_radius(
            lat,
            lon,
            horizontal_accuracy_m.max(0.0) + params.outdoor_clearance_m,
        );
        let buildings = if self.covers(lat, lon) {
            self.decoded_buildings(lat, lon, 1)?
        } else {
            Vec::new()
        };
        let fix = CoreFix {
            lat,
            lon,
            horizontal_accuracy_m,
            speed_mps: None,
        };
        Ok(ptiles_core::estimate_indoor_outdoor(
            &fix,
            &buildings,
            coverage_complete,
            &params,
        ).into())
    }

    /// Businesses within `radius_m` of `(lat, lon)`, searching the
    /// containing cell (plus ring-1 neighbors when `ring == 1`).
    /// Business-layer only.
    pub fn businesses_near(
        &self,
        lat: f64,
        lon: f64,
        ring: u8,
        radius_m: f64,
    ) -> Result<Vec<BusinessInfo>, PtilesError> {
        if self.kind != LayerKind::Business {
            return Err(PtilesError::UnsupportedForLayer {
                layer: self.kind.as_str().to_string(),
            });
        }
        validate_ring(ring)?;
        let businesses = self.decoded_business(lat, lon, ring)?;
        Ok(businesses
            .iter()
            .filter(|b| haversine_distance_m(lat, lon, b.lat, b.lon) <= radius_m)
            .map(|b| BusinessInfo {
                osm_id: b.osm_id,
                name: b.name.clone(),
                location: LatLon { lat: b.lat, lon: b.lon },
                category_idx: b.category_idx,
                phone: b.phone.clone(),
                website: b.website.clone(),
                operating_status: b.operating_status.clone(),
                source_type: b.source_type,
                source_id: b.source_id.clone(),
                confidence: b.confidence,
            })
            .collect())
    }

    /// Every trail in the cell containing `(lat, lon)`, plus ring-1 neighbors
    /// when `ring == 1`. Trails-layer only.
    pub fn trails(&self, lat: f64, lon: f64, ring: u8) -> Result<Vec<TrailInfo>, PtilesError> {
        self.require(LayerKind::Trails)?;
        validate_ring(ring)?;
        Ok(self
            .decoded_trails(lat, lon, ring)?
            .iter()
            .map(|t| TrailInfo {
                osm_id: t.osm_id,
                name: t.name.clone(),
                trail_type: t.trail_type.clone(),
                surface: t.surface.clone(),
                sac_scale: t.sac_scale.clone(),
                developed: core_trail_is_developed(&t.trail_type),
                is_trailhead: t.geom_type == 1,
                geom_type: t.geom_type,
                geometry: geometry_of(&t.coords),
            })
            .collect())
    }

    /// The trail under `(lat, lon)` — "which path am I walking on". Trailhead
    /// points are skipped (they have no centreline); use
    /// [`PtilesLayer::nearest_trailhead`] for those. Trails-layer only.
    pub fn nearest_trail(
        &self,
        lat: f64,
        lon: f64,
        ring: u8,
    ) -> Result<Option<WayInfo>, PtilesError> {
        self.require(LayerKind::Trails)?;
        validate_ring(ring)?;
        let trails = self.decoded_trails(lat, lon, ring)?;
        Ok(ptiles_core::nearest_trail(lat, lon, &trails).map(|w| WayInfo {
            osm_id: trails.get(w.index).map(|t| t.osm_id),
            ..WayInfo::from(w)
        }))
    }

    /// The nearest trailhead — where a trail network is entered, which is what
    /// a caller planning to start a walk wants. Trails-layer only.
    pub fn nearest_trailhead(
        &self,
        lat: f64,
        lon: f64,
        ring: u8,
    ) -> Result<Option<PointInfo>, PtilesError> {
        self.require(LayerKind::Trails)?;
        validate_ring(ring)?;
        let trails = self.decoded_trails(lat, lon, ring)?;
        Ok(ptiles_core::nearest_trailhead(lat, lon, &trails).map(PointInfo::from))
    }

    /// Every park in the query cells. Parks-layer only.
    pub fn parks(&self, lat: f64, lon: f64, ring: u8) -> Result<Vec<ParkInfo>, PtilesError> {
        self.require(LayerKind::Parks)?;
        validate_ring(ring)?;
        Ok(self
            .decoded_parks(lat, lon, ring)?
            .iter()
            .map(|p| ParkInfo {
                osm_id: p.osm_id,
                name: p.name.clone(),
                park_type: p.park_type.clone(),
                geometry: geometry_of(&p.coords),
            })
            .collect())
    }

    /// The park containing `(lat, lon)`, else the nearest park boundary.
    /// Check `inside` before telling a user they are in it. Parks-layer only.
    pub fn park_at(&self, lat: f64, lon: f64, ring: u8) -> Result<Option<AreaInfo>, PtilesError> {
        self.require(LayerKind::Parks)?;
        validate_ring(ring)?;
        let parks = self.decoded_parks(lat, lon, ring)?;
        Ok(ptiles_core::park_at(lat, lon, &parks).map(AreaInfo::from))
    }

    /// Every water feature in the query cells. Water-layer only.
    pub fn water(&self, lat: f64, lon: f64, ring: u8) -> Result<Vec<WaterInfo>, PtilesError> {
        self.require(LayerKind::Water)?;
        validate_ring(ring)?;
        Ok(self
            .decoded_water(lat, lon, ring)?
            .iter()
            .map(|w| WaterInfo {
                osm_id: w.osm_id,
                name: w.name.clone(),
                water_type: w.water_type.clone(),
                geom_type: w.geom_type,
                width: w.width,
                ref_feature_id: w.ref_feature_id,
                geometry: geometry_of(&w.coords),
            })
            .collect())
    }

    /// The water body containing `(lat, lon)`, else the nearest water feature.
    /// River centrelines are linestrings and never report `inside`.
    /// Water-layer only.
    pub fn water_at(&self, lat: f64, lon: f64, ring: u8) -> Result<Option<AreaInfo>, PtilesError> {
        self.require(LayerKind::Water)?;
        validate_ring(ring)?;
        let water = self.decoded_water(lat, lon, ring)?;
        Ok(ptiles_core::water_at(lat, lon, &water).map(AreaInfo::from))
    }

    /// Every rail feature in the query cells. Rail-layer only.
    pub fn rail(&self, lat: f64, lon: f64, ring: u8) -> Result<Vec<RailInfo>, PtilesError> {
        self.require(LayerKind::Rail)?;
        validate_ring(ring)?;
        Ok(self
            .decoded_rail(lat, lon, ring)?
            .iter()
            .map(|r| RailInfo {
                osm_id: r.osm_id,
                name: r.name.clone(),
                rail_type: r.rail_type.clone(),
                geom_type: r.geom_type,
                geometry: geometry_of(&r.coords),
            })
            .collect())
    }

    /// The rail track under `(lat, lon)`. Station points are skipped; use
    /// [`PtilesLayer::nearest_station`] for those. Rail-layer only.
    pub fn nearest_rail(
        &self,
        lat: f64,
        lon: f64,
        ring: u8,
    ) -> Result<Option<WayInfo>, PtilesError> {
        self.require(LayerKind::Rail)?;
        validate_ring(ring)?;
        let rail = self.decoded_rail(lat, lon, ring)?;
        Ok(ptiles_core::nearest_rail(lat, lon, &rail).map(|w| WayInfo {
            osm_id: rail.get(w.index).map(|r| r.osm_id),
            ..WayInfo::from(w)
        }))
    }

    /// The nearest station or halt point. Rail-layer only.
    pub fn nearest_station(
        &self,
        lat: f64,
        lon: f64,
        ring: u8,
    ) -> Result<Option<PointInfo>, PtilesError> {
        self.require(LayerKind::Rail)?;
        validate_ring(ring)?;
        let rail = self.decoded_rail(lat, lon, ring)?;
        Ok(ptiles_core::nearest_station(lat, lon, &rail).map(PointInfo::from))
    }

    /// Every camera in the query cells. Camera-layer only.
    pub fn cameras(&self, lat: f64, lon: f64, ring: u8) -> Result<Vec<CameraInfo>, PtilesError> {
        self.require(LayerKind::Camera)?;
        validate_ring(ring)?;
        Ok(self
            .decoded_cameras(lat, lon, ring)?
            .iter()
            .map(camera_info)
            .collect())
    }

    /// Which cameras can see `(lat, lon)`, nearest first -- without the
    /// occlusion half of the answer, since a camera file alone knows nothing
    /// about what stands in the way. Every in-range camera therefore reports
    /// a clear sight line here. Use [`PtilesStack::cameras_seeing`], which
    /// reads the buildings layer too, when that matters. Camera-layer only.
    pub fn cameras_seeing(
        &self,
        lat: f64,
        lon: f64,
        ring: u8,
        range_m: f64,
    ) -> Result<Vec<CameraViewInfo>, PtilesError> {
        self.require(LayerKind::Camera)?;
        validate_ring(ring)?;
        let cameras = self.decoded_cameras(lat, lon, ring)?;
        Ok(camera_views(lat, lon, &cameras, &[], range_m))
    }

    /// Business name search over a `{STATE}.business_name_index.ptiles`
    /// sidecar (open a `PtilesLayer` on that file, not the main
    /// `business.ptiles` file). Index-accelerated: correct for
    /// case-insensitive prefix queries; substring queries only surface a
    /// hit when the substring starts at the name's first character (see
    /// `core::business_search`'s module doc for why). `limit` caps the
    /// returned, score-ranked hit count. `BusinessNameIndex`-layer only.
    pub fn search_business(
        &self,
        query: String,
        limit: u32,
    ) -> Result<Vec<BusinessSearchHit>, PtilesError> {
        if self.kind != LayerKind::BusinessNameIndex {
            return Err(PtilesError::UnsupportedForLayer {
                layer: self.kind.as_str().to_string(),
            });
        }
        let hits = self.search_business_indexed_dispatch(&query, limit as usize)?;
        Ok(hits
            .iter()
            .map(|h| BusinessSearchHit {
                name: h.name.clone(),
                category_idx: h.category_idx,
                location: LatLon { lat: h.lat, lon: h.lon },
                score: h.score,
            })
            .collect())
    }
}

// --- AdminLayer: point -> jurisdiction lookup -------------------------------

/// An opened admin file (`US.admin.ptiles`). Separate from `PtilesLayer`
/// because admin is a lookup-grid layer, not block-per-cell.
#[derive(uniffi::Object)]
pub struct AdminLayer {
    file: CoreAdminFile,
}

#[uniffi::export]
impl AdminLayer {
    /// Open an admin file, local path or `http(s)://` URL.
    #[uniffi::constructor]
    pub fn open(path: String) -> Result<Arc<Self>, PtilesError> {
        let file = if is_url(&path) {
            let source =
                HttpSource::open(&path).map_err(|e| PtilesError::from_source(&path, &e))?;
            CoreAdminFile::open(source).map_err(|e| PtilesError::from_file(&path, &e))?
        } else {
            let source = FileSource::open(&path).map_err(|e| PtilesError::Open {
                path: path.clone(),
                message: e.to_string(),
            })?;
            CoreAdminFile::open(source).map_err(|e| PtilesError::from_file(&path, &e))?
        };
        Ok(Arc::new(AdminLayer { file }))
    }

    /// Jurisdiction (country/state/county/zip/timezone) covering `(lat, lon)`,
    /// or `None` if the lookup grid has no entry for that cell.
    pub fn admin_at(&self, lat: f64, lon: f64) -> Option<AdminInfo> {
        self.file.admin_at(lat, lon).map(AdminInfo::from)
    }
}

// --- AddressLayer: reverse/forward address lookup ---------------------------

/// One decoded address (`{osm_id, housenumber, street}`; location is the cell).
#[derive(Debug, Clone, uniffi::Record)]
pub struct AddressRecord {
    pub osm_id: i64,
    pub housenumber: String,
    pub street: String,
}

impl From<CoreAddressRecord> for AddressRecord {
    fn from(r: CoreAddressRecord) -> Self {
        AddressRecord { osm_id: r.osm_id, housenumber: r.housenumber, street: r.street }
    }
}

enum AnyAddress {
    File(CoreAddressFile<FileSource>),
    Http(CoreAddressFile<HttpSource>),
}

/// An opened `.address.ptiles` file. Separate from `PtilesLayer` because
/// address uses a v2 merged-block index, not the v1 block reader.
#[derive(uniffi::Object)]
pub struct AddressLayer {
    file: AnyAddress,
}

#[uniffi::export]
impl AddressLayer {
    /// Open an address file, local path or `http(s)://` URL.
    #[uniffi::constructor]
    pub fn open(path: String) -> Result<Arc<Self>, PtilesError> {
        let file = if is_url(&path) {
            let src = HttpSource::open(&path).map_err(|e| PtilesError::from_source(&path, &e))?;
            AnyAddress::Http(
                CoreAddressFile::open(src).map_err(|e| PtilesError::from_file(&path, &e))?,
            )
        } else {
            let src = FileSource::open(&path).map_err(|e| PtilesError::Open {
                path: path.clone(),
                message: e.to_string(),
            })?;
            AnyAddress::File(
                CoreAddressFile::open(src).map_err(|e| PtilesError::from_file(&path, &e))?,
            )
        };
        Ok(Arc::new(AddressLayer { file }))
    }

    /// Reverse lookup: all addresses in the cell(s) covering `(lat, lon)`
    /// (`ring == 1` adds neighbors).
    pub fn addresses_at(&self, lat: f64, lon: f64, ring: u8) -> Result<Vec<AddressRecord>, PtilesError> {
        Ok(self
            .core_addresses_at(lat, lon, ring)?
            .into_iter()
            .map(AddressRecord::from)
            .collect())
    }

    /// Forward lookup: addresses near `(lat, lon)` matching `housenumber` +
    /// `street` (accent/case-insensitive).
    pub fn find_address(
        &self,
        lat: f64,
        lon: f64,
        ring: u8,
        housenumber: String,
        street: String,
    ) -> Result<Vec<AddressRecord>, PtilesError> {
        let recs = match &self.file {
            AnyAddress::File(f) => f.find_address(lat, lon, ring, &housenumber, &street),
            AnyAddress::Http(f) => f.find_address(lat, lon, ring, &housenumber, &street),
        }
        .map_err(|e| PtilesError::Decode { message: e.to_string() })?;
        Ok(recs.into_iter().map(AddressRecord::from).collect())
    }
}

// Private: the core records, positions intact. `AddressRecord` drops lat/lon
// at the FFI boundary, and `PtilesStack::locate` needs them to measure.
impl AddressLayer {
    fn core_addresses_at(
        &self,
        lat: f64,
        lon: f64,
        ring: u8,
    ) -> Result<Vec<CoreAddressRecord>, PtilesError> {
        match &self.file {
            AnyAddress::File(f) => f.addresses_at(lat, lon, ring),
            AnyAddress::Http(f) => f.addresses_at(lat, lon, ring),
        }
        .map_err(|e| PtilesError::Decode { message: e.to_string() })
    }
}

// --- PtilesStack: cross-layer scoring for one state -------------------------

/// Groups up to one roads/buildings/business `PtilesLayer` for a single
/// state/region and scores a [`Fix`] across whichever of them are present --
/// the shape a CoreLocation caller wants: one `CLLocation`-derived `Fix` in,
/// one ranked candidate list out, without the caller re-deriving H3 cells or
/// juggling three separate decode calls per fix. Mirrors the CLI's
/// `--serve` cross-layer scoring path (`handle_serve_line`), not the
/// single-file one-shot path.
#[derive(uniffi::Object)]
pub struct PtilesStack {
    roads: Option<Arc<PtilesLayer>>,
    buildings: Option<Arc<PtilesLayer>>,
    business: Option<Arc<PtilesLayer>>,
    trails: Option<Arc<PtilesLayer>>,
    parks: Option<Arc<PtilesLayer>>,
    water: Option<Arc<PtilesLayer>>,
    camera: Option<Arc<PtilesLayer>>,
    addresses: Option<Arc<AddressLayer>>,
}

#[uniffi::export]
impl PtilesStack {
    /// The three layers [`PtilesStack::score`] needs. Unchanged signature --
    /// use [`PtilesStack::with_layers`] to add the ones
    /// [`PtilesStack::locate`] reads.
    #[uniffi::constructor]
    pub fn new(
        roads: Option<Arc<PtilesLayer>>,
        buildings: Option<Arc<PtilesLayer>>,
        business: Option<Arc<PtilesLayer>>,
    ) -> Arc<Self> {
        Arc::new(PtilesStack {
            roads,
            buildings,
            business,
            trails: None,
            parks: None,
            water: None,
            camera: None,
            addresses: None,
        })
    }

    /// Every layer this stack can use. `score` reads roads/buildings/business,
    /// `locate` reads roads/trails/addresses/parks/water; pass whichever files
    /// the region actually has and the rest stay silent.
    #[uniffi::constructor]
    pub fn with_layers(
        roads: Option<Arc<PtilesLayer>>,
        buildings: Option<Arc<PtilesLayer>>,
        business: Option<Arc<PtilesLayer>>,
        trails: Option<Arc<PtilesLayer>>,
        parks: Option<Arc<PtilesLayer>>,
        water: Option<Arc<PtilesLayer>>,
        camera: Option<Arc<PtilesLayer>>,
        addresses: Option<Arc<AddressLayer>>,
    ) -> Arc<Self> {
        Arc::new(PtilesStack {
            roads,
            buildings,
            business,
            trails,
            parks,
            water,
            camera,
            addresses,
        })
    }

    /// Indoor/outdoor estimate using this stack's building layer. A stack with
    /// no building layer returns `Uncertain/IncompleteCoverage`, not `Outdoor`.
    pub fn indoor_outdoor(
        &self,
        lat: f64,
        lon: f64,
        horizontal_accuracy_m: f64,
    ) -> Result<IndoorOutdoorEstimate, PtilesError> {
        if let Some(layer) = &self.buildings {
            return layer.indoor_outdoor(lat, lon, horizontal_accuracy_m);
        }
        let fix = CoreFix {
            lat,
            lon,
            horizontal_accuracy_m,
            speed_mps: None,
        };
        Ok(ptiles_core::estimate_indoor_outdoor(
            &fix,
            &[],
            false,
            &ptiles_core::IndoorOutdoorParams::default(),
        ).into())
    }

    /// Which cameras can see `(lat, lon)`, nearest first -- "is anything
    /// pointed at me right now".
    ///
    /// The camera layer answers who is in range and aimed at you; the
    /// buildings layer answers what stands in the way. Without a buildings
    /// layer every in-range camera reports a clear sight line, which is the
    /// honest reading of "nothing known to be in the way" and errs toward
    /// naming a camera rather than omitting one -- the direction every
    /// assumption in `core::cameras_seeing` leans. Empty when this stack
    /// holds no camera layer.
    ///
    /// `range_m <= 0` uses `ptiles_core::CAMERA_RANGE_M` (50 m).
    pub fn cameras_seeing(
        &self,
        lat: f64,
        lon: f64,
        ring: u8,
        range_m: f64,
    ) -> Result<Vec<CameraViewInfo>, PtilesError> {
        validate_ring(ring)?;
        let Some(layer) = &self.camera else {
            return Ok(Vec::new());
        };
        let cameras = layer.decoded_cameras(lat, lon, ring)?;
        let buildings: Vec<ptiles_core::ViewBuilding> = match &self.buildings {
            Some(b) => b
                .decoded_buildings(lat, lon, ring)?
                .into_iter()
                .map(|b| ptiles_core::ViewBuilding {
                    coords: b.coords,
                    height_m: b.height_m,
                    building_type: b.building_type,
                })
                .collect(),
            None => Vec::new(),
        };
        let range = if range_m > 0.0 { range_m } else { ptiles_core::CAMERA_RANGE_M };
        Ok(camera_views(lat, lon, &cameras, &buildings, range))
    }

    /// Reverse geocode across the stack: the way under the point (road and
    /// trail compete on distance alone — see `core::locate`), the nearest
    /// address, and the park/water the point falls in.
    ///
    /// Layers the stack does not hold simply do not contribute; a stack with
    /// only roads still answers, it just never reports a trail. Rail is
    /// deliberately absent: standing on a track is not a place you navigate
    /// from, and letting it win "what am I on" against the road beside it
    /// would answer confidently and wrongly. Query the rail layer directly
    /// ([`PtilesLayer::nearest_rail`]) when that is the question.
    pub fn locate(&self, lat: f64, lon: f64, ring: u8) -> Result<LocatedInfo, PtilesError> {
        validate_ring(ring)?;
        let roads = match &self.roads {
            Some(layer) => layer.decoded_roads(lat, lon, ring)?,
            None => Vec::new(),
        };
        let trails = match &self.trails {
            Some(layer) => layer.decoded_trails(lat, lon, ring)?,
            None => Vec::new(),
        };
        let addresses = match &self.addresses {
            Some(layer) => layer.core_addresses_at(lat, lon, ring)?,
            None => Vec::new(),
        };
        let located = ptiles_core::locate(lat, lon, &roads, &trails, &addresses);

        let park = match &self.parks {
            Some(layer) => {
                let parks = layer.decoded_parks(lat, lon, ring)?;
                ptiles_core::park_at(lat, lon, &parks).map(AreaInfo::from)
            }
            None => None,
        };
        let water = match &self.water {
            Some(layer) => {
                let water = layer.decoded_water(lat, lon, ring)?;
                ptiles_core::water_at(lat, lon, &water).map(AreaInfo::from)
            }
            None => None,
        };

        Ok(LocatedInfo {
            nearest_way: located.nearest_way.map(WayInfo::from),
            on_way: located.on_way.map(WayInfo::from),
            address: located.address.map(NearbyAddressInfo::from),
            park,
            water,
        })
    }

    /// Compute a bounded route entirely from installed PTiles layers.
    ///
    /// The corridor is the endpoint bounding box plus a 1.5 km-ish end cap,
    /// capped by `cells_for_bounds` at 512 H3 cells. That keeps a mistaken
    /// coast-to-coast request from turning into an unbounded download or graph.
    /// Driving uses roads. Trail mode combines pedestrian-legal roads with the
    /// trails layer, so a path can connect to a trailhead through quiet streets.
    #[allow(clippy::too_many_arguments)]
    pub fn offline_route(
        &self,
        start_lat: f64,
        start_lon: f64,
        end_lat: f64,
        end_lon: f64,
        mode: OfflineRouteMode,
        avoid_highways: bool,
        avoid_intersections: bool,
    ) -> Result<OfflineRoute, PtilesError> {
        let roads_layer = self.roads.as_ref().ok_or_else(|| PtilesError::Routing {
            message: "no roads layer is installed".to_string(),
        })?;
        let lat_span = (start_lat - end_lat).abs();
        let lon_span = (start_lon - end_lon).abs();
        let lat_margin = 0.015_f64.max(lat_span * 0.15);
        let lon_margin = 0.020_f64.max(lon_span * 0.15);
        let cells = cells_for_bounds(
            start_lat.min(end_lat) - lat_margin,
            start_lon.min(end_lon) - lon_margin,
            start_lat.max(end_lat) + lat_margin,
            start_lon.max(end_lon) + lon_margin,
        )
        .map_err(|e| PtilesError::InvalidBounds { message: e.to_string() })?;

        let mut segments = roads_layer.decoded_roads_for_cells(&cells)?;
        if mode == OfflineRouteMode::Trail {
            if let Some(trails_layer) = &self.trails {
                let trails = trails_layer.decoded_trails_for_cells(&cells)?;
                segments.extend(core_trail_segments(&trails));
            }
        }
        let mut decoded_segments = segments.len() as u32;
        let profile = match mode {
            OfflineRouteMode::Driving => RouteProfile::Driving,
            OfflineRouteMode::Trail => RouteProfile::Foot,
        };
        let prefs = RoutePrefs { profile, avoid_highways, avoid_intersections };
        let snap_radius = if mode == OfflineRouteMode::Driving { 250.0 } else { 120.0 };
        let mut attempt = route_roads_diagnostic(
            &segments,
            &[],
            start_lat,
            start_lon,
            end_lat,
            end_lon,
            snap_radius,
            prefs,
        );

        // A disconnected corridor means both ends snapped but the road joining
        // them arcs outside the box -- a river crossing or an interchange just
        // past the edge. Widening pulls it in: measured on the Tennessee pack,
        // Savannah to the midpoint of Camden goes from Disconnected to a 70.9 km
        // route this way. It only helps when the box has room left, which is why
        // the caller also splits long legs.
        if attempt == Err(ptiles_core::route_graph::RouteFailure::Disconnected) {
            if let Some(wider) = widened_corridor(
                start_lat, start_lon, end_lat, end_lon, lat_margin, lon_margin, cells.len(),
            ) {
                let mut widened = roads_layer.decoded_roads_for_cells(&wider)?;
                if mode == OfflineRouteMode::Trail {
                    if let Some(trails_layer) = &self.trails {
                        let trails = trails_layer.decoded_trails_for_cells(&wider)?;
                        widened.extend(core_trail_segments(&trails));
                    }
                }
                if widened.len() > segments.len() {
                    decoded_segments = widened.len() as u32;
                    attempt = route_roads_diagnostic(
                        &widened, &[], start_lat, start_lon, end_lat, end_lon, snap_radius, prefs,
                    );
                }
            }
        }
        let route = attempt.map_err(|e| PtilesError::Routing { message: format!("{e:?}") })?;
        Ok(OfflineRoute {
            distance_m: route.distance_m,
            duration_s: route.duration_s,
            path: route.path.into_iter().map(|p| LatLon { lat: p[0], lon: p[1] }).collect(),
            decoded_segments,
        })
    }

    /// Score `fix` against whichever layers this stack holds, at the fix's
    /// cell (plus ring-1 neighbors when `ring == 1`). Uses
    /// `ptiles_core::scoring::ScoringParams::default()` -- tunable weights
    /// aren't exposed yet; add a params record if a caller needs to retune.
    pub fn score(&self, fix: Fix, ring: u8) -> Result<Vec<Candidate>, PtilesError> {
        validate_ring(ring)?;
        let roads = match &self.roads {
            Some(layer) => layer.decoded_roads(fix.lat, fix.lon, ring)?,
            None => Vec::new(),
        };
        let buildings = match &self.buildings {
            Some(layer) => layer.decoded_buildings(fix.lat, fix.lon, ring)?,
            None => Vec::new(),
        };
        let businesses = match &self.business {
            Some(layer) => layer.decoded_business(fix.lat, fix.lon, ring)?,
            None => Vec::new(),
        };
        let core_fix = CoreFix {
            lat: fix.lat,
            lon: fix.lon,
            horizontal_accuracy_m: fix.horizontal_accuracy_m,
            speed_mps: fix.speed_mps,
        };
        let candidates =
            score_candidates(&core_fix, &roads, &buildings, &businesses, &ScoringParams::default());
        Ok(candidates.iter().map(to_candidate).collect())
    }
}

/// Corridor widenings to try on a disconnected route, widest first.
///
/// One fixed scale does not work: the cell budget follows the box *area*, so
/// 2.5x fits a 100 km north-south corridor (406 cells) and is rejected outright
/// for a 50 km diagonal (376 cells at 1x). A rejected widening is a silent
/// no-op, so this walks down until one fits.
const DISCONNECTED_RETRY_SCALES: [f64; 4] = [2.5, 2.0, 1.6, 1.3];

/// The widest corridor that still fits the cell cap and holds more cells than
/// the original. `None` when nothing fits, which means a retry would only
/// repeat the same work.
fn widened_corridor(
    start_lat: f64,
    start_lon: f64,
    end_lat: f64,
    end_lon: f64,
    lat_margin: f64,
    lon_margin: f64,
    base_cells: usize,
) -> Option<Vec<u64>> {
    DISCONNECTED_RETRY_SCALES.iter().find_map(|scale| {
        cells_for_bounds(
            start_lat.min(end_lat) - lat_margin * scale,
            start_lon.min(end_lon) - lon_margin * scale,
            start_lat.max(end_lat) + lat_margin * scale,
            start_lon.max(end_lon) + lon_margin * scale,
        )
        .ok()
        .filter(|cells| cells.len() > base_cells)
    })
}

/// One manoeuvre in a route's turn queue.
#[derive(Debug, Clone, uniffi::Record)]
pub struct TurnInfo {
    /// `depart`, `left`, `slight_right`, `u_turn`, `arrive`, and so on.
    pub maneuver: String,
    /// Signed bearing change in degrees; positive is right.
    pub delta_deg: f64,
    /// Metres from the route start to this manoeuvre.
    pub along_m: f64,
    pub location: LatLon,
    pub road_name: Option<String>,
    pub road_ref: Option<String>,
    pub road_class: Option<String>,
}

/// Where one GPS fix puts you on the route being followed.
#[derive(Debug, Clone, uniffi::Record)]
pub struct NavStateInfo {
    /// The fix pulled onto the route line.
    pub location: LatLon,
    /// How far the raw fix was from the line.
    pub offset_m: f64,
    pub along_m: f64,
    pub remaining_m: f64,
    /// Heading of the route ahead, degrees clockwise from north.
    pub bearing_deg: f64,
    /// Index into the turn queue of the next manoeuvre, if any remain.
    pub next_turn: Option<u32>,
    pub distance_to_turn_m: f64,
    /// True for this fix alone. One bad fix is not a wrong turn -- require
    /// several in a row before rerouting.
    pub off_route: bool,
}

/// A route being followed.
///
/// The path, its cumulative distances, and the turn queue stay on this side,
/// so a position update is one small call rather than re-serialising the whole
/// route at every fix.
#[derive(uniffi::Object)]
pub struct Navigator {
    path: Vec<[f64; 2]>,
    cum: Vec<f64>,
    turns: Vec<ptiles_core::nav::Turn>,
    // navigate() takes the previous snap index to keep the search local, and
    // uniffi hands out &self, so the cursor needs its own lock.
    last_index: std::sync::Mutex<usize>,
}

#[uniffi::export]
impl Navigator {
    /// `roads` only names the turns -- pass an empty list for an unnamed
    /// queue. `name_radius_m` of 0 falls back to 30 m.
    #[uniffi::constructor]
    pub fn new(path: Vec<LatLon>, roads: Vec<RoadInfo>, name_radius_m: f64) -> Arc<Self> {
        let path: Vec<[f64; 2]> = path.iter().map(|p| [p.lon, p.lat]).collect();
        let segments: Vec<ptiles_core::roads::RoadSegment> = roads
            .into_iter()
            .map(|r| ptiles_core::roads::RoadSegment {
                osm_id: r.osm_id,
                road_class: r.road_class,
                coords: r.geometry.iter().map(|p| [p.lon, p.lat]).collect(),
                name: r.name,
                ref_tag: None,
                oneway: None,
                speed_limit_kmh: None,
                lanes: None,
                surface: None,
                bridge_tunnel: None,
            })
            .collect();
        let cum = ptiles_core::nav::cumulative_m(&path);
        let radius = if name_radius_m > 0.0 { name_radius_m } else { 30.0 };
        let turns = ptiles_core::nav::turn_queue(&path, &segments, radius);
        Arc::new(Navigator { path, cum, turns, last_index: std::sync::Mutex::new(0) })
    }

    /// The queue, in order: `depart`, every manoeuvre, `arrive`.
    pub fn turns(&self) -> Vec<TurnInfo> {
        self.turns
            .iter()
            .map(|t| TurnInfo {
                maneuver: t.maneuver.as_str().to_string(),
                delta_deg: t.delta_deg,
                along_m: t.along_m,
                location: LatLon { lat: t.lat, lon: t.lon },
                road_name: t.road_name.clone(),
                road_ref: t.road_ref.clone(),
                road_class: t.road_class.clone(),
            })
            .collect()
    }

    /// None when the route is too short to follow, or the fix cannot be
    /// snapped to it at all.
    pub fn update(&self, lat: f64, lon: f64, accuracy_m: f64) -> Option<NavStateInfo> {
        let last = *self.last_index.lock().ok()?;
        let state = ptiles_core::nav::navigate(
            &self.path,
            &self.cum,
            &self.turns,
            lat,
            lon,
            accuracy_m,
            last,
        )?;
        if let Ok(mut cursor) = self.last_index.lock() {
            *cursor = state.index;
        }
        Some(NavStateInfo {
            location: LatLon { lat: state.lat, lon: state.lon },
            offset_m: state.offset_m,
            along_m: state.along_m,
            remaining_m: state.remaining_m,
            bearing_deg: state.bearing_deg,
            next_turn: state.next_turn.map(|i| i as u32),
            distance_to_turn_m: state.distance_to_turn_m,
            off_route: state.off_route,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_errors_keep_transport_semantics_across_ffi() {
        let path = "https://example.test/TN.roads.ptiles";

        let network = SourceError::HttpNetwork {
            url: path.to_string(),
            message: "offline".to_string(),
        };
        assert!(matches!(
            PtilesError::from_source(path, &network),
            PtilesError::Network { path: p, message }
                if p == path && message == "offline"
        ));

        let status = SourceError::HttpStatus {
            url: path.to_string(),
            status: 404,
            offset: 0,
            end: 255,
        };
        assert!(matches!(
            PtilesError::from_source(path, &status),
            PtilesError::NotFound { path: p, status: 404 } if p == path
        ));

        let range = SourceError::RangeNotSupported {
            url: path.to_string(),
            status: 200,
        };
        assert!(matches!(
            PtilesError::from_source(path, &range),
            PtilesError::RangeUnsupported { path: p, status: 200 } if p == path
        ));
    }

    #[test]
    fn structural_source_and_file_errors_stay_open_errors() {
        let bounds = SourceError::OutOfBounds { offset: 9, needed: 4, len: 10 };
        assert!(matches!(
            PtilesError::from_source("local.ptiles", &bounds),
            PtilesError::Open { path, message }
                if path == "local.ptiles" && message.contains("offset 9")
        ));

        let bad_magic = FileError::BadMagic { found: *b"NOTILES" };
        assert!(matches!(
            PtilesError::from_file("bad.ptiles", &bad_magic),
            PtilesError::Open { path, message }
                if path == "bad.ptiles" && message.contains("bad magic")
        ));

        let wrapped = FileError::Source(SourceError::HttpNetwork {
            url: "https://host/file".to_string(),
            message: "dns".to_string(),
        });
        assert!(matches!(
            PtilesError::from_file("https://host/file", &wrapped),
            PtilesError::Network { message, .. } if message == "dns"
        ));
    }

    #[test]
    fn layer_kind_accepts_every_published_stem_and_numeric_version_suffix() {
        let cases = [
            ("roads", LayerKind::Roads),
            ("buildings_v8", LayerKind::BuildingsV8),
            ("business", LayerKind::Business),
            ("business_name_index", LayerKind::BusinessNameIndex),
            ("trails_v1", LayerKind::Trails),
            ("parks", LayerKind::Parks),
            ("water_v1", LayerKind::Water),
            ("rail", LayerKind::Rail),
            ("camera_v12", LayerKind::Camera),
        ];
        for (stem, expected) in cases {
            let local = format!("/data/TN.{stem}.ptiles");
            let remote = format!("https://host/maps/TN.{stem}.ptiles");
            assert_eq!(LayerKind::from_path(&local), Some(expected), "{local}");
            assert_eq!(LayerKind::from_path(&remote), Some(expected), "{remote}");
            assert_eq!(expected.as_str(), match expected {
                LayerKind::Roads => "roads",
                LayerKind::BuildingsV8 => "buildings_v8",
                LayerKind::Business => "business",
                LayerKind::BusinessNameIndex => "business_name_index",
                LayerKind::Trails => "trails",
                LayerKind::Parks => "parks",
                LayerKind::Water => "water",
                LayerKind::Rail => "rail",
                LayerKind::Camera => "camera",
            });
        }
    }

    #[test]
    fn layer_kind_rejects_unknown_or_fake_version_suffixes() {
        for path in [
            "TN.signals.ptiles",
            "TN.roads_latest.ptiles",
            "TN.roads_v.ptiles",
            "TN.roads_v2_backup.ptiles",
            "roads.ptiles",
            "",
        ] {
            assert_eq!(LayerKind::from_path(path), None, "{path}");
        }
    }

    #[test]
    fn url_detection_is_strictly_http_or_https() {
        assert!(is_url("http://host/file.ptiles"));
        assert!(is_url("https://host/file.ptiles"));
        assert!(!is_url("HTTPS://host/file.ptiles"));
        assert!(!is_url("ftp://host/file.ptiles"));
        assert!(!is_url("/tmp/http://file.ptiles"));
    }

    #[test]
    fn geometry_conversion_flips_decoder_lon_lat_to_ffi_lat_lon() {
        let geometry = geometry_of(&[[-86.80, 36.10], [-86.79, 36.11]]);
        assert_eq!(geometry.len(), 2);
        assert_eq!(geometry[0].lat, 36.10);
        assert_eq!(geometry[0].lon, -86.80);
        assert_eq!(geometry[1].lat, 36.11);
        assert_eq!(geometry[1].lon, -86.79);
        assert!(geometry_of(&[]).is_empty());
    }

    #[test]
    fn candidate_conversion_preserves_values_and_maps_every_kind() {
        let cases = [
            (CoreCandidateKind::Road, CandidateKind::Road),
            (CoreCandidateKind::Building, CandidateKind::Building),
            (CoreCandidateKind::Business, CandidateKind::Business),
        ];
        for (core_kind, ffi_kind) in cases {
            let core = CoreCandidate {
                kind: core_kind,
                osm_id: -42,
                name: Some("place".to_string()),
                distance_m: 3.5,
                score: 0.75,
            };
            let got = to_candidate(&core);
            assert_eq!(got.kind, ffi_kind);
            assert_eq!(got.osm_id, -42);
            assert_eq!(got.name.as_deref(), Some("place"));
            assert_eq!(got.distance_m, 3.5);
            assert_eq!(got.score, 0.75);
        }
    }

    #[test]
    fn grouping_points_by_cell_preserves_first_seen_group_and_point_order() {
        let points = [
            LatLon { lat: 36.1600, lon: -86.7800 },
            LatLon { lat: 35.7800, lon: -78.6400 },
            LatLon { lat: 36.1601, lon: -86.7801 },
            LatLon { lat: 35.7801, lon: -78.6401 },
        ];
        let groups = PtilesLayer::group_by_cell(&points);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].1, vec![0, 2]);
        assert_eq!(groups[1].1, vec![1, 3]);
        assert_eq!(groups[0].0, cell_for_coord(points[0].lat, points[0].lon));
        assert_eq!(groups[1].0, cell_for_coord(points[1].lat, points[1].lon));
        assert!(PtilesLayer::group_by_cell(&[]).is_empty());
    }

    fn margins(start: (f64, f64), end: (f64, f64)) -> (f64, f64) {
        let lat_span = (start.0 - end.0).abs();
        let lon_span = (start.1 - end.1).abs();
        (0.015_f64.max(lat_span * 0.15), 0.020_f64.max(lon_span * 0.15))
    }

    fn base_cells(start: (f64, f64), end: (f64, f64)) -> usize {
        let (lat_margin, lon_margin) = margins(start, end);
        cells_for_bounds(
            start.0.min(end.0) - lat_margin,
            start.1.min(end.1) - lon_margin,
            start.0.max(end.0) + lat_margin,
            start.1.max(end.1) + lon_margin,
        )
        .expect("the base corridor must fit")
        .len()
    }

    #[test]
    fn a_short_route_widens_by_the_full_step() {
        let start = (35.0, -88.0);
        let end = (35.45, -88.0);
        let (lat_margin, lon_margin) = margins(start, end);
        let base = base_cells(start, end);

        let wider = widened_corridor(start.0, start.1, end.0, end.1, lat_margin, lon_margin, base)
            .expect("a short route has room to widen");

        assert!(wider.len() > base);
    }

    #[test]
    fn a_diagonal_route_widens_after_the_widest_step_is_rejected() {
        // ~50 km diagonal: 376 cells at 1x, and 2.5x blows the 512-cell cap.
        // A single fixed scale gave up here and left the route disconnected.
        let start = (35.0, -88.0);
        let end = (35.315, -87.685);
        let (lat_margin, lon_margin) = margins(start, end);
        let base = base_cells(start, end);

        assert!(
            cells_for_bounds(
                start.0 - lat_margin * 2.5,
                start.1 - lon_margin * 2.5,
                end.0 + lat_margin * 2.5,
                end.1 + lon_margin * 2.5,
            )
            .is_err(),
            "this case exists because the widest step is rejected",
        );

        let wider = widened_corridor(start.0, start.1, end.0, end.1, lat_margin, lon_margin, base)
            .expect("a smaller widening must still be found");
        assert!(wider.len() > base);
        assert!(wider.len() <= ptiles_core::MAX_BOUNDS_CELLS);
    }

    #[test]
    fn a_route_with_no_room_left_reports_no_widening() {
        let start = (35.0, -88.0);
        let end = (36.4, -86.6);
        let (lat_margin, lon_margin) = margins(start, end);

        assert!(widened_corridor(start.0, start.1, end.0, end.1, lat_margin, lon_margin, 1).is_none());
    }

    #[test]
    fn a_navigator_walks_an_l_shaped_route() {
        // East for ~900 m, then north for ~1.1 km: one left turn.
        let path = vec![
            LatLon { lat: 35.0, lon: -88.0 },
            LatLon { lat: 35.0, lon: -87.99 },
            LatLon { lat: 35.01, lon: -87.99 },
        ];
        let nav = Navigator::new(path, Vec::new(), 0.0);
        let turns = nav.turns();

        assert_eq!(turns.first().unwrap().maneuver, "depart");
        assert_eq!(turns.last().unwrap().maneuver, "arrive");
        assert!(turns.iter().any(|t| t.maneuver == "left"), "{turns:?}");

        // Halfway down the first leg: on route, with distance still to run.
        let state = nav.update(35.0, -87.995, 5.0).expect("a fix on the line snaps");
        assert!(!state.off_route);
        assert!(state.remaining_m > 900.0, "{}", state.remaining_m);
        assert!(state.distance_to_turn_m > 0.0);
    }

    #[test]
    fn a_fix_far_from_the_line_reads_as_off_route() {
        let path = vec![
            LatLon { lat: 35.0, lon: -88.0 },
            LatLon { lat: 35.0, lon: -87.99 },
        ];
        let nav = Navigator::new(path, Vec::new(), 0.0);

        // ~1.1 km north of the route, with a good fix: nothing to blame it on.
        let state = nav.update(35.01, -87.995, 5.0).expect("still snaps, just far");
        assert!(state.off_route);
        assert!(state.offset_m > 500.0, "{}", state.offset_m);
    }

    #[test]
    fn ring_validation_reports_the_rejected_value() {
        assert!(validate_ring(0).is_ok());
        assert!(validate_ring(1).is_ok());
        assert!(matches!(validate_ring(2), Err(PtilesError::InvalidRing { ring: 2 })));
        assert!(matches!(validate_ring(u8::MAX), Err(PtilesError::InvalidRing { ring: 255 })));
    }
}
