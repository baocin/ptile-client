# ptiles-client

## Purpose

Rust workspace decoding the PTiles binary geospatial format: `core` (decoder
library), `wasm` (browser wasm-bindgen bridge), `cli` (native JSON bridge for
Rookery), `fuzz` (crash-testing harness). See README.md for full layout,
build, and test instructions.

## Ownership

- Kino infrastructure.

## Local Contracts

- Tracked inside the `/home/aoi/kino` git repository (this directory has no
  git dir of its own).
- Related to ptile-client (main Rust client) and ptiles-browser (WASM viewer).
- `src/lib.rs` is a legacy seed, superseded by `core/` + `wasm/`; kept until
  wasm parity is confirmed in the demo, then removed.

## Child DOX Index

No subdirectories with their own AGENTS.md.
