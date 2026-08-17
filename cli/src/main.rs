//! ptiles-cli: rookery bridge over ptiles-core.
//!
//! Local and remote files: `--path` (one-shot) and per-layer files under
//! `--data-dir`/`--remote-base` (serve) accept either a filesystem path or an
//! `http(s)://` URL -- picked by a scheme sniff (`is_url`), matching
//! `ptiles-core`'s `FileSource`/`HttpSource` split
//! (`~/.hermes/plans/ptiles-client-extraction-plan.md`, Addendum 2 item 1).
//!
//! Modes:
//! - one-shot: `--path <file.ptiles|https://.../file.ptiles> --lat <f64> --lon <f64> [--query road|roads|intersection|buildings|business|trail|trails|trailhead|park|parks|water|waters|rail|rails|station|cameras|camera|locate|all] [--ring 1]`
//!   Opens a single `.ptiles` file (local or remote), resolves the H3 res-7
//!   cell for the point (plus ring-1 neighbors if `--ring 1`), decodes the
//!   block(s) with the decoder matching the file's layer (inferred from its
//!   `<state>.<layer>.ptiles` filename), and prints one JSON object to stdout.
//! - `--serve --data-dir <dir>`: pre-opens every `*.ptiles` file under `dir`
//!   (grouped by state + layer parsed from the filename), then reads JSON
//!   lines from stdin. `--serve --remote-base <https://host/path/> --states
//!   TN,US`: same, but for each state and each queried layer (`roads`,
//!   `buildings_v8`, `business`, `trails`, `parks`, `water`, `rail`,
//!   `camera`) opens
//!   `<remote_base><state>.<layer>.ptiles` over HTTP instead of scanning a
//!   local directory -- a state/layer combination that 404s or errors is
//!   skipped (eprintln), not fatal, since not every state has every layer.
//!   `--serve` accepts either `--data-dir` or `--remote-base` (not both).
//!
//!   `--serve` JSON lines:
//!   `{"lat":..,"lon":..,"query":"building|road|roads|business|trail|park|
//!   water|rail|camera|locate|all","state":?,
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
//!
//!   A separate request shape, `{"query":"business_search","name":"waffle",
//!   "state":?,"limit":?}`, does business name search instead of a lat/lon
//!   lookup (no `lat`/`lon` required). `--serve --data-dir`/`--remote-base`
//!   also pre-open each state's `business_name_index.ptiles` sidecar
//!   alongside its three layer files, when present; `limit` defaults to 50.
//!   Responds `{"state":..,"method":"indexed"|"brute_force","hits":[..]}` or
//!   `{"error":"..."}` -- falls back to brute-force search over the main
//!   `business.ptiles` file (see `ptiles_core::business_search`'s module
//!   doc) when a state has no name-index sidecar loaded, matching the
//!   one-shot `--query business-search`/`--national` CLI path.

use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use ptiles_core::{
    cell_center, cell_for_coord, decode_buildings, decode_business_versioned, decode_road_block,
    decode_roads, nearest_intersection, nearest_road, score_candidates,
    search_business_brute_force, search_business_indexed, Building, Business, BusinessHit,
    Candidate, CandidateKind, FileSource, Fix, HttpSource, PtilesFile, RoadSegment, ScoringParams,
};
use ptiles_core::{point_in_polygon, AddressFile, AdminFile, PtilesSource};
use serde_json::{json, Value};

/// USPS state/territory abbreviations + DC -- the full set `--national`
/// iterates when no local directory listing is available (i.e. against
/// `--remote-base`, where there's no directory to scan and 404s for states
/// without a business-name-index file are expected and skipped).
const ALL_US_STATES: &[&str] = &[
    "AL", "AK", "AZ", "AR", "CA", "CO", "CT", "DE", "FL", "GA", "HI", "ID", "IL", "IN", "IA", "KS",
    "KY", "LA", "ME", "MD", "MA", "MI", "MN", "MS", "MO", "MT", "NE", "NV", "NH", "NJ", "NM", "NY",
    "NC", "ND", "OH", "OK", "OR", "PA", "RI", "SC", "SD", "TN", "TX", "UT", "VT", "VA", "WA", "WV",
    "WI", "WY", "DC",
];

/// The layer a `.ptiles` file holds, inferred from its filename
/// (`<state>.<layer>.ptiles`). `places` files are still ignored -- nothing
/// here decodes them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Layer {
    Roads,
    BuildingsV8,
    Business,
    Trails,
    Parks,
    Water,
    Rail,
    Camera,
}

impl Layer {
    /// The published snapshots version the filename stem
    /// (`TN.business_v4.ptiles`, `TN.roads_v2.ptiles`, `TN.buildings_v9.ptiles`),
    /// while the local corpus does not (`TN.business.ptiles`). Strip a trailing
    /// `_v<N>` so both name shapes resolve to the same layer -- the schema
    /// version comes from the header, never the filename.
    fn from_filename_token(token: &str) -> Option<Layer> {
        let base = match token.rsplit_once("_v") {
            Some((stem, digits)) if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) => stem,
            _ => token,
        };
        match base {
            "roads" => Some(Layer::Roads),
            "buildings" => Some(Layer::BuildingsV8),
            "business" => Some(Layer::Business),
            "trails" => Some(Layer::Trails),
            "parks" => Some(Layer::Parks),
            "water" => Some(Layer::Water),
            "rail" => Some(Layer::Rail),
            "camera" => Some(Layer::Camera),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Layer::Roads => "roads",
            Layer::BuildingsV8 => "buildings_v8",
            Layer::Business => "business",
            Layer::Trails => "trails",
            Layer::Parks => "parks",
            Layer::Water => "water",
            Layer::Rail => "rail",
            Layer::Camera => "camera",
        }
    }
}

/// Query kinds accepted on `--query` / the `"query"` JSON field.
///
/// `Road` ("road") is the singular nearest-road-to-point lookup. `Roads`
/// ("roads") is the plan-addendum bulk query: every decoded segment in the
/// containing cell (plus ring-1 neighbors when requested).
/// The same singular/plural rule runs through the trail/park/water/rail
/// kinds: singular is the lookup ("which one am I on/in"), plural is every
/// feature in the query cells. `locate` is the cross-layer answer and only
/// means anything under `--serve`, where more than one layer is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QueryKind {
    Road,
    Roads,
    Intersection,
    Buildings,
    Business,
    Trail,
    Trails,
    Trailhead,
    Park,
    Parks,
    Water,
    Waters,
    Rail,
    Rails,
    Station,
    Cameras,
    Camera,
    Locate,
    All,
}

impl QueryKind {
    fn parse(s: &str) -> Option<QueryKind> {
        match s {
            "road" => Some(QueryKind::Road),
            "roads" => Some(QueryKind::Roads),
            "intersection" => Some(QueryKind::Intersection),
            "building" | "buildings" => Some(QueryKind::Buildings),
            "business" => Some(QueryKind::Business),
            "trail" => Some(QueryKind::Trail),
            "trails" => Some(QueryKind::Trails),
            "trailhead" => Some(QueryKind::Trailhead),
            "park" => Some(QueryKind::Park),
            "parks" => Some(QueryKind::Parks),
            "water" => Some(QueryKind::Water),
            "waters" => Some(QueryKind::Waters),
            "rail" => Some(QueryKind::Rail),
            "rails" => Some(QueryKind::Rails),
            "station" => Some(QueryKind::Station),
            "cameras" => Some(QueryKind::Cameras),
            "camera" => Some(QueryKind::Camera),
            "locate" => Some(QueryKind::Locate),
            "all" => Some(QueryKind::All),
            _ => None,
        }
    }

    fn wants(self, layer: Layer) -> bool {
        match self {
            QueryKind::All => true,
            QueryKind::Road | QueryKind::Roads | QueryKind::Intersection => layer == Layer::Roads,
            QueryKind::Buildings => layer == Layer::BuildingsV8,
            QueryKind::Business => layer == Layer::Business,
            QueryKind::Trail | QueryKind::Trails | QueryKind::Trailhead => layer == Layer::Trails,
            QueryKind::Park | QueryKind::Parks => layer == Layer::Parks,
            QueryKind::Water | QueryKind::Waters => layer == Layer::Water,
            QueryKind::Rail | QueryKind::Rails | QueryKind::Station => layer == Layer::Rail,
            // `camera` reads buildings too, when the state has them -- see
            // `handle_serve_line`. One-shot against the camera file alone
            // still answers, without the occlusion half.
            QueryKind::Cameras => layer == Layer::Camera,
            QueryKind::Camera => matches!(layer, Layer::Camera | Layer::BuildingsV8),
            // Cross-layer: roads and trails together, plus whatever else the
            // state has open. Never satisfied by a single layer.
            QueryKind::Locate => matches!(layer, Layer::Roads | Layer::Trails),
        }
    }
}

fn main() {
    let mut args = pico_args::Arguments::from_env();

    if args.contains("--supported-formats") {
        print!("{}", ptiles_core::supported_formats());
        return;
    }

    // `--query cells --bounds min_lat,min_lon,max_lat,max_lon`: viewport ->
    // cell-list query (docs/INTEGRATION.md's first step). No `.ptiles` file
    // involved -- pure H3 geometry -- so it's handled before `--path` is
    // required, unlike every other `--query` kind.
    let query_peek: Option<String> = args.opt_value_from_str("--query").unwrap_or(None);
    if query_peek.as_deref() == Some("cells") {
        let bounds: String = args.value_from_str("--bounds").unwrap_or_else(|e| {
            eprintln!("ptiles-cli: --query cells requires --bounds min_lat,min_lon,max_lat,max_lon ({e})");
            std::process::exit(2);
        });
        let [min_lat, min_lon, max_lat, max_lon] = parse_bounds(&bounds).unwrap_or_else(|e| {
            eprintln!("ptiles-cli: {e}");
            std::process::exit(2);
        });
        let result = match ptiles_core::cells_for_bounds(min_lat, min_lon, max_lat, max_lon) {
            Ok(cells) => json!({"cells": cells.into_iter().map(|c| format!("{c:x}")).collect::<Vec<_>>()}),
            Err(e) => json!({"error": e.to_string()}),
        };
        println!("{}", serde_json::to_string_pretty(&result).unwrap());
        return;
    }

    // `--query business-search --name <query> [--state XX | --national]
    // [--limit N] [--data-dir <dir>|--remote-base <url>]`: business name
    // search over `{STATE}.business_name_index.ptiles` sidecar file(s), not
    // a lat/lon lookup against one already-known layer file -- handled here,
    // before `--path` is required, same as the `cells` peek above.
    if query_peek.as_deref() == Some("business-search") {
        run_business_search_cli(&mut args);
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
    // Parsed as options first: `address-search` is the one query that can
    // answer without a point ("where is 919 Broadway" needs no viewport), and
    // requiring --lat/--lon there would make a whole-state search pretend to
    // be a local one.
    let lat_opt: Option<f64> = args.opt_value_from_str("--lat").unwrap_or(None);
    let lon_opt: Option<f64> = args.opt_value_from_str("--lon").unwrap_or(None);

    if query_peek.as_deref() == Some("address-search") {
        let number: String = args.opt_value_from_str("--number").unwrap_or(None).unwrap_or_default();
        let street: String = args.opt_value_from_str("--street").unwrap_or(None).unwrap_or_default();
        if number.trim().is_empty() && street.trim().is_empty() {
            eprintln!("ptiles-cli: --query address-search needs --number and/or --street");
            std::process::exit(2);
        }
        let limit: usize = args.opt_value_from_str("--limit").unwrap_or(None).unwrap_or(25);
        let near = match (lat_opt, lon_opt) {
            (Some(la), Some(lo)) => Some((la, lo)),
            _ => None,
        };
        run_address_search(&path, &number, &street, near, limit);
        return;
    }

    let lat: f64 = lat_opt.unwrap_or_else(|| {
        eprintln!("ptiles-cli: --lat is required");
        std::process::exit(2);
    });
    let lon: f64 = lon_opt.unwrap_or_else(|| {
        eprintln!("ptiles-cli: --lon is required");
        std::process::exit(2);
    });
    // `--query admin`: point -> jurisdiction lookup against an admin file
    // (`US.admin.ptiles`). Admin is a lookup-grid layer, not block-per-cell, so
    // it bypasses the `OpenedLayer`/`--ring` machinery entirely.
    if query_peek.as_deref() == Some("admin") {
        run_admin_query(&path, lat, lon);
        return;
    }

    // `--query address` (reverse: addresses in the covering cell(s), honoring
    // `--ring`) or `--query address-find --number N --street S` (forward). Like
    // admin, address bypasses the block-per-cell `OpenedLayer` (it uses a v2
    // merged-block index).
    if matches!(
        query_peek.as_deref(),
        Some("address") | Some("address-find") | Some("address-search")
    ) {
        let ring: u32 = args.opt_value_from_str("--ring").unwrap_or(None).unwrap_or(0);
        let find = if query_peek.as_deref() == Some("address-find") {
            let number: String = args.value_from_str("--number").unwrap_or_else(|e| {
                eprintln!("ptiles-cli: --query address-find requires --number ({e})");
                std::process::exit(2);
            });
            let street: String = args.value_from_str("--street").unwrap_or_else(|e| {
                eprintln!("ptiles-cli: --query address-find requires --street ({e})");
                std::process::exit(2);
            });
            Some((number, street))
        } else {
            None
        };
        run_address_query(&path, lat, lon, ring as u8, find);
        return;
    }

    // `--query` was already consumed by the `cells` peek above (pico-args
    // removes matched flags from `args`), so reuse that parse rather than
    // asking `args` for it again (it would come back empty).
    let query: Option<String> = query_peek;
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
                eprintln!("ptiles-cli: unknown --query {s:?} (expected road|roads|intersection|buildings|business|trail|trails|trailhead|park|parks|water|waters|rail|rails|station|cameras|camera|locate|all)");
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
/// Parse a `--bounds min_lat,min_lon,max_lat,max_lon` string into exactly four
/// f64s. Pure (no process exit / no stderr) so it's unit-testable; the caller
/// in `main` maps the `Err(String)` to an eprintln + exit(2).
fn parse_bounds(bounds: &str) -> Result<[f64; 4], String> {
    let parts: Vec<f64> = bounds
        .split(',')
        .map(|s| s.trim().parse::<f64>())
        .collect::<Result<_, _>>()
        .map_err(|e| {
            format!("--bounds must be 4 comma-separated numbers min_lat,min_lon,max_lat,max_lon ({e})")
        })?;
    parts.try_into().map_err(|v: Vec<f64>| {
        format!("--bounds must be exactly 4 comma-separated numbers, got {}", v.len())
    })
}

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

/// `--query admin`: open an admin file (local or URL) and print the
/// jurisdiction covering `(lat, lon)` as JSON (`{"admin": {...}|null}`).
fn run_admin_query(path_or_url: &str, lat: f64, lon: f64) {
    let admin = if is_url(path_or_url) {
        HttpSource::open(path_or_url)
            .map_err(|e| e.to_string())
            .and_then(|s| AdminFile::open(s).map_err(|e| e.to_string()))
    } else {
        FileSource::open(path_or_url)
            .map_err(|e| e.to_string())
            .and_then(|s| AdminFile::open(s).map_err(|e| e.to_string()))
    };
    let admin = match admin {
        Ok(a) => a,
        Err(e) => {
            eprintln!("ptiles-cli: could not open admin file {path_or_url:?}: {e}");
            std::process::exit(2);
        }
    };
    let value = match admin.admin_at(lat, lon) {
        Some(info) => json!({"admin": {
            "country": info.country,
            "state": info.state,
            "county": info.county,
            "zip": info.zip,
            "timezone": info.timezone,
            "boundary_flags": info.boundary_flags,
        }}),
        None => json!({"admin": null}),
    };
    println!("{}", serde_json::to_string_pretty(&value).unwrap());
}

/// `--query address` / `address-find`: open an address file (local or URL) and
/// print reverse (enumerate) or forward (match number+street) results.
fn run_address_query(path_or_url: &str, lat: f64, lon: f64, ring: u8, find: Option<(String, String)>) {
    let result = if is_url(path_or_url) {
        HttpSource::open(path_or_url)
            .map_err(|e| e.to_string())
            .and_then(|s| address_result(s, lat, lon, ring, &find))
    } else {
        FileSource::open(path_or_url)
            .map_err(|e| e.to_string())
            .and_then(|s| address_result(s, lat, lon, ring, &find))
    };
    match result {
        Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap()),
        Err(e) => {
            eprintln!("ptiles-cli: address query failed for {path_or_url:?}: {e}");
            std::process::exit(2);
        }
    }
}

/// `--query address-search`: forward geocode over the whole file, with an
/// optional `--lat/--lon` hint that orders the walk and the results.
fn run_address_search(
    path_or_url: &str,
    number: &str,
    street: &str,
    near: Option<(f64, f64)>,
    limit: usize,
) {
    let result = if is_url(path_or_url) {
        HttpSource::open(path_or_url)
            .map_err(|e| e.to_string())
            .and_then(|s| address_search_result(s, number, street, near, limit))
    } else {
        FileSource::open(path_or_url)
            .map_err(|e| e.to_string())
            .and_then(|s| address_search_result(s, number, street, near, limit))
    };
    match result {
        Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap()),
        Err(e) => {
            eprintln!("ptiles-cli: address search failed for {path_or_url:?}: {e}");
            std::process::exit(2);
        }
    }
}

fn address_search_result<S: PtilesSource>(
    source: S,
    number: &str,
    street: &str,
    near: Option<(f64, f64)>,
    limit: usize,
) -> Result<Value, String> {
    let file = AddressFile::open(source).map_err(|e| e.to_string())?;
    let records = file
        .search_address(number, street, near, limit)
        .map_err(|e| e.to_string())?;
    let addresses: Vec<Value> = records
        .iter()
        .map(|r| {
            json!({
                "osm_id": r.osm_id,
                "housenumber": r.housenumber,
                "street": r.street,
                "lat": r.lat,
                "lon": r.lon,
                "source": r.source.name(),
            })
        })
        .collect();
    Ok(json!({"addresses": addresses, "count": records.len()}))
}

fn address_result<S: PtilesSource>(
    source: S,
    lat: f64,
    lon: f64,
    ring: u8,
    find: &Option<(String, String)>,
) -> Result<Value, String> {
    let file = AddressFile::open(source).map_err(|e| e.to_string())?;
    let records = match find {
        Some((number, street)) => file
            .find_address(lat, lon, ring, number, street)
            .map_err(|e| e.to_string())?,
        None => file.addresses_at(lat, lon, ring).map_err(|e| e.to_string())?,
    };
    let addresses: Vec<Value> = records
        .iter()
        .map(|r| {
            json!({
                "osm_id": r.osm_id,
                "housenumber": r.housenumber,
                "street": r.street,
                "lat": r.lat,
                "lon": r.lon,
                "source": r.source.name(),
            })
        })
        .collect();
    Ok(json!({"addresses": addresses, "count": records.len()}))
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

    /// One cell's record bytes -- `read_block` for a v1 layer, the sliced-out
    /// cell for a v2 (merged-block) one. Errors and misses both read as "no
    /// records here", same as `OpenedLayer::read_block`.
    fn read_cell(&self, cell: u64) -> Option<Vec<u8>> {
        match self {
            AnyFile::File(f) => f.read_cell(cell).ok().flatten(),
            AnyFile::Http(f) => f.read_cell(cell).ok().flatten(),
        }
    }

    /// Header schema version — `decode_road_block` needs it to know whether a
    /// trailing intersection table is present (v2+).
    fn version(&self) -> u8 {
        match self {
            AnyFile::File(f) => f.header().version,
            AnyFile::Http(f) => f.header().version,
        }
    }

    /// Business-name-index search (`{STATE}.business_name_index.ptiles`
    /// sidecar), dispatched to whichever backend this file opened as --
    /// same pattern as `read_block`. Not layer-gated here; callers only
    /// open this variant against a name-index file (see `run_business_search`,
    /// `--serve`'s `name_index` field), unlike `OpenedLayer::query` which is
    /// gated by `Layer`.
    fn search_business(&self, query: &str, limit: usize) -> Result<Vec<BusinessHit>, String> {
        match self {
            AnyFile::File(f) => search_business_indexed(f, query, limit).map_err(|e| e.to_string()),
            AnyFile::Http(f) => search_business_indexed(f, query, limit).map_err(|e| e.to_string()),
        }
    }

    /// Brute-force business search over a main `.business.ptiles` file --
    /// the fallback used when a state has no `business_name_index.ptiles`
    /// sidecar (true of the real deployed dataset at
    /// `https://maps.mydatatimeline.com/maps/`, which only hosts the main
    /// business file; the sidecar is generated locally, see
    /// `core::business_search`'s module doc).
    fn search_business_brute_force(&self, query: &str, limit: usize) -> Result<Vec<BusinessHit>, String> {
        match self {
            AnyFile::File(f) => search_business_brute_force(f, query, limit).map_err(|e| e.to_string()),
            AnyFile::Http(f) => search_business_brute_force(f, query, limit).map_err(|e| e.to_string()),
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
                // Per cell, not `blocks_for`: v4 coordinates are offsets from
                // the cell centre, so the block and its cell must stay paired.
                let version = self.file.version();
                for &cell in &cells {
                    let Some(block) = self.read_block(cell) else { continue };
                    if let Ok(mut b) = decode_business_versioned(&block, version, cell) {
                        businesses.append(&mut b);
                    }
                }
            }
            // `score_candidates` ranks roads, buildings and businesses only --
            // a trail or a lake is not a thing a GPS fix "is at" in the sense
            // the scorer means, so these layers contribute nothing here. They
            // answer through `query` / `locate` instead.
            Layer::Trails | Layer::Parks | Layer::Water | Layer::Rail | Layer::Camera => {}
        }
        (roads, buildings, businesses)
    }

    fn query(&self, lat: f64, lon: f64, ring: u32, query_kind: QueryKind) -> Value {
        let cells = self.cells_for(lat, lon, ring);

        match self.layer {
            Layer::Roads => {
                let blocks = self.blocks_for(&cells);
                // "am I at an intersection?" — decode the v2 intersection
                // table (needs `decode_road_block` + the header version) and
                // return the nearest labeled intersection within threshold.
                if query_kind == QueryKind::Intersection {
                    let version = self.file.version();
                    let mut intersections = Vec::new();
                    for block in &blocks {
                        match decode_road_block(block, version) {
                            Ok((_roads, mut ix)) => intersections.append(&mut ix),
                            Err(e) => {
                                return json!({"error": format!("decode_road_block: {e}")})
                            }
                        }
                    }
                    let nearest = nearest_intersection(
                        lat,
                        lon,
                        &intersections,
                        ptiles_core::DEFAULT_THRESHOLD_M,
                    )
                    .map(|ni| {
                        let [ix_lon, ix_lat] = intersections[ni.index].coords();
                        json!({
                            "lat": ix_lat,
                            "lon": ix_lon,
                            "distance_m": ni.distance_m,
                            "intersection_type": ni.intersection_type,
                        })
                    });
                    return json!({
                        "nearest_intersection": nearest,
                        "candidate_count": intersections.len(),
                    });
                }
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
                let version = self.file.version();
                let mut businesses: Vec<Business> = Vec::new();
                for &cell in &cells {
                    let Some(block) = self.read_block(cell) else { continue };
                    match decode_business_versioned(&block, version, cell) {
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
            Layer::Trails => {
                let trails = match self.decode_all(&cells, ptiles_core::decode_trails) {
                    Ok(t) => t,
                    Err(e) => return json!({"error": format!("decode_trails: {e}")}),
                };
                match query_kind {
                    QueryKind::Trails => {
                        let all: Vec<Value> = trails.iter().map(trail_json).collect();
                        json!({"trails": all, "candidate_count": trails.len()})
                    }
                    QueryKind::Trailhead => {
                        let head = ptiles_core::nearest_trailhead(lat, lon, &trails);
                        json!({
                            "nearest_trailhead": head.as_ref().map(point_json),
                            "candidate_count": trails.len(),
                        })
                    }
                    _ => {
                        let trail = ptiles_core::nearest_trail(lat, lon, &trails);
                        json!({
                            "nearest_trail": trail.as_ref().map(way_json),
                            "candidate_count": trails.len(),
                        })
                    }
                }
            }
            Layer::Parks => {
                let parks = match self.decode_all(&cells, ptiles_core::decode_parks) {
                    Ok(p) => p,
                    Err(e) => return json!({"error": format!("decode_parks: {e}")}),
                };
                if query_kind == QueryKind::Parks {
                    let all: Vec<Value> = parks.iter().map(park_json).collect();
                    return json!({"parks": all, "candidate_count": parks.len()});
                }
                let park = ptiles_core::park_at(lat, lon, &parks);
                json!({
                    "park": park.as_ref().map(area_json),
                    "candidate_count": parks.len(),
                })
            }
            Layer::Water => {
                let water = match self.decode_all(&cells, ptiles_core::decode_water) {
                    Ok(w) => w,
                    Err(e) => return json!({"error": format!("decode_water: {e}")}),
                };
                if query_kind == QueryKind::Waters {
                    let all: Vec<Value> = water.iter().map(water_json).collect();
                    return json!({"water_features": all, "candidate_count": water.len()});
                }
                let at = ptiles_core::water_at(lat, lon, &water);
                json!({
                    "water": at.as_ref().map(area_json),
                    "candidate_count": water.len(),
                })
            }
            Layer::Rail => {
                let rail = match self.decode_all(&cells, ptiles_core::decode_rail) {
                    Ok(r) => r,
                    Err(e) => return json!({"error": format!("decode_rail: {e}")}),
                };
                match query_kind {
                    QueryKind::Rails => {
                        let all: Vec<Value> = rail.iter().map(rail_json).collect();
                        json!({"rail": all, "candidate_count": rail.len()})
                    }
                    QueryKind::Station => {
                        let station = ptiles_core::nearest_station(lat, lon, &rail);
                        json!({
                            "nearest_station": station.as_ref().map(point_json),
                            "candidate_count": rail.len(),
                        })
                    }
                    _ => {
                        let track = ptiles_core::nearest_rail(lat, lon, &rail);
                        json!({
                            "nearest_rail": track.as_ref().map(way_json),
                            "candidate_count": rail.len(),
                        })
                    }
                }
            }
            Layer::Camera => {
                let cameras = match self.decode_all(&cells, ptiles_core::decode_cameras) {
                    Ok(c) => c,
                    Err(e) => return json!({"error": format!("decode_cameras: {e}")}),
                };
                if query_kind == QueryKind::Cameras {
                    let all: Vec<Value> = cameras.iter().map(camera_json).collect();
                    return json!({"cameras": all, "candidate_count": cameras.len()});
                }
                // One-shot against the camera file alone: no buildings are
                // open, so nothing can occlude and every in-range camera
                // reports a clear sight line. `--serve` with a buildings
                // layer loaded is where the occlusion half of the answer
                // comes from; the flag says which answer this is.
                let views = ptiles_core::cameras_seeing(
                    lat,
                    lon,
                    &cameras,
                    &[],
                    ptiles_core::CAMERA_RANGE_M,
                );
                json!({
                    "seen_by": views.iter().map(|v| camera_view_json(v, &cameras)).collect::<Vec<_>>(),
                    "occlusion_checked": false,
                    "candidate_count": cameras.len(),
                })
            }
        }
    }

    /// Decode every block for `cells` with `decode`, concatenated. The
    /// trails/parks/water/rail decoders need neither the header version nor
    /// the cell (unlike roads v2 and business v4), so one helper serves all
    /// four.
    /// These layers ship with a 38-byte index, so one compressed block holds
    /// several cells behind a table; `read_cell` slices the requested one out
    /// (feeding a whole merged block to a record decoder yields garbage
    /// records, not an error -- see `core::merged`).
    fn decode_all<T>(
        &self,
        cells: &[u64],
        decode: fn(&[u8]) -> Result<Vec<T>, ptiles_core::DecodeError>,
    ) -> Result<Vec<T>, ptiles_core::DecodeError> {
        let mut out = Vec::new();
        for &cell in cells {
            let Some(records) = self.file.read_cell(cell) else {
                continue;
            };
            out.append(&mut decode(&records)?);
        }
        Ok(out)
    }
}

/// Find the building whose polygon contains `(lat, lon)`, falling back to
/// the nearest centroid within 50 m if none contains it. Containment comes
/// from `ptiles_core::point_in_polygon`, which the park/water lookups also
/// use -- it lived here as a private copy until those needed it too.
fn find_building(lat: f64, lon: f64, buildings: &[Building]) -> Option<&Building> {
    for b in buildings {
        if point_in_polygon(lat, lon, &b.coords) {
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

/// `{STATE}.business_name_index.ptiles` sidecar location, local or remote --
/// `remote_base` (already `/`-terminated by its callers) takes priority when
/// present, otherwise `<data_dir>/<state>.business_name_index.ptiles`.
fn business_name_index_location(state: &str, remote_base: Option<&str>, data_dir: &Path) -> String {
    match remote_base {
        Some(base) => format!("{base}{state}.business_name_index.ptiles"),
        None => data_dir.join(format!("{state}.business_name_index.ptiles")).to_string_lossy().into_owned(),
    }
}

/// `<data_dir>/<state>.business.ptiles`, or `<remote_base><state>.business.ptiles`
/// -- the main business file, used as the brute-force fallback location
/// when a state has no `business_name_index.ptiles` sidecar.
fn business_location(state: &str, remote_base: Option<&str>, data_dir: &Path) -> String {
    match remote_base {
        Some(base) => format!("{base}{state}.business.ptiles"),
        None => data_dir.join(format!("{state}.business.ptiles")).to_string_lossy().into_owned(),
    }
}

/// Search one state: prefer the `business_name_index.ptiles` sidecar
/// (index-accelerated) when present, falling back to brute-force over the
/// main `business.ptiles` file when it isn't -- true of the real deployed
/// dataset (`https://maps.mydatatimeline.com/maps/`), which only hosts the
/// main business file, not the locally-generated sidecar. Returns `None`
/// only when *neither* file could be opened (caller treats that as a
/// skippable 404, not an error).
fn business_search_one_state(
    state: &str,
    name: &str,
    limit: usize,
    remote_base: Option<&str>,
    data_dir: &Path,
) -> Option<Value> {
    let index_loc = business_name_index_location(state, remote_base, data_dir);
    if let Ok(file) = AnyFile::open(&index_loc) {
        return Some(match file.search_business(name, limit) {
            Ok(hits) => json!({
                "state": state,
                "method": "indexed",
                "hits": hits.iter().map(business_hit_json).collect::<Vec<_>>(),
            }),
            Err(e) => json!({"state": state, "error": e}),
        });
    }

    let business_loc = business_location(state, remote_base, data_dir);
    match AnyFile::open(&business_loc) {
        Ok(file) => Some(match file.search_business_brute_force(name, limit) {
            Ok(hits) => json!({
                "state": state,
                "method": "brute_force",
                "hits": hits.iter().map(business_hit_json).collect::<Vec<_>>(),
            }),
            Err(e) => json!({"state": state, "error": e}),
        }),
        Err(_) => None,
    }
}

/// `--query business-search --name <n> --national`: search every state's
/// name-index sidecar and stream one JSON line per state as results come in
/// (rather than buffering the whole national result set), so a slow scan
/// over many remote states is tolerable to watch. States without a
/// name-index file (a 404 against `--remote-base`, or simply absent from
/// `--data-dir`) are skipped with an `eprintln`, not fatal.
fn run_business_search_national(name: &str, limit: usize, remote_base: Option<&str>, data_dir: &Path) {
    let start = std::time::Instant::now();
    let mut states_searched = 0usize;
    let mut total_hits = 0usize;
    let stdout = io::stdout();
    let mut out = stdout.lock();

    let mut search_and_emit = |state: &str, remote_base: Option<&str>, data_dir: &Path| {
        match business_search_one_state(state, name, limit, remote_base, data_dir) {
            Some(result) => {
                states_searched += 1;
                if let Some(hits) = result.get("hits").and_then(|h| h.as_array()) {
                    total_hits += hits.len();
                }
                let _ = writeln!(out, "{}", serde_json::to_string(&result).unwrap());
                let _ = out.flush();
            }
            None => eprintln!(
                "ptiles-cli --national: skipping {state} (no name-index or business file found)"
            ),
        }
    };

    match remote_base {
        Some(base) => {
            let base = if base.ends_with('/') { base.to_string() } else { format!("{base}/") };
            for &state in ALL_US_STATES {
                search_and_emit(state, Some(&base), data_dir);
            }
        }
        None => {
            let entries = match std::fs::read_dir(data_dir) {
                Ok(e) => e,
                Err(e) => {
                    eprintln!("ptiles-cli --national: cannot read data dir {data_dir:?}: {e}");
                    std::process::exit(1);
                }
            };
            // States present under `data_dir` as either a name-index
            // sidecar or a main business file -- de-duplicated, since a
            // state could have both.
            let mut states: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
            for entry in entries.flatten() {
                let path = entry.path();
                let Some(fname) = path.file_name().and_then(|n| n.to_str()) else { continue };
                if let Some(state) = fname.strip_suffix(".business_name_index.ptiles") {
                    states.insert(state.to_string());
                } else if let Some(state) = fname.strip_suffix(".business.ptiles") {
                    states.insert(state.to_string());
                }
            }
            for state in &states {
                search_and_emit(state, None, data_dir);
            }
        }
    }

    eprintln!(
        "ptiles-cli --national: searched {states_searched} state(s), {total_hits} total hit(s), {:?} elapsed",
        start.elapsed()
    );
}

fn run_business_search_cli(args: &mut pico_args::Arguments) {
    let name: String = args.value_from_str("--name").unwrap_or_else(|e| {
        eprintln!("ptiles-cli: --query business-search requires --name <query> ({e})");
        std::process::exit(2);
    });
    let limit: usize = args.opt_value_from_str("--limit").unwrap_or(None).unwrap_or(50);
    let state: Option<String> = args.opt_value_from_str("--state").unwrap_or(None);
    let national = args.contains("--national");
    let remote_base: Option<String> = args.opt_value_from_str("--remote-base").unwrap_or(None);
    let data_dir: PathBuf = args
        .opt_value_from_str("--data-dir")
        .unwrap_or(None)
        .unwrap_or_else(|| PathBuf::from("/home/aoi/kino/data/ptiles"));

    if national && state.is_some() {
        eprintln!("ptiles-cli: --query business-search: pass --state OR --national, not both");
        std::process::exit(2);
    }
    if !national && state.is_none() {
        eprintln!("ptiles-cli: --query business-search requires --state XX or --national");
        std::process::exit(2);
    }

    if national {
        run_business_search_national(&name, limit, remote_base.as_deref(), &data_dir);
    } else {
        let state = state.unwrap();
        let remote_base = remote_base.map(|b| if b.ends_with('/') { b } else { format!("{b}/") });
        let result = business_search_one_state(&state, &name, limit, remote_base.as_deref(), &data_dir)
            .unwrap_or_else(|| json!({"state": state, "error": "no name-index or business file found"}));
        println!("{}", serde_json::to_string_pretty(&result).unwrap());
    }
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

fn way_json(w: &ptiles_core::NearbyWay) -> Value {
    json!({
        "kind": w.kind,
        "name": w.name,
        "class": w.class,
        "distance_m": w.distance_m,
        "snapped": [w.snapped.0, w.snapped.1],
        "on_it": w.on_it,
    })
}

fn area_json(a: &ptiles_core::NearbyArea) -> Value {
    json!({
        "kind": a.kind,
        "name": a.name,
        "class": a.class,
        "distance_m": a.distance_m,
        "inside": a.inside,
    })
}

fn point_json(p: &ptiles_core::NearbyPoint) -> Value {
    json!({
        "kind": p.kind,
        "name": p.name,
        "class": p.class,
        "lat": p.lat,
        "lon": p.lon,
        "distance_m": p.distance_m,
    })
}

/// `[lon, lat]` decoder order flipped to the `[lat, lon]` this CLI emits
/// everywhere -- same convention as `road_segment_json`'s geometry.
fn geometry_json(coords: &[[f64; 2]]) -> Vec<[f64; 2]> {
    coords.iter().map(|c| [c[1], c[0]]).collect()
}

fn trail_json(t: &ptiles_core::TrailFeature) -> Value {
    json!({
        "osm_id": t.osm_id,
        "name": t.name,
        "trail_type": t.trail_type,
        "geom_type": t.geom_type,
        "surface": t.surface,
        "sac_scale": t.sac_scale,
        "developed": ptiles_core::trail_is_developed(&t.trail_type),
        "geometry": geometry_json(&t.coords),
    })
}

fn park_json(p: &ptiles_core::ParkFeature) -> Value {
    json!({
        "osm_id": p.osm_id,
        "name": p.name,
        "park_type": p.park_type,
        "geometry": geometry_json(&p.coords),
    })
}

fn water_json(w: &ptiles_core::WaterFeature) -> Value {
    json!({
        "osm_id": w.osm_id,
        "name": w.name,
        "water_type": w.water_type,
        "geom_type": w.geom_type,
        "width": w.width,
        "ref_feature_id": w.ref_feature_id,
        "geometry": geometry_json(&w.coords),
    })
}

fn rail_json(r: &ptiles_core::RailFeature) -> Value {
    json!({
        "osm_id": r.osm_id,
        "name": r.name,
        "rail_type": r.rail_type,
        "geom_type": r.geom_type,
        "geometry": geometry_json(&r.coords),
    })
}

fn camera_json(c: &ptiles_core::Camera) -> Value {
    json!({
        "osm_id": c.osm_id,
        "lat": c.lat,
        "lon": c.lon,
        "device_type": c.device_type,
        "placement": c.placement,
        "camera_type": c.camera_type,
        "direction": c.direction,
        "angle": c.angle,
        "operator": c.operator,
        "name": c.name,
        "ref": c.ref_tag,
    })
}

/// One camera's answer to "can it see me", with enough of the camera itself
/// that a caller need not join back to the listing.
fn camera_view_json(v: &ptiles_core::CameraView, cameras: &[ptiles_core::Camera]) -> Value {
    let cam = &cameras[v.index];
    json!({
        "osm_id": v.osm_id,
        "name": cam.name,
        "operator": cam.operator,
        "camera_type": cam.camera_type,
        "lat": cam.lat,
        "lon": cam.lon,
        "distance_m": v.distance_m,
        "bearing_deg": v.bearing_deg,
        "aimed_at_you": v.aimed_at_you,
        "aim_assumed": v.aim_assumed,
        "line_of_sight": v.line_of_sight,
        "blocked_by": v.blocked_by,
        "sees": v.sees,
    })
}

fn business_hit_json(h: &BusinessHit) -> Value {
    json!({
        "name": h.name,
        "lat": h.lat,
        "lon": h.lon,
        "category_idx": h.category_idx,
        "score": h.score,
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
        "source_type": b.source_type,
        "source_id": b.source_id,
        "confidence": b.confidence,
    })
}

// --- --serve mode ---------------------------------------------------------

/// One state's set of opened layer files (only the layers this CLI queries).
#[derive(Default)]
struct StateFiles {
    roads: Option<OpenedLayer>,
    buildings: Option<OpenedLayer>,
    business: Option<OpenedLayer>,
    trails: Option<OpenedLayer>,
    parks: Option<OpenedLayer>,
    water: Option<OpenedLayer>,
    rail: Option<OpenedLayer>,
    camera: Option<OpenedLayer>,
    /// `business_name_index.ptiles` sidecar, when present. Not an
    /// `OpenedLayer` -- it's not one of the `Layer` variants (a different
    /// index shape, see `core::business_search`), so it's stored as a bare
    /// `AnyFile` and searched via `AnyFile::search_business`.
    name_index: Option<AnyFile>,
}

impl StateFiles {
    fn set(&mut self, layer: Layer, opened: OpenedLayer) {
        let slot = match layer {
            Layer::Roads => &mut self.roads,
            Layer::BuildingsV8 => &mut self.buildings,
            Layer::Business => &mut self.business,
            Layer::Trails => &mut self.trails,
            Layer::Parks => &mut self.parks,
            Layer::Water => &mut self.water,
            Layer::Rail => &mut self.rail,
            Layer::Camera => &mut self.camera,
        };
        *slot = Some(opened);
    }

    /// The decoded features `locate` needs from a layer this state may not
    /// have. A missing file is not an error: it just contributes nothing.
    fn decode_from<T>(
        layer: &Option<OpenedLayer>,
        lat: f64,
        lon: f64,
        ring: u32,
        decode: fn(&[u8]) -> Result<Vec<T>, ptiles_core::DecodeError>,
    ) -> Vec<T> {
        match layer {
            Some(l) => l
                .decode_all(&l.cells_for(lat, lon, ring), decode)
                .unwrap_or_default(),
            None => Vec::new(),
        }
    }
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
        let Some(path_str) = path.to_str() else {
            continue;
        };

        if layer_token == "business_name_index" {
            match AnyFile::open(path_str) {
                Ok(file) => {
                    states
                        .entry(state.to_string())
                        .or_default()
                        .name_index = Some(file);
                }
                Err(e) => eprintln!("ptiles-cli --serve: skipping {path:?}: {e}"),
            }
            continue;
        }

        let Some(layer) = Layer::from_filename_token(layer_token) else {
            continue;
        };
        let opened = match OpenedLayer::open(path_str, layer) {
            Ok(o) => o,
            Err(e) => {
                eprintln!("ptiles-cli --serve: skipping {path:?}: {e}");
                continue;
            }
        };
        states
            .entry(state.to_string())
            .or_default()
            .set(layer, opened);
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
        let mut entry = StateFiles::default();
        for layer in [
            Layer::Roads,
            Layer::BuildingsV8,
            Layer::Business,
            Layer::Trails,
            Layer::Parks,
            Layer::Water,
            Layer::Rail,
            Layer::Camera,
        ] {
            let url = format!("{base}{state}.{}.ptiles", layer.as_str());
            match OpenedLayer::open(&url, layer) {
                Ok(opened) => entry.set(layer, opened),
                Err(e) => {
                    eprintln!("ptiles-cli --serve --remote-base: skipping {url}: {e}");
                }
            }
        }
        // Sidecar name-index file: rarely hosted remotely (the real
        // deployed dataset only serves the main business file), so a 404
        // here is expected and just means business_search falls back to
        // brute-force -- not logged as loudly as the three layers above.
        let name_index_url = format!("{base}{state}.business_name_index.ptiles");
        if let Ok(file) = AnyFile::open(&name_index_url) {
            entry.name_index = Some(file);
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

/// `{"query":"business_search","name":..,"state":?,"limit":?}` handler --
/// see `handle_serve_line`, which dispatches here before its own `lat`/`lon`
/// requirement. `state` falls back the same way `handle_serve_line` does
/// (sole loaded state, or an error if ambiguous). Prefers the state's
/// pre-loaded `name_index` sidecar (index-accelerated); falls back to
/// brute-force over the pre-loaded `business` layer's file when no sidecar
/// was loaded for that state (matching the one-shot CLI path's fallback).
fn handle_business_search_line(req: &Value, states: &HashMap<String, StateFiles>) -> Value {
    let name = match req.get("name").and_then(Value::as_str) {
        Some(n) => n,
        None => return json!({"error": "missing or non-string \"name\""}),
    };
    let limit = req.get("limit").and_then(Value::as_u64).unwrap_or(50) as usize;

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

    if let Some(file) = &state_files.name_index {
        return match file.search_business(name, limit) {
            Ok(hits) => json!({
                "method": "indexed",
                "hits": hits.iter().map(business_hit_json).collect::<Vec<_>>(),
            }),
            Err(e) => json!({"error": e}),
        };
    }
    if let Some(business_layer) = &state_files.business {
        return match business_layer.file.search_business_brute_force(name, limit) {
            Ok(hits) => json!({
                "method": "brute_force",
                "hits": hits.iter().map(business_hit_json).collect::<Vec<_>>(),
            }),
            Err(e) => json!({"error": e}),
        };
    }
    json!({"error": "no business_name_index or business layer loaded for this state"})
}

fn handle_serve_line(line: &str, states: &HashMap<String, StateFiles>) -> Value {
    let req: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return json!({"error": format!("invalid JSON: {e}")}),
    };

    // `{"query":"business_search","name":..,"state":?,"limit":?}`: business
    // name search, not a lat/lon lookup -- handled before `lat`/`lon` are
    // required below, since this request shape doesn't carry them.
    if req.get("query").and_then(Value::as_str) == Some("business_search") {
        return handle_business_search_line(&req, states);
    }

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

    // Cross-layer reverse geocode: roads and trails compete on distance
    // alone (see `core::locate`), and the park/water the point falls in are
    // reported alongside. No address layer is opened by `--serve`, so the
    // address slot of `core::locate` stays empty here; `--query address`
    // against an address file answers that.
    if query_kind == QueryKind::Locate {
        let roads = StateFiles::decode_from(&state_files.roads, lat, lon, ring, decode_roads);
        let trails =
            StateFiles::decode_from(&state_files.trails, lat, lon, ring, ptiles_core::decode_trails);
        let parks =
            StateFiles::decode_from(&state_files.parks, lat, lon, ring, ptiles_core::decode_parks);
        let water =
            StateFiles::decode_from(&state_files.water, lat, lon, ring, ptiles_core::decode_water);
        let located = ptiles_core::locate(lat, lon, &roads, &trails, &[]);
        return json!({
            "nearest_way": located.nearest_way.as_ref().map(way_json),
            "on_way": located.on_way.as_ref().map(way_json),
            "park": ptiles_core::park_at(lat, lon, &parks).as_ref().map(area_json),
            "water": ptiles_core::water_at(lat, lon, &water).as_ref().map(area_json),
        });
    }

    // "Can a camera see me": the camera layer answers who is in range and
    // aimed at you, the buildings layer answers what stands in the way. A
    // state with no buildings file still gets the first half, flagged, rather
    // than a silently optimistic clear sight line.
    if query_kind == QueryKind::Camera {
        if state_files.camera.is_none() {
            return json!({"error": "no camera layer loaded for this state"});
        }
        let cameras = StateFiles::decode_from(
            &state_files.camera,
            lat,
            lon,
            ring,
            ptiles_core::decode_cameras,
        );
        let buildings = match &state_files.buildings {
            Some(layer) => layer.candidates_for(lat, lon, ring).1,
            None => Vec::new(),
        };
        let view_buildings: Vec<ptiles_core::ViewBuilding> = buildings
            .iter()
            .map(|b| ptiles_core::ViewBuilding {
                coords: b.coords.clone(),
                height_m: b.height_m,
                building_type: b.building_type.clone(),
            })
            .collect();
        let views = ptiles_core::cameras_seeing(
            lat,
            lon,
            &cameras,
            &view_buildings,
            ptiles_core::CAMERA_RANGE_M,
        );
        return json!({
            "seen_by": views.iter().map(|v| camera_view_json(v, &cameras)).collect::<Vec<_>>(),
            "occlusion_checked": state_files.buildings.is_some(),
            "candidate_count": cameras.len(),
        });
    }

    // Single-layer queries against the layers `--serve` opens beyond the
    // scoring three. Answered by the same `OpenedLayer::query` the one-shot
    // path uses, so the two cannot drift; a state missing that file says so
    // rather than silently answering with a different layer's shape.
    let single = match query_kind {
        QueryKind::Trail | QueryKind::Trails | QueryKind::Trailhead => {
            Some((Layer::Trails, &state_files.trails))
        }
        QueryKind::Park | QueryKind::Parks => Some((Layer::Parks, &state_files.parks)),
        QueryKind::Water | QueryKind::Waters => Some((Layer::Water, &state_files.water)),
        QueryKind::Rail | QueryKind::Rails | QueryKind::Station => {
            Some((Layer::Rail, &state_files.rail))
        }
        QueryKind::Cameras => Some((Layer::Camera, &state_files.camera)),
        _ => None,
    };
    if let Some((kind, slot)) = single {
        return match slot {
            Some(layer) => layer.query(lat, lon, ring, query_kind),
            None => json!({"error": format!("no {} layer loaded for this state", kind.as_str())}),
        };
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_kind_parse_known_and_unknown() {
        assert_eq!(QueryKind::parse("road"), Some(QueryKind::Road));
        assert_eq!(QueryKind::parse("roads"), Some(QueryKind::Roads));
        assert_eq!(QueryKind::parse("intersection"), Some(QueryKind::Intersection));
        assert_eq!(QueryKind::parse("building"), Some(QueryKind::Buildings));
        assert_eq!(QueryKind::parse("buildings"), Some(QueryKind::Buildings));
        assert_eq!(QueryKind::parse("business"), Some(QueryKind::Business));
        assert_eq!(QueryKind::parse("all"), Some(QueryKind::All));
        assert_eq!(QueryKind::parse("nope"), None);
        assert_eq!(QueryKind::parse(""), None);
    }

    #[test]
    fn query_kind_wants_layer_routing() {
        assert!(QueryKind::All.wants(Layer::Roads));
        assert!(QueryKind::All.wants(Layer::BuildingsV8));
        assert!(QueryKind::All.wants(Layer::Business));
        assert!(QueryKind::Road.wants(Layer::Roads));
        assert!(!QueryKind::Road.wants(Layer::Business));
        assert!(QueryKind::Roads.wants(Layer::Roads));
        assert!(!QueryKind::Buildings.wants(Layer::Roads));
        assert!(QueryKind::Buildings.wants(Layer::BuildingsV8));
        assert!(QueryKind::Business.wants(Layer::Business));
        assert!(!QueryKind::Business.wants(Layer::BuildingsV8));
        // Singular is the lookup, plural is the listing -- both against the
        // same layer, and never against another one.
        for (kind, layer) in [
            (QueryKind::Trail, Layer::Trails),
            (QueryKind::Trails, Layer::Trails),
            (QueryKind::Trailhead, Layer::Trails),
            (QueryKind::Park, Layer::Parks),
            (QueryKind::Parks, Layer::Parks),
            (QueryKind::Water, Layer::Water),
            (QueryKind::Waters, Layer::Water),
            (QueryKind::Rail, Layer::Rail),
            (QueryKind::Rails, Layer::Rail),
            (QueryKind::Station, Layer::Rail),
        ] {
            assert!(kind.wants(layer), "{kind:?} should want {layer:?}");
            assert!(!kind.wants(Layer::Business), "{kind:?} must not want business");
        }
        // `locate` is cross-layer: roads and trails feed it, nothing else.
        assert!(QueryKind::Locate.wants(Layer::Roads));
        assert!(QueryKind::Locate.wants(Layer::Trails));
        assert!(!QueryKind::Locate.wants(Layer::Parks));
    }

    #[test]
    fn layer_from_filename_token_and_roundtrip() {
        assert_eq!(Layer::from_filename_token("roads"), Some(Layer::Roads));
        assert_eq!(Layer::from_filename_token("buildings_v8"), Some(Layer::BuildingsV8));
        assert_eq!(Layer::from_filename_token("business"), Some(Layer::Business));
        assert_eq!(Layer::from_filename_token("water"), Some(Layer::Water));
        assert_eq!(Layer::from_filename_token("trails_v1"), Some(Layer::Trails));
        assert_eq!(Layer::from_filename_token("places"), None);
        assert_eq!(Layer::from_filename_token("business_name_index"), None);
        assert_eq!(Layer::Roads.as_str(), "roads");
        assert_eq!(Layer::BuildingsV8.as_str(), "buildings_v8");
        assert_eq!(Layer::Business.as_str(), "business");
        // `as_str` is what `--serve --remote-base` builds URLs from, so each
        // variant must round-trip back to itself.
        for layer in [
            Layer::Roads,
            Layer::Business,
            Layer::Trails,
            Layer::Parks,
            Layer::Water,
            Layer::Rail,
        ] {
            assert_eq!(Layer::from_filename_token(layer.as_str()), Some(layer));
        }
    }

    #[test]
    fn layer_from_path_local_and_url() {
        assert_eq!(layer_from_path("/data/TN.roads.ptiles"), Some(Layer::Roads));
        assert_eq!(
            layer_from_path("https://host/maps/TN.buildings_v8.ptiles"),
            Some(Layer::BuildingsV8)
        );
        assert_eq!(layer_from_path("http://host/US.business.ptiles"), Some(Layer::Business));
        // Unknown 2nd token and missing token both yield None.
        // Versioned stems from the published snapshots resolve the same way.
        assert_eq!(
            layer_from_path("https://host/maps/TN.business_v4.ptiles"),
            Some(Layer::Business)
        );
        assert_eq!(layer_from_path("/data/TN.roads_v2.ptiles"), Some(Layer::Roads));
        assert_eq!(
            layer_from_path("/data/TN.buildings_v9.ptiles"),
            Some(Layer::BuildingsV8)
        );
        assert_eq!(layer_from_path("/data/TN.water_v1.ptiles"), Some(Layer::Water));
        assert_eq!(layer_from_path("/data/TN.water.ptiles"), Some(Layer::Water));
        assert_eq!(layer_from_path("/data/TN.places.ptiles"), None);
        assert_eq!(layer_from_path("/data/noextension"), None);
    }

    #[test]
    fn is_url_scheme_sniff() {
        assert!(is_url("http://x/y"));
        assert!(is_url("https://x/y"));
        assert!(!is_url("/local/path.ptiles"));
        assert!(!is_url("ftp://x"));
        assert!(!is_url(""));
    }

    #[test]
    fn validate_ring_bounds() {
        assert!(validate_ring(0).is_ok());
        assert!(validate_ring(1).is_ok());
        assert!(validate_ring(2).is_err());
        assert!(validate_ring(99).is_err());
    }

    #[test]
    fn parse_bounds_valid() {
        assert_eq!(
            parse_bounds("36.0, -87.0, 36.2, -86.6").unwrap(),
            [36.0, -87.0, 36.2, -86.6]
        );
        assert_eq!(parse_bounds("1,2,3,4").unwrap(), [1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn parse_bounds_wrong_arity_and_nonnumeric() {
        assert!(parse_bounds("1,2,3").is_err(), "3 values must be rejected");
        assert!(parse_bounds("1,2,3,4,5").is_err(), "5 values must be rejected");
        assert!(parse_bounds("1,2,three,4").is_err(), "non-numeric must be rejected");
        assert!(parse_bounds("").is_err());
    }

    #[test]
    fn point_in_polygon_basics() {
        // Unit square (lon/lat pairs). Point inside, outside, and a degenerate
        // ring (<3 points) which must never be "inside".
        let square = [[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]];
        assert!(point_in_polygon(0.5, 0.5, &square));
        assert!(!point_in_polygon(2.0, 2.0, &square));
        assert!(!point_in_polygon(0.5, 0.5, &[[0.0, 0.0], [1.0, 1.0]]));
    }

    #[test]
    fn index_location_helpers_local_and_remote() {
        let dir = Path::new("/data");
        assert_eq!(
            business_name_index_location("TN", None, dir),
            "/data/TN.business_name_index.ptiles"
        );
        assert_eq!(
            business_name_index_location("TN", Some("https://h/maps/"), dir),
            "https://h/maps/TN.business_name_index.ptiles"
        );
        assert_eq!(business_location("TN", None, dir), "/data/TN.business.ptiles");
        assert_eq!(
            business_location("TN", Some("https://h/maps/"), dir),
            "https://h/maps/TN.business.ptiles"
        );
    }

    #[test]
    fn candidates_json_shape() {
        let cands = vec![Candidate {
            kind: CandidateKind::Road,
            osm_id: 42,
            name: Some("Main St".to_string()),
            distance_m: 5.0,
            score: 0.9,
        }];
        let v = candidates_json(&cands);
        let arr = v.as_array().expect("array");
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["kind"], "road");
        assert_eq!(arr[0]["osm_id"], 42);
        assert_eq!(arr[0]["name"], "Main St");
    }

    // --- serve-line dispatch error paths (no data files needed) ------------

    fn empty_states() -> HashMap<String, StateFiles> {
        HashMap::new()
    }

    #[test]
    fn serve_line_invalid_json_errors() {
        let r = handle_serve_line("not json", &empty_states());
        assert!(r.get("error").is_some(), "invalid JSON must produce an error line: {r}");
    }

    #[test]
    fn serve_line_missing_lat_lon_errors() {
        let r = handle_serve_line(r#"{"lon":-86.78}"#, &empty_states());
        assert!(r["error"].as_str().unwrap().contains("lat"));
        let r = handle_serve_line(r#"{"lat":36.16}"#, &empty_states());
        assert!(r["error"].as_str().unwrap().contains("lon"));
    }

    #[test]
    fn serve_line_unknown_query_errors() {
        let r = handle_serve_line(r#"{"lat":36.16,"lon":-86.78,"query":"bogus"}"#, &empty_states());
        assert!(r["error"].as_str().unwrap().contains("unknown query"));
    }

    #[test]
    fn serve_line_bad_ring_errors() {
        let r = handle_serve_line(
            r#"{"lat":36.16,"lon":-86.78,"query":"all","ring":2}"#,
            &empty_states(),
        );
        assert!(r["error"].as_str().unwrap().contains("ring"));
    }

    #[test]
    fn serve_line_unknown_state_errors() {
        let r = handle_serve_line(
            r#"{"lat":36.16,"lon":-86.78,"state":"ZZ"}"#,
            &empty_states(),
        );
        assert!(r["error"].as_str().unwrap().contains("unknown state"));
    }

    #[test]
    fn serve_line_no_state_with_zero_loaded_errors() {
        // No state specified and zero states loaded -> ambiguity error, not a
        // panic on states.values().next().
        let r = handle_serve_line(r#"{"lat":36.16,"lon":-86.78}"#, &empty_states());
        assert!(r.get("error").is_some());
    }

    #[test]
    fn business_search_line_missing_name_errors() {
        let r = handle_business_search_line(
            &json!({"query": "business_search"}),
            &empty_states(),
        );
        assert!(r["error"].as_str().unwrap().contains("name"));
    }

    #[test]
    fn business_search_line_no_layers_errors() {
        // Name present but no state/layers loaded -> ambiguity error (zero
        // states), exercising the state-selection branch without I/O.
        let r = handle_business_search_line(
            &json!({"query": "business_search", "name": "waffle"}),
            &empty_states(),
        );
        assert!(r.get("error").is_some());
    }
}
