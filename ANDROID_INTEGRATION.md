# Android integration notes

For the Rookery Android client (`clients/android/app/src/main/java/com/rookery/rook/`), whose
`movement/` package is the original this repo's `ptiles-motion` was ported from.

Two audiences, and the split matters:

- **What the app could send that would make the classifier better.** Requests, not requirements.
- **What ptile-client does when the app does not send it.** Guarantees. Missing data is normal —
  a sensor is off, a permission is denied, an exporter is a version behind — so nothing here
  depends on the app changing.

---

## 1. `accel_mean` and `accel_window_s` in the GPX export

`AccelStats::calculate` computes five values (`motion/src/movement.rs`):

| field | GPX element (`label-gpx/SCHEMA.md`) | exported today |
| --- | --- | --- |
| `variance` | `accel_variance` | yes |
| `dominant_frequency` | `accel_freq` | yes |
| `step_count` | `accel_steps` | yes |
| `mean_magnitude` | `accel_mean` | **no** |
| `window_duration_s` | `accel_window_s` | **no** |

The app already has both — `AccelStats.calculate` in `movement/AccelStats.kt` returns
`meanMagnitude` and `windowDuration`, they just are not written to the GPX. So this is an exporter
change, not new sensor work: two more elements inside the per-point `<extensions>`.

```xml
<extensions>
  <speed>0.1</speed><accuracy>8.0</accuracy>
  <accel_variance>0.02</accel_variance><accel_freq>0.3</accel_freq><accel_steps>0</accel_steps>
  <accel_mean>9.81</accel_mean>          <!-- m/s^2 -->
  <accel_window_s>4.0</accel_window_s>   <!-- seconds -->
</extensions>
```

### Why they are worth sending

Nothing in the current accel table reads either field, so this is **latent, not broken**. It
becomes real for anything that needs to know what the phone was doing rather than how much it
shook:

- **Mean magnitude separates a phone in a pocket from a phone on a car seat.** Both can show low
  variance while driving; a pocket sits at ~9.8 m/s² with the gravity vector through it, a device
  flat on a seat reads differently. That is the signal that would let the classifier stop leaning
  on GPS speed to recognise a vehicle at rest.
- **Window duration says how much evidence a reading represents.** `variance = 0.02` over 4 seconds
  and over 0.2 seconds are not equally good arguments, and a confidence value that ignores the
  difference is overstating the short one.

Neither can be reconstructed from the other three, which is why they need to come from the device.

### Do not send `0` for a value you do not have

Send the element or leave it out. `0.0` is a *reading*: a mean magnitude of zero means free fall,
and a zero-length window means no window. `label-gpx/SCHEMA.md`'s "absent vs zero" rule is not
pedantry — a fixture that cannot tell the two apart teaches the classifier that "sensor off" and
"perfectly still" are the same state.

The same applies to `<accuracy>`: an omitted accuracy means unknown, and `0.0` claims a perfect
fix, which is the most trusted value there is.

---

## 2. What ptile-client guarantees when data is missing

`ptiles-motion` is built for partial input, and this is tested rather than asserted:

**A cell id that H3 does not recognise is an error, not a position.**
`cell_center` answers `(0.0, 0.0)` for an invalid id, and that fallback is how a
masked lookup key -- the low filler bits cleared for an index probe -- silently
became null island, putting every v8/v9 building it positioned ~9,700 km from
where it belonged. Well-formed records, wrong planet, no error. There is now
`try_cell_center` returning `Option`, and `decode_buildings_for_cell(bytes, cell)`
which derives the centre from the same id the caller already holds, so the wrong
centre is unrepresentable rather than merely discouraged. Prefer it to
`decode_buildings`; the wasm export of the same name does the same thing.

**Absence lives in the type, not in a sentinel value.** `mean_magnitude` and `window_duration_s`
are `Option<f64>`. A three-field reading deserializes with those two as `None`, never `0.0`, so a
partial window is never half-interpreted as an absent one. A future rule that wants mean magnitude
has to handle `None` explicitly — the compiler makes it, and
`no_accel_window_classifies_like_an_empty_one` fails if the two cases ever stop agreeing without
someone deciding they should.

`variance`, `dominant_frequency` and `step_count` stay plain numbers, because every producer sends
them and `0` is a meaningful reading for each (a still phone, no cadence, no steps).

**Every input to `classify` is optional**, and each of them is genuinely absent on some real fix:

```rust
classify(
    inst_speed_mps: Option<f64>,   // platform did not report a speed
    gps_accuracy_m: Option<f64>,   // platform did not report accuracy
    nearest_road:  Option<&RoadContext>,  // no tile answer here
    accel:         Option<&AccelStats>,   // no accelerometer window for this fix
) -> Vote
```

- **No speed** → derived from consecutive positions by `MotionClassifier`, which is also what
  smooths GPS noise. Callers do not need to compute speed themselves, and should not: a second
  implementation is a second set of thresholds to disagree about.
- **A speed that is negative, NaN or infinite** is treated as not reported, not as slow. Driver
  artefacts do not become measurements.
- **No accuracy** means unknown and the 30 m trust gate stays open; a *reported* accuracy worse than
  30 m (or non-finite) closes it and the accelerometer decides alone.
- **No road context** drops the priors and the speed bands answer. This is the common case away from
  mapped roads, not an error.
- **No accelerometer** falls to the table's catch-all. `None` and `AccelStats::EMPTY` classify
  identically today.
- **A gap in the fixes** longer than `max_gap_ms` (30 s) clears the smoothing window, so speeds from
  before the gap never average into the ones after it. A reported speed still seeds the fresh
  window, because it describes that instant; a position-derived speed measured *across* the gap is
  discarded.
- **Non-monotonic or duplicate timestamps** yield no derived speed rather than a division by zero or
  a negative interval.
- **Coordinates at the poles or the antimeridian, NaN, infinity** do not panic —
  `adversarial_fixes_never_panic` in `motion/tests/gpx_replay.rs` feeds all of them in one sequence.
- **`classify` never returns `Unknown`.** That state belongs to the debouncer before it has
  committed to anything; a vote is always a real answer, so a UI showing votes never shows
  "unknown" forever.

### The one thing missing data cannot buy back

With no accelerometer and no road context, **a cold-started classifier cannot establish a walk
below 2.2 m/s.** The stateless walking floor is ~5 mph, so a trace that never first establishes
Walking still votes `Stationary`. That is measured, not theoretical: 94 minutes of real walking at 1.21 m/s in
`test-fixtures/gpx/tn-maryville-hike-1063250.gpx` votes `Stationary` for all 1,124 of its points
(`speed_alone_cannot_see_a_stroll_but_can_see_a_jog`).

Once Walking is established, sequence-aware callers preserve it while smoothed GPS speed remains
above the 0.5 m/s stationary ceiling. This keeps slow, spatially continuous stretches inside a
walking journey (notably `nc-umstead-trails-1184467`) without preventing a real stop from settling
to `Stationary`.

Two things fix it, in order of how much they cost the phone:

1. **Road context.** A footway hit at 1.2 m/s votes `Walking` at 0.90 confidence. On Android this is
   `PtilesContext.contextAt` → the `road` field, which is already implemented and already returns
   `NearestRoad`; the classifier just has to be passed it instead of `null`. Verified against real
   decoded road records in `motion/tests/road_context.rs`.
2. **An accelerometer window.** Step cadence between 1 and 3 Hz with any variance votes `Walking`
   without needing GPS at all — which is also what carries indoors, where GPS is worst.

---

## 3. Where the shared logic lives now

| Android | ptile-client |
| --- | --- |
| `movement/MovementClassifier.kt` `emit()` | `motion/src/movement.rs` `classify()` |
| `movement/AccelStats.kt` | `AccelStats` in the same file |
| `movement/VoteDebouncer.kt` | `VoteDebouncer`, same CHRE defaults |
| `movement/MovementType.kt` | `MovementType` — five lowercase names, same order |
| `movement/RoadContext.kt` | `RoadContext`, minus the unused snapped lat/lon |
| — (new) | `TrafficControl`: at a mapped signal/stop/give-way the Driving→Stationary grace period stretches from 150 s to 5 min |

Two behaviour differences from the Kotlin, both deliberate:

- **Road context is live, not dormant.** The Kotlin comment says callers pass `nearestRoad = null` in
  v0. Here `RoadContext::from_nearest` converts a `nearest_road` hit directly, and the branches are
  tested against a real Nashville roads block.
- **Non-finite GPS accuracy closes the trust gate.** Kotlin's `gpsAccuracyM > 30.0` is false for
  `NaN`, so a garbage accuracy was treated as a good fix.

Still omitted in both: the gridlock stationary-fraction override and the trailing 5-minute motion
features, which need a GPS trailing window nobody collects yet.

### Reaching it from Android

The `ffi/` crate exposes ptiles-core through UniFFI, which is what `PtilesContext.kt` already uses
for building/road lookups. `ptiles-motion` is **not** exposed there yet — today the classifier is
reachable from the browser (`wasm/`, used by `label-gpx/`) and from Rust. If the app wants to drop
its Kotlin copy and call this one, the work is a UniFFI wrapper over `classify`, `VoteDebouncer` and
`MotionClassifier`; the types are already `no_std` + `serde` and carry no framework dependencies.

Until then the two implementations have to be kept in step by hand, and the tests here are the
reference for what "in step" means: `motion/tests/` plus the unit tests in `motion/src/movement.rs`.

## 4. Gaps found while integrating, and what closed them

Five things the app hit. Four are fixed in the FFI; one cannot be, and saying so
is the honest answer.

### Errors now distinguish offline from out-of-coverage

`PtilesError` was `Open | UnknownLayer | Decode | UnsupportedForLayer |
InvalidRing`, so an unreachable host and a file that does not exist both arrived
as `Open` — opposite situations, one worth retrying and one never. `core` had
always distinguished them (`SourceError::HttpNetwork` vs `HttpStatus` vs
`RangeNotSupported`); the FFI was flattening it away. Three new variants carry it
across:

| variant | means | do |
| --- | --- | --- |
| `Network { path, message }` | DNS, TLS, connection refused/reset, timeout | you are offline: retry later, fall back to cache |
| `NotFound { path, status }` | the server answered 404/403/… | that layer is not published: do not retry |
| `RangeUnsupported { path, status }` | server ignored `Range` (200, not 206) | server/CDN misconfiguration, not a data problem |
| `Open { path, message }` | local file missing, bad magic, unsupported version | a local/structural problem |
| `InvalidBounds { message }` | malformed or oversized bbox (see prefetch) | shrink the region |

So `stateFor`'s offline fallback no longer has to guess:

```kotlin
try { layer.nearestRoad(lat, lon) }
catch (e: PtilesException.Network)  { useCachedContext() }   // offline
catch (e: PtilesException.NotFound) { markLayerUnavailable() } // never coming
```

### Batch queries: one block read per cell, not per point

`buildingsAt(points)`, `nearestRoadsAt(points, thresholdM)` and
`nearestIntersectionsAt(points, thresholdM)` take a list and return one answer
per input, in order. Internally they group by H3 cell, so a day of tracking
(~12,300 points across a few dozen res-7 cells) costs a few dozen block reads and
decompressions rather than 12,300. A test pins the ratio: eight points in one
cell must touch at most two blocks.

`PtilesLayer` also now memoizes decompressed blocks, including the *absence* of a
block, so repeated queries in a cell you have already touched are free. That was
the missing half: `HttpSource` cached byte ranges already, but every query still
re-ran zstd over the block. `cachedBlockCount()` and `clearCache()` let a caller
see and bound it.

This is what makes per-point enrichment viable instead of per-segment sampling.

### bbox prefetch: the middle ground

`prefetchBbox(minLat, minLon, maxLat, maxLon)` fetches and caches every block
covering a region in one pass, then every query inside it is served from memory.
Between range-reading forever and downloading 118 MB of CA roads.

Capped at 512 H3 res-7 cells (~2,600 km², a metropolitan area — not a state). A
larger box is an `InvalidBounds` error, not a truncated prefetch: a partial
region that reports success is worse than a refusal, because the caller then
trusts data it does not have. Walk a bigger area in tiles.

### Layer metadata and coverage

`metadata()` returns `LayerMetadata`: layer name, path, schema version, coverage
bbox, feature count, block count, byte length, and — since the format carries no
build date — the HTTP `Last-Modified` and `ETag` captured at open. For a file you
range-read rather than download, those two are the only provenance there is: they
are how you answer "is this TN.roads from 2024 or last week?". `None` for a local
file, or a server that does not send them.

Free: every field comes from the 256-byte header already read at `open()` and
that same first response. No extra request.

`covers(lat, lon)` answers the cheap question first — outside the bbox nothing
exists and no range read can improve on that. Inside it does *not* promise a
block: the corpus slice has a whole-state bbox and 48 cells, which is exactly the
distinction that caught out the first version of the tests here.

**One caveat, recorded because the number lies:** `feature_count` is 0 on the
**v3** business layers — a builder bug (it compares a string to an int) — while
the records decode fine. Treat 0 as unknown, not empty. On the published
`business_v4` files the count is correct, and worth checking: v4 records have no
length prefix, so "decoded fewer records than the index claims" is the only cheap
signal that the byte stream desynchronised rather than the block being short.

### The intersection vocabulary, and the one that does not exist

`intersectionTypeName(t)` → `traffic_signals` | `stop` | `give_way` |
`roundabout` | `junction` (0/unrecognised). `intersectionHoldsTraffic(t)` is the
signals/stop/give-way group — the distinction the motion classifier uses to tell
a red light from an arrival. Both come from `ptiles_core::intersection_type_name`,
so the vocabulary has one home; `label-gpx`'s hand-written copy of the same five
strings is deleted, and the wasm build exports the same two functions.

**`categoryIdx` does have a vocabulary, and an earlier version of this note said
it did not. Correction:** the builder publishes `{ST}.business_categories.json`
next to the layers (11 KB for TN, `{"categories": [...]}`), so the mapping is one
plain fetch away — no `aux` change and no client-side invention needed.

Read it **1-based**: the builder assigns `i + 1` over a 0-based array, so
`categoryIdx == 0` means "no category" and `n` names `categories[n - 1]`. Reading
it 0-based labels every POI with its neighbour's category and nothing errors.

The array mixes full taxonomy paths (`"Business and Professional Services >
Office"`) with bare slugs (`"church_cathedral"`); take the last `>`-separated
segment and turn underscores into spaces. `label-gpx/js/context.js`'s
`categoryLabel` is the reference implementation. The sidecar carries no version,
so keep the raw index too — a label is a lookup against a file with its own
vintage, the index is what the layer actually holds.

The version-sniffing `decode_business` now **refuses** a v4 block
(`DecodeError::CellRequired`) instead of decoding it against an origin of
(0, 0). That silent success was the bug: records parsed cleanly and came back a
few hundred metres off Null Island, which every caller downstream reads as "no
businesses here". `PtilesLayer.businessesNear` already routes through the
versioned path, so nothing in the FFI changes -- but a caller reaching for the
raw decoder now gets an error that names the fix.

**New in `BusinessInfo`: `sourceType`, `sourceId`, `confidence`,** from an
extended-attributes trailer every record carries and the decoder used to skip
entirely. `sourceType` is 1 = Overture, 2 = Foursquare; `sourceId` is that
dataset's own id (a GERS uuid, or a Foursquare venue id) and is the only stable
handle back to the source. Skipping the trailer was harmless in v3, whose length
prefix resynchronises every record, and fatal in v4, which has none: the stream
desynchronised after record #1 and produced thousands of well-formed garbage
records before dying with `unexpected end of input`.

(Signals are unaffected: `.signals` records already carry their type as a string,
decoded from the format's own table. `BuildingInfo.category` likewise.)

### Change points, as a second opinion on the classifier

`ptiles_motion::significant_shifts` answers "where did the motion actually change?"
from the speed series alone -- Welch's t-test on adjacent windows, no thresholds
and no movement vocabulary, Bonferroni-corrected across the candidates tested and
thinned so one change reports once. It returns an index, a timestamp, the signed
t statistic, the p-value, and the corrected level it was accepted at.

Worth having on the phone for two reasons. It is a cheap sanity check on the
classifier: a committed transition with no shift near it usually means the
debouncer reacted to noise, and a shift with no transition usually means a real
change the thresholds missed. And it needs no map and no accelerometer, so it
works where everything else degrades.

Also exported through wasm as `significant_shifts(t_ms, speed_mps, config)`, which
is what `label-gpx`'s "Speed & shifts" view draws.

## 5. On the second implementation in the server

The server's `location/ptiles/` — 13 files: header parsing, block offsets, zstd,
H3 lookup, admin/buildings/business readers, geometry, scoring — is a full
reimplementation of what `ptiles-core` does, and `scoring.ts` is
`PtilesStack.score`'s algorithm written a second time. With the Kotlin reading
`buildings_v8` while that TS header says v7/v8, that is three lineages of one
format drifting apart, and the drift shows up as a wrong answer rather than a
build error.

A wasm build deletes the TS port, and it already exists: `wasm/` is built for the
browser today (`web-demo/`, `label-gpx/`) and the same crate builds for Node with
one flag —

```sh
wasm-pack build wasm --target nodejs --out-dir ../wasm-pkg --release
```

— which is how `label-gpx`'s own test suite drives it under `node --test`. So the
server-side move is mechanical rather than exploratory:

1. Vendor or publish `wasm-pkg/` and `require()` it where `location/ptiles/`
   is imported today.
2. Replace the readers with `parse_header` / `parse_index_layout` /
   `index_entries_absolute` / `decompress_block` / `merged_cell_slice` and the
   per-layer `decode_*` exports — the same functions `web-demo/js/ptiles.js`
   calls, which is 574 lines of JavaScript holding *zero* format knowledge and is
   the shape to copy.
3. Replace `scoring.ts` with `score_candidates`, so the ranking has one
   definition. This is the one that changes behavior: expect small differences,
   and treat the Rust as correct (it is what the golden fixtures and the
   conformance corpus test).
4. Delete `location/ptiles/`.

Two things that will bite in Node specifically: `osm_id` on business records can
exceed 2^53 and arrives as a `BigInt`, and the address layer must be sliced by
the entry's *stored* cell id rather than the masked lookup key — masked silently
returns zero records where stored returns all of them.

## 6. Building labeled fixtures

`label-gpx/` (<https://steele.red/ptile-label-gpx/>) takes a GPX from the app, classifies it,
lets a human correct the segments, and exports the labeled result in the format `SCHEMA.md`
describes. An app-produced GPX with sensor extensions is the input that makes the strongest
fixtures — the six committed OSM traces have position and time only, so they cannot exercise the
accel fallback or the accuracy gate at all.

Its exported file marks what was measured versus computed (`derived="true"`) and which segments a
human touched (`source="human"`), so a test can tell evidence from the classifier agreeing with
itself.
