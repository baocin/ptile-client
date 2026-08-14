# Motion pipeline: accelerometer to recorded GPS point

Paths are from the repo root. Android line numbers are as of this commit.

## The short answer

**Nothing in the motion classifier causes a GPS fix to be recorded.** Every fix
the FusedLocationProvider delivers is written, unconditionally. The classifier
only decides which `<trk>` the point lands in and what the badge says.

That is worth stating plainly because the question "what accel-to-motion
classifier causes a GPS recording?" assumes a gate that does not exist. The two
loops are independent:

| Loop | Rate | Owner | Effect |
|---|---|---|---|
| Location | `AppSettings.gpsIntervalSeconds` (3-60 s, default 3) | `TraceService.requestLocationUpdates` (`android/app/src/main/java/com/steele/looky/location/TraceService.kt:256`) | every delivered fix is appended to the GPX |
| Classification | 1 Hz | `TraceService.begin` (`TraceService.kt:237`, `CLASSIFICATION_INTERVAL_MS` at `:66`) | re-labels the last known position; writes nothing |

`record` (`TraceService.kt:284`) appends on arrival — `recorder.append(...)` at
`TraceService.kt:297` — with no movement test in front of it. A stationary
phone at 3 s polling produces a point every 3 s, all labelled `Stationary`.

PTiles *does* return a sampling recommendation (`SamplingAdvice`,
`motion/src/sampling.rs:70`) that would raise and lower the GPS interval with
the movement class — 60 s while stationary, 2 s while driving
(`motion/src/sampling.rs:169-193`). **Looky deliberately ignores it.** The user's
chosen rates win; see the recording-settings section of `android/HANDOFF.md`.
If that ever changes, the classifier becomes a gate and this document is wrong.

## Stage 1: samples

`MotionEngine.onSensorChanged` (`android/.../location/MotionEngine.kt:212`)
appends `event.values[0..2]` to three parallel `ArrayList<Float>`, capped at 300
entries (`:224`). The listener is registered in `MotionEngine.register`
(`:100`) with a delay of `1_000_000 / rateHz` **microseconds** and
`maxReportLatencyUs = 0` (no batching), on a dedicated `HandlerThread` — not
the main looper. That thread matters: the sensor event queue between the HAL
and the looper is small and fixed, so a main thread busy drawing the vector map
does not delay samples, it loses them.

The delay is a *hint*. Android rounds it to a rate the hardware supports and may
deliver less. `MotionEngine.deliveredRateHz` (`:76`) is therefore measured, not
assumed: `measuredHz(samples, spanMs)` (`:34`) over a 2 s window
(`RATE_WINDOW_MS`, `:43`). This is what should be trusted over
`AppSettings.accelerometerRateHz`.

## Stage 2: AccelStats

Once a second, `MotionEngine.classify` (`:159`) folds the window into an
`AccelStats` via `accelStatsFromSamples` (`:170`), which is
`AccelStats::calculate` in `motion/src/movement.rs:220`. It needs at least 3
samples (`MotionEngine.kt:161`) or it passes a zeroed struct.

The **measured** rate is passed, not the configured one (`MotionEngine.kt:168`).
The native side divides by it twice, so a wrong rate corrupts two fields:

| Field | Computed as | Line |
|---|---|---|
| `variance` | population variance (`/n`) of `sqrt(x²+y²+z²)` | `movement.rs:232-239` |
| `mean_magnitude` | mean of the same series | `movement.rs:231` |
| `dominant_frequency` | `sample_rate_hz / best_autocorrelation_lag` | `movement.rs:327` |
| `step_count` | `round(dominant_frequency * window_seconds)` — derived from cadence, not counted | `movement.rs:329` |
| `window_duration_s` | `n / sample_rate_hz` | `movement.rs:249` |

Cadence comes from autocorrelation (`detect_steps`, `movement.rs:278`), not peak
counting, and has three ways to return nothing:

- fewer than `MIN_SAMPLES = 8` samples (`movement.rs:284`, `:294`);
- signal energy below `1e-4` after removing the mean (`:299`);
- best normalised correlation below `PERIODICITY_MIN = 0.4` (`:291`, `:323`).

The lag scan is bounded to `min_lag = max(round(rate/4), 2)` through
`max_lag = min(round(rate/0.5), n-1)` (`movement.rs:287-289`) — a **0.5-4.0 Hz**
cadence band. This is the mechanism by which a wrong declared rate erases a real
gait: declare 50 Hz over samples that arrived at 10 and a 2 Hz walk is searched
for at the wrong lags, so `dominant_frequency` and `step_count` both come back
zero and the phone reads as still.

`AccelStats` has **no sample-count field**. Nothing downstream knows how many
samples the window held.

## Stage 3: the vote

`MotionEngine.classify` builds a `MotionObservation` (`MotionEngine.kt:179`)
carrying the stats, a `LocationSample`, and the nearest-road context from
`PtilesRepository.nearbyRoadContext`, and calls
`AdaptiveMotionSession::observe` (`motion/src/sampling.rs:650`).

`observe` in order:

1. validates the location and pushes it into a smoothing classifier (`:652-662`);
2. picks `effective_speed`: the platform's `speed_mps` when finite and >= 0,
   otherwise the smoothed speed (`:663-665`);
3. **evidence gate** (`:674`): with neither a speed nor an accel window, the
   vote is `Unknown` at 0.0 confidence and the debouncer is *not* ticked, so
   wall-clock across a GPS gap cannot age its way into `Stationary`;
4. otherwise calls `classify_with_history` (`movement.rs:399`) and ticks the
   debouncer (`sampling.rs:696`).

### Thresholds, in the order they are tested

All in `motion/src/movement.rs`. Constants at `:334-346`:
`STATIONARY_CEILING_MPS = 0.5`, `WALKING_CEILING_MPS = 2.2`,
`DRIVING_FLOOR_MPS = 8.9`, `GPS_ACCURACY_GATE_M = 30.0`.
(`RUNNING_SPEED_HINT_MPS = 2.6` at `:359` is a labelling aid and is read by no
classifier — running is decided by cadence, never by speed.)

With usable GPS (`classify_with_history`):

| Test | Verdict | Line |
|---|---|---|
| accuracy > 30 m or non-finite, and speed > 8.9 | Driving 0.85 | `:423-428` |
| accuracy > 30 m otherwise | falls through to accel-only | `:423` |
| speed > 2.0 within 15 m of a road, bearing aligned (< 25 deg or > 155 deg mod 180) | Driving 0.95 | `:440-452` |
| same, perpendicular (70-110 deg) and speed > 5.0 | Driving 0.90 | `:440-452` |
| highway within 10 m, speed > 2.2 (vetoed past 5 m if walking cadence with `step_count > 4`) | Driving 0.95 | `:459-464` |
| footpath within 5 m, speed > 1.1 | Walking 0.90 | `:459-473` |
| vehicular way within 10 m, speed > 2.2 | Driving 0.85 | `:459-473` |
| more than 50 m from any way, speed 0.5-2.2 | Walking 0.90 | `:459-473` |
| was stably Driving, speed > 0.3, no gait, vehicle context | Driving 0.75 | `:493-499` |
| was stably Walking, accel silent, speed 0.5-2.2 | Walking 0.75 | `:508-515` |
| speed > 8.9 | Driving 0.90 | `:517-534` |
| speed > 5.0 with a reporting accel window and no gait | Driving 0.80 | `:517-534` |
| speed > 2.2 | Walking 0.85 | `:517-534` |

"Gait" is the shared predicate `dominant_frequency in 1.0..=3.0 && step_count > 3
&& variance > 0.01` (`:483-485`).

Accel-only, first match wins (`classify_accel_only`, `:540-557`) — `f` is
`dominant_frequency`, `v` is `variance`:

| Test | Verdict |
|---|---|
| `f > 2.5 && v > 0.3` | Running 0.50 |
| `f > 1.0 && v > 0.01` | Walking 0.60 |
| `step_count > 0 && v > 0.02` | Walking 0.40 |
| `f < 1.0 && v < 1.0` | Stationary 0.70 |
| `f < 1.0 && 1.0 <= v < 5.0` | Driving 0.40 |
| otherwise | Stationary 0.85 |

Note the last row: an all-zero `AccelStats` votes `Stationary` at 0.85 — a
confident claim made from no evidence. `has_signal()` (`movement.rs:213`) exists
to tell that apart from a genuinely still phone; the classifier does not use it.

## Stage 4: the debounce

`VoteDebouncer::tick_at` (`movement.rs:748`). Defaults from
`DebounceConfig::default` (`:597-609`):

- `majority_window: 5` — the vote must be the majority (`len/2 + 1`) of the last
  five (`:764-772`);
- `min_continuous: 3` — three consecutive agreeing majorities;
- `default_latency_ms: 60_000` — **and** 60 s of elapsed time;
- `rapid_latency_ms: 15_000` — 15 s instead, when the change is into Driving;
- `vehicle_sticky_ms: 150_000` — Driving will not fall to Stationary for 150 s;
- `signal_sticky_ms: 300_000` within `signal_radius_m: 25.0` of a mapped
  signal or stop (`:733-746`).

Both conditions are required (`:753`). At the 1 Hz classification rate that is
"three seconds of agreement, but not before a minute has passed" for most
transitions. `DebounceConfig::responsive()` (`:628-636`) drops that to 3 s and a
3-vote window; Looky uses `defaultAdaptiveMotionConfig`, i.e. the 60 s one.

**This debounce is the reason a movement label lags reality by up to a minute,
and it is deliberate.** `MotionEngine.reset` (`MotionEngine.kt:105`) replaces
the whole session rather than clearing the sample window, because the debounce
state is what makes "Driving" survive the end of a drive.

## Stage 5: what is written

`observe` returns `AdaptiveMotionUpdate` (`ffi/src/motion.rs:785`) — the
debounced `movement`, the raw `vote`, the smoothed speed, the traffic-control
flag, and the ignored `sampling` advice. `MotionEngine.classify` reduces it to
`MotionResult(movement, vote.confidence, stats)` (`MotionEngine.kt:194`).

`TraceService.record` passes that straight to `TraceRecorder.append`
(`TraceService.kt:297`), which opens a new `<trk>` whenever the debounced
movement name changes and writes the point with `<accel_variance>` in its
extensions (`TraceRecorder.kt:122`). The 1 Hz classification loop
(`TraceService.kt:237`) publishes to `TraceBus` for the UI and writes nothing.

## Where each number lives

| Number | Where |
|---|---|
| GPS interval, 3-60 s | `AppSettings.gpsIntervalSeconds` |
| Accelerometer rate, 10-100 Hz | `AppSettings.accelerometerRateHz` |
| Classification interval, 1 s | `TraceService.CLASSIFICATION_INTERVAL_MS:66` |
| Watchdog re-subscribe, 6x interval floored at 60 s | `TraceService.staleAfterMs:73` |
| Delivered-rate window, 2 s | `MotionEngine.RATE_WINDOW_MS:43` |
| Accel window cap, 300 samples | `MotionEngine.kt:224` |
| Cadence band, 0.5-4.0 Hz; min 8 samples; periodicity 0.4 | `motion/src/movement.rs:284-291` |
| Speed thresholds 0.5 / 2.2 / 8.9 m/s, accuracy gate 30 m | `motion/src/movement.rs:334-346` |
| Debounce 5-vote window, 3 continuous, 60 s / 15 s, 150 s sticky | `motion/src/movement.rs:597-609` |
| Confidence gate 0.60 (sampling advice only) | `motion/src/sampling.rs:190` |
| Staleness limits shown in the diagnostics sheet | `model/Models.kt` `motionStaleness` |
