# GOAL: prove it renders, then finish the edges

The index-layout work is done and committed. What's missing is the criterion
that actually matters: **nobody has confirmed a layer draws on screen.** Two
readers now agree on nine real files at the byte level, and that is still
compatible with the map showing nothing.

This directory has no git dir of its own — it is tracked inside `~/kino`.
Commit there. New files under `projects/` need `git add -f`: `.gitignore:57`
in `~/kino` ignores `/projects/`, so untracked additions are refused while
tracked files stage normally.

## Already done — do not redo

- `cd23579` — Rust core reads both index widths (19 B and 38 B), three offset
  bases (absolute, relative, and corrected for a `blocks_offset` that
  overshoots), and slices merged blocks via `PtilesFile::read_cell`.
  `core/tests/index_layout.rs` (19 tests) + `core/tests/real_layers.rs` (11).
- `9a3aaa2` — the same detection in `demo/index.html`, plus `mergedCellSlice`
  and `fetchCellRecords`. `demo/test/index_reader.test.mjs` (21 tests).
- `cargo test --workspace && node --test "demo/test/*.test.mjs"` is green.
- Detected layouts, identical in both readers: roads/water/business 19 B
  absolute, buildings_v8 19 B relative, parks/rail/places/signals/camera 38 B
  absolute.

See `AGENTS.md` for the index-layout reference and how to run both suites.

## Task 1 — browser confirmation (the point of this goal)

Confirm each of roads, water, parks, rail, buildings, camera, signals puts
shapes on the map.

**Parks and Rail are the tell.** They use the 38-byte index and have been dark
far longer than signals/camera. If they now render, the fix is real end to end.
If they don't, there is a second failure downstream of the index that every
existing test passes straight through — find that.

Three attempts were burned on this harness. Do not repeat them:

- **The page wraps its script in an IIFE.** `page.evaluate` cannot reach `map`,
  `ptilesLayers`, or any reader. `map.setView is not a function` is what that
  looks like. Either measure the DOM, or add a small deliberate test hook to
  `demo/index.html` (something like `window.__ptiles = {...}` set once at the
  end) — a hook is honest and worth committing if it makes this checkable
  forever.
- **The checkboxes are not Playwright-visible.** `page.check()` times out on
  "element is not visible". Set `.checked` and dispatch
  `new Event('change', {bubbles:true})` instead.
- **Unchecking a layer does not clear its Leaflet group.** This invalidated the
  whole measurement: baseline read 1238 shapes with every layer off. Do not
  diff before/after within one page load. **Reload the page for each layer**
  and count with only that one enabled.
- **Do not zoom by mouse wheel.** Blind wheel-zoom put the map over empty
  forest at max zoom, where every layer correctly draws nothing — including
  roads. Navigate with the page's own `#coordInput` + the Lookup button
  (`36.1627, -86.7816` for Nashville), then click `.leaflet-control-zoom-out`
  about five times to reach ~z14.
- **Screenshot before trusting any count.** The forest screenshot is what
  revealed the harness was lying; a number alone would not have.
- Serve with `python3 -m http.server 8899 --bind 127.0.0.1` from `demo/`, and
  confirm `curl -s -o /dev/null -w '%{http_code}'` returns 200 before driving
  the browser.

A run where roads and water report zero is a broken harness, not a finding.
Treat them as the control: if the control fails, fix the harness first.

### What the live data will and won't prove

The demo fetches from `maps.mydatatimeline.com`, which **still serves the
broken `US.signals`/`US.camera`** — 42-byte declared stride, and an index
recording one point per cell. Loading those exercises the `corrected` offset
path against real bytes, which is genuinely useful, but they will render sparse
and that is the data's fault, not the reader's.

To exercise the good data, serve the freshly built files from
`~/kino/projects/ptiles/tiles/US.{signals,camera}.ptiles` locally and point the
demo at them. Worth doing both — broken-file and correct-file are different
code paths now.

## Task 2 — index parse performance

`US.signals.ptiles` has 108,166 entries and `parsePtilesIndex` builds an object
literal per entry. Typed arrays (`BigUint64Array` for cells, `Float64Array` for
offsets, `Uint32Array` for lengths) plus a lazy accessor would avoid that.

This was left undone deliberately: `BusinessReader` indexes into `.entries` in
five places (`demo/index.html` around lines 552-585 and 1176), and changing the
shape under it risked breaking business search to chase an unmeasured win.
Either migrate those call sites properly or leave it — do not half-do it.

**Benchmark first.** Measure index parse and block decode against the local
`US.signals.ptiles` before changing anything, and report both numbers. If the
parse is already cheap relative to the range fetch, say so and skip the
rewrite; that is a fine outcome.

## Task 3 — Rookery consumers

`cli/` is the native JSON bridge. Rookery snaps GPS to roads via
`server/src/server/location/ptiles/RoadsReader.ts` -> `/api/location/road` and
wants the same for the new layers:

- `queryNearestSignal(lat, lon)` -> `{ osmId, signalType, distanceMeters }`,
  point haversine, radius ~30 m. Purpose: a stop beside a signal is a
  controlled stop, not an arrival — it suppresses false "you arrived" events.
- `queryNearestCamera(lat, lon)` -> `{ osmId, deviceType, distanceMeters, direction }`.
- `ptilesFetch.ts` `FILE_ALLOWLIST` needs `signals` and `camera`.

Build on `PtilesFile::read_cell`, not `read_block` — the latter hands back a
merged block, and a record decoder will parse its header as records rather than
erroring. `cameras_near_road` in `core/src/camera.rs` is O(cameras x segments)
with a bbox cull; check it against a dense real cell before calling it done.

## Standing constraints

- Do not change any on-disk format. signals and camera are v1 and stay v1.
  Every `.ptiles` kind is versioned independently — the version byte is scoped
  to its magic and there is no release-wide version. See
  `SUPPORTED_FORMATS.md`.
- Do not touch `~/kino/projects/ptiles/scripts/build_points.py` unless a test
  proves the builder wrong.
- Do not publish anything to `maps.mydatatimeline.com`.
- `demo/index.html` is the only source for the UI. `steele.red/ptiles` is a
  symlink to `demo/` that steele.red's `build.py` dereferences into `output/`.
  There is deliberately no `index.html` at this repo's root; do not recreate
  one.
- Check `demo/index.html`'s mtime before and after editing — another session
  has had it open recently.
- `maps.mydatatimeline.com` 403s the default python-urllib User-Agent; send a
  browser UA. Blocks carry no content size, so use a streaming zstd
  decompressor, not one-shot `decompress()`.

## Done means

- A committed, repeatable harness that loads the page per layer and reports
  shapes drawn, with roads and water passing as controls.
- Every layer confirmed rendering, or a named defect for any that doesn't.
- A screenshot in the report, not just counts.
- Benchmark numbers recorded, whatever they say.
- `cargo test --workspace && node --test "demo/test/*.test.mjs"` still green.
