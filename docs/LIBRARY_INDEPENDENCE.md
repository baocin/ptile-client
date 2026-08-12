# Library independence audit

Scope: `core/`, `ffi/`, `wasm/`. Question: has anything from the Looky Android
app or the steele.red/ptiles web demo leaked into the library, such that a
third party building a different application on PTiles would inherit someone
else's product decisions?

Read-only audit against the working tree at commit `14694c0` plus the
uncommitted changes to `core/src/admin.rs` and `ffi/src/lib.rs`.

## Verdict

The library is in good shape. The boundary is being held in the places where
it matters most and where it would have been easiest to break.

Concretely, all four things that would have been the obvious leaks are on the
correct side of the line:

- The **business search ranking** — "name similarity pulled toward you by
  distance", the thing that makes a search feel right in one app and wrong in
  another — is in Kotlin, at
  `android/app/src/main/java/com/steele/looky/offline/PtilesRepository.kt:753`
  (`rankByNameAndDistance`). The library's own scoring
  (`core/src/business_search.rs:90`, `score_match`) only reports exact / prefix
  / substring, which is a property of the index format, not of a product.
- The **pack CDN base URL** is in the app
  (`android/app/src/main/java/com/steele/looky/offline/MapPackDownloader.kt:24`),
  not in the library. `core/src/http_source.rs` takes whatever URL it is given.
- The **baked state and county boundary geometry** is in the APK's assets
  (`us_state_bounds.txt`, `us_county_bounds.txt`), not compiled into a crate.
- **No screen, zoom, pixel, tile or canvas concept exists anywhere in `core/`,
  `ffi/` or `wasm/`.** The only viewport vocabulary is in doc comments in
  `core/src/query.rs`, explaining why `MAX_BOUNDS_CELLS` exists. The API is
  entirely lat/lon/metres/H3.

What leakage there is clusters in one place: **routing policy that ended up in
`ffi/` instead of `core/`**, tuned against one dataset and one caller, and not
overridable. Plus one genuine correctness bug in `core/` where a US/English
naming convention is used as a data field. Seven findings below, none of them
architectural, all of them small diffs.

There is also a broader US-centrism running through the project that is worth
naming honestly and separating from Looky: the *format* and the *published
data* are US-shaped (state-named pack files, an admin layer whose fields are
country/state/county/zip/tz). The decoders faithfully decode that. That is
data-pipeline US-centrism, not application leakage, and it is category (a)
below — but it does mean "application-agnostic" and "geography-agnostic" are
two different claims, and only the first one currently holds.

---

## (a) Genuine format and domain concepts — these belong here

Not findings. Listed so the (c) list can be read against them.

- `core/src/admin.rs:36-55` — `AdminGridEntry` / `AdminInfo` carry
  `country / state / county / zip / tz`. These are the on-disk fields of the
  admin layer (`core/src/admin.rs:28-29` documents the 16-byte grid entry
  layout). A decoder must name what the format stores. The format is
  US-shaped; the decoder is correct.
- `ffi/src/lib.rs:670-735` — `LayerKind` and its `_v<N>` suffix stripping.
  This is the PTiles publishing convention, mirrored from `cli/src/main.rs`.
  It predates Looky.
- `core/src/business_search.rs:1-42` — the first-letter bucket index. The
  module doc is unusually good here: it states plainly that the sidecar is a
  prefix accelerator and not a substring index, rather than promising search it
  cannot deliver.
- `core/src/nav.rs:46-106` — `Maneuver`, its thresholds, `as_str`. Turn
  semantics are a domain concept, and the serialization is pinned so a JS and a
  Kotlin caller cannot disagree on the string.
- `core/src/camera.rs`, `core/src/viewshed.rs`, `core/src/ev.rs`,
  `core/src/trails.rs` (`sac_scale`) — layer-specific domain vocabulary, all of
  it from the underlying data, none of it from an app.
- `core/src/query.rs:26` — `MAX_BOUNDS_CELLS = 512`. A refusal rather than a
  silent truncation. Correct behaviour for a library.

## (b) Reasonable general-purpose conveniences

Also not findings.

- `ffi/src/lib.rs:1363` `buildings_at`, `:1393` `nearest_roads_at`, `:1445`
  `nearest_intersections_at` — batch variants that group by H3 cell so a run of
  points costs one block read. Any caller enriching a track wants this; it is
  not shaped around a screen.
- `ffi/src/lib.rs:1322` `prefetch_bbox`, `:1345` `cached_block_count`, `:1350`
  `clear_cache` — cache control the caller can actually reason about.
- `ffi/src/lib.rs:2090` `PtilesStack::with_layers` — every layer optional, "pass
  whichever files the region actually has". Correct shape.
- `ffi/src/lib.rs:2395` `Navigator` — despite arriving in a commit titled
  `feat(android)`, this object contains nothing Looky-specific. It holds the
  path, the cumulative distances and the turn queue on the Rust side so a
  position update is one small call, takes `roads` only to name turns, and
  accepts an empty road list for an unnamed queue. A delivery app, a boat, or a
  desktop tool would want exactly this. **Not leakage.**
- `ffi/src/motion.rs` — its module doc explains it exists precisely because
  callers were re-implementing the classifier in Kotlin against drifting copies
  of the thresholds. That is the anti-leakage argument, applied correctly.

---

## (c) Real leakage — findings

### 1. `admin_level` is inferred from the English string `" County"`

`core/src/admin.rs:182` (uncommitted change):

```rust
let admin_level = if name.ends_with(" County") { 6 } else { 4 };
```

Why it is leakage: this is a decoder in `core/` deriving a structured data
field from a US-English naming convention, to serve one app's need to draw
county lines at coarse zoom (commit `14694c0`, "county lines at coarse zoom").
The format stores no level field, so the decoder invents one from a substring.
It is wrong for Louisiana parishes, Alaska boroughs and census areas, Virginia
independent cities, and every jurisdiction outside the United States. It is
also silent: a ring that should be level 6 is reported as level 4, and a caller
drawing state lines gets county lines mixed in with no way to tell.

The doc comment added alongside it (`core/src/admin.rs:72-74`) is honest about
the mechanism — it cites the writer, `build_admin.py:353` — which makes this a
known shortcut rather than an accident. But it lives in the wrong crate.

Smallest fix: make the guess visible instead of authoritative. Change
`admin_level: u8` to `admin_level: Option<u8>` and return `None` when the name
carries no signal, or keep the field and add `level_inferred: bool`. Either way
the caller decides whether to trust it. The real fix is a writer change (emit
the level), and this decoder should be marked as a stopgap pending that.

### 2. Corridor routing policy lives in `ffi/`, not `core/`

`ffi/src/lib.rs:2209-2291` (`PtilesStack::offline_route`) and `:2334-2353`
(`widened_corridor`).

Why it is leakage: this is not FFI translation, it is a routing search
strategy — how wide a corridor to cut, which cells to pull, what to do when the
graph comes back disconnected. It exists only in the uniffi binding, so the
wasm/web-demo caller cannot use it (`wasm/src/lib.rs:577` `route_from_segments`
makes JS assemble its own segment set) and has quietly re-derived its own
corridor logic in JavaScript. Two callers, two corridor policies, one of which
has had the widening fix and the other of which has not. The next binding makes
it three.

Smallest fix: move corridor construction and the disconnected retry into
`core::route_graph` as a function taking a `CorridorPrefs` struct with the
current values as `Default`. `ffi::offline_route` becomes a thin call; `wasm/`
gains the same behaviour for free.

### 3. Corridor margins are fixed degrees, tuned near 35°N

`ffi/src/lib.rs:2224-2225`:

```rust
let lat_margin = 0.015_f64.max(lat_span * 0.15);
let lon_margin = 0.020_f64.max(lon_span * 0.15);
```

Why it is leakage: a degree of longitude is 91 km at 35°N (Tennessee, where
this was measured) and 56 km at 60°N. The same call therefore cuts a corridor
roughly 1.8 km wide in Nashville and 1.0 km wide in Anchorage — the end cap
silently shrinks by nearly half as you go north, and the doc comment's claim of
"a 1.5 km-ish end cap" (`ffi/src/lib.rs:2203`) is only true in the latitude
band the app was tested in.

Smallest fix: express the margin in metres and convert:
`lon_margin = margin_m / (111_320.0 * lat.to_radians().cos())`, with the same
`.max(span * 0.15)` proportional term. Roughly four lines.

### 4. Snap radii are hardcoded per mode with no caller override

`ffi/src/lib.rs:2247`:

```rust
let snap_radius = if mode == OfflineRouteMode::Driving { 250.0 } else { 120.0 };
```

Why it is leakage: 250 m is the right default for a phone in a car on a US
road network. It is wrong for a boat, a bicycle courier working alleys, a
warehouse, or a country with a sparser digitised network — and there is no way
to say so. The rest of this API is good about defaults (`nearest_road` takes
`threshold_m`, `Navigator::new` takes `name_radius_m` with a 0-means-default
convention); this one call is the exception.

Smallest fix: add `snap_radius_m: f64` to the signature, `<= 0.0` meaning "use
the per-mode default", matching the convention already used at
`ffi/src/lib.rs:1391` and `:2427`.

### 5. The retry constants and their doc are written against one dataset and one caller

`ffi/src/lib.rs:2259-2264` and `:2323-2329`:

> `measured on the Tennessee pack, Savannah to the midpoint of Camden goes from
> Disconnected to a 70.9 km route this way. It only helps when the box has room
> left, which is why the caller also splits long legs.`

Why it is leakage: two separate problems in one comment. First,
`DISCONNECTED_RETRY_SCALES = [2.5, 2.0, 1.6, 1.3]` is a ladder tuned by
measurement on a single state's roads pack (commit `b7eedc0`: "45 city pairs,
14 routed before, 38 after"). That is fine as a default and dishonest as a
constant with no override. Second, the library documents its own contract in
terms of a workaround the *caller* is expected to implement — Looky's leg
splitting. A library should either do the splitting or state the limit
neutrally; it should not describe the app's compensation as part of its
behaviour.

Smallest fix: fold the scales into the `CorridorPrefs` from finding 2, and
rewrite the doc to state the limit ("a corridor that needs more than
`MAX_BOUNDS_CELLS` is refused; split the request into legs") without naming what
one particular caller does about it.

### 6. `OfflineRouteMode::Trail` is Looky's screen name, not the library's concept

`ffi/src/lib.rs:155-160`.

Why it is leakage: `core` already has the right vocabulary —
`RouteProfile::Foot` (`core/src/route_graph.rs`), which `offline_route` maps to
at `ffi/src/lib.rs:2244`. "Trail" is the name of a screen in the Looky app
(`android/.../ui/TrailScreen.kt`). Worse, the enum variant conflates two
things: the routing profile (foot) and a data decision (also pull the trails
layer and merge it into the graph, `ffi/src/lib.rs:2235-2240`). A caller who
wants pedestrian routing on roads only, or trails-only routing, cannot say so.

Smallest fix: rename the variant to `Foot`. If the layer-merge decision needs
to be separable later, that is a second parameter, not a second enum variant —
but the rename alone costs one line plus regenerated bindings.

### 7. `<state>.<layer>.ptiles` in a user-facing error string

`ffi/src/lib.rs:86`:

```rust
#[error("could not infer layer from filename {path:?} (expected <state>.<layer>.ptiles)")]
```

Why it is leakage (mildly): the first token is discarded — `ffi/src/lib.rs:691`
literally binds it as `let _state = parts.next()?`. The parser does not care
what it is. Calling it "state" in the error a third-party developer will read
advertises a US administrative unit as part of the file naming contract, and
sends anyone publishing packs for, say, French départements looking for a
setting that does not exist. Same wording at `ffi/src/lib.rs:889`.

Smallest fix: say `<region>`. Two string edits. The same applies to the crate
doc at `ffi/src/lib.rs:9-11` and the section header at `:2042`, which describe
the whole design as being about "one state's three files".

---

## Adjacent, not leakage, but worth knowing

- **`ffi/src/bin/spread_probe.rs` is untracked.** It is a throwaway probe that
  hardcodes Looky's `featuresAround` sampling steps, its `capFeatures` cap
  values, its 1080x1742 canvas (line 136), and Nashville as the default origin
  (line 28). Its own line 2 says "Not shipped; delete after the numbers are
  recorded." It is currently untracked, so it has not leaked into the crate —
  just do not commit it. If the measurement is worth keeping, it belongs in
  `android/` or a scratch directory, not in `ffi/src/bin/`.
- **`AdminLayer::polygons_in`** (`ffi/src/lib.rs:1971`, uncommitted) decodes the
  entire ~6.2K-ring / 611K-vertex polygon table on every call and then filters
  by bbox. Called per map redraw that is a lot of repeated work. Not leakage —
  the bbox parameter is the right API — but it wants a decoded-table cache
  behind it, the same way `PtilesLayer` caches blocks.
- **Two `Navigator` implementations.** `ffi/src/lib.rs:2395` and
  `wasm/src/lib.rs:772` wrap the same `core::nav`, but the wasm one also
  exposes `name_turn`, `probe` and `length_m` and the uniffi one does not. Not
  leakage, but the two binding surfaces are drifting, and turn-by-turn is
  exactly where a divergence gets noticed on a road.
- **`core/src/http_source.rs:332,389`** hardcode `maps.mydatatimeline.com` in
  live tests. Tests only, and they are clearly live tests, but it does couple
  the core test suite to one operator's CDN.

## What would keep it clean

One rule covers most of the above: **if a decision has a number in it that
someone measured, it belongs in `core` behind a params struct with a
`Default`, not in a binding.** Findings 2 through 5 are all the same mistake,
and all four disappear together.
