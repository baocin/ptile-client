//! ptiles-cli: rookery bridge over ptiles-core.
//!
//! Modes:
//! - one-shot: `--path <file.ptiles> --lat <f64> --lon <f64> [--query roads|buildings|business|all] [--ring 1]`
//!   Opens a single `.ptiles` file, resolves the H3 res-7 cell for the point
//!   (plus ring-1 neighbors if `--ring 1`), decodes the block(s) with the
//!   decoder matching the file's layer (inferred from its `<state>.<layer>.ptiles`
//!   filename), and prints one JSON object to stdout.
//! - `--serve --data-dir <dir>`: pre-opens every `*.ptiles` file under `dir`
//!   (grouped by state + layer parsed from the filename), then reads JSON
//!   lines from stdin: `{"lat":..,"lon":..,"query":"building|road|business|all","state":?}`.
//!   `state` is optional; if omitted, the sole state present in the data dir
//!   is used, or an `{"error":...}` line if more than one state is loaded.
//!   Responds with one JSON line per request:
//!   `{"building":..|null,"nearest_road":{..}|null,"business":[..]}`.
//!   Malformed input or per-query decode failures produce `{"error":"..."}`
//!   lines -- the serve loop never crashes on bad input.

use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use ptiles_core::{
    cell_center, cell_for_coord, decode_buildings, decode_business, decode_roads, nearest_road,
    Building, Business, FileSource, PtilesFile, RoadSegment,
};
use serde_json::{json, Value};

/// Workaround for a `core/file.rs` bug found while wiring up this CLI:
/// `PtilesFile::read_block` always treats `IndexEntry::block_offset` as an
/// absolute file offset. The Python reference (`ptiles/buildings.py:317-318`,
/// `BuildingsReader.__init__`) detects that `.buildings_v8.ptiles` files
/// actually store block offsets **relative to `header.blocks_offset`**
/// (`self._relative_offsets = first_off < self._header["blocks_offset"]`) and
/// adjusts every read accordingly -- `PtilesFile` does not do this
/// detection/adjustment at all, so `read_block` silently returns corrupt
/// bytes (confirmed against `TN.buildings_v8.ptiles`: the "block" at the raw
/// index offset starts 4117 bytes before the actual zstd frame, and adding
/// `header.blocks_offset` back in lines it up exactly, matching a Python
/// `zstandard` decompress of the same corrected range). Rather than patch
/// `core/file.rs` (out of scope for this task -- reported to the core owner
/// instead), buildings_v8 blocks are read here via a small raw reader that
/// mirrors `PtilesFile::open`/`read_block` using only core's public
/// header/index parsing plus a local dict-fallback zstd decompress.
mod buildings_v8_workaround {
    use ptiles_core::header::HEADER_SIZE;
    use ptiles_core::{parse_index, FileSource, Header, IndexEntry, PtilesSource};
    use ruzstd::decoding::{BlockDecodingStrategy, Dictionary, FrameDecoder};

    pub struct RawFile {
        source: FileSource,
        header: Header,
        index: Vec<IndexEntry>,
        dict: Vec<u8>,
        relative_offsets: bool,
    }

    impl RawFile {
        pub fn open(path: &std::path::Path) -> Result<RawFile, String> {
            let source = FileSource::open(path).map_err(|e| format!("open: {e}"))?;
            let mut header_buf = [0u8; HEADER_SIZE];
            source
                .read_exact_at(0, &mut header_buf)
                .map_err(|e| format!("read header: {e}"))?;
            let header = Header::parse(&header_buf).map_err(|e| format!("parse header: {e}"))?;

            let dict = if header.dict_length > 0 {
                let mut buf = vec![0u8; header.dict_length as usize];
                source
                    .read_exact_at(header.dict_offset, &mut buf)
                    .map_err(|e| format!("read dict: {e}"))?;
                buf
            } else {
                Vec::new()
            };

            let mut index_buf = vec![0u8; header.index_length as usize];
            source
                .read_exact_at(header.index_offset, &mut index_buf)
                .map_err(|e| format!("read index: {e}"))?;
            let index = parse_index(&index_buf).map_err(|e| format!("parse index: {e}"))?;

            // Same detection the Python `BuildingsReader` uses.
            let relative_offsets = index
                .first()
                .is_some_and(|e| e.block_offset < header.blocks_offset);

            Ok(RawFile {
                source,
                header,
                index,
                dict,
                relative_offsets,
            })
        }

        pub fn read_block(&self, cell: u64) -> Result<Option<Vec<u8>>, String> {
            let Some(entry) = binary_search(&self.index, cell) else {
                return Ok(None);
            };
            let abs_offset = if self.relative_offsets {
                self.header.blocks_offset + entry.block_offset
            } else {
                entry.block_offset
            };
            let mut compressed = vec![0u8; entry.block_length as usize];
            self.source
                .read_exact_at(abs_offset, &mut compressed)
                .map_err(|e| format!("read block at {abs_offset}: {e}"))?;
            decompress_with_dict_fallback(&compressed, &self.dict).map(Some)
        }
    }

    fn binary_search(index: &[IndexEntry], cell: u64) -> Option<&IndexEntry> {
        index.binary_search_by_key(&cell, |e| e.h3_cell).ok().map(|i| &index[i])
    }

    /// Mirrors `core/file.rs::decompress_with_dict_fallback` (that function
    /// isn't public -- duplicated here rather than exported for a one-off
    /// workaround).
    fn decompress_with_dict_fallback(compressed: &[u8], dict: &[u8]) -> Result<Vec<u8>, String> {
        if !dict.is_empty() {
            if let Ok(parsed_dict) = Dictionary::decode_dict(dict) {
                let mut decoder = FrameDecoder::new();
                if decoder.add_dict(parsed_dict).is_ok() {
                    if let Some(out) = try_decode_all(&mut decoder, compressed) {
                        return Ok(out);
                    }
                }
            }
        }
        let mut decoder = FrameDecoder::new();
        try_decode_all(&mut decoder, compressed)
            .ok_or_else(|| "zstd decompress failed (dict and plain both failed)".to_string())
    }

    fn try_decode_all(decoder: &mut FrameDecoder, compressed: &[u8]) -> Option<Vec<u8>> {
        let mut input: &[u8] = compressed;
        decoder.reset(&mut input).ok()?;
        decoder.decode_blocks(&mut input, BlockDecodingStrategy::All).ok()?;
        decoder.collect()
    }
}

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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QueryKind {
    Roads,
    Buildings,
    Business,
    All,
}

impl QueryKind {
    fn parse(s: &str) -> Option<QueryKind> {
        match s {
            "road" | "roads" => Some(QueryKind::Roads),
            "building" | "buildings" => Some(QueryKind::Buildings),
            "business" => Some(QueryKind::Business),
            "all" => Some(QueryKind::All),
            _ => None,
        }
    }

    fn wants(self, layer: Layer) -> bool {
        match self {
            QueryKind::All => true,
            QueryKind::Roads => layer == Layer::Roads,
            QueryKind::Buildings => layer == Layer::BuildingsV8,
            QueryKind::Business => layer == Layer::Business,
        }
    }
}

fn main() {
    let mut args = pico_args::Arguments::from_env();

    if args.contains("--serve") {
        let data_dir: PathBuf = args
            .opt_value_from_str("--data-dir")
            .unwrap_or(None)
            .unwrap_or_else(|| PathBuf::from("/home/aoi/kino/data/ptiles"));
        run_serve(&data_dir);
        return;
    }

    let path: PathBuf = match args.value_from_str("--path") {
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
                "ptiles-cli: could not infer layer from filename {:?} (expected <state>.<layer>.ptiles)",
                path
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

    let result = opened.query(lat, lon, ring);
    println!("{}", serde_json::to_string_pretty(&result).unwrap());
}

fn layer_from_path(path: &Path) -> Option<Layer> {
    let name = path.file_name()?.to_str()?;
    let mut parts = name.split('.');
    let _state = parts.next()?;
    let layer_token = parts.next()?;
    Layer::from_filename_token(layer_token)
}

/// Backing reader for an opened layer file: `PtilesFile` for layers whose
/// index offsets are absolute (roads, business -- confirmed against real
/// `TN.*.ptiles` data), or the `buildings_v8_workaround::RawFile` for
/// buildings_v8, whose index offsets are relative (see that module's doc
/// comment for why `PtilesFile` can't be used as-is there).
enum Backend {
    Standard(PtilesFile<FileSource>),
    BuildingsV8(buildings_v8_workaround::RawFile),
}

/// One opened `.ptiles` file plus the metadata needed to decode its blocks
/// and answer queries against it.
struct OpenedLayer {
    layer: Layer,
    backend: Backend,
}

impl OpenedLayer {
    fn open(path: &Path, layer: Layer) -> Result<OpenedLayer, String> {
        let backend = if layer == Layer::BuildingsV8 {
            Backend::BuildingsV8(buildings_v8_workaround::RawFile::open(path)?)
        } else {
            let source = FileSource::open(path).map_err(|e| format!("open: {e}"))?;
            let file = PtilesFile::open(source).map_err(|e| format!("parse header/index: {e}"))?;
            Backend::Standard(file)
        };
        Ok(OpenedLayer { layer, backend })
    }

    fn read_block(&self, cell: u64) -> Option<Vec<u8>> {
        match &self.backend {
            Backend::Standard(file) => file.read_block(cell).ok().flatten(),
            Backend::BuildingsV8(raw) => raw.read_block(cell).ok().flatten(),
        }
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

    fn query(&self, lat: f64, lon: f64, ring: u32) -> Value {
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
fn find_building<'a>(lat: f64, lon: f64, buildings: &'a [Building]) -> Option<&'a Building> {
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
        let opened = match OpenedLayer::open(&path, layer) {
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
        let response = handle_serve_line(&line, &states);
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
    let mut business: Value = Value::Array(Vec::new());

    if matches!(query_kind, QueryKind::Buildings | QueryKind::All) {
        if let Some(layer) = &state_files.buildings {
            let r = layer.query(lat, lon, ring);
            if let Some(e) = r.get("error") {
                return json!({"error": e});
            }
            building = r.get("building").cloned().unwrap_or(Value::Null);
        }
    }
    if matches!(query_kind, QueryKind::Roads | QueryKind::All) {
        if let Some(layer) = &state_files.roads {
            let r = layer.query(lat, lon, ring);
            if let Some(e) = r.get("error") {
                return json!({"error": e});
            }
            nearest_road = r.get("nearest_road").cloned().unwrap_or(Value::Null);
        }
    }
    if matches!(query_kind, QueryKind::Business | QueryKind::All) {
        if let Some(layer) = &state_files.business {
            let r = layer.query(lat, lon, ring);
            if let Some(e) = r.get("error") {
                return json!({"error": e});
            }
            business = r.get("business").cloned().unwrap_or(Value::Array(Vec::new()));
        }
    }

    json!({
        "building": building,
        "nearest_road": nearest_road,
        "business": business,
    })
}
