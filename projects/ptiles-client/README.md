# ptiles-client

Rust workspace for the [PTiles binary geospatial format](https://github.com/baocin/ptiles):
a `no_std`-optional decoder core, a WASM bridge for the browser, a native CLI
bridge for Rookery, and a fuzz harness. Extracted from the original
`wasm-bindgen`-only prototype (`src/lib.rs`, kept as a legacy reference —
see below) into a layered workspace so the decoding logic can be shared
between the browser (WASM) and native (CLI) consumers.

## Workspace layout

```
core/    ptiles-core   — decoder library (buildings, roads, water, parks, rail, business),
                          H3 cell lookup, nearest-road proximity search. no_std-optional.
wasm/    ptiles-wasm   — thin wasm-bindgen wrapper over ptiles-core, for browser use.
cli/     ptiles-cli    — native binary: one-shot lat/lon query, or a `--serve`
                          JSON-lines bridge for Rookery.
fuzz/    (not a workspace member — has its own [workspace] table)
                       — cargo-fuzz targets for buildings/roads/business decoders.
src/lib.rs             — LEGACY SEED, the original monolithic wasm-bindgen prototype.
                          Kept until wasm/ has confirmed parity in the demo, then removed.
test-fixtures/          — golden fixtures (real block bytes + Python-reference-decoded
                          JSON) used by core/tests/golden.rs. Checked into git.
```

## Crates

### `core` (`ptiles-core`)

Decoder library. Parses the PTiles header/index, decompresses zstd blocks
(with per-layer dictionary fallback), and decodes each layer's binary record
format into typed Rust structs (`RoadSegment`, `Building`, `Business`,
`WaterFeature`, `ParkFeature`, `RailFeature`). Also provides:

- `cell_for_coord` / `neighbor_cells` / `cell_center` — H3 resolution-7 cell
  lookup (wraps `h3o`).
- `nearest_road` — haversine point-to-linestring proximity search.

**Features:**

- `std` (default) — pulls in `std` for `thiserror`, `h3o`, `ruzstd`. Disable
  for `no_std` embedding (`cargo check -p ptiles-core --no-default-features`).
- `serde` — derives `Serialize`/`Deserialize` on all decoded structs (needed
  by both `wasm` and `cli`).

### `wasm` (`ptiles-wasm`)

wasm-bindgen exports matching the legacy `pkg/ptiles_client.d.ts` contract
field-for-field: `decode_roads`, `decode_water`, `decode_buildings`,
`decode_parks`, `decode_rail`, `decode_business`, plus a new optional
`decompress_block(compressed, dict)` export so JS callers can eventually drop
their own zstd-wasm dependency. `osm_id` values outside the safe JS integer
range (business layer) serialize as `bigint` rather than panicking.

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
  --query roads   # roads | buildings | business | all
  [--ring 1]       # also check the six neighboring res-7 cells
```

Serve mode (JSON lines on stdin/stdout, for long-lived integration):

```sh
ptiles-cli --serve --data-dir /path/to/ptiles-data
# stdin:  {"lat":36.16,"lon":-86.78,"query":"all","state":"TN"}
# stdout: {"building":null,"nearest_road":{...},"business":[...]}
```

`state` is optional if only one state's files are loaded. Malformed
input, unknown state, or per-layer decode errors produce
`{"error":"..."}` lines instead of crashing the loop.

Known gap: `core/file.rs::PtilesFile::read_block` assumes
`IndexEntry::block_offset` is always an absolute file offset. For
`*.buildings_v8.ptiles` files it is actually relative to
`header.blocks_offset` (matching the Python reference's
`BuildingsReader._relative_offsets` detection in `ptiles/buildings.py`).
`cli/src/main.rs` works around this locally (`buildings_v8_workaround`
module); `core` itself has not been patched, since that fix belongs to
whoever owns `file.rs`.

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
- **WASM golden test** (`wasm/test/golden.mjs`, run via `node`) — same
  fixtures, run through the built `wasm-pkg/` bindings, to confirm parity
  between the `core` decoders and their WASM wrapper.
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

## Known gaps

- `PtilesFile::read_block`'s absolute-vs-relative block offset bug (see
  `cli` section above) — worked around in the CLI, not fixed in `core`.
- `wasm`'s `decompress_block` has only a negative/smoke test (rejects
  garbage input); no fixture pairs raw compressed bytes with its dictionary,
  so there is no round-trip golden case for it yet.
- `src/lib.rs` (the legacy seed) is unmaintained and only kept as a
  reference until `wasm/` parity is confirmed in the demo.
