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

That is also why two like-labelled spans are only merged when their provenance matches. Slicing 25
minutes out of a 67-minute driving stretch used to merge straight back and export the whole 67 as
`source="human"` — one drag, laundered into a claim about ground nobody looked at. Splitting a segment
no longer marks either half human either: a split is a statement about a boundary, not about what
lies on both sides of it.

## Using it

1. Pick a `.gpx` file. Both flavors work: a plain OSM trace (`<trkpt>` + `<time>`, like the six in
   `test-fixtures/gpx/`) or a rook-flavour file that already carries per-point sensor extensions and
   a resolved context.
2. The trace is classified immediately, road-blind, and the proposed segments appear in the table.
   This costs no network at all.
3. **Classify with map context** does the rest in one action, behind a sheet that names each phase:
   read the layers for the cells this trace touches, resolve a road and intersection per segment,
   then classify again with those priors. This is where a stroll becomes visible (a footway hit at
   1.2 m/s votes walking where speed alone votes stationary) and where a stop at a signal stops
   reading as an arrival. Segments you have already labelled are preserved — a classifier pass never
   overwrites a human decision.
4. **Click the map** to ask what is there: the building under the pointer, the addresses within
   250 m, the businesses within 150 m. Pick a segment and **Attach to segment** writes it into that
   segment's context, which exports as `rook:building` / `rook:addresses` / `rook:businesses`.
   A 12-minute stop is "stationary" either way; whether it happened at the hardware store is what
   makes the fixture worth keeping.
5. Fix the rest: relabel from the dropdown, click a vertex of the selected segment to split there,
   `↑` to merge into the previous one, `Ctrl+Z` to undo (20 deep).
6. **Download labeled GPX** writes one `<trk>` per segment named by label.

A rook file's existing `<rook:context>` is kept as-is rather than recomputed. Those traces were
annotated in the field; this snapshot is `2026-08-07` and the fixture traces are from 2011-2020, so
re-resolving would silently replace what was true with what is true now.

## Reading the screen

**The ribbon** under the toolbar is the trace's whole timeline, to scale. The table is ordinal — one
row per segment, whether it lasted 90 seconds or 40 minutes — and the ribbon is temporal, which is
the view you actually label against. Bands you have edited are drawn solid with a thick base; bands
still as the classifier proposed them are dimmed with a hairline. That is the `source="auto"` versus
`source="human"` distinction from `SCHEMA.md`, and it decides whether a segment counts as evidence,
so it is in the picture rather than buried in a column. Click a band to select it.

**Speed & shifts** (toolbar) draws the speed profile under the ribbon, with a marker wherever a
Welch t-test says the motion genuinely changed. This is a different question from the classifier's
transitions: no thresholds and no movement vocabulary are involved, only whether the mean speed moved
by more than noise explains, with the significance level Bonferroni-corrected by the number of
candidate positions tested. The two disagreeing is the useful case — a shift with no segment boundary
near it is usually a real change the thresholds missed, and a boundary with no shift near it is
usually the debouncer reacting to noise.

The chart is banded by the classifier's own speed thresholds — stationary, walking, driving — with a
dashed line at the stateless tree's 2.2 m/s walking floor, the level below which speed alone cannot
see a walk at all. Those numbers come from `wasm.motion_thresholds()`, never from a copy here: a chart
whose bands disagree with the classifier is worse than a chart with no bands. There is no running
band, and that is not an omission — `Running` comes from accelerometer cadence, never from speed, so
a speed axis has nothing honest to draw for it.

**Drag a rectangle on the chart to cut a slice.** The time range becomes its own segment, labelled by
the dominant speed band among the samples *inside the box*, bucketed by `wasm.speed_band` so the
label is the classifier's vocabulary rather than a JavaScript opinion. The vertical extent is the
reason to drag a rectangle instead of brushing a time range: pull the top edge below a GPS spike and
the spike stops voting. The status line reports the share it won by, because "62% walking" and "98%
walking" are different claims about the same slice.

Marker prominence follows the size of the change, not the p-value: a real drive produces dozens of
changes that are all far past any threshold, so ranking them by p-value would make a 15 m/s
motorway entry look like a 1 m/s corner. Nothing is hidden; small changes are simply drawn faintly.
The **sensitivity** control sets how many samples are compared on each side — `fine` (6) finds a pause
at a junction, `coarse` (24) finds the change from town driving to open road, and neither is more
correct. The detector lives in `motion/src/shifts.rs` with its own tests, including the t-distribution
checked against published critical values.

**The overview strip** at the very top is the whole trace, always. The box on it is the current zoom
window: drag its body to pan, its edges to resize, click empty track to jump, double-click to reset.
Wheel over the chart zooms about the pointer. The overview, the ribbon and the speed chart share one
window, so zooming anywhere zooms everything; the table and legend deliberately do not filter, because
a segment you meant to fix should never vanish because of where you were looking — in-window rows get
a marker instead.

The speed axis **rescales to the window**. That is a trade: two windows cannot be compared by eye, but
zooming into a 12-minute stop in a 125-minute drive shows its detail instead of a flat line pinned to
the bottom of a 33 m/s axis. The axis labels are what keep it honest, and they thin themselves out
when there is no room rather than printing numbers on top of each other.

**Boundary handles** sit on the ribbon, one per interior boundary. Drag one to move where a segment
ends — one gesture instead of split-then-merge. Both sides are marked human, because moving a boundary
asserts that these points belong to that label and those to the other.

**The basemap switch** (bottom-left of the map) chooses between OSM raster tiles and the ptiles
layers — water, parks, rail, trails, roads, and buildings from zoom 15 — decoded in the page from the
same files the road context comes from. The raster tiles are always right about the world; the vector ones are
right about *what the classifier read*. When a label turns on footway-versus-traffic-lane, the second
is the honest backdrop, and flipping between them is the quickest way to catch the tiles and the
layer disagreeing.

With a trace open, the vector basemap draws **only the cells the trace occupies** — the ones its
points land in, plus the neighbours it clips within 180 m of a boundary, probed with four offsets per
point (all local `cell_for_coord` calls, no I/O). That is the space argument: a 90 km drive crosses
about 60 cells, where the viewport at a working zoom asks for several hundred, nearly all of them
nowhere near anything you can label. Panning away from the trace fetches nothing and says so.
With no trace open it falls back to the viewport, which needs zoom 11 or closer to stay under the
512-cell bounds cap. It reports what it spent next to the switch: `2561 features · 20 requests ·
3.5 MB` for a trail run in NC.

Colour follows one rule: chrome is achromatic, hue is data. Every saturated colour on the page is a
movement label, so the basemap is deliberately desaturated and the interactive accent is a cyan that
is not one of the five label hues.

## How it is put together

| file | what it is |
| --- | --- |
| `index.html` | page + CSS. Leaflet from unpkg, `preferCanvas` (a 2,000-vertex polyline as SVG is sluggish to pan). |
| `js/basemap.js` | the two backdrops: OSM raster tiles, or the ptiles layers drawn from the same files the classifier reads. |
| `js/chart.js` | the speed profile and the significant shifts, as inline SVG. |
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

- Businesses often fail to decode on the published `business_v4` layer, and the card says so instead
  of showing an empty list. Reproducible outside the browser: the downtown-Nashville cell of
  `TN.business_v4.ptiles` is a 966,179-byte block and `decode_business_v4` stops at offset 929,329
  asking for 57,875 more bytes. Buildings and addresses are unaffected.
- No sensor synthesis. A plain OSM trace exports derived speed (flagged `derived="true"`) and leaves
  accuracy and accel absent rather than inventing plausible-looking values to match the label —
  those would be numbers generated from the answer, and `SCHEMA.md`'s `synthetic` attribute exists to
  keep that honest if it is ever added.
- One file at a time, no batch mode.
- `US.admin` still means a 28 MB grid download, so admin fields stay opt-in.
