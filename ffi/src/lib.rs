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
    cell_center, cell_for_coord, decode_buildings, decode_business_versioned, decode_road_block,
    decode_roads, haversine_distance_m, nearest_intersection as core_nearest_intersection,
    decode_trails, nearest_road as core_nearest_road, nearest_trail as core_nearest_trail,
    neighbor_cells, score_candidates, search_business_indexed, trail_is_developed, TrailFeature,
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

/// The trail a point is on or nearest to.
///
/// `osm_id` is i64 here, not u64 like `NearestRoad`: the trails decoder carries
/// the id as a signed delta and OSM ids for derived ways can be negative, so
/// widening it to unsigned would corrupt exactly those records.
#[derive(Debug, Clone, uniffi::Record)]
pub struct NearestTrail {
    pub osm_id: i64,
    pub name: Option<String>,
    /// OSM `highway` value for the trail: `path`, `track`, `footway`, ...
    pub trail_type: String,
    /// Tread surface: `dirt`, `gravel`, `paved`, ... Empty when untagged.
    pub surface: String,
    /// SAC hiking difficulty when tagged (`hiking`, `mountain_hiking`, ...).
    pub sac_scale: String,
    /// True for a made trail rather than a desire path -- what
    /// `core::trail_is_developed` decides, kept here so callers do not
    /// re-implement the classification from `trail_type`.
    pub developed: bool,
    pub snapped_lat: f64,
    pub snapped_lon: f64,
    pub distance_m: f64,
    /// True when the point is close enough to be ON the trail rather than
    /// merely near it, using core's own threshold.
    pub on_it: bool,
    pub geometry: Vec<LatLon>,
}

/// One trail feature, as stored. Unlike [`NearestTrail`] this includes
/// trailhead points, which carry a single coordinate.
#[derive(Debug, Clone, uniffi::Record)]
pub struct TrailInfo {
    pub osm_id: i64,
    pub name: Option<String>,
    pub trail_type: String,
    pub surface: String,
    pub sac_scale: String,
    pub developed: bool,
    /// True for a trailhead marker rather than a length of trail.
    pub is_trailhead: bool,
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
    /// `{STATE}.trails_v1.ptiles` -- paths, tracks and trailheads. A separate
    /// kind from Roads because the record framing differs (see `core::trails`),
    /// even though both answer "which way am I on".
    TrailsV1,
    BuildingsV8,
    Business,
    /// `{STATE}.business_name_index.ptiles` -- the name-search sidecar, see
    /// `core::business_search`. Separate from `Business` since it's a
    /// different file with a different (first-letter-bucket) index shape,
    /// only usable via `PtilesLayer::search_business`.
    BusinessNameIndex,
}

impl LayerKind {
    fn from_path(path: &str) -> Option<LayerKind> {
        let name = std::path::Path::new(path).file_name()?.to_str()?;
        let mut parts = name.split('.');
        let _state = parts.next()?;
        match parts.next()? {
            "roads" => Some(LayerKind::Roads),
            "trails_v1" => Some(LayerKind::TrailsV1),
            "buildings_v8" => Some(LayerKind::BuildingsV8),
            "business" => Some(LayerKind::Business),
            "business_name_index" => Some(LayerKind::BusinessNameIndex),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            LayerKind::Roads => "roads",
            LayerKind::TrailsV1 => "trails_v1",
            LayerKind::BuildingsV8 => "buildings_v8",
            LayerKind::Business => "business",
            LayerKind::BusinessNameIndex => "business_name_index",
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

    fn decoded_trails(&self, lat: f64, lon: f64, ring: u8) -> Result<Vec<TrailFeature>, PtilesError> {
        let mut trails = Vec::new();
        for cell in self.cells_for(lat, lon, ring) {
            let Some(block) = self.block(cell)? else {
                continue;
            };
            let mut t = decode_trails(&block).map_err(|e| PtilesError::Decode {
                message: e.to_string(),
            })?;
            trails.append(&mut t);
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
        .find(|b| point_in_polygon(lon, lat, &b.coords))
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

    /// The trail the point is on or nearest to, or `None` beyond the search
    /// radius. Trails-layer only.
    ///
    /// Trailhead POINTS are skipped by `core::nearest_trail`, deliberately:
    /// this answers "which trail am I walking on", and the nearest thing to a
    /// walker on a path is the path, not the sign at its entrance.
    ///
    /// Searches ring-1 like `nearest_intersection` rather than ring-0 like
    /// `nearest_road`. A trail is a long thin feature that routinely runs along
    /// a cell edge for its whole length, so a ring-0 answer would miss the trail
    /// underfoot whenever the walker is on the far side of the boundary.
    pub fn nearest_trail(&self, lat: f64, lon: f64) -> Result<Option<NearestTrail>, PtilesError> {
        if self.kind != LayerKind::TrailsV1 {
            return Err(PtilesError::UnsupportedForLayer {
                layer: self.kind.as_str().to_string(),
            });
        }
        let trails = self.decoded_trails(lat, lon, 1)?;
        let Some(way) = core_nearest_trail(lat, lon, &trails) else {
            return Ok(None);
        };
        let trail = &trails[way.index];
        Ok(Some(NearestTrail {
            osm_id: trail.osm_id,
            name: way.name.clone(),
            trail_type: way.class.clone(),
            surface: trail.surface.clone(),
            sac_scale: trail.sac_scale.clone(),
            developed: trail_is_developed(&trail.trail_type),
            snapped_lat: way.snapped.0,
            snapped_lon: way.snapped.1,
            distance_m: way.distance_m,
            on_it: way.on_it,
            geometry: geometry_of(&trail.coords),
        }))
    }

    /// Every trail feature in the cell containing `(lat, lon)`, plus ring-1
    /// neighbours when `ring == 1`. Includes trailhead points, which
    /// `nearest_trail` skips -- a caller drawing a map wants them.
    /// Trails-layer only.
    pub fn trails(&self, lat: f64, lon: f64, ring: u8) -> Result<Vec<TrailInfo>, PtilesError> {
        if self.kind != LayerKind::TrailsV1 {
            return Err(PtilesError::UnsupportedForLayer {
                layer: self.kind.as_str().to_string(),
            });
        }
        if ring > 1 {
            return Err(PtilesError::InvalidRing { ring });
        }
        Ok(self
            .decoded_trails(lat, lon, ring)?
            .into_iter()
            .map(|t| TrailInfo {
                osm_id: t.osm_id,
                name: t.name.clone(),
                trail_type: t.trail_type.clone(),
                surface: t.surface.clone(),
                sac_scale: t.sac_scale.clone(),
                developed: trail_is_developed(&t.trail_type),
                is_trailhead: t.geom_type == 1,
                geometry: geometry_of(&t.coords),
            })
            .collect())
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

/// Ray-casting point-in-polygon, `(lon, lat)`-ordered ring -- copied from
/// `cli/src/main.rs::point_in_polygon` (core intentionally has no
/// polygon-containment helper; see that function's doc comment).
fn point_in_polygon(x: f64, y: f64, coords: &[[f64; 2]]) -> bool {
    if coords.len() < 3 {
        return false;
    }
    let mut inside = false;
    let n = coords.len();
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (coords[i][0], coords[i][1]);
        let (xj, yj) = (coords[j][0], coords[j][1]);
        if ((yi > y) != (yj > y)) && (x < (xj - xi) * (y - yi) / (yj - yi) + xi) {
            inside = !inside;
        }
        j = i;
    }
    inside
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
        let recs = match &self.file {
            AnyAddress::File(f) => f.addresses_at(lat, lon, ring),
            AnyAddress::Http(f) => f.addresses_at(lat, lon, ring),
        }
        .map_err(|e| PtilesError::Decode { message: e.to_string() })?;
        Ok(recs.into_iter().map(AddressRecord::from).collect())
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
}

#[uniffi::export]
impl PtilesStack {
    #[uniffi::constructor]
    pub fn new(
        roads: Option<Arc<PtilesLayer>>,
        buildings: Option<Arc<PtilesLayer>>,
        business: Option<Arc<PtilesLayer>>,
    ) -> Arc<Self> {
        Arc::new(PtilesStack { roads, buildings, business })
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
