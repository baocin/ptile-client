//! ptiles-cli: rookery bridge over ptiles-core.
//!
//! Local and remote files: `--path` (one-shot) and per-layer files under
//! `--data-dir`/`--remote-base` (serve) accept either a filesystem path or an
//! `http(s)://` URL -- picked by a scheme sniff (`is_url`), matching
//! `ptiles-core`'s `FileSource`/`HttpSource` split
//! (`~/.hermes/plans/ptiles-client-extraction-plan.md`, Addendum 2 item 1).
//!
//! Modes:
//! - one-shot: `--path <file.ptiles|https://.../file.ptiles> --lat <f64> --lon <f64> [--query roads|buildings|business|all] [--ring 1]`
//!   Opens a single `.ptiles` file (local or remote), resolves the H3 res-7
//!   cell for the point (plus ring-1 neighbors if `--ring 1`), decodes the
//!   block(s) with the decoder matching the file's layer (inferred from its
//!   `<state>.<layer>.ptiles` filename), and prints one JSON object to stdout.
//! - `--serve --data-dir <dir>`: pre-opens every `*.ptiles` file under `dir`
//!   (grouped by state + layer parsed from the filename), then reads JSON
//!   lines from stdin. `--serve --remote-base <https://host/path/> --states
//!   TN,US`: same, but for each state and each of the three queried layers
//!   (`roads`, `buildings_v8`, `business`) opens
//!   `<remote_base><state>.<layer>.ptiles` over HTTP instead of scanning a
//!   local directory -- a state/layer combination that 404s or errors is
//!   skipped (eprintln), not fatal, since not every state has every layer.
//!   `--serve` accepts either `--data-dir` or `--remote-base` (not both).
//!
//!   `--serve` JSON lines:
//!   `{"lat":..,"lon":..,"query":"building|road|roads|business|all","state":?,
//!   "ring":0|1,"accuracy_m":?,"speed_mps":?}`.
//!   `state` is optional; if omitted, the sole state present in the data dir
//!   is used, or an `{"error":...}` line if more than one state is loaded.
//!   `ring` defaults to 0 (center cell only); 1 includes the H3 ring-1
//!   neighbors; anything else is rejected with an `{"error":...}` line.
//!   `"query":"roads"` returns every decoded road segment in the query
//!   cell(s) under `"roads"` (vs. `"road"`, which returns only the
//!   nearest-road match under `"nearest_road"`, same as before).
//!   When `accuracy_m` is present, the response includes `"candidates"`:
//!   ranked GPS-fix scoring output (see `ptiles_core::scoring`) built from
//!   whichever of roads/buildings/business this state has loaded.
//!   Responds with one JSON line per request:
//!   `{"building":..|null,"nearest_road":{..}|null,"business":[..],"roads":?,"candidates":?}`.
//!   Malformed input or per-query decode failures produce `{"error":"..."}`
//!   lines -- the serve loop never crashes on bad input.

use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use ptiles_core::{
    cell_center, cell_for_coord, decode_buildings, decode_business, decode_roads, nearest_road,
    score_candidates, Building, Business, Candidate, CandidateKind, FileSource, Fix, HttpSource,
    PtilesFile, RoadSegment, ScoringParams,
};
use serde_json::{json, Value};

/// The layer a `.ptiles` file holds, inferred from its filename
/// (`<state>.<layer>.ptiles`). Only the three layers this CLI queries are
/// modeled -- water/parks/rail/places files are ignored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Layer {
    Roads,
    BuildingsV8,
    Business,
}

impl Layer {
    fn from_filename_token(token: &str) -> Option<Layer> {
        match token {
            "roads" => Some(Layer::Roads),
            "buildings_v8" => Some(Layer::BuildingsV8),
            "business" => Some(Layer::Business),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Layer::Roads => "roads",
            Layer::BuildingsV8 => "buildings_v8",
            Layer::Business => "business",
        }
    }
}

/// Query kinds accepted on `--query` / the `"query"` JSON field.
///
/// `Road` ("road") is the singular nearest-road-to-point lookup. `Roads`
/// ("roads") is the plan-addendum bulk query: every decoded segment in the
/// containing cell (plus ring-1 neighbors when requested).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QueryKind {
    Road,
    Roads,
    Buildings,
    Business,
    All,
}

impl QueryKind {
    fn parse(s: &str) -> Option<QueryKind> {
        match s {
            "road" => Some(QueryKind::Road),
            "roads" => Some(QueryKind::Roads),
            "building" | "buildings" => Some(QueryKind::Buildings),
            "business" => Some(QueryKind::Business),
            "all" => Some(QueryKind::All),
            _ => None,
        }
    }

    fn wants(self, layer: Layer) -> bool {
        match self {
            QueryKind::All => true,
            QueryKind::Road | QueryKind::Roads => layer == Layer::Roads,
            QueryKind::Buildings => layer == Layer::BuildingsV8,
            QueryKind::Business => layer == Layer::Business,
        }
    }
}

fn main() {
    let mut args = pico_args::Arguments::from_env();

    if args.contains("--supported-formats") {
        print!("{}", ptiles_core::supported_formats());
        return;
    }

    if args.contains("--serve") {
        let remote_base: Option<String> = args.opt_value_from_str("--remote-base").unwrap_or(None);
        if let Some(remote_base) = remote_base {
            let states: String = args.opt_value_from_str("--states").unwrap_or(None).unwrap_or_else(|| {
                eprintln!("ptiles-cli --serve --remote-base: --states TN,US,... is required");
                std::process::exit(2);
            });
            run_serve_remote(&remote_base, &states);
        } else {
            let data_dir: PathBuf = args
                .opt_value_from_str("--data-dir")
                .unwrap_or(None)
                .unwrap_or_else(|| PathBuf::from("/home/aoi/kino/data/ptiles"));
            run_serve(&data_dir);
        }
        return;
    }

    let path: String = match args.value_from_str("--path") {
        Ok(p) => p,
        Err(e) => {
            eprintln!("ptiles-cli: --path is required for one-shot mode ({e})");
            std::process::exit(2);
        }
    };
    let lat: f64 = args.value_from_str("--lat").unwrap_or_else(|e| {
        eprintln!("ptiles-cli: --lat is required ({e})");
        std::process::exit(2);
    });
    let lon: f64 = args.value_from_str("--lon").unwrap_or_else(|e| {
        eprintln!("ptiles-cli: --lon is required ({e})");
        std::process::exit(2);
    });
    let query: Option<String> = args.opt_value_from_str("--query").unwrap_or(None);
    let ring: u32 = args.opt_value_from_str("--ring").unwrap_or(None).unwrap_or(0);
    let accuracy_m: Option<f64> = args.opt_value_from_str("--accuracy-m").unwrap_or(None);
    let speed_mps: Option<f64> = args.opt_value_from_str("--speed-mps").unwrap_or(None);

    if let Err(e) = validate_ring(ring) {
        println!("{}", serde_json::to_string_pretty(&json!({"error": e})).unwrap());
        std::process::exit(1);
    }

    let query_kind = match query.as_deref() {
        Some(s) => match QueryKind::parse(s) {
            Some(q) => q,
            None => {
                eprintln!("ptiles-cli: unknown --query {s:?} (expected roads|buildings|business|all)");
                std::process::exit(2);
            }
        },
        None => QueryKind::All,
    };

    let layer = match layer_from_path(&path) {
        Some(l) => l,
        None => {
            eprintln!(
                "ptiles-cli: could not infer layer from filename {path:?} (expected <state>.<layer>.ptiles)"
            );
            std::process::exit(2);
        }
    };

    if !query_kind.wants(layer) {
        eprintln!(
            "ptiles-cli: --query {:?} does not match this file's layer ({})",
            query_kind,
            layer.as_str()
        );
        std::process::exit(2);
    }

    let opened = match OpenedLayer::open(&path, layer) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("ptiles-cli: failed to open {path:?}: {e}");
            std::process::exit(1);
        }
    };

    let mut result = opened.query(lat, lon, ring, query_kind);

    if let Some(accuracy_m) = accuracy_m {
        // Scoring only has real signal against roads/buildings/business
        // together; a one-shot query is scoped to a single layer's file, so
        // scan just that layer's decoded candidates for this fix. (--serve
        // scores across all three layers -- see handle_serve_line.)
        let fix = Fix { lat, lon, horizontal_accuracy_m: accuracy_m, speed_mps };
        let (roads, buildings, businesses) = opened.candidates_for(lat, lon, ring);
        let candidates = score_candidates(&fix, &roads, &buildings, &businesses, &ScoringParams::default());
        if let Value::Object(ref mut map) = result {
            map.insert("candidates".to_string(), candidates_json(&candidates));
        }
    }

    println!("{}", serde_json::to_string_pretty(&result).unwrap());
}

/// Ring is opt-in and center-cell-default per the plan addendum; only 0 or 1
/// are supported (ring-1 neighbors), so reject anything larger explicitly
/// rather than silently truncating.
fn validate_ring(ring: u32) -> Result<(), String> {
    if ring > 1 {
        Err(format!("ring {ring} not supported (only 0 or 1)"))
    } else {
        Ok(())
    }
}

fn candidates_json(candidates: &[Candidate]) -> Value {
    Value::Array(
        candidates
            .iter()
            .map(|c| {
                let kind = match c.kind {
                    CandidateKind::Road => "road",
                    CandidateKind::Building => "building",
                    CandidateKind::Business => "business",
                };
                json!({
                    "kind": kind,
                    "osm_id": c.osm_id,
                    "name": c.name,
                    "distance_m": c.distance_m,
                    "score": c.score,
                })
            })
            .collect(),
    )
}

/// True for `http://`/`https://` -- the scheme sniff that picks `HttpSource`
/// vs. `FileSource` everywhere this CLI opens a `.ptiles` file (one-shot
/// `--path`, `--serve --data-dir`/`--remote-base` per-layer files).
fn is_url(s: &str) -> bool {
    s.starts_with("http://") || s.starts_with("https://")
}

/// Infer the `<state>.<layer>.ptiles` layer token from a local path or a
/// URL's final path segment.
fn layer_from_path(path_or_url: &str) -> Option<Layer> {
    let name = if is_url(path_or_url) {
        path_or_url.rsplit('/').next()?
    } else {
        Path::new(path_or_url).file_name()?.to_str()?
    };
    let mut parts = name.split('.');
    let _state = parts.next()?;
    let layer_token = parts.next()?;
    Layer::from_filename_token(layer_token)
}

/// `PtilesFile` over either a local file or an HTTP(S) URL. `PtilesFile<S>`
/// is generic over its source, but this CLI needs one concrete type it can
/// store in `OpenedLayer`/`StateFiles` uniformly, so this enum picks the
/// backend at open time (scheme sniff) and forwards the two calls
/// (`read_block`, `index`) `OpenedLayer` needs.
enum AnyFile {
    File(PtilesFile<FileSource>),
    Http(PtilesFile<HttpSource>),
}

impl AnyFile {
    fn open(path_or_url: &str) -> Result<AnyFile, String> {
        if is_url(path_or_url) {
            let source = HttpSource::open(path_or_url).map_err(|e| format!("open: {e}"))?;
            let file = PtilesFile::open(source).map_err(|e| format!("parse header/index: {e}"))?;
            Ok(AnyFile::Http(file))
        } else {
            let source = FileSource::open(path_or_url).map_err(|e| format!("open: {e}"))?;
            let file = PtilesFile::open(source).map_err(|e| format!("parse header/index: {e}"))?;
            Ok(AnyFile::File(file))
        }
    }

    fn read_block(&self, cell: u64) -> Result<Option<Vec<u8>>, String> {
        match self {
            AnyFile::File(f) => f.read_block(cell).map_err(|e| e.to_string()),
            AnyFile::Http(f) => f.read_block(cell).map_err(|e| e.to_string()),
        }
    }

}

/// One opened `.ptiles` file (local or remote) plus the metadata needed to
/// decode its blocks and answer queries against it. `PtilesFile` handles
/// both absolute and relative block offsets (detected per-file in
/// `PtilesFile::open`), so no per-layer backend distinction is needed beyond
/// the local-vs-HTTP split in `AnyFile`.
struct OpenedLayer {
    layer: Layer,
    file: AnyFile,
}

impl OpenedLayer {
    fn open(path_or_url: &str, layer: Layer) -> Result<OpenedLayer, String> {
        let file = AnyFile::open(path_or_url)?;
        Ok(OpenedLayer { layer, file })
    }

    fn read_block(&self, cell: u64) -> Option<Vec<u8>> {
        self.file.read_block(cell).ok().flatten()
    }

    /// Cells to fetch for a query point: the center cell, plus ring-1
    /// neighbors when `ring >= 1` (per the plan's addendum: ring is opt-in,
    /// default is center-cell-only).
    fn cells_for(&self, lat: f64, lon: f64, ring: u32) -> Vec<u64> {
        let center = cell_for_coord(lat, lon);
        let mut cells = vec![center];
        if ring >= 1 {
            cells.extend(ptiles_core::neighbor_cells(center));
        }
        cells
    }

    fn blocks_for(&self, cells: &[u64]) -> Vec<Vec<u8>> {
        cells.iter().filter_map(|&c| self.read_block(c)).collect()
    }

    /// Decode this layer's blocks for the query cells (center + ring-1 if
    /// requested), returning `(roads, buildings, businesses)` -- exactly one
    /// of the three is populated, matching `self.layer`. Used to feed
    /// `score_candidates` for one-shot `--accuracy-m` requests.
    fn candidates_for(
        &self,
        lat: f64,
        lon: f64,
        ring: u32,
    ) -> (Vec<RoadSegment>, Vec<Building>, Vec<Business>) {
        let cells = self.cells_for(lat, lon, ring);
        let mut roads = Vec::new();
        let mut buildings = Vec::new();
        let mut businesses = Vec::new();
        match self.layer {
            Layer::Roads => {
                for block in self.blocks_for(&cells) {
                    if let Ok(mut r) = decode_roads(&block) {
                        roads.append(&mut r);
                    }
                }
            }
            Layer::BuildingsV8 => {
                for &cell in &cells {
                    let Some(block) = self.read_block(cell) else { continue };
                    let (center_lat, center_lon) = cell_center(cell);
                    if let Ok(mut b) = decode_buildings(&block, center_lat, center_lon) {
                        buildings.append(&mut b);
                    }
                }
            }
            Layer::Business => {
                for block in self.blocks_for(&cells) {
                    if let Ok(mut b) = decode_business(&block) {
                        businesses.append(&mut b);
                    }
                }
            }
        }
        (roads, buildings, businesses)
    }

    fn query(&self, lat: f64, lon: f64, ring: u32, query_kind: QueryKind) -> Value {
        let cells = self.cells_for(lat, lon, ring);

        match self.layer {
            Layer::Roads => {
                let blocks = self.blocks_for(&cells);
                let mut roads: Vec<RoadSegment> = Vec::new();
                for block in &blocks {
                    match decode_roads(block) {
                        Ok(mut r) => roads.append(&mut r),
                        Err(e) => return json!({"error": format!("decode_roads: {e}")}),
                    }
                }
                if query_kind == QueryKind::Roads {
                    let segments: Vec<Value> = roads.iter().map(road_segment_json).collect();
                    return json!({"roads": segments, "candidate_count": roads.len()});
                }
                let nearest = nearest_road(lat, lon, &roads, ptiles_core::DEFAULT_THRESHOLD_M * 2.0)
                    .map(|nr| nearest_road_json(&nr, &roads));
                json!({"nearest_road": nearest, "candidate_count": roads.len()})
            }
            Layer::BuildingsV8 => {
                let mut buildings: Vec<Building> = Vec::new();
                for &cell in &cells {
                    let Some(block) = self.read_block(cell) else {
                        continue;
                    };
                    let (center_lat, center_lon) = cell_center(cell);
                    match decode_buildings(&block, center_lat, center_lon) {
                        Ok(mut b) => buildings.append(&mut b),
                        Err(e) => return json!({"error": format!("decode_buildings: {e}")}),
                    }
                }
                let building = find_building(lat, lon, &buildings).map(building_json);
                json!({"building": building, "candidate_count": buildings.len()})
            }
            Layer::Business => {
                let blocks = self.blocks_for(&cells);
                let mut businesses: Vec<Business> = Vec::new();
                for block in &blocks {
                    match decode_business(block) {
                        Ok(mut b) => businesses.append(&mut b),
                        Err(e) => return json!({"error": format!("decode_business: {e}")}),
                    }
                }
                let nearby: Vec<Value> = businesses
                    .iter()
                    .filter(|b| ptiles_core::haversine_distance_m(lat, lon, b.lat, b.lon) <= 200.0)
                    .map(business_json)
                    .collect();
                json!({"business": nearby, "candidate_count": businesses.len()})
            }
        }
    }
}

/// Find the building whose polygon contains `(lat, lon)`, falling back to
/// the nearest centroid within 50 m if none contains it. Point-in-polygon
/// (ray casting over `coords`, which are `[lon, lat]` pairs) and the
/// fallback distance search are CLI-local -- core has no polygon-containment
/// helper (out of scope per the plan; buildings.rs only decodes geometry).
fn find_building(lat: f64, lon: f64, buildings: &[Building]) -> Option<&Building> {
    for b in buildings {
        if point_in_polygon(lon, lat, &b.coords) {
            return Some(b);
        }
    }
    buildings
        .iter()
        .map(|b| (b, ptiles_core::haversine_distance_m(lat, lon, b.centroid_lat, b.centroid_lon)))
        .filter(|(_, d)| *d <= 50.0)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(b, _)| b)
}

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

fn nearest_road_json(nr: &ptiles_core::NearestRoad, roads: &[RoadSegment]) -> Value {
    let road = &roads[nr.road_index];
    json!({
        "osm_id": road.osm_id,
        "name": road.name,
        "road_class": road.road_class,
        "snapped": [nr.snapped.0, nr.snapped.1],
        "distance_m": nr.distance_m,
        "geometry": road.coords.iter().map(|c| [c[1], c[0]]).collect::<Vec<_>>(),
    })
}

fn road_segment_json(road: &RoadSegment) -> Value {
    json!({
        "osm_id": road.osm_id,
        "name": road.name,
        "road_class": road.road_class,
        "geometry": road.coords.iter().map(|c| [c[1], c[0]]).collect::<Vec<_>>(),
    })
}

fn building_json(b: &Building) -> Value {
    json!({
        "osm_id": b.osm_id,
        "building_type": b.building_type,
        "name": b.name,
        "category": b.category,
        "centroid": [b.centroid_lat, b.centroid_lon],
    })
}

fn business_json(b: &Business) -> Value {
    json!({
        "osm_id": b.osm_id,
        "name": b.name,
        "lat": b.lat,
        "lon": b.lon,
        "category_idx": b.category_idx,
        "phone": b.phone,
        "website": b.website,
        "operating_status": b.operating_status,
    })
}

// --- --serve mode ---------------------------------------------------------

/// One state's set of opened layer files (only the layers this CLI queries).
struct StateFiles {
    roads: Option<OpenedLayer>,
    buildings: Option<OpenedLayer>,
    business: Option<OpenedLayer>,
}

fn run_serve(data_dir: &Path) {
    let mut states: HashMap<String, StateFiles> = HashMap::new();

    let entries = match std::fs::read_dir(data_dir) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("ptiles-cli --serve: cannot read data dir {data_dir:?}: {e}");
            std::process::exit(1);
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let mut parts = name.splitn(3, '.');
        let (Some(state), Some(layer_token)) = (parts.next(), parts.next()) else {
            continue;
        };
        let Some(layer) = Layer::from_filename_token(layer_token) else {
            continue;
        };
        let Some(path_str) = path.to_str() else {
            continue;
        };
        let opened = match OpenedLayer::open(path_str, layer) {
            Ok(o) => o,
            Err(e) => {
                eprintln!("ptiles-cli --serve: skipping {path:?}: {e}");
                continue;
            }
        };
        let entry = states.entry(state.to_string()).or_insert_with(|| StateFiles {
            roads: None,
            buildings: None,
            business: None,
        });
        match layer {
            Layer::Roads => entry.roads = Some(opened),
            Layer::BuildingsV8 => entry.buildings = Some(opened),
            Layer::Business => entry.business = Some(opened),
        }
    }

    eprintln!(
        "ptiles-cli --serve: loaded states {:?} from {:?}",
        states.keys().collect::<Vec<_>>(),
        data_dir
    );

    serve_loop(&states);
}

/// `--serve --remote-base <base> --states TN,US`: same per-state
/// roads/buildings_v8/business layer set as `run_serve`, but each file is
/// `<base><state>.<layer>.ptiles` fetched over HTTP instead of scanned from a
/// local directory. A state missing a given layer (404/error) just doesn't
/// get that layer populated -- not every state has every layer.
fn run_serve_remote(remote_base: &str, states_csv: &str) {
    let base = if remote_base.ends_with('/') {
        remote_base.to_string()
    } else {
        format!("{remote_base}/")
    };

    let mut states: HashMap<String, StateFiles> = HashMap::new();

    for state in states_csv.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let mut entry = StateFiles { roads: None, buildings: None, business: None };
        for layer in [Layer::Roads, Layer::BuildingsV8, Layer::Business] {
            let url = format!("{base}{state}.{}.ptiles", layer.as_str());
            match OpenedLayer::open(&url, layer) {
                Ok(opened) => match layer {
                    Layer::Roads => entry.roads = Some(opened),
                    Layer::BuildingsV8 => entry.buildings = Some(opened),
                    Layer::Business => entry.business = Some(opened),
                },
                Err(e) => {
                    eprintln!("ptiles-cli --serve --remote-base: skipping {url}: {e}");
                }
            }
        }
        states.insert(state.to_string(), entry);
    }

    eprintln!(
        "ptiles-cli --serve --remote-base: loaded states {:?} from {base}",
        states.keys().collect::<Vec<_>>()
    );

    serve_loop(&states);
}

fn serve_loop(states: &HashMap<String, StateFiles>) {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        let response = handle_serve_line(&line, states);
        let _ = writeln!(out, "{}", serde_json::to_string(&response).unwrap());
        let _ = out.flush();
    }
}

fn handle_serve_line(line: &str, states: &HashMap<String, StateFiles>) -> Value {
    let req: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return json!({"error": format!("invalid JSON: {e}")}),
    };

    let lat = match req.get("lat").and_then(Value::as_f64) {
        Some(v) => v,
        None => return json!({"error": "missing or non-numeric \"lat\""}),
    };
    let lon = match req.get("lon").and_then(Value::as_f64) {
        Some(v) => v,
        None => return json!({"error": "missing or non-numeric \"lon\""}),
    };
    let query_str = req.get("query").and_then(Value::as_str).unwrap_or("all");
    let query_kind = match QueryKind::parse(query_str) {
        Some(q) => q,
        None => return json!({"error": format!("unknown query {query_str:?}")}),
    };
    let ring = req.get("ring").and_then(Value::as_u64).unwrap_or(0) as u32;
    if let Err(e) = validate_ring(ring) {
        return json!({"error": e});
    }
    let accuracy_m = req.get("accuracy_m").and_then(Value::as_f64);
    let speed_mps = req.get("speed_mps").and_then(Value::as_f64);

    let state_files = match req.get("state").and_then(Value::as_str) {
        Some(s) => match states.get(s) {
            Some(f) => f,
            None => return json!({"error": format!("unknown state {s:?}")}),
        },
        None => {
            if states.len() == 1 {
                states.values().next().unwrap()
            } else {
                return json!({
                    "error": format!(
                        "\"state\" is required: {} states loaded ({:?})",
                        states.len(),
                        states.keys().collect::<Vec<_>>()
                    )
                });
            }
        }
    };

    let mut building: Value = Value::Null;
    let mut nearest_road: Value = Value::Null;
    let mut roads_list: Value = Value::Null;
    let mut business: Value = Value::Array(Vec::new());

    let mut decoded_roads: Vec<RoadSegment> = Vec::new();
    let mut decoded_buildings: Vec<Building> = Vec::new();
    let mut decoded_businesses: Vec<Business> = Vec::new();

    if matches!(query_kind, QueryKind::Buildings | QueryKind::All) {
        if let Some(layer) = &state_files.buildings {
            let r = layer.query(lat, lon, ring, query_kind);
            if let Some(e) = r.get("error") {
                return json!({"error": e});
            }
            building = r.get("building").cloned().unwrap_or(Value::Null);
        }
    }
    if matches!(query_kind, QueryKind::Road | QueryKind::Roads | QueryKind::All) {
        if let Some(layer) = &state_files.roads {
            let r = layer.query(lat, lon, ring, query_kind);
            if let Some(e) = r.get("error") {
                return json!({"error": e});
            }
            nearest_road = r.get("nearest_road").cloned().unwrap_or(Value::Null);
            if let Some(rs) = r.get("roads") {
                roads_list = rs.clone();
            }
        }
    }
    if matches!(query_kind, QueryKind::Business | QueryKind::All) {
        if let Some(layer) = &state_files.business {
            let r = layer.query(lat, lon, ring, query_kind);
            if let Some(e) = r.get("error") {
                return json!({"error": e});
            }
            business = r.get("business").cloned().unwrap_or(Value::Array(Vec::new()));
        }
    }

    let mut response = json!({
        "building": building,
        "nearest_road": nearest_road,
        "business": business,
    });
    if query_kind == QueryKind::Roads {
        if let Value::Object(ref mut map) = response {
            map.insert("roads".to_string(), roads_list);
        }
    }

    if let Some(accuracy_m) = accuracy_m {
        // Full-cross-layer scoring, unlike the one-shot path (which is
        // scoped to a single opened file): decode whichever layers this
        // state has and score across all of them together.
        if let Some(layer) = &state_files.roads {
            decoded_roads = layer.candidates_for(lat, lon, ring).0;
        }
        if let Some(layer) = &state_files.buildings {
            decoded_buildings = layer.candidates_for(lat, lon, ring).1;
        }
        if let Some(layer) = &state_files.business {
            decoded_businesses = layer.candidates_for(lat, lon, ring).2;
        }
        let fix = Fix { lat, lon, horizontal_accuracy_m: accuracy_m, speed_mps };
        let candidates = score_candidates(
            &fix,
            &decoded_roads,
            &decoded_buildings,
            &decoded_businesses,
            &ScoringParams::default(),
        );
        if let Value::Object(ref mut map) = response {
            map.insert("candidates".to_string(), candidates_json(&candidates));
        }
    }

    response
}
