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
    cell_center, cell_for_coord, decode_buildings, decode_business, decode_road_block,
    decode_roads, haversine_distance_m, nearest_intersection as core_nearest_intersection,
    nearest_road as core_nearest_road, neighbor_cells, score_candidates, search_business_indexed,
    Building, Business, Candidate as CoreCandidate, CandidateKind as CoreCandidateKind, FileSource,
    Fix as CoreFix, HttpSource, Intersection, PtilesFile, RoadSegment, ScoringParams,
};
use ptiles_core::{AdminFile as CoreAdminFile, AdminInfo as CoreAdminInfo};
use ptiles_core::{AddressFile as CoreAddressFile, AddressRecord as CoreAddressRecord};

uniffi::setup_scaffolding!();

// --- Errors -----------------------------------------------------------------

/// Flat UniFFI error enum. Every error path anywhere in this crate collapses
/// into one of these variants; wrapped source errors are stringified
/// (`{0}`/`{message}`) rather than exposed as nested UniFFI error types,
/// which keeps the generated Swift/Kotlin error surface small and stable.
#[derive(Debug, thiserror::Error, uniffi::Error)]
#[uniffi(flat_error)]
pub enum PtilesError {
    #[error("failed to open {path}: {message}")]
    Open { path: String, message: String },
    #[error("could not infer layer from filename {path:?} (expected <state>.<layer>.ptiles)")]
    UnknownLayer { path: String },
    #[error("block decode failed: {message}")]
    Decode { message: String },
    #[error("this operation is not supported on a {layer} layer")]
    UnsupportedForLayer { layer: String },
    #[error("ring {ring} not supported (only 0 or 1)")]
    InvalidRing { ring: u8 },
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
            "buildings_v8" => Some(LayerKind::BuildingsV8),
            "business" => Some(LayerKind::Business),
            "business_name_index" => Some(LayerKind::BusinessNameIndex),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            LayerKind::Roads => "roads",
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
    fn read_block(&self, cell: u64) -> Result<Option<Vec<u8>>, PtilesError> {
        let result = match self {
            AnyFile::File(f) => f.read_block(cell).map_err(|e| e.to_string()),
            AnyFile::Http(f) => f.read_block(cell).map_err(|e| e.to_string()),
        };
        result.map_err(|message| PtilesError::Decode { message })
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

/// One opened `.ptiles` file (local path or `http(s)://` URL), its layer
/// inferred from the filename (`<state>.<layer>.ptiles`), wrapping
/// `AnyFile` -- see that type's doc comment for why this isn't
/// `PtilesFile<FileSource>` directly anymore.
#[derive(uniffi::Object)]
pub struct PtilesLayer {
    kind: LayerKind,
    file: AnyFile,
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
        let file = if is_url(&path) {
            let source = HttpSource::open(&path).map_err(|e| PtilesError::Open {
                path: path.clone(),
                message: e.to_string(),
            })?;
            AnyFile::Http(PtilesFile::open(source).map_err(|e| PtilesError::Open {
                path,
                message: e.to_string(),
            })?)
        } else {
            let source = FileSource::open(&path).map_err(|e| PtilesError::Open {
                path: path.clone(),
                message: e.to_string(),
            })?;
            AnyFile::File(PtilesFile::open(source).map_err(|e| PtilesError::Open {
                path,
                message: e.to_string(),
            })?)
        };
        Ok(Arc::new(PtilesLayer { kind, file }))
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

    fn decoded_roads(&self, lat: f64, lon: f64, ring: u8) -> Result<Vec<RoadSegment>, PtilesError> {
        let mut roads = Vec::new();
        for cell in self.cells_for(lat, lon, ring) {
            let Some(block) = self.file.read_block(cell)? else {
                continue;
            };
            let mut r = decode_roads(&block).map_err(|e| PtilesError::Decode {
                message: e.to_string(),
            })?;
            roads.append(&mut r);
        }
        Ok(roads)
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
            let Some(block) = self.file.read_block(cell)? else {
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
            let Some(block) = self.file.read_block(cell)? else {
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
        let mut businesses = Vec::new();
        for cell in self.cells_for(lat, lon, ring) {
            let Some(block) = self.file.read_block(cell)? else {
                continue;
            };
            let mut b = decode_business(&block).map_err(|e| PtilesError::Decode {
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

#[uniffi::export]
impl PtilesLayer {
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
        let found = buildings
            .iter()
            .find(|b| point_in_polygon(lon, lat, &b.coords))
            .or_else(|| {
                buildings
                    .iter()
                    .map(|b| (b, haversine_distance_m(lat, lon, b.centroid_lat, b.centroid_lon)))
                    .filter(|(_, d)| *d <= 50.0)
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(b, _)| b)
            });
        Ok(found.map(|b| BuildingInfo {
            osm_id: b.osm_id,
            name: b.name.clone(),
            building_type: b.building_type.clone(),
            category: b.category.clone(),
            centroid: LatLon { lat: b.centroid_lat, lon: b.centroid_lon },
        }))
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
        let open_err = |e: String| PtilesError::Open { path: path.clone(), message: e };
        let file = if is_url(&path) {
            let source = HttpSource::open(&path).map_err(|e| open_err(e.to_string()))?;
            CoreAdminFile::open(source).map_err(|e| open_err(e.to_string()))?
        } else {
            let source = FileSource::open(&path).map_err(|e| open_err(e.to_string()))?;
            CoreAdminFile::open(source).map_err(|e| open_err(e.to_string()))?
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
        let open_err = |e: String| PtilesError::Open { path: path.clone(), message: e };
        let file = if is_url(&path) {
            let s = HttpSource::open(&path).map_err(|e| open_err(e.to_string()))?;
            AnyAddress::Http(CoreAddressFile::open(s).map_err(|e| open_err(e.to_string()))?)
        } else {
            let s = FileSource::open(&path).map_err(|e| open_err(e.to_string()))?;
            AnyAddress::File(CoreAddressFile::open(s).map_err(|e| open_err(e.to_string()))?)
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
