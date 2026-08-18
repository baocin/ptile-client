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
- `web-demo/index.html` is what `steele.red/ptiles` serves.
  `steele.red/ptiles` is an absolute symlink to `web-demo/`, and
  steele.red's `build.py` dereferences it (`shutil.copytree`,
  `symlinks=False`) into `output/ptiles/`, which is what Cloudflare Pages
  serves at <https://steele.red/ptiles/>. `steele.red/ptiles-legacy` is the
  same arrangement pointing at `demo/`. Edits are not live until
  `build.py` runs on `hino-omarchy` -- the symlink is absolute and resolves
  nowhere else. See `demo/README.md` for the full chain.
- `demo/index.html` is the **older** UI, kept deliberately. Same page, but it
  hand-decodes the format in JavaScript -- header, both index entry widths,
  offset base, merged-block slicing, the coarse index and three record layouts
  -- all of which `web-demo` removed in favour of `ptiles-core` in wasm. It is
  served at <https://steele.red/ptiles-legacy/> so the two can be compared
  live. They render identical feature counts on all seven layers.
  Its reader is a real module (`web-demo/js/ptiles.js`) rather than functions
  inlined in HTML, so `web-demo/test/ptiles.test.mjs` exercises the shipping
  code instead of a regex-scraped copy of it.
- There is deliberately no `index.html` at the repo root. One existed as a
  stale orphan copy of `demo/index.html`, referenced by nothing; it was
  deleted on 2026-07-26 and survives only in git history. Do not recreate it.
  The rule this used to state -- "a second copy of this UI is always a bug" --
  was about *orphan* copies that drift silently. `web-demo/` is the opposite:
  it exists to remove a second copy of the format decoders, it is reachable
  from a URL, and `web-demo/test/render_check.py` fails if it stops matching
  `demo/`. When it has proven itself, `demo/` goes away and the count returns
  to one.

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

## Native libraries in `android/app/src/main/jniLibs`

`libptiles_ffi.so` is committed, for both ABIs, so the Android app builds
without a Rust toolchain or the NDK — CI only builds `target/debug` for
binding generation, so nothing else produces them.

**Rebuild them whenever the FFI changes, but commit them only when cutting a
build worth keeping.** They are 4.3 MB each, stripped, and do not delta against
their previous version: every commit that includes them costs 8.6 MB of
history forever. One session that rebuilt on each change put 36 versions and
155 MB into this repo — 94% of it — and the history had to be rewritten to get
it back.

The working copy is what the APK is built from, so an uncommitted rebuild is
still the thing you are testing. Committing it is a separate decision, and the
right moment is a release rather than an edit.
