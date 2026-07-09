# Browser Corridor Routing Implementation Plan

> **For Hermes:** Use subagent-driven-development skill to implement this plan task-by-task.

**Goal:** Point-to-point on-road routing in the browser (WASM + Range-fetched `.roads.ptiles` only), fast enough for a phone.

**Architecture:** Port the proven single-pass corridor from `timeline/ptiles/src/router.rs` into `ptiles-core`, export thin wasm. No highway sidecar, no PTILESU, no daemon. Efficiency comes from **fewer cells + fewer edges in the graph**, not a second format.

**Tech Stack:** `ptiles-core` (no_std-friendly graph), `ptiles-wasm` (`route` export), existing Range open in ptiles2 / demo JS.

---

## Constraints (do not violate)

| Rule                             | Why                                                        |
| -------------------------------- | ---------------------------------------------------------- |
| Only base `*.roads.ptiles`       | User: no external services; highway files deleted          |
| No new PTILES magic for v1       | Roads already have geometry + class + oneway + speed       |
| No whole-state graph             | Phone RAM; GOAL peak ~300MB native was already the ceiling |
| Highways-only backbone abandoned | GOAL.md: fragmented at state borders                       |

## What failed before (do not revive)

```
highways extract → graph only motorway/trunk/primary → A*
```

State-border cells often have no continuous high-class path. Origin/dest live on residential. Two-phase stitch was brittle.

## What works (reuse)

From `timeline/ptiles/src/router.rs` + `ptiles/GOAL.md`:

1. **Corridor cells** along great-circle (`corridor_tiles`)
2. **Single-pass** all `profile_allows("driving")` classes in those cells
3. **Snap endpoints** by microdegree grid merge (`build_graph` 50_000 scale)
4. **A\*** with haversine time heuristic (130 km/h cap)
5. **Widen corridor** once if origin/dest not same component

Native Nashville→Memphis ~8s / ~300MB was phone-daemon ok; browser must be **tighter** (Range RTT + WASM heap).

---

## Efficiency strategy (browser / phone)

### Budget targets (acceptance)

| Distance | Max wall time (warm network)         | Max decompressed graph RAM |
| -------- | ------------------------------------ | -------------------------- |
| ≤5 km    | ≤1 s                                 | ≤30 MB                     |
| ≤50 km   | ≤3 s                                 | ≤80 MB                     |
| ≤200 km  | ≤8 s                                 | ≤150 MB                    |
| >200 km  | progressive path OK; full path ≤15 s | ≤200 MB hard abort         |

If over budget: widen is **not** free — abort with `"widen would exceed budget"` and return partial arterial attempt if any.

### Levers (in order — stop when budget met)

1. **Corridor, not rings** — already in daemon for long routes.
2. **Class LOD _inside_ the corridor** (not a second file):
   - **End caps** (k-ring 1–2 around A and B): all driving classes (need residential connect).
   - **Middle cells**: only `motorway..tertiary` (+ links). Same continuous graph, fewer edges; no border-only highway isolation because ends still have locals.
3. **Vertex decimation on middle edges** — keep endpoints + every Nth vertex for geometry; graph uses segment endpoints only (already true if you only node polyline vertices that are junctions… **ponytail:** keep daemon node-every-vertex first; decimate only if node count > ~200k).
4. **Bidirectional A\*** when `node_count > 50_000` — same weights, meet in middle; ~2× fewer expansions on long corridors.
5. **Parallel Range decompress** — existing zstd workers; cap 4.
6. **Progressive UI** — paint arterial path as soon as middle connects; refine ends.

### Explicit non-goals (v1)

- PTILESU portal matrices
- CH / CRP / ALT preprocess
- Highway sidecar files
- Turn restrictions / lanes
- Multi-modal transit

Add only if phone budgets fail on real TN/cross-state routes after LOD + bi-A\*.

---

## Data flow

```
A,B → corridor_tiles(interval, width)
    → Range-fetch + zstd each missing cell block (cache)
    → decode_roads (existing wasm)
    → filter edges: end-cap full / middle arterial
    → build_graph (port)
    → bi-A* or A*
    → geometry polyline [lat,lon]...
```

JS owns I/O. WASM owns graph + search. Matches existing no-I/O wasm contract.

---

### Task 1: Port `build_graph` + A\* into ptiles-core

**Objective:** Pure core routing primitives with no H3 dependency beyond cell list input.

**Files:**

- Create: `ptiles-client/core/src/route_graph.rs`
- Modify: `ptiles-client/core/src/lib.rs` (mod + re-export)
- Test: unit tests in `route_graph.rs`

**Step 1: Failing test — merge endpoints + shortest path**

```rust
#[test]
fn two_segments_share_endpoint_and_path() {
    // A--B and B--C residential; route A→C length ~ sum
}
```

**Step 2:** Implement minimal:

- `build_graph(roads, profile) -> Graph { adj, coords_geo }`
- Port microdegree key merge from daemon (`* 50_000`)
- Port `profile_allows` + speed table (copy constants; no refactor dance)
- `astar(src, dst)` single-source first

**Step 3:** `cargo test -p ptiles-core route_graph -- --nocapture`

**Step 4:** Commit when green.

---

### Task 2: Corridor cell list in core (or JS)

**Objective:** Produce H3 res-7 cell hex list for A→B without loading roads.

**Files:**

- Prefer reuse: wasm already has `cell_for_coord`, `neighbor_cells` / h3 in JS
- Create only if needed: `core/src/corridor.rs` sampling great-circle → cells

**Ponytail default:** implement `corridorCells(lat1,lon1,lat2,lon2,width,intervalM)` in **JS** with `h3-js` (already on ptiles2). Core stays graph-only.

Adaptive table (from GOAL, phone-tighter):

| dist    | interval | width (gridDisk k)       |
| ------- | -------- | ------------------------ |
| <50 km  | 2 km     | 3                        |
| 50–200  | 3 km     | 4                        |
| 200–500 | 5 km     | 5                        |
| >500    | 8 km     | 5 + middle arterial-only |

---

### Task 3: Class LOD filter before build_graph

**Objective:** Cut edge count without highway files.

**Files:**

- `core/src/route_graph.rs` — `fn keep_road(class, zone: EndCap|Middle) -> bool`

```rust
// ponytail: end caps need residential; middle is arterial spine
fn keep_road(class: &str, middle: bool) -> bool {
    if !middle { return profile_allows("driving", class); }
    matches!(class,
        "motorway"|"motorway_link"|"trunk"|"trunk_link"|
        "primary"|"primary_link"|"secondary"|"secondary_link"|
        "tertiary"|"tertiary_link")
}
```

**Test:** synthetic graph where middle-only residential would disconnect; with end caps connected.

---

### Task 4: Bidirectional A\* (gate on size)

**Objective:** Long corridors stay interactive on phone.

**Files:** `route_graph.rs`

- If `n <= 50_000`: existing A\*
- Else: bi-A\* meet-in-middle on time weights

**Test:** same path cost as unidirectional A\* on small fixture (equality).

---

### Task 5: Wasm export `route_on_roads`

**Objective:** JS passes pre-decoded segments (or raw blocks) + A/B; wasm returns path.

**Files:**

- Modify: `ptiles-client/wasm/src/lib.rs`
- Rebuild: `wasm-pack build --target web --out-dir .../ptiles2/lib/client --out-name ptiles_client`

**API (minimal):**

```ts
// option A (ponytail): JS decodes blocks, passes flat arrays
route_from_segments(segments_js, lat1, lon1, lat2, lon2) -> {
  distance_m, duration_s, path: [[lat,lon],...]
} | null
```

Prefer option A: no zstd in routing wasm path; reuse existing decompress.

**Snap:** nearest node within 100 m of A and of B (reuse proximity helpers if cheap; else brute nodes once).

---

### Task 6: ptiles2 Route mode uses graph

**Objective:** Replace crow-flies with real path.

**Files:**

- Modify: `steele.red/ptiles2/index.html` `nearestRoadAt` / `handleRouteClick`

Flow:

1. Route ON → click A, click B
2. `corridorCells` → decompress cells (roads reader)
3. `decode_roads` each → concat segments with zone tags
4. `route_from_segments`
5. Draw path; status `12.3 km · 14 min`
6. Fail → one widen (width+2) if under budget; else error

**No OSRM. No :9352.**

---

### Task 7: Phone budget guards

**Objective:** Never OOM tab.

**Files:** same JS + core

- Cap corridor cells at e.g. **800** (abort)
- Cap nodes after build at **250_000** (abort)
- Stream status: `loading 40/120 cells…`
- `blockCache` already exists — keep

**Test:** unit test abort when synthetic cap hit.

---

### Task 8: Golden route vs daemon (optional offline)

**Objective:** Same A/B as GOAL Nashville→Memphis within ~25% distance of daemon (not OSRM).

**Files:** `core/tests/route_smoke.rs` behind `#[ignore]` if no TN.roads on CI.

Run local only:

```bash
cargo test -p ptiles-core route_smoke -- --ignored --nocapture
```

---

## File touch summary

| Path                                    | Change                           |
| --------------------------------------- | -------------------------------- |
| `ptiles-client/core/src/route_graph.rs` | **new** graph + A* + bi-A* + LOD |
| `ptiles-client/core/src/lib.rs`         | export                           |
| `ptiles-client/wasm/src/lib.rs`         | `route_from_segments`            |
| `steele.red/ptiles2/index.html`         | corridor JS + call wasm          |
| `ptiles-client/docs/plans/…`            | this plan                        |

Do **not** touch: highway extract, PTILESU builder, docker `ptiles-route`.

---

## Acceptance criteria

- [ ] Route A→B in Memphis metro draws road-following polyline (not dashed crow)
- [ ] Status shows distance + duration from graph weights
- [ ] No network calls except `maps…/TN.roads.ptiles` Range
- [ ] ≤50 km on desktop Chromium ≤3 s after roads index open
- [ ] Widen once max; no infinite ring expand
- [ ] `cargo test -p ptiles-core` green
- [ ] No new `.ptiles` format

---

## Gotchas

1. **Coord order:** roads `coords` are `[lon,lat]`; Leaflet wants `[lat,lon]`. Daemon `build_graph` geo is `[lon,lat]` — keep one convention and convert at draw boundary only.
2. **Node merge threshold:** daemon uses 50_000 micro (2e-5 deg ≈ 2 m). Do not invent a second grid.
3. **Oneway:** respect flags or routes will cut the wrong way and look “broken”.
4. **WASM memory:** one graph alloc; free/drop between routes. No global highway cache.
5. **Middle arterial-only** can still fail in rural gaps — widen + temporarily allow secondary residential in middle on retry **before** adding files.
6. GOAL native times include full disk; browser is Range-bound — parallelize cells, don’t serial-for-loop without `Promise.all` batches (~12).

---

## Order of work

```
Task1 graph → Task3 LOD → Task4 bi-A* → Task5 wasm → Task2 corridor JS → Task6 UI → Task7 caps
```

Task8 only if local TN.roads available.

---

## skipped (add when)

| Skipped                                               | Add when                                                                                                 |
| ----------------------------------------------------- | -------------------------------------------------------------------------------------------------------- |
| PTILESU portals                                       | US-scale interactive after LOD+bi-A\* still >15s                                                         |
| CH preprocess                                         | product needs <100 ms repeated routes on same state                                                      |
| Highway sidecar                                       | never for connectivity; only as optional size win if middle LOD insufficient **and** border stitch fixed |
| Bidirectional Dijkstra / fancy heuristics             | bi-A\* not enough                                                                                        |
| Full daemon feature parity (profiles walking/cycling) | driving works; copy `profile_allows` branches                                                            |

---

## One-line thesis

**Same roads file, thinner corridor graph (end-cap full + middle arterial + A\*/bi-A\*), not a second layer.**
