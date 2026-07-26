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
- Browser corridor routing keeps the first pass within `ROUTE_MAX_CELLS`; a
  failed route may use one denser arterial retry with an explicit bounded
  budget in `demo/index.html`.
- `demo/index.html` is the single source of truth for the browser UI.
  `projects/steele.red/ptiles` is an absolute symlink to `demo/`, and
  steele.red's `build.py` dereferences it (`shutil.copytree`,
  `symlinks=False`) into `output/ptiles/`, which is what Cloudflare Pages
  serves at <https://steele.red/ptiles/>. Edits are not live until
  `build.py` runs on `hino-omarchy` -- the symlink is absolute and resolves
  nowhere else. See `demo/README.md` for the full chain.
- There is deliberately no `index.html` at the repo root. One existed as a
  stale orphan copy of `demo/index.html`, referenced by nothing; it was
  deleted on 2026-07-26 and survives only in kino git history. Do not
  recreate it -- a second copy of this UI is always a bug.

## Child DOX Index

No subdirectories with their own AGENTS.md.
