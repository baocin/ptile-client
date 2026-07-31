# Handoff: ptiles browser corridor routing

**Date:** 2026-07-09  
**Repos:** `steele.red/ptiles` + ptiles-client (core/wasm)  
**Status:** Short same-city routes often work. Same-state city pairs (~50–300 km) and cross-state routes can still fail, but the browser now retries failed long routes with a denser bounded arterial corridor. Not production-ready.

---

## Goal

Point-to-point on-road routing in the browser using only Range-fetched `*.roads.ptiles` + WASM. No OSRM, no routing daemon, no PTILESU.

## What ships where

| Piece                | Path                                                              | Role                                                                         |
| -------------------- | ----------------------------------------------------------------- | ---------------------------------------------------------------------------- |
| Graph + A\*          | `ptiles-client/core/src/route_graph.rs`                           | Build graph from decoded segments, snap, A*/bi-A*, path geometry             |
| Wasm export          | `ptiles-client/wasm/src/lib.rs` → `route_from_segments`           | JS passes segments; returns `{distance_m, duration_s, path:[[lat,lon],...]}` |
| UI                   | `demo/index.html`                                                 | Corridor cells (h3-js), Range I/O, zstd, snap, draw                          |
| Live                 | `https://steele.red/ptiles/`                                      | S3 + CloudFront                                                              |
| Data                 | `https://maps.mydatatimeline.com/maps/{ST}.roads.ptiles`          | Per-state files, Accept-Ranges                                               |
| Plan                 | `ptiles-client/docs/plans/2026-07-09-browser-corridor-routing.md` | Original design                                                              |
| Reference that works | `timeline/ptiles/src/router.rs` + `ptiles/mvp` (daemon :9352)     | Ring widen + full graph; **not** in browser                                  |

**Commits (approx):**

- `68074a2` — core graph + densified path
- steele.red: `de451da` … `fa983c8` — UI wire, multi-state, sparse corridor

---

## Architecture (actual)

```
click A, B
  → stateAt(lat,lon) / statesAlong(A,B)   # JS bboxes, not polygons
  → open {ST}.roads.ptiles (cached in roadsByState)
  → corridorCells(A,B)                    # h3 res-7, sparse disk
  → Range + zstd each cell (parallel batches)
  → decode_roads (wasm)
  → filter DRIVING / ARTERIAL by zone
  → route_from_segments(segs, zone_middle, A, B, snap_m)
  → draw orange polyline
```

**Contract:** JS owns I/O + cell list. Core owns graph + search only.

### Wasm API

```ts
route_from_segments(
  segments: { coords: [lon,lat][], road_class, oneway?, speed_limit_kmh? }[],
  zone_middle: boolean[],  // true = arterial-only middle
  lat1, lon1, lat2, lon2,
  snap_m?: number
) -> { distance_m, duration_s, path: [lat,lon][] } | null
```

Rebuild wasm:

```bash
cd wasm
wasm-pack build --target web \
  --out-dir demo/lib/client \
  --out-name ptiles_client
```

Deploy UI:

```bash
AWS_PROFILE=steele-red-deploy \
  aws s3 sync <steele.red checkout>/output/ptiles/ s3://steele.red/ptiles/
# invalidate CF dist E1X2E2N30TVNGX paths /ptiles/*
```

---

## What works (verified locally on TN.roads)

| Pair                                 | ~km | Result                        |
| ------------------------------------ | --- | ----------------------------- |
| Nashville downtown short hop (~2 km) | 2   | OK, ~2.4 km path              |
| Synthetic L-shape segments           | —   | unit tests 4/4                |
| Snap driving classes (not footway)   | —   | OK if correct state file open |

## What fails (verified)

Local repro (node + local `TN.roads.ptiles` + current corridor params):

| Pair                | cells | art segs | route    |
| ------------------- | ----- | -------- | -------- |
| Nash→Mem (~316 km)  | 143   | ~10k     | **null** |
| Nash→Chat (~182 km) | 143   | ~10k     | **null** |
| Nash→Knox (~258 km) | 122   | ~9k      | **null** |
| Nash→Knox widen w=2 | 400   | ~11k     | **null** |

**Root cause class (not "TN only"):**

1. **Corridor too thin for connectivity.** Sparse sampling (interval 8–20 km, width 1) leaves gaps in the road graph. A\* never connects A and B. Fat corridors (w≥3, thousands of cells) load too slow / OOM-ish in browser.
2. **Node merge + path geometry.** Early path was chord between merged nodes (looked like "not on road"). Partially fixed by densifying edge geom + `MERGE_THRESH=5`. City-scale still fails for (1).
3. **Multi-state.** Endpoint-only readers missed mid-corridor states; `statesAlong` added. Cross-state still limited by (1) and per-file state borders (roads don't cross state boundary cells in one file).
4. **Snap state.** Dropdown-only snap → Atlanta with TN selected = no road. Fixed with `stateAt` + `roadsByState` cache. Prefer `currentState` on bbox overlap (Memphis TN/AR).
5. **Live deploy lag.** CloudFront often served pre-route crow-flies build; always `?v=N` after deploy.

**Not the main bug:** "TN is preloaded." All states have `maps.mydatatimeline.com/maps/{ST}.roads.ptiles`. Failure is graph connectivity under cell budget, not missing data for short same-state hops once the right file is open.

---

## Why MVP worked

`ptiles/mvp` + timeline daemon:

- Loads rings around A/B and **widens** until connected (or highways path).
- Full native RAM/time budget (~300 MB / multi-second OK).
- Browser path tried to replace that with a thin corridor + hard cell caps → **disconnected graphs**.

Porting daemon `route_rings` widen policy (or a server-side `/route`) is the honest path for city-to-city.

---

## Core knobs (`route_graph.rs`)

| Constant             | Now                               | Notes                                             |
| -------------------- | --------------------------------- | ------------------------------------------------- |
| `MERGE_THRESH`       | 5 (~11 m)                         | Was 10 (~22 m); tighter = more verts, better draw |
| `NODE_CAP`           | 250_000                           | Abort build if larger                             |
| `BI_ASTAR_MIN_NODES` | 50_000                            | Bi-A\* only above this                            |
| Driving classes      | no footway/path; service optional | UI filters service by default                     |
| Path output          | stitch `edge_geom` centerline     | Not bare node chords                              |

Tests: `cargo test -p ptiles-core route_graph --lib` (4 tests).

---

## UI knobs (`demo/index.html`)

| Setting           | Current                                | Pain                                       |
| ----------------- | -------------------------------------- | ------------------------------------------ |
| `ROUTE_MAX_CELLS` | 400                                    | Cap for phone; thin spine if over          |
| `corridorParams`  | short: w=2 full; long: w=1 arterial    | First long pass is sparse; retry is denser |
| Long retry        | w=2, 8 km spine interval, 900-cell cap | Extra Range/decode cost only after a miss  |
| Batch Range       | 24                                     | Parallel decompress                        |
| Snap              | driving only, project on segment       | footway snap was crow/nubs                 |
| Multi-state       | `statesAlong` + `roadsByState`         | Border cells still weak                    |

---

## Recommended next steps (priority)

1. **Prove connectivity offline**  
   On TN.roads, binary-search minimal corridor (width, interval) that routes Nash→Knox / Nash→Mem. Record cell count, segs, wall time. That sets browser budget truth.

2. **Match daemon short-route policy**  
   For dist < ~50–80 km: ring expand around A∪B (k=1,2,3…) like `route_rings` instead of fixed sparse corridor. Stop at first connected path.

3. **Long route: don't pretend browser A\* on all locals**  
   Either:
   - arterial-only spine with **much** better corridor continuity (or highway layer if restored), or
   - **server route API** (existing daemon) + browser draw only.

4. **Cross-state**  
   Union cells from all `statesAlong`; at borders, snap/merge graph nodes across readers (same lat/lon micro-key). Today each file is isolated — state line can hard-cut paths.

5. **Perf**  
   Cache decoded segments by cell; cancel in-flight route on new click; don't re-fetch end-cap cells on widen; stream status is already there.

6. **Deploy checklist**  
   Sync `output/ptiles/` + wasm, CF invalidate `/ptiles/*`, verify live HTML contains `route_from_segments` and `statesAlong`, open with `?v=N`.

---

## Local repro commands

```bash
# unit
cargo test -p ptiles-core route_graph --lib

# wasm
cd ptiles-client/wasm && wasm-pack build --target web \
  --out-dir ../../demo/lib/client --out-name ptiles_client

# data
# TN.roads.ptiles  (~32 MB)
# or Range: maps.mydatatimeline.com/maps/TN.roads.ptiles

# UI local
cd steele.red && python3 serve.py   # :8000 → /ptiles/
```

---

## Explicit non-goals (v1 plan — still hold unless you reopen)

- PTILESU portal matrices
- CH / CRP preprocess
- Highway sidecar files (deleted historically)
- Turn restrictions / multi-modal

---

## One-line summary for the next team

**Browser routing graph code is real and short hops work; city-to-city fails because the corridor cell set is too sparse to keep the graph connected under a phone cell budget — either adopt daemon ring-widen / server route, or find a proven cell budget that still connects on TN before polishing the UI.**

skipped: full daemon port, progressive arterial paint, legal state polygons — add when connectivity is proven offline first.
