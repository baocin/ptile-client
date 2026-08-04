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

    node --test "web-demo/test/*.test.mjs"      # reader vs the corpus
    python3 web-demo/test/render_check.py       # the page, in a browser

`render_check.py` is the parity gate. It counts what each layer draws and must
match what `demo/test/render_check.py` reports for the legacy page. At the time
of writing both give roads 25781, water 141, bldgs 26488, parks 110, rail 2,
camera 2, signal 762.

Two things that will waste your time otherwise, both learned the hard way:

- PTILES Mode gates all rendering. Without clicking `#btnPtiles` a layer fetches
  its index and never requests a block: reader present, group on the map, zero
  features, no error.
- Several layer checkboxes ship `checked`, so "set it and dispatch change" does
  nothing. Dispatch the event unconditionally.

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
