# web-demo

The ptiles browser demo, decoding through `ptiles-core` compiled to wasm.

Served at <https://steele.red/ptiles/> via a symlink from the `steele.red`
repo. The original `demo/`, which decodes the format in hand-written
JavaScript, is kept alongside at <https://steele.red/ptiles-legacy/> for
comparison until this one has proven itself.

## Why it exists

Every format bug this project has had came from two implementations of the same
byte layout disagreeing: a JS reader hardcoding a 19-byte index stride while the
generator emitted 38, a builder computing `index_length` at 42, a business
decoder rounding every `osm_id` past 2^53. The failure mode is a silently empty
layer, so each one was found days or weeks late.

`demo/index.html` is that second implementation. This page removes it: about
12.5 KB of header parsing, both index entry widths, offset-base selection,
merged-block slicing, the PTCI coarse index, and three record layouts -- one of
which, the camera layout, the page carried three separate times.

Leaflet and h3-js stay. They are not second implementations of *this* format,
and mixing core's H3 with h3-js for different calls would recreate the very
problem this removes.

## What the page answers

Drawing the seven layers is the least of it. Everything below reads fields the
decoders were already producing and nothing was consuming.

**Click a point.** Nearest building and the businesses in or near it, with
phone, website, address, brand, email and socials. The street: name, ref, class,
one-way, speed limit, lanes, surface, bridge/tunnel. The junction and its
control nodes, if you clicked one.

The street snap prefers a drivable road when one is within 25 m and falls back
to any class. Measured across eight downtown Nashville points, seven were nearer
an unnamed footway or service way than to the street the click clearly meant —
but click a pedestrian plaza with no road nearby and you still get the path you
clicked, named as one.

**Jurisdiction.** County, ZIP and timezone from `US.admin.ptiles`. Opt-in behind
a link because the lookup grid is 28 MB (1,785,304 H3 cells × 16 B) and
`AdminReader` wants all of it. The grid is sorted by `h3_cell` at a fixed
stride, so a range binary search over ~21 small GETs would remove the download
entirely; that is the upgrade, and it needs a wasm export that resolves one
entry against the string tables rather than requiring the whole grid.

**Line of sight.** What is visible from a point at a given eye height, with
heights doing the work — a 2D shadow test hides a tower behind a bungalow. The
panel says how much of the answer rests on guessed heights, because most
published buildings carry none.

**Which cameras can see you.** Cameras within the radius are culled by bearing
against their published `direction` and `angle`, then the survivors ride the
observer's own viewshed call as a 1 m, 4 m-tall synthetic footprint: line of
sight is reciprocal, so "is that pole visible" *is* "does that camera have a
line to me". No second geometry path. Only about a third of records publish a
facing; one without is counted as pointing at you rather than quietly cleared.

**View finder** — the same reciprocity, run the other way. Pick a tag, get the
buildings that can see it: `water_type` (river, stream, lake, canal),
`park_type`, `rail_type`, or a business by name. Linear and areal targets are
sampled every 60 m up to 24 points and the results unioned, so a building counts
if it can see any part of the bank.

Business *category* is deliberately absent from that list. It is published as a
bare table index with no sidecar to resolve it against (`core/src/business.rs`),
so "everywhere you can see a fast-food place" is not a question this data can
answer, however much the UI looks like it should.

Every one of these under-reports rather than over-reports. `viewshed` assumes an
uncertain building is tall when it occludes and short when it is the target, so
a guessed height costs visibility instead of inventing it.

## Layout

    index.html          the page; no byte-level code left in it
    js/ptiles.js        the reader: ranges, ETag cache, block cache. No format
                        knowledge -- every "what do these bytes mean" question
                        goes to wasm
    lib/client/         wasm-pack --target web output
    lib/                corp-safe-tiles.js, h3-js
    test/ptiles.test.mjs    the reader against conformance/corpus/
    test/render_check.py    the real page in chromium, against the live host

`js/ptiles.js` takes the wasm namespace as a parameter rather than importing it,
so the same file runs against a `--target web` build in the browser and a
`--target nodejs` build under `node --test`. That is what makes it testable at
all; the legacy decoders can only be reached by regex-scraping a 2656-line HTML
file.

## Rebuilding the wasm

    PATH="$HOME/.cargo/bin:$PATH" wasm-pack build wasm --target web \
      --out-dir ../web-demo/lib/client --out-name ptiles_client --release

## Verifying

    node --test web-demo/test/ptiles.test.mjs   # reader vs the corpus
    python3 web-demo/test/render_check.py       # the page, in a browser

`node --test` needs a `--target nodejs` build in `wasm-pkg/`; `render_check.py`
needs the `--target web` one under `lib/client/` and network access, since the
tiles come from the live host either way.

`render_check.py` runs seven phases. The first is the parity gate: it counts
what each layer draws and must match what `demo/test/render_check.py` reports
for the legacy page — at the time of writing both give roads 25781, water 141,
bldgs 26488, parks 110, rail 2, camera 2, signal 762.

The rest guard behaviour a feature count cannot see. Three assertions carry the
weight, because each one is false on a plausible stub that returns a number:

- **Raising the eye to 90 m must reveal more than 1.7 m does.** Nothing else
  distinguishes a real 2.5D viewshed from a 2D shadow test.
- **Halving the view finder's radius must find fewer buildings, and raising the
  target off the ground must find more.** The same property, from the far end.
- **Toggling 3D must make the building count grow.** Cells memoize in
  `bldgs.rendered`, so a mode guard that fails to clear them looks exactly like
  "this area has no heights".

Two things that will waste your time otherwise, both learned the hard way:

- PTILES Mode gates all rendering. Without clicking `#btnPtiles` a layer fetches
  its index and never requests a block: reader present, group on the map, zero
  features, no error.
- Several layer checkboxes ship `checked`, so "set it and dispatch change" does
  nothing. Dispatch the event unconditionally. The same applies to `#viewTag`:
  setting `.value` fires nothing, and the business-name field's visibility hangs
  off the change handler.

`window.__ptiles` exposes the hooks the harness drives — `featureCounts`,
`losAt`, `viewFinderAt`, `bldgHeightCoverage`, `setView`, `intersectionAt`.
A browser cannot read "has a view" off a polygon's fill colour.

## Performance

The concern was that wasm decode would be too slow -- `demo/test/bench_wasm.html`
measured it ~30x slower than hand-rolled JS per cell. That benchmark isolates
the wasm boundary crossing on synthetic records; on a real page the cost is
range requests and zstd. Measured time from PTILES Mode to a stable render, same
viewport, live host:

    roads   7.86s -> 7.44s   0.95x
    water   6.49s -> 6.99s   1.08x
    bldgs   8.92s -> 11.87s  1.33x
    parks   7.67s -> 8.97s   1.17x
    signal  8.12s -> 8.68s   1.07x
    camera  3.88s -> 3.90s   1.00x

Worst case is buildings, the layer with 26,488 features, at 1.33x.
