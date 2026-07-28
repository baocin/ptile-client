# ptile-client

Rust workspace for the [PTiles binary geospatial format](https://github.com/baocin/ptiles).
`no_std` decoder core, WASM browser bridge, native CLI, fuzz harness.

> Development happens in the `kino` monorepo under `projects/ptile-client`.
> This repository is a mirror; it has no `.git` of its own locally.

## Crates

| Crate    | What                                                                              |
| -------- | --------------------------------------------------------------------------------- |
| `core`   | `no_std`-optional decoder library — zero-alloc block parser for all PTiles layers |
| `wasm`   | wasm-bindgen bridge — decode PTiles in the browser via WebAssembly                |
| `cli`    | Native JSON bridge for Rookery — pipe lat/lon → JSON feature                      |
| `ffi`    | C ABI surface, incl. Android/Apple targets                                        |
| `motion` | Movement classification over decoded features                                      |
| `fuzz`   | AFL/libfuzzer harness — crash-testing byte-level decoders                         |

`src/lib.rs` at the root is the superseded wasm-bindgen client, kept out of the
workspace as a porting reference. `demo/` is the browser demo (below).

## Quick Start

```bash
cargo build --workspace
cargo test --workspace

# WASM. wasm-pack resolves /usr/bin/rustc by default, which has no wasm32
# target -- put rustup's toolchain first or the build fails on sysroot.
PATH="$HOME/.cargo/bin:$PATH" \
  wasm-pack build wasm --target web --release \
  --out-dir ../demo/lib/client --out-name ptiles_client
```

## Reading a file

Index layout is **detected, never assumed**. Two entry widths and three block
offset bases exist in published files, and the header can disagree with its own
index:

| width | layers |
| --- | --- |
| 19 B | roads, water, business, buildings_v8 |
| 38 B | parks, rail, places, signals, camera |

Offsets are absolute, relative to `blocks_offset` (buildings_v8), or absolute
with a correction when `blocks_offset` overshoots where the index really ends.
`PtilesFile::layout()` reports which was chosen.

Layers with a 38-byte index also pack several cells per compressed block, so
**use `read_cell`, not `read_block`** — the latter hands a record decoder a cell
table it will parse as records and return plausible garbage rather than an
error.

```rust
let file = PtilesFile::open(FileSource::open(path)?)?;
let bytes = file.read_cell(cell)?.expect("cell present");
let signals = ptiles_core::decode_signals(&bytes)?;
```

Every `.ptiles` kind is versioned independently — the version byte is scoped to
its magic, there is no release-wide version, and a new kind starts at 1. See
[`SUPPORTED_FORMATS.md`](SUPPORTED_FORMATS.md), which is generated from
`ptiles_core::SUPPORTED_FORMATS` and asserted against it by a test.

## Tests

```bash
cargo test --workspace
node --test "demo/test/*.test.mjs"

# Browser checks, with `python3 -m http.server 8899 --bind 127.0.0.1` in demo/
python3 demo/test/render_check.py        # every layer draws
python3 demo/test/intersection_check.py  # junction panel
python3 demo/test/cache_check.py         # warm open makes one request
python3 demo/test/coarse_check.py        # coarse lookup pulls a fraction
```

`core/tests/index_layout.rs` covers the width × offset-base matrix and
adversarial input; `real_layers.rs` runs against real files and fails if none
are found, so an empty data directory can't pass for coverage.
`demo/test/differential.html` requires the JavaScript and wasm decoders to agree
field-for-field — they are separate implementations on purpose (wasm decode
measured ~30x slower across the boundary, see `bench_wasm.html`), so drift is a
test failure rather than a silent mis-render.

## Demo

Click any building in the US: https://steele.red/ptiles

`demo/index.html` is the only source for that page — `steele.red/ptiles` is a
symlink to `demo/` which steele.red's `build.py` dereferences into its output.
Changes are not live until that build runs.

It opens files over HTTP Range requests, never downloading a whole layer, and
caches each layer's header, dictionary and index in the Cache API keyed by
ETag. A warm load costs one 256-byte request instead of ~4.5 MB.

## Live Tiles

```
https://maps.mydatatimeline.com/maps/v4-20260711/{ST}.{layer}.ptiles
https://maps.mydatatimeline.com/maps/US.{signals,camera}.ptiles
```

Per-state: `buildings_v9`, `business_v4`, `highways_v2`, `business_name_index`,
`address_v1`, `water_v1`, `places_v1`, `parks_v1`, `rail_v1`.
US-wide: `admin`, `signals`, `camera`.

`signals` and `camera` carry a coarse index in the header's `aux` region — a
sampled cell→position map, ~5 KiB, letting a point lookup fetch one short run
of the real index instead of all 4 MB of it. The browser demo uses it; the Rust
core reads the whole index and does not yet.

## License

MIT
