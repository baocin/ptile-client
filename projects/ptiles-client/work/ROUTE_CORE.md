# GOAL

Implement browser-ready corridor graph routing in ptiles-core + thin wasm export. No highway files, no PTILESU, no I/O in wasm.

# WORKDIR

/home/aoi/kino/projects/ptiles-client

# DO NOT TOUCH

- /home/aoi/kino/projects/steele.red (parent does UI later)
- docker / daemon / timeline ptiles (reference only)
- highway extract / PTILESU
- New dependencies beyond what's already in Cargo.toml

# AUTHORITY (conflicts)

1. /home/aoi/kino/projects/ptiles-client/docs/plans/2026-07-09-browser-corridor-routing.md
2. /home/aoi/kino/projects/ptiles/GOAL.md (highways-only backbone abandoned; single-pass corridor)
3. Daemon graph logic to port from: /home/aoi/kino/projects/timeline/ptiles/src/router.rs (`build_graph`, `astar_with_pred`, `profile_allows`, speed table, 50_000 micro merge)
4. Existing: core/src/roads.rs RoadSegment coords are [lon, lat]; core/src/proximity.rs has haversine_distance_m

# PONYTAIL FULL

- Shortest working code
- no_std + alloc friendly (core already is)
- No fancy abstractions
- One unit-test module with assert-based tests in the same file
- Mark shortcuts with `// ponytail: ...`

# TASKS

## 1. Create core/src/route_graph.rs

Implement:

```rust
pub struct RouteResult {
    pub distance_m: f64,
    pub duration_s: f64,
    pub path: Vec<[f64; 2]>, // [lat, lon] for Leaflet convenience
}

/// zone_middle[i] = true means road i is in corridor middle (arterial-only filter)
pub fn route_roads(
    roads: &[RoadSegment],
    zone_middle: &[bool], // len == roads.len() or empty = all end-cap (all driving)
    lat1: f64, lon1: f64,
    lat2: f64, lon2: f64,
    snap_m: f64, // default 100
) -> Option<RouteResult>
```

Port from daemon (copy constants, don't over-abstract):

- Node key: `(lon*50_000).round() as i32`, same for lat (daemon build_graph)
- profile_allows driving only (v1)
- Speed table from daemon router.rs (~805-826)
- oneway: if oneway is Some("forward") only A→B edge; "reverse" only B→A; else both
- Weight = time centiseconds like daemon (meters/speed \* 100)
- Keep road if: !middle || class in motorway..tertiary (+ links)
- Snap A/B to nearest node within snap_m via haversine_distance_m
- A\* with haversine heuristic at 130 km/h (admissible) — copy daemon idea
- If node_count > 50_000: bidirectional A* meeting in middle; else uni A*
- Cap: if nodes > 250_000 after build return None
- Reconstruct path as [lat,lon] (daemon geo is [lon,lat] — convert)

Also export:

```rust
pub fn profile_allows_driving(class: &str) -> bool
pub fn keep_road_class(class: &str, middle: bool) -> bool
```

## 2. Wire core/src/lib.rs

```rust
pub mod route_graph;
pub use route_graph::{route_roads, RouteResult, keep_road_class};
```

## 3. Unit tests in route_graph.rs

At least:

1. Two collinear residential segments sharing endpoint → path length ≈ sum, not crow
2. Middle filter drops residential in middle but end-cap residential still connects synthetic A-B via arterial middle
3. Oneway forward prevents reverse route (None or long way)
4. Empty roads → None

Run: `cargo test -p ptiles-core route_graph -- --nocapture`
Must PASS.

## 4. Wasm export in wasm/src/lib.rs

```rust
#[wasm_bindgen]
pub fn route_from_segments(
    segments_js: JsValue, // array of {coords:[[lon,lat],...], road_class:string, oneway?:string|null, speed_limit_kmh?:number|null, name?:...}
    zone_middle: JsValue, // array of bool, same length, or empty/null
    lat1: f64, lon1: f64,
    lat2: f64, lon2: f64,
    snap_m: Option<f64>,
) -> Result<JsValue, JsValue>
```

Deserialize segments into Vec<RoadSegment> (only fields needed; osm_id=0 ok).
Call route_roads. Return null if None, else `{distance_m, duration_s, path: [[lat,lon],...]}` via existing `to_js`.

## 5. Build wasm (optional if slow)

```bash
cd /home/aoi/kino/projects/ptiles-client/wasm && wasm-pack build --target web --out-dir ../demo/pkg --out-name ptiles_wasm 2>&1 | tail -20
```

If wasm-pack fails env, still leave Rust green; parent can rebuild.

# PROGRESS

- Append one line to /home/aoi/kino/projects/ptiles-client/work/ROUTE_CORE.log after each major step: ISO time + what changed
- On finish write /home/aoi/kino/projects/ptiles-client/work/ROUTE_CORE_DONE.md with absolute paths + `cargo test` summary
- On fail write /home/aoi/kino/projects/ptiles-client/work/BLOCKER.md (one line) AND ROUTE_CORE_DONE.md with FAIL

# DONE WHEN

- [ ] cargo test -p ptiles-core route_graph passes
- [ ] route_from_segments exists in wasm/src/lib.rs
- [ ] ROUTE_CORE_DONE.md written
- On fail: BLOCKER.md with one-line reason

# RULES

- Ponytail full
- No sudo
- No fabricated tool output
- Do not invent binary artifacts
- Read daemon build_graph/astar before coding (router.rs ~753-1084)
