# ptile-client

Rust workspace for the [PTiles binary geospatial format](https://github.com/baocin/ptiles).
`no_std` decoder core, WASM browser bridge, native CLI, fuzz harness.

## Crates

| Crate    | What                                                                              |
| -------- | --------------------------------------------------------------------------------- |
| `core`   | `no_std`-optional decoder library — zero-alloc block parser for all PTiles layers |
| `wasm`   | wasm-bindgen bridge — decode PTiles in the browser via WebAssembly                |
| `cli`    | Native JSON bridge for Rookery — pipe lat/lon → JSON feature                      |
| `ffi`    | UniFFI surface — Swift, Kotlin, Python bindings                                   |
| `motion` | Movement classification over decoded features                                      |
| `fuzz`   | AFL/libfuzzer harness — crash-testing byte-level decoders                         |

`src/lib.rs` at the root is the superseded wasm-bindgen client, kept out of the
workspace as a porting reference. `web-demo/` is the browser demo (below).

## Quick Start

```bash
cargo build --workspace
cargo test --workspace

# WASM. wasm-pack resolves /usr/bin/rustc by default, which has no wasm32
# target -- put rustup's toolchain first or the build fails on sysroot.
PATH="$HOME/.cargo/bin:$PATH" \
  wasm-pack build wasm --target web --release \
  --out-dir ../web-demo/lib/client --out-name ptiles_client

# ...and again for `node --test`, which needs the nodejs target
PATH="$HOME/.cargo/bin:$PATH" \
  wasm-pack build wasm --target nodejs --release --out-dir ../wasm-pkg
```

## Reading a file

Index layout is **detected, never assumed**. Two entry widths and three block
offset bases exist in published files, and the header can disagree with its own
index:

| width | layers |
| --- | --- |
| 19 B | roads, water, business, buildings_v8 |
| 38 B | parks, rail, trails, ev, places, signals, camera |

Offsets are absolute, relative to `blocks_offset` (buildings_v8), or absolute
with a correction when `blocks_offset` overshoots where the index really ends.
`PtilesFile::layout()` reports which was chosen.

Layers with a 38-byte index also pack several cells per compressed block, so
**use `read_cell`, not `read_block`** — the latter hands a record decoder a cell
table it will parse as records and return plausible garbage rather than an
error.

```rust
let file = PtilesFile::open(FileSource::open(path)?)?;
let bytes = file.read_cell(cell)?.expect("cell present");
let signals = ptiles_core::decode_signals(&bytes)?;
```

Every `.ptiles` kind is versioned independently — the version byte is scoped to
its magic, there is no release-wide version, and a new kind starts at 1. See
[`SUPPORTED_FORMATS.md`](SUPPORTED_FORMATS.md), which is generated from
`ptiles_core::SUPPORTED_FORMATS` and asserted against it by a test.

## Queries

Decoding is the floor. `core` also answers the questions the layers exist for,
so a caller does not re-derive them per platform:

| | |
| --- | --- |
| `locate` | the road or trail under a point, the nearest address, and whether you are *on* it or merely near it |
| `park_at` / `water_at` | the park or water body you are in, else the nearest boundary |
| `nearest_trail` / `nearest_trailhead` | which path you are walking, and where a network is entered — separate questions, so separate answers |
| `nearest_rail` / `nearest_station` | the track under you, and the platform you would board at |
| `cameras_seeing` | which surveillance cameras can see a point: in range, aimed at it, and not blocked by a building |
| `plan_charge_stops` | the charging stops a drive needs, given the range left |
| `viewshed` | what is visible from a point at a given eye height |

Surfaces differ, and the table above is `core`. `locate`, the park/water/trail/
rail lookups and `cameras_seeing` reach all three of the CLI, the wasm bridge
and the FFI. `viewshed` and `plan_charge_stops` are wasm-only today: both were
written for the browser demo, and neither has a CLI subcommand or a UniFFI
record yet.

Two of those lean their uncertainty in opposite directions, on purpose.
`viewshed` assumes an unmeasured building is tall, so a marginal sight line
comes out blocked and it under-reports what can be seen. `cameras_seeing`
inverts that: "blocked" is the reassuring answer there, and telling someone
they are unobserved on the strength of a guessed height is the mistake worth
avoiding. Every flag it returns says which parts are inference.

`plan_charge_stops` drives 80% of the range it is given
(`ptiles_core::CHARGE_RESERVE`), because a driver arriving at a charger on 0%
has already been stranded once by a queue, a broken unit or a headwind. It
prefers a stop in the far half of each leg, and the most powerful charger among
those; a leg with nothing in reach reports the shortfall rather than inventing
a plan.

Routing has two profiles. `RouteProfile::Driving` is the road graph;
`RouteProfile::Foot` routes trails, tracks, footways and steps at walking
speed, ignores posted limits and one-way tags (both describe vehicles), and
excludes motorways. `trail_segments` feeds the trails layer into the same graph
builder, so a walk down a path and along a residential street is one route.

## Tests

```bash
cargo test --workspace
node --test web-demo/test/ptiles.test.mjs   # the wasm reader vs conformance/corpus/
node --test "demo/test/*.test.mjs"          # the legacy JS reader
python3 -m unittest conformance.test_helpers # corpus slicer/checker byte helpers

# The page itself, in chromium. Serves web-demo/ on its own port; tiles come
# from the live host, so this needs network.
python3 web-demo/test/render_check.py

# The routing modes, in chromium against the live tiles. A walk must come out
# slower than the drive over the same pair of points, and an EV leg with too
# little range must produce stops and a longer route -- the assertions a page
# that silently ignores the controls cannot pass.
python3 web-demo/test/route_check.py

# Legacy page checks
python3 demo/test/intersection_check.py  # junction panel
python3 demo/test/cache_check.py         # warm open makes one request
python3 demo/test/coarse_check.py        # coarse lookup pulls a fraction
```

`core/tests/index_layout.rs` covers the width × offset-base matrix and
adversarial input; `real_layers.rs` runs against real files and fails if none
are found, so an empty data directory can't pass for coverage.
`demo/test/differential.html` requires the JavaScript and wasm decoders to agree
field-for-field — they are separate implementations on purpose (wasm decode
measured ~30x slower across the boundary, see `bench_wasm.html`), so drift is a
test failure rather than a silent mis-render.

Two assertions in `web-demo/test/render_check.py` are worth knowing about,
because they are the ones a plausible stub cannot pass: raising the observer's
eye must reveal *more* buildings, and shrinking the view finder's radius must
reveal *fewer*. A bare count is green on a 2D shadow test that ignores height
entirely, which is the bug both modes exist to avoid.

## Demo

Click any building in the US: https://steele.red/ptiles

`web-demo/` is the source for that page; `demo/`, which hand-decodes the format
in JavaScript, is kept at https://steele.red/ptiles-legacy for comparison.
Both URLs are symlinks that the site's `build.py` dereferences into its output,
so editing `web-demo/` changes nothing until a deploy runs:

```bash
./web-demo/deploy.sh            # dry run: what would change
./web-demo/deploy.sh --apply    # build the site, sync, verify the live headers
```

Rebuild the wasm first. The page loads `web-demo/lib/client/`, which is a
*different* bundle from the `wasm-pkg/` the node tests use — building one and
not the other leaves the page calling exports that are not there.

It opens files over HTTP Range requests, never downloading a whole layer, and
caches each layer's header, dictionary and index in the Cache API keyed by
ETag. A warm load costs one 256-byte request instead of ~4.5 MB.

Beyond drawing the layers, the page answers questions off the same bytes:

| | |
| --- | --- |
| Click a point | nearest building, the businesses in it, the street (class, one-way, speed limit, lanes, surface, bridge/tunnel), the junction and its controls, and — after an opt-in load — county, ZIP and timezone |
| Layers | roads, water, parks, rail, trails, buildings (2D or extruded), EV chargers, cameras and signals, each drawn straight from its own file |
| EV | every public charging station in view, filled when it charges fast; click one for power, bays, connectors, access and network |
| Line of sight | what is visible from a point at a given eye height, and which nearby cameras have a clear line back to you |
| View finder | the reverse: which buildings can see a river, a park, a railway or a named business |
| Route | A* over the road graph, within a corridor of cells |
| Trails only | the same A* under core's foot profile: paths, tracks, footways and steps, walking speeds, one-way tags ignored, motorways excluded. Quiet streets link the fragments, because the path through one park does not touch the path through the next |
| EV range | type the miles left on the dash and the route plans charging stops from the `ev` layer, driving only 80% of that range per leg and re-routing through each stop |

Line of sight is reciprocal, which is why it and the view finder need no
geometry of their own: both run `viewshed` from the far end and read the answer
back.

## Live Tiles

```
https://maps.mydatatimeline.com/maps/2026-08-07/{ST}.{layer}.ptiles
https://maps.mydatatimeline.com/maps/US.{admin,signals,camera}.ptiles
```

Per-state, in the dated snapshot the demo reads: `buildings_v9`,
`business_v4`, `roads_v2`, `water_v1`, `parks_v1`, `rail_v1`, `trails_v1`,
`places_v1`, `address_v2`, `ev_v1`. US-wide, at the flat `/maps/` root:
`admin`, `signals`, `camera`.

The older flat `/maps/{ST}.{layer}.ptiles` layout is still served and is what
`SUPPORTED_FORMATS.md`'s notes were written against; the dated directory is
the current build and the one `web-demo`'s `PTILES_BASE` points at.

`ev_v1` is the newest layer: 13,443 charging stations across the 51 files,
built by `scripts/build_ev.py` in the [ptiles
repo](https://github.com/baocin/ptiles) from OSM `amenity=charging_station`.
Power and connector are tagged on roughly a third of them, and decode to an
explicit unknown on the rest rather than to a zero.

`buildings_v8` is the only filename carrying a version, and it is part of the
name rather than a claim about the contents — the version lives in the header.
The reader accepts 8 and 9 there; every published file is 8 today.

Two gaps to know about before building on this:

- **`{ST}.business_name_index.ptiles` is not published.** The client's
  index-accelerated name search is written and tested against a local copy, but
  every state checked (TN, CA, NY, GA, TX) 404s on the host, so it falls back
  to scanning the whole business file — measured at over 180 s for one query.
- **Every published business file reports `feature_count = 0`,** a builder bug.
  Records decode fine; only the header count is wrong.

`signals` and `camera` carry a coarse index in the header's `aux` region — a
sampled cell→position map, ~5 KiB, letting a point lookup fetch one short run
of the real index instead of all 4 MB of it. The browser demo uses it; the Rust
core reads the whole index and does not yet.

## License

MIT
