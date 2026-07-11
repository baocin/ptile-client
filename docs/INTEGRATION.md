# Building a map UI on ptiles-client

This is the "how do I actually wire a Leaflet/Mapbox/whatever map up to
`.ptiles` files" doc. Part 1 documents the demo's real loading mechanics
(reference behavior, not code to copy — see repo root's CLAUDE.md). Part 2
translates that pattern to this library's actual API, for both the wasm+JS
target and the native (CLI/FFI) target. Part 3 is pitfalls.

## 1. How the demo (steele.red/ptiles/index.html) actually loads data

The demo's own doc comments in this repo may suggest "range request per
block" — that is **not** what it does. Reading the code precisely
(`index.html`'s `PtilesDemoReader`/`LayerReader`/`BusinessReader`, all three
built the same way):

```js
resp = await fetch(url);                 // whole file, one GET, no Range header
buf = new Uint8Array(await resp.arrayBuffer());
h = parsePtilesHeader(buf);               // reads the 256-byte header out of buf
dict = buf.subarray(h.dictOffset, ...);   // dict slice, still just a view into buf
{entries, cellMap} = parsePtilesIndex(buf, h);
```

So the real pattern is: **one whole-file HTTP GET per layer per state**, no
Range requests at all, then everything else happens against the in-memory
`ArrayBuffer`:

- `parsePtilesHeader`/`parsePtilesIndex` run once, right after the fetch
  completes, and are cached on the reader object for the file's lifetime
  (`this.header`, `this.entries`, `this.cellMap`, `this.dict`) — never
  re-parsed per query.
- `cellMap` is a `Map<bigint, index>` keyed by each index entry's H3 cell
  **masked to the res-7 boundary** (`cb & 0xffffffffffe00000n`) so a lookup
  by any res-7 cell hex string is an O(1) map hit, not a linear/binary scan
  of the index array per query.
- The zstd **dictionary is shared**: one `dict` `Uint8Array` slice per open
  file, passed into `zstdDecompress(compressed, dict)` for every block from
  that file. It is never refetched or re-decoded per block.
- Per H3 cell, only that cell's compressed block bytes
  (`data.subarray(abs, abs + entry.blockLength)`) are decompressed — the
  rest of the buffer just sits there unread. This is the actual "efficiency"
  in the demo: I/O happens once (whole file), CPU work (zstd decompress) is
  scoped to exactly the cells the viewport needs.
- **Per-cell dedup**: each layer's `rendered` `Set<bigint>` (of masked cell
  ints) is checked before decompressing — `renderPtilesForCells` skips a
  cell it has already rendered, so panning back over previously-visited
  territory costs zero additional decompress calls, only re-adds of
  already-built Leaflet layers (which it also doesn't do — already-rendered
  cells are simply skipped entirely, layers persist).
- **Viewport → cells**: on `moveend`/`zoomend` (debounced 600ms,
  `scheduleViewportRender`), the current `map.getBounds()` rectangle is
  turned into cells via `h3.polygonToCells([sw, nw-ish corners...], 7)`,
  falling back to `h3.gridDisk(centerCell, rings)` (rings = 1/3/8/15
  depending on zoom level) if `polygonToCells` throws. The result is hard
  capped at **300 cells** (`cells.slice(0, 300)`) before rendering, and
  rendering only kicks in at `zoom >= 10` at all — below that it just shows
  a "zoom in to render" status and does nothing.
- **Neighbor cells** appear in exactly two places, both **ring-1, not
  polyfill**: `BusinessReader.query` (point lookup) expands the center cell
  with `h3.gridRing(cellHex, 1)` (6 neighbors) so a business just across a
  cell boundary from the query point is still found; nothing else touches
  ring/neighbor cells.
- Concrete request counts: for a single state+layer, total HTTP requests
  = **1** (the whole-file GET), no matter how many cells the user later
  pans/zooms through, and no matter how many times "PTILES Mode" is
  toggled off and on (the reader + its `dict`/`cellMap` are cached on
  `ptilesLayers[key].reader` and only refetched if explicitly cleared).
  Building layers alone reach ~5-6 files loaded (roads, water on
  activation, then parks/rail/buildings only if their checkbox is ticked)
  — still 1 request each, not per-tile.

This whole-file-download approach only works because these are small,
per-state single-layer files (a few MB compressed) served from a plain
static file host with no Range support assumed. It is **not** what
`ptiles-core`'s `HttpSource` does, and not the pattern to reach for if your
files are large or you want true progressive/partial loading — see part 2.

## 2. The equivalent pattern on this library's API

`ptiles-core` gives you a second, better option the demo doesn't use: real
HTTP Range requests via `HttpSource`, so you never download bytes you don't
need. Pick per your deployment:

### Native (CLI, FFI, or any Rust host: `HttpSource` / `FileSource`)

```rust
use ptiles_core::{HttpSource, PtilesFile, cells_for_bounds, MAX_BOUNDS_CELLS};

let source = HttpSource::open("https://maps.mydatatimeline.com/maps/TN.roads.ptiles")?;
let file = PtilesFile::open(source)?;   // header + dict + index: at most 3 Range requests total

// viewport -> cells (see query.rs::cells_for_bounds, added alongside this doc)
let cells = cells_for_bounds(min_lat, min_lon, max_lat, max_lon)?; // errors above MAX_BOUNDS_CELLS (512)

for cell in cells {
    if let Some(block) = file.read_block(cell)? {  // 1 Range request per *new* cell only
        let roads = ptiles_core::decode_roads(&block)?;
        // render `roads`
    }
}
```

This mirrors the demo's caching exactly, just with real partial fetches
instead of a whole-file download:

- `PtilesFile::open` = header (1 req or free from prefetch) + dict (1 req,
  only if it doesn't fit in the 64 KiB prefetch — small/dict-less layers
  like parks/rail need **zero** extra requests) + index (same). See
  `core/src/http_source.rs`'s module doc: `HttpSource` eagerly prefetches
  the first 64 KiB at construction in **one** request, and for layers whose
  dict+index fit inside that window (measured true for parks/rail/business
  in this repo's real fixtures), `open()` costs exactly that one request.
  Roads/buildings train larger (~512 KB) dictionaries and cost one more.
- `file.read_block(cell)` = exactly one Range GET per cell **the first
  time** it's requested — `HttpSource` has its own read-through
  `(offset, len)` cache (`core/src/http_source.rs`), so re-reading the same
  cell (re-pan back over old territory) is served from memory, zero
  network cost, same as the demo's `rendered` Set dedup but general (works
  even if you don't build your own dedup layer on top).
- The dict is loaded once per `PtilesFile` and reused by
  `read_block`/`decompress_with_dict_fallback` internally for every block —
  same sharing the demo does manually.
- `cells_for_bounds` replaces the demo's `h3.polygonToCells` + manual
  `.slice(0, 300)` cap: it errors (`BoundsError::TooManyCells`) instead of
  silently truncating past `MAX_BOUNDS_CELLS` (512) cells, so callers find
  out their viewport/zoom combination is too coarse instead of getting a
  partially-rendered map with no signal why.

Measured request count for this path: see
`core/src/http_source.rs`'s `request_count_for_open_plus_one_query_is_small`
test (uses `HttpSource::request_count()`) — `open()` + one `read_block()`
against the real `TN.roads.ptiles` file over the network.

### wasm + JS (browser)

The wasm boundary is deliberately **not async** and does **no I/O** — JS
owns fetching, wasm owns decode/decompress/query logic (see
`wasm/src/lib.rs`'s module doc). The JS side of the split is expected to
look like the demo's own fetch+decompress code, not like `HttpSource`
(wasm can't easily share a Rust-side HTTP connection pool with the
browser's own fetch stack, and doing Range requests from wasm would need
its own `fetch`-with-`Range`-header plumbing in JS anyway — so JS keeps
that job):

```js
import init, { cells_for_bounds, decode_roads, decompress_block } from "./pkg/ptiles_wasm.js";
await init();

// 1. viewport -> cells (replaces h3.polygonToCells + manual slice(0,300);
//    wasm-side cap is MAX_BOUNDS_CELLS=512 and returns an Error instead of
//    silently truncating)
const cells = cells_for_bounds(minLat, minLon, maxLat, maxLon); // -> string[] of lowercase hex cell ids

// 2. per new cell (dedup this yourself, e.g. a `rendered` Set like the demo's,
//    or lean on an HTTP cache -- wasm has no read-through cache of its own):
//    fetch just that cell's block bytes via Range, then decompress+decode in wasm
const range = await fetchByteRange(url, entry.blockOffset, entry.blockLength); // your own Range fetch, using the index you parsed from the header once
const raw = decompress_block(range, dict);       // dict: also fetched/cached once per file, by you
const roads = decode_roads(raw);                 // wasm-side decode, no I/O
```

Concretely, port these three demo behaviors into your JS loader (wasm gives
you the decode step, not these three):

1. Parse header/dict/index **once** per file open and hold them for the
   file's lifetime (same fields the demo caches: header, dict bytes, and
   an index you can binary-search or map by masked cell — `ptiles-core`'s
   `IndexEntry`/`binary_search` shape if you want to mirror it exactly, or
   your own `Map` like the demo's `cellMap`).
2. Do real Range fetches per block instead of the demo's whole-file
   download, if your files are large enough that this matters (state-level
   `.ptiles` files are small enough that whole-file was a reasonable choice
   for the demo; don't copy that call blindly for bigger extracts).
3. Dedup by cell before fetching/decompressing (a `Set` of already-rendered
   cell strings, exactly like the demo's `ptilesLayers[key].rendered`).

## 3. Pitfalls

- **Range request efficiency**: one Range request per cell is fine at
  demo-scale viewport cell counts (tens, capped at `MAX_BOUNDS_CELLS`), but
  if you're panning fast, an eager viewport handler firing on every
  `move` event (not `moveend`) will fire far more requests than the demo's
  debounced (600ms) `scheduleViewportRender`. Debounce your viewport→cells
  call, not just your render call.
- **Dict reuse**: never re-fetch or re-parse the zstd dictionary per block
  — it's one blob shared by every block in a layer's file. Fetch it once
  at file-open time (part of `PtilesFile::open`'s 1-3 requests, or your own
  JS-side open step) and hold a reference in whatever object represents
  "this layer's open file," not per-query.
- **Neighbor cells are ring-1, not a buffer zone you control**: this
  library's `neighbor_cells` (and the demo's one use of `gridRing(cell,1)`)
  is exactly 6 cells, no more — it exists for "query point near a cell
  boundary" cases (business radius lookup), not for viewport prefetch.
  Don't reach for it to "prefetch a margin around the viewport" — use
  `cells_for_bounds` on a bbox you've already padded, so the padding amount
  is explicit and adjustable instead of hardcoded to one ring.
- **`cells_for_bounds` is an approximation, not exact polygon coverage**:
  it's a flood fill seeded at the bbox center, using each hex's boundary
  vertices to test bbox overlap (see `core/src/query.rs`'s doc comment on
  the function) rather than `h3o`'s `geom`/`TilerBuilder` feature (which
  would need `std` + the `geo`/`geo-types` crates, at odds with this
  crate's no_std-optional design). For any normal several-cells-wide
  viewport bbox it matches true polyfill; don't feed it a bbox so thin
  (sub-cell-width in one dimension) that you depend on exact edge-cell
  coverage.
- **The 512-cell cap is a hard stop, not a truncation**: unlike the demo's
  silent `.slice(0, 300)`, `cells_for_bounds` returns an `Err` above
  `MAX_BOUNDS_CELLS`. Handle it (zoom in, or split the viewport into
  tiles) — don't just log-and-ignore, or your map will render nothing at
  low zoom with no visible reason why.
- **Don't conflate "block for a cell" with "everything near a point"**:
  `read_block`/`decode_*` operate one cell at a time. A point query near a
  cell edge still needs the ring-1 neighbor cells checked too (see
  `neighbor_cells`) — `cells_for_bounds` is for viewport rendering, not a
  drop-in replacement for point-lookup neighbor expansion.
