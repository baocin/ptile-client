# label-gpx

A page that turns a GPS trace into a labeled motion fixture:
**upload a GPX → the classifier proposes segments → correct them → download a labeled GPX**.

Live at <https://steele.red/ptile-label-gpx/>. Static: no server, no build step, no npm
dependencies. Parsing and classification run in the tab; map context is read from the public tile
host by byte range.

The output format is [`SCHEMA.md`](SCHEMA.md), which is the source of truth for it — the Android
exporter and `motion/tests/` both read against that file, not against this page's behavior.

## Why it exists

`ptiles-motion` classifies motion, and everything that tests it either hand-builds inputs or
replays traces with no ground truth. `motion/tests/gpx_replay.rs` can assert "the drive reads as
driving", but it cannot assert "this 40-second stretch is the user walking from the car to the
door" — nobody wrote that down. This is where it gets written down.

The labeling is *auto-split then correct*: the classifier goes first and proposes segments, and the
human fixes what it got wrong. That is much faster than labeling from scratch, at the cost of
anchoring — so every segment records whether a human touched it (`source="human"`, `edited="true"`).
**A test that asserts classifier output against `source="auto"` labels is asserting that the
classifier agrees with itself.** Only edited segments are evidence.

## Using it

1. Pick a `.gpx` file. Both flavors work: a plain OSM trace (`<trkpt>` + `<time>`, like the six in
   `test-fixtures/gpx/`) or a rook-flavour file that already carries per-point sensor extensions and
   a resolved context.
2. The trace is classified immediately, road-blind, and the proposed segments appear in the table.
   This costs no network at all.
3. **Resolve map context** reads the roads layer for every H3 cell the trace touches and attaches a
   road + intersection to each segment.
4. **Re-classify with context** re-runs the classifier with those priors. This is where a stroll
   becomes visible (a footway hit at 1.2 m/s votes walking where speed alone votes stationary) and
   where a stop at a signal stops reading as an arrival. Segments you have edited are preserved.
5. Fix the rest: relabel from the dropdown, click a vertex of the selected segment to split there,
   `^` to merge into the previous one, `Ctrl+Z` to undo (20 deep).
6. **Download labeled GPX** writes one `<trk>` per segment named by label.

A rook file's existing `<rook:context>` is kept as-is rather than recomputed. Those traces were
annotated in the field; this snapshot is `2026-08-07` and the fixture traces are from 2011-2020, so
re-resolving would silently replace what was true with what is true now.

## How it is put together

| file | what it is |
| --- | --- |
| `index.html` | page + CSS. Leaflet from unpkg, `preferCanvas` (a 2,000-vertex polyline as SVG is sluggish to pan). |
| `js/app.js` | wiring: file input, map, table, buttons. Only the browser-specific parts. |
| `js/gpx.js` | the only XML in the project. `DOMParser` in, `XMLSerializer` out. |
| `js/segments.js` | classify → coalesce → split/merge/relabel/undo. No DOM, no fetch, so `node --test` drives it. |
| `js/context.js` | snapshot base, layer filenames, state bboxes, per-state layer cache, sampled resolution. |
| `js/ptiles.js` | **symlink** to `../web-demo/js/ptiles.js` — range cache, block cache, prefetch coalescing. |
| `lib` | **symlink** to `../web-demo/lib` — the wasm client and `corp-safe-tiles.js`. |

Both symlinks are deliberate. `createPtiles(wasm)` already takes the wasm namespace as a parameter
so more than one page can use it, and two independently-built copies of a 550 KB
`ptiles_client_bg.wasm` under the same filename, both served `no-cache`, differing in which exports
exist, is a worse bug than it sounds. `steele.red`'s build dereferences symlinks when it copies, so
the deployed site gets real files.

`DOMParser` is not a convenience: the rook format comes from an app that is still changing, so the
reader has to treat every field as optional and ignore what it does not recognise, and the DOM gives
that by construction. `XMLSerializer` likewise owns escaping — `&` in a business name, `?a=1&b=2` in
a URL — correctly in both attribute and text position, where a `.replace()` chain does not.

### Rebuilding the wasm client

The page needs `MovementTracker` and `accel_stats`, which only exist in bundles built after the
motion port:

```sh
PATH="$HOME/.cargo/bin:$PATH" wasm-pack build wasm --target web \
  --out-dir ../web-demo/lib/client --out-name ptiles_client --release
grep -c MovementTracker web-demo/lib/client/ptiles_client.d.ts   # must be > 0
```

## The request budget

Resolving road context per point would mean thousands of range reads *and*, worse, thousands of full
block decodes: `nearest_road` and `nearest_intersection` each decode the whole roads block on every
call, and a downtown cell holds tens of thousands of features. So:

| phase | requests |
| --- | --- |
| cell map (`cell_for_coord` per point, pure wasm) | 0 |
| open the state's roads layer (header live, then dict + index in parallel) | 3 |
| one coalesced `prefetch` over every cell the trace touches | ~5-15 |
| first-pass classification, road-blind | 0 |
| context for each segment, from 5 sampled points | 0 new |

A 1,187-point, 90 km trace is ~11-21 requests and a few MB cold; a warm reload is ETag revalidations
with no bodies. The status bar shows the live counters from `js/ptiles.js`, so the claim is
falsifiable rather than decorative.

`ponytail:` five sampled points per segment is an approximation — a segment that starts on a footway
and ends on a road gets one of the two. The upgrade is a `RoadIndex` class in `wasm/src/lib.rs` that
decodes a block once in its constructor and answers `nearest(lat, lon)` from a grid; that makes
per-point resolution free, deletes the sampling entirely, and helps `web-demo` too.

## Tests

```sh
# The labeling pipeline, against the real wasm and the real traces.
PATH="$HOME/.cargo/bin:$PATH" wasm-pack build wasm --target nodejs --out-dir ../wasm-pkg --release
node --test label-gpx/test/segments.test.mjs

# gpx.js and the page itself, in chromium (node has no DOMParser).
python3 label-gpx/test/round_trip.py            # --headed to watch
```

`segments.test.mjs` also asserts the label dropdown matches the `MovementType` enum in
`motion/src/movement.rs`, so a variant added in Rust and not here fails a test instead of quietly
becoming unlabelable.

Serve over HTTP when developing, never `file://`: the Cache API is undefined on an insecure origin,
so every reload re-downloads the ~940 KB layer open.

```sh
python3 -m http.server -d label-gpx 8080
```

## Deploying

```sh
./label-gpx/deploy.sh            # dry run
./label-gpx/deploy.sh --apply
```

Pushing this repo deploys nothing — the site is a static S3 bucket with no watcher on git.

Two one-time changes in the `steele.red` repo, which `deploy.sh` checks for and refuses to run
without:

```sh
ln -s ~/kino/projects/ptile-client/label-gpx ~/kino/projects/steele.red/ptile-label-gpx
# then add "ptile-label-gpx" to STATIC_DIRS in ~/kino/projects/steele.red/build.py
```

## Not in v1

- `rook:context` covers admin, road and intersection. Buildings, addresses and businesses are read
  and preserved from input files but not generated.
- No sensor synthesis. A plain OSM trace exports derived speed (flagged `derived="true"`) and leaves
  accuracy and accel absent rather than inventing plausible-looking values to match the label —
  those would be numbers generated from the answer, and `SCHEMA.md`'s `synthetic` attribute exists to
  keep that honest if it is ever added.
- One file at a time, no batch mode.
- `US.admin` still means a 28 MB grid download, so admin fields stay opt-in.
