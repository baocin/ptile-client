# ptiles-client

Rust workspace for the [PTiles binary geospatial format](https://github.com/baocin/ptiles):
a `no_std`-optional decoder core, a WASM bridge for the browser, a native CLI
bridge for Rookery, a UniFFI bridge for iOS/Android, and a fuzz harness.
Extracted from the original `wasm-bindgen`-only prototype (`src/lib.rs`, kept
as a legacy reference — see below) into a layered workspace so the decoding
logic can be shared between the browser (WASM), native (CLI), and mobile
(FFI) consumers.

## Workspace layout

```
core/    ptiles-core   — decoder library (buildings, roads, water, parks, rail, business),
                          H3 cell lookup, nearest-road proximity search. no_std-optional.
wasm/    ptiles-wasm   — thin wasm-bindgen wrapper over ptiles-core, for browser use.
cli/     ptiles-cli    — native binary: one-shot lat/lon query, or a `--serve`
                          JSON-lines bridge for Rookery.
ffi/     ptiles-ffi    — UniFFI bridge over ptiles-core, generating Swift and
                          Kotlin bindings for iOS/macOS/Android consumers.
motion/  ptiles-motion — stateful GPS motion classification (stationary/walking/
                          driving) over a timestamped fix sequence. no_std-optional.
fuzz/    (not a workspace member — has its own [workspace] table)
                       — cargo-fuzz targets for buildings/roads/business decoders.
src/lib.rs             — LEGACY SEED, the original monolithic wasm-bindgen prototype.
                          Kept until wasm/ has confirmed parity in the demo, then removed.
test-fixtures/          — golden fixtures (real block bytes + Python-reference-decoded
                          JSON) used by core/tests/golden.rs. Checked into git.
```

## Documentation

- [`docs/INTEGRATION.md`](docs/INTEGRATION.md) — integration guide for
  consumers (Rookery/CLI, browser/wasm, iOS/Android/FFI), covering the
  file-open/layer-inference convention, remote (HTTP) files, and the
  scoring/search APIs above.

## Demo

[`demo/`](demo/) is a static Leaflet-based web demo built entirely on this
repo's `ptiles-wasm` build — no JS reimplementation of header/index parsing
or H3 math (both are thin wasm wrappers over `ptiles-core`, see
`wasm/src/lib.rs`'s `parse_header`/`parse_index_entries`/
`find_block_for_cell`/`cell_for_coord`/`cell_center`/`neighbor_cells`). State
selector, per-layer toggles (roads/water/parks/rail/buildings), business
name search (sidecar-first, brute-force fallback), and click-for-nearest-road.
Deployed via GitHub Pages using `.github/workflows/pages.yml`; see
[`demo/README.md`](demo/README.md) for local-serve instructions and a CORS
finding for the real data host.

## Crates

### `core` (`ptiles-core`)

Decoder library. Parses the PTiles header/index, decompresses zstd blocks
(with per-layer dictionary fallback), and decodes each layer's binary record
format into typed Rust structs (`RoadSegment`, `Building`, `Business`,
`WaterFeature`, `ParkFeature`, `RailFeature`). Also provides:

- `cell_for_coord` / `neighbor_cells` / `cell_center` — H3 resolution-7 cell
  lookup (wraps `h3o`).
- `nearest_road` — haversine point-to-linestring proximity search.
- `nearest_intersection` — "am I at an intersection?": nearest labeled
  intersection point from a roads block's v2 intersection table, within a
  threshold, plus its traffic-control type (signal/stop/give_way/roundabout).
  Reports a mapped intersection *point*, not junction degree — the format
  stores no road-to-node topology.
- `AdminFile` — point → jurisdiction lookup (`US.admin.ptiles`, a lookup-grid
  layer): `admin_at(lat, lon)` returns country/state/county/zip/timezone.
- `AddressFile` — address layer (`{STATE}.address.ptiles`, v2 merged-block
  index): `addresses_at(lat, lon, ring)` (reverse enumeration) and
  `find_address(lat, lon, ring, number, street)` (forward, accent/case-insensitive).
- Business name search is now **accent- and case-insensitive** (`fold_name`:
  NFD + diacritic strip + `ß`→`ss`), so `eclair` matches `Éclair`. The indexed
  path probes both the folded-letter bucket and the legacy catch-all bucket so
  it works against sidecars built before the fix.
- `score_candidates` — GPS emission-probability scoring for a fix against
  road/building/business candidates (see "GPS candidate scoring" below).

**Features:**

- `std` (default) — pulls in `std` for `thiserror`, `h3o`, `ruzstd`. Disable
  for `no_std` embedding (`cargo check -p ptiles-core --no-default-features`).
- `serde` — derives `Serialize`/`Deserialize` on all decoded structs (needed
  by both `wasm` and `cli`).
- `http` — adds `HttpSource`, a `PtilesSource` implementation that opens a
  `.ptiles` file served over `http(s)://` instead of the local filesystem,
  using range requests (see "Remote (HTTP) files" below). Pulls in `ureq`
  (blocking, rustls-backed — no local TLS/OpenSSL dependency). Off by
  default; `wasm` does not enable it, so it never reaches the wasm32 build.

### Remote (HTTP) files

`ptiles-core`'s `HttpSource` (behind the `http` feature) opens a `.ptiles`
file served over plain HTTP range requests — no server-side changes needed,
any static file host that supports `Range` works. On `open()` it eagerly
prefetches the first 64 KiB in one request (small/no-dict layers' header +
index are usually served entirely out of that prefetch, for free), then
caches each subsequent exact `(offset, len)` read. A non-`206` response to a
range request is reported as `SourceError::RangeNotSupported` rather than
silently falling back to a full download.

Request-efficiency evidence (real file, `TN.roads.ptiles`, 33 MB, over
`https://maps.mydatatimeline.com/maps/`): `PtilesFile::open()` costs 3 HTTP
requests (prefetch + dict + index); one subsequent `read_block()` query
costs exactly 1 more request (4 total for open + one query) — see
`core/src/http_source.rs`'s `request_count_for_open_plus_one_query_is_small`
test, which asserts against `HttpSource::request_count()`.

The CLI exposes this directly — see "Remote/URL paths" under `cli` below.
`ffi/`'s `PtilesLayer::open` also scheme-sniffs the same way (any
`http(s)://` path opens over `HttpSource`; anything else opens over the
local filesystem), so the same remote support is available to Swift/Kotlin
consumers with no API change.

### `wasm` (`ptiles-wasm`)

wasm-bindgen exports matching the legacy `pkg/ptiles_client.d.ts` contract
field-for-field: `decode_roads`, `decode_water`, `decode_buildings`,
`decode_parks`, `decode_rail`, `decode_business`, plus a new optional
`decompress_block(compressed, dict)` export so JS callers can eventually drop
their own zstd-wasm dependency. `osm_id` values outside the safe JS integer
range (business layer) serialize as `bigint` rather than panicking.

Additional exports (road geometry + scoring, mirroring the CLI's):

- `nearest_road(block_bytes, lat, lon, threshold_m?)` — decodes a roads block
  and returns the closest segment as `{osm_id, name, road_class, snapped:
  [lat, lon], distance_m, geometry: [[lat, lon], ...]}`, or `null` if nothing
  is within `threshold_m` (default 50m). Coordinates are `[lat, lon]` in the
  output (the on-disk/core representation is `[lon, lat]`).
- `nearest_intersection(block_bytes, lat, lon, threshold_m?)` — decodes a
  roads block's v2 intersection table and returns the nearest intersection as
  `{lat, lon, distance_m, intersection_type}` (type 1 signal / 2 stop /
  3 give_way / 4 roundabout), or `null` if nothing is within `threshold_m`
  (default 50m).
- `AdminReader` — constructed once from the admin file's `aux` (grid) and
  decompressed `dict` (string tables) byte ranges; `admin_at(lat, lon)` returns
  the jurisdiction. Kept as a reusable object so the ~28 MB grid is decoded
  once, not per query.
- `address_cell(block_bytes, cell_hex)` — decode the addresses for one cell
  from a decompressed address merged-block.
- `roads_in_block(block_bytes)` — full decoded segment list for one block
  (same shape as `decode_roads`). Ring-1 neighbor-cell lookup is a JS-side
  concern: call this once per cell you decide to fetch.
- `score_candidates(fix_json, roads_block, buildings_block, business_block,
  cell_center_lat, cell_center_lon)` — see "GPS candidate scoring" below.
  Pass an empty `Uint8Array` for any layer you want to skip; `roads_block` is
  required.

Build:

```sh
cargo install wasm-pack   # once
wasm-pack build wasm --target nodejs --out-dir ../wasm-pkg
```

Golden-test the build:

```sh
node wasm/test/golden.mjs
```

### `cli` (`ptiles-cli`)

Native JSON bridge over `ptiles-core`, for Rookery or any process that wants
point queries without embedding Rust/WASM.

One-shot:

```sh
ptiles-cli --path /path/to/TN.roads.ptiles --lat 36.16 --lon -86.78 \
  --query road    # road | roads | intersection | buildings | business | all | business-search | admin | address | address-find
  [--ring 1]      # 0 (default, center cell only) or 1 (also check the six neighboring res-7 cells)
  [--accuracy-m 10] [--speed-mps 8]   # optional: adds ranked "candidates" via score_candidates
```

`--query road` returns the single nearest segment, enriched with geometry:
`{"nearest_road":{"osm_id":...,"name":...,"road_class":...,
"snapped":[lat,lon],"distance_m":...,"geometry":[[lat,lon],...]}}`.

`--query intersection` answers "am I at an intersection?": the nearest
labeled intersection within 50m from the roads block's v2 intersection table:
`{"nearest_intersection":{"lat":...,"lon":...,"distance_m":...,
"intersection_type":N}|null,"candidate_count":N}` (type 1 signal / 2 stop /
3 give_way / 4 roundabout). It reports a mapped intersection point, not
junction degree.

`--query admin` (against `US.admin.ptiles`) returns the jurisdiction covering
the point: `{"admin":{"country":...,"state":...,"county":...,"zip":...,
"timezone":...,"boundary_flags":N}|null}`.

`--query address` (against `{STATE}.address.ptiles`) enumerates addresses in
the covering cell(s): `{"addresses":[{"osm_id":...,"housenumber":...,
"street":...}, ...],"count":N}`. `--query address-find --number N --street S`
returns only the accent/case-insensitive matches.

`--query roads` returns every decoded segment in the query cell(s) (ring-0,
or ring-0+1 with `--ring 1`): `{"roads":[{"osm_id":...,"name":...,
"road_class":...,"geometry":[[lat,lon],...]}, ...],"candidate_count":N}`.
`--ring` values other than `0`/`1` are rejected: `{"error":"ring N not
supported (only 0 or 1)"}` (exit 1 in one-shot mode; serve mode returns the
error line and keeps looping).

Serve mode (JSON lines on stdin/stdout, for long-lived integration):

```sh
ptiles-cli --serve --data-dir /path/to/ptiles-data
# stdin:  {"lat":36.16,"lon":-86.78,"query":"all","state":"TN","ring":1,"accuracy_m":10,"speed_mps":8}
# stdout: {"building":null,"nearest_road":{...},"business":[...],"candidates":[...]}
```

`state` is optional if only one state's files are loaded. `ring`,
`accuracy_m`, and `speed_mps` are all optional. Malformed input, unknown
state, unsupported `ring`, or per-layer decode errors produce
`{"error":"..."}` lines instead of crashing the loop.

#### Remote/URL paths

`--path`, and per-layer files under `--serve --data-dir`, accept
`http(s)://` URLs as well as local paths (scheme-sniffed automatically —
`http`/`https` prefix opens over `HttpSource`, anything else opens over the
filesystem):

```sh
ptiles-cli --path https://maps.mydatatimeline.com/maps/TN.roads.ptiles \
  --lat 36.1627 --lon -86.7816 --query road
```

For serving multiple states straight off a remote host, without a local
`--data-dir` mirror, use `--serve --remote-base <url> --states TN,US`. This
opens `<remote-base><state>.{roads,buildings_v8,business}.ptiles` per state
over HTTP; a missing state/layer combination is skipped (logged to stderr)
rather than failing the whole serve loop:

```sh
ptiles-cli --serve --remote-base https://maps.mydatatimeline.com/maps/ --states TN,US
```

Requires `ptiles-core`'s `http` feature (enabled by default in `cli`'s own
`Cargo.toml` dependency on `ptiles-core`).

### GPS candidate scoring

When `accuracy_m` (one-shot: `--accuracy-m`) is supplied, the response gains
a `"candidates"` array: `[{"kind":"Road"|"Building"|"Business","osm_id":...,
"name":...,"distance_m":...,"score":...}, ...]`, ranked descending by score.
Scoring is implemented in `ptiles-core::scoring` (`score_candidates`), not
in the CLI/wasm layers, so the same emission model is available to future
FFI (Swift/Kotlin) consumers.

Model: each candidate's emission score is `exp(-distance_m^2 / (2 *
sigma^2))`, where `sigma = accuracy_m` (CoreLocation's `horizontalAccuracy`)
and `distance_m` is point-to-segment distance for roads, 0 if inside /
distance-to-edge otherwise for building polygons. `--speed-mps` (optional)
gates the weighting: speed above roughly 3 m/s up-weights road candidates,
near-zero/absent speed up-weights buildings. Weights are `ScoringParams`
fields, not hardcoded constants. This is *not* a position filter or gravity
well — it returns ranked candidates with scores and leaves any state
tracking (e.g. an HMM over fixes) to the caller; that's deferred to a future
routing phase.

### Business name search

`ptiles-core::business_search` provides indexed-first, brute-force-fallback
name search over a state's business/POI layer:

- `search_business_indexed` — for states that have the
  `<state>.business_name_index.ptiles` sidecar (magic `PTILESX`, built by
  `scripts/build_business_name_index.py` from the corresponding
  `.business.ptiles`), buckets the query into one of 28 letter-keyed blocks
  via `name_to_key` (public, so `wasm` callers can pick the bucket without a
  round trip) and scans only that block.
- `search_business_brute_force` — falls back to a full linear scan of every
  block in a plain `.business.ptiles` file when no sidecar is present
  (slower, but works against the real deployed dataset today, which does
  not yet host the sidecar).
- `match_business_name_block` — the pure, no-I/O block-matching primitive
  both of the above share; also exported to `wasm` as
  `match_business_name_block` alongside `key_for_business_name_query` so a
  JS caller can fetch one HTTP range, decompress, and match without any
  business-record parsing of its own.

Exposed at every layer: `ptiles-cli --query business-search --name '...'`
(one-shot and `--serve`, single-state or `--national`, local or
`--remote-base`), `ptiles-ffi`'s `PtilesLayer::search_business`, and the
`demo/` app's search box (sidecar-first with brute-force fallback).

### Bounds / viewport queries

`ptiles_core::query::cells_for_bounds(min_lat, min_lon, max_lat, max_lon) ->
Result<Vec<u64>, BoundsError>` returns every res-7 H3 cell covering a
lat/lon bounding box, for viewport-driven feature loading (map pan/zoom)
rather than the single-point/ring APIs above. Rejects degenerate input
(`min >= max`) and oversized boxes. Exposed to `wasm` as `cells_for_bounds`
and used by `demo/js/app.js` to decide which blocks to fetch per map
viewport.

### `ffi` (`ptiles-ffi`)

UniFFI bridge (proc-macro mode, no `.udl` file — see `ffi/README.md` for the
rationale) exposing `ptiles-core` to Swift and Kotlin. Two opaque objects:

- `PtilesLayer` — opens one `.ptiles` file, layer inferred from the
  `<state>.<layer>.ptiles` filename convention (same rule as the CLI's
  `Layer` enum). Methods: `nearest_road`, `nearest_intersection`, `roads`
  (ring 0/1), `building`, `businesses_near`; each errors if called against a
  mismatched layer.
- `AdminLayer` — `admin_at(lat, lon)` → jurisdiction, for `US.admin.ptiles`.
- `AddressLayer` — `addresses_at(lat, lon, ring)` (reverse) and
  `find_address(lat, lon, ring, number, street)` (forward), for
  `{STATE}.address.ptiles`.
- `PtilesStack` — holds up to one roads/buildings/business `PtilesLayer`
  together and exposes `score(fix, ring)`, a thin wrapper over
  `ptiles_core::score_candidates`, for CoreLocation-style callers scoring one
  fix across all open layers at once.

Generated bindings are checked into `ffi/bindings/{swift,kotlin}/`.
Regeneration commands, the Android cross-compile recipe (verified on Linux
with `cargo-ndk`), and the iOS/macOS recipe (**requires a Mac** — Apple
targets cannot be built on this Linux host) are documented in
[`ffi/README.md`](ffi/README.md).

### `motion` (`ptiles-motion`)

Stateful GPS motion classification, kept out of `ptiles-core` because core is
deliberately stateless single-fix (its `Fix` has no timestamp). Feed a
`MotionClassifier` a sequence of `TimedFix` (a core `Fix` + monotonic
`t_ms`) via `push`, and read back a `MotionState` — `Unknown`, `Stationary`,
`Walking`, or `Driving`. It prefers the platform `speed_mps` when present and
otherwise derives speed from consecutive positions (`haversine_distance_m` /
Δt), smooths over a window, and debounces band changes with a dwell counter so
a single outlier can't flip the state. Low-accuracy fixes and large time gaps
are gated out. `MotionClassifier::smoothed_speed_mps()` can populate
`Fix.speed_mps` before a `score_candidates` call so core's existing binary
road/stationary gate sees a denoised speed — no core scoring semantics change.
`no_std + alloc`, same constraints as core (`cargo build -p ptiles-motion
--no-default-features` builds). FFI/wasm handles for the stateful classifier
are a planned follow-up.

```sh
cargo test -p ptiles-ffi   # 9 integration tests against real TN.*.ptiles fixtures
```

### `fuzz`

Not a workspace member (has its own `[workspace]` table in
`fuzz/Cargo.toml`, so it can pin `ptiles-core` with `default-features =
false` independently of the main workspace — `h3o`'s `std` feature does not
build on the nightly toolchain used for fuzzing, but the block decoders
under test don't need `std`/H3 anyway). Targets: `decode_buildings`,
`decode_roads`, `decode_business`.

```sh
cargo install cargo-fuzz
rustup component add rust-src --toolchain nightly
cargo +nightly fuzz run decode_roads     # or decode_buildings / decode_business
```

## Test strategy

- **Unit tests** (`core/src/*.rs`, `#[cfg(test)]` modules) — decoder edge
  cases: empty/truncated input, varint/zigzag roundtrips, degenerate
  geometry, H3 determinism. Run with `cargo test -p ptiles-core`.
- **Golden tests** (`core/tests/golden.rs`) — decode real, checked-in block
  bytes (`test-fixtures/golden/<layer>.block.bin`) and assert the result
  matches a JSON snapshot produced by the Python reference decoder
  (`test-fixtures/golden/<layer>.golden.json`). One case per layer.
- **Integration test** (`core/tests/gpx_snap.rs`) — snaps a real GPX track
  to decoded road geometry within a 100m threshold, exercising
  `PtilesFile` + `decode_roads` + `nearest_road` together against real data
  under `~/kino/data/ptiles/`.
- **Scoring unit tests** (`core/src/scoring.rs`) — synthetic fixes: a
  fast-moving fix ranks the road candidate first, a stationary fix inside a
  building footprint ranks that building first, and widening sigma flattens
  the ranking (scores converge).
- **CLI integration test** (`cli/tests/roads_query.rs`) — against real data
  under `~/kino/data/ptiles/` (skipped gracefully if absent): `roads` query
  shape and ring-0 vs ring-1 candidate-count monotonicity, `--ring 2`
  rejection, and the enriched `nearest_road` shape.
- **WASM golden test** (`wasm/test/golden.mjs`, run via `node`) — same
  fixtures, run through the built `wasm-pkg/` bindings, to confirm parity
  between the `core` decoders and their WASM wrapper, plus `roads_in_block`,
  `nearest_road`, and `score_candidates` cases.
- **Fuzz** (`fuzz/`) — best-effort crash discovery on the three decoders
  most exposed to untrusted/corrupt input.

Run everything host-side:

```sh
cargo build --workspace
cargo test --workspace
cargo check -p ptiles-core --no-default-features
cargo clippy --workspace --all-targets
```

### Regenerating golden fixtures

```sh
python3 test-fixtures/extract_golden.py
```

Requires the real `TN.*.ptiles` files under `~/kino/data/ptiles/` and the
Python reference decoders (`../ptiles/ptiles/*.py`) on `PYTHONPATH`. Writes
`test-fixtures/golden/<layer>.{block.bin,golden.json,meta.json}` for all six
layers — commit the results (`test-fixtures/`, including `golden/`, is
intentionally tracked, not gitignored).

## Format-version policy

`ptiles-core` rejects any `.ptiles` file whose magic/version byte pair isn't
in its `SUPPORTED_FORMATS` table (`core/src/versions.rs`) — `PtilesFile::open`
fails closed with `FileError::UnsupportedVersion` right after magic
validation, before touching the dict/index, rather than guessing at an
unverified layout. The table is populated only from bytes actually observed
in real files (`od`-inspected under `~/kino/data/ptiles/`), not copied
uncross-checked from `SPEC.md`.

See [`SUPPORTED_FORMATS.md`](./SUPPORTED_FORMATS.md) for the generated
table (also available at runtime via `ptiles-cli --supported-formats`, and
as `ptiles_core::supported_formats()` for FFI/wasm callers). A
`core/tests/supported_formats_doc.rs` drift guard keeps the checked-in
markdown in sync with the table in code. Notably, the real
`TN.business.ptiles` uses magic `PTILESB` version 3, not `SPEC.md`'s
documented `PTILESI` version 2 — `SPEC.md` is stale for that layer;
`versions.rs` follows the real bytes.

Magics with no local sample file (`PTILESA` admin, `PTILESD` addr, `PTILESU`
routing) are deliberately absent from the table — any such file is rejected
with an empty `supported` set until a real sample is inspected and a table
entry is added.

## Known gaps

- `PtilesFile::read_block`'s absolute-vs-relative block offset bug (see
  `cli` section above) — worked around in the CLI, not fixed in `core`.
- `wasm`'s `decompress_block` has only a negative/smoke test (rejects
  garbage input); no fixture pairs raw compressed bytes with its dictionary,
  so there is no round-trip golden case for it yet.
- `src/lib.rs` (the legacy seed) is unmaintained and only kept as a
  reference until `wasm/` parity is confirmed in the demo.
- `nearest_intersection` reports a mapped intersection *point* and its
  traffic-control type, but not junction *degree*: the roads format stores no
  road-to-node topology, so a true multi-way junction and a tagged road
  endpoint are indistinguishable from the data alone. Storing node degree is a
  request for the next roads format version — see [`docs/ROADMAP.md`](docs/ROADMAP.md).
- `ptiles-motion` classification is native-only for now; the stateful
  `MotionClassifier` is not yet exposed through FFI (Swift/Kotlin) or wasm.
