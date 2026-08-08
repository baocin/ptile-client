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

With no accelerometer and no road context, **the classifier cannot see a walk below 2.2 m/s.** The
stateless walking floor is ~5 mph, so a stroll votes `Stationary`. That is measured, not
theoretical: 94 minutes of real walking at 1.21 m/s in
`test-fixtures/gpx/tn-maryville-hike-1063250.gpx` votes `Stationary` for all 1,124 of its points
(`speed_alone_cannot_see_a_stroll_but_can_see_a_jog`).

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

## 4. Building labeled fixtures

`label-gpx/` (<https://steele.red/ptile-label-gpx/>) takes a GPX from the app, classifies it,
lets a human correct the segments, and exports the labeled result in the format `SCHEMA.md`
describes. An app-produced GPX with sensor extensions is the input that makes the strongest
fixtures — the six committed OSM traces have position and time only, so they cannot exercise the
accel fallback or the accuracy gate at all.

Its exported file marks what was measured versus computed (`derived="true"`) and which segments a
human touched (`source="human"`), so a test can tell evidence from the classifier agreeing with
itself.
