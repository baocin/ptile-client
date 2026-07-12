# ptiles-core fuzz targets

Three libFuzzer targets over the pure `&[u8]` block decoders:

- `decode_buildings` — `ptiles_core::decode_buildings(data, DUMMY_LAT, DUMMY_LON)`
  with a fixed dummy H3 cell center (36.16, -86.78); coords don't affect
  parsing robustness, just the output values.
- `decode_roads` — `ptiles_core::decode_roads(data)`
- `decode_business` — `ptiles_core::decode_business(data)`

## Setup

Requires nightly + cargo-fuzz (libFuzzer needs nightly `-Z` sanitizer flags):

```
rustup toolchain install nightly
rustup component add rust-src --toolchain nightly
cargo install cargo-fuzz
```

`fuzz/Cargo.toml` depends on `ptiles-core` with `default-features = false` —
the `std` feature pulls in `h3o/std`, which currently fails to build on
recent nightly (`mul_add` not yet stable as const fn, see
`h3o-0.10.0/src/math/functions-std.rs:58`). The decoders under test don't
need `std` or H3, so this is just an unblocking workaround, not a fix to
h3o itself.

## Running

```
cd fuzz  # or run from anywhere in the workspace; cargo-fuzz finds fuzz/
cargo +nightly fuzz run decode_buildings -- -max_total_time=60
cargo +nightly fuzz run decode_roads -- -max_total_time=60
cargo +nightly fuzz run decode_business -- -max_total_time=60
```

Crashes land in `fuzz/artifacts/<target>/`; corpus accumulates in
`fuzz/corpus/<target>/`. Both are gitignored.

## Status (2026-07-07)

Ran each target for ~60s (nightly-1.92.0):

- `decode_buildings`: 3,073,822 executions, no crashes.
- `decode_roads`: 1,771,778 executions, no crashes.
- `decode_business`: 1,261,477 executions, no crashes.

No panics found; no fixes needed in `core/src/` as a result of this pass.
Longer runs (minutes to hours) would give higher confidence but were out of
scope for this best-effort pass.
