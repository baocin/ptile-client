# ptiles-client

## Purpose

Rust workspace decoding the PTiles binary geospatial format: `core` (decoder
library), `wasm` (browser wasm-bindgen bridge), `cli` (native JSON bridge for
Rookery), `fuzz` (crash-testing harness). See README.md for full layout,
build, and test instructions.

## Ownership

- Kino infrastructure.

## Local Contracts

- Related to ptile-client (main Rust client) and ptiles-browser (WASM viewer).
- `src/lib.rs` is a legacy seed, superseded by `core/` + `wasm/`; kept until
  wasm parity is confirmed in the demo, then removed.
- Browser corridor routing keeps the first pass within `ROUTE_MAX_CELLS`; a
  failed route may use one denser arterial retry with an explicit bounded
  budget in `demo/index.html`.
- `demo/index.html` is the single source of truth for the browser UI.
  `steele.red/ptiles` is an absolute symlink to `demo/`, and
  steele.red's `build.py` dereferences it (`shutil.copytree`,
  `symlinks=False`) into `output/ptiles/`, which is what Cloudflare Pages
  serves at <https://steele.red/ptiles/>. Edits are not live until
  `build.py` runs on `hino-omarchy` -- the symlink is absolute and resolves
  nowhere else. See `demo/README.md` for the full chain.
- There is deliberately no `index.html` at the repo root. One existed as a
  stale orphan copy of `demo/index.html`, referenced by nothing; it was
  deleted on 2026-07-26 and survives only in git history. Do not
  recreate it -- a second copy of this UI is always a bug.

## Index layouts

Two entry widths and three offset bases exist in published files. Which a file
uses is a property of the generator that wrote it, not of the layer, so both
readers detect rather than assume. Never hardcode a stride.

| width | layers | notes |
| --- | --- | --- |
| 19 B | roads, water, business, buildings_v8 | SPEC.md v1 |
| 38 B | parks, rail, places, signals, camera | merged-block v2; bbox at bytes 8..24 is written as zeros |

Offset bases: `Absolute`; `Relative` to `blocks_offset` (buildings_v8); and
`AbsoluteCorrected`, for files whose `blocks_offset` overshoots where the index
really ends because it was computed at the wrong stride. The published
`US.signals`/`US.camera` overshoot by `count * 4`.

A 38-byte index read as 19-byte does **not** error — offset and length come
out of the zeroed bbox, so every cell looks empty and the layer renders
nothing, silently. Detection therefore validates entry 0's `block_length`
against the bytes rather than trusting `index_length`.

38-byte layers also use **merged blocks**: several cells behind a 12-byte
header and a cell table. Use `PtilesFile::read_cell` (Rust) or
`fetchCellRecords` (JS), not `read_block`/`fetchDecompressEntry` — handing a
whole merged block to a record decoder parses the header as records and yields
plausible garbage, not an error.

Every `.ptiles` kind is versioned independently; the version byte is scoped to
its magic and there is no release-wide version. See `SUPPORTED_FORMATS.md`.

## Tests

```sh
cargo test --workspace && node --test "demo/test/*.test.mjs"
```

- `core/tests/index_layout.rs` — width x offset-base matrix and adversarial
  input. Asserts *which* layout was detected, not just that bytes came back.
- `core/tests/real_layers.rs` — every published layer, real bytes. Fails if no
  fixture is found, so an empty data directory can't look green.
- `demo/test/index_reader.test.mjs` — extracts the reader functions out of
  `index.html` by name and runs them against the same real files.

## Child DOX Index

No subdirectories with their own AGENTS.md.
