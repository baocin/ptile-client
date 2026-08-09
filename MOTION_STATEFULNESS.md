# PTiles Motion: Stateless, Stateful, and Configurable Pieces

`ptiles-motion` contains several related mechanisms which answer different
questions. Some are pure calculations over one fix or one supplied batch;
others retain a timeline and cannot produce the same answer without the same
prior calls. Treating all of them as “the classifier” obscures where history,
configuration, latency, and reset behavior actually live.

## Short version

```text
raw accelerometer window
        |
        v
AccelStats::calculate()                         stateless
        |
        +--------------------+
                             v
fix + speed + map context -> classify() ------> Vote         stateless
                             or
                             classify_with_history()          pure, history is input
                                                        |
                                                        v
                                              VoteDebouncer   stateful
                                                        |
                                                        v
                                            stable MovementType

observations -> AdaptiveMotionSession -> movement + SamplingAdvice   stateful
                                            |
                                            v
                              host-owned sensor/location adapter

timestamped GPS fixes -> MotionClassifier -> smoothed speed   stateful
                              |              + speed-only state
                              +---- fallback speed for classify()

complete speed series -> significant_shifts() -> boundaries   stateless batch
```

For a rich live classifier, the intended composition is:

1. Obtain or derive a usable speed.
2. Calculate an accelerometer-window summary when samples are available.
3. Query PTiles for nearby road and traffic-control context.
4. Call `classify` or `classify_with_history` to produce one explainable
   `Vote`.
5. Feed the vote and a caller-supplied monotonic timestamp into
   `VoteDebouncer` to obtain the stable state.

`MotionClassifier` is both a useful speed smoother/fallback and a simpler
speed-only classifier. It is not a replacement for the richer
GPS + accelerometer + map-context decision tree.

## What “stateless” means here

A stateless function’s output depends only on its arguments and the library
version. It does not read a system clock, remember a previous call, mutate a
global, or perform I/O. Replaying the same arguments returns the same result.

A function may consume historical data and still be stateless. For example,
`classify_with_history` accepts the previous committed state explicitly, and
`significant_shifts` accepts a whole time series. Neither retains that history
after returning.

A stateful object retains information between calls. Replaying only the latest
call is insufficient; the caller must preserve the object or replay its full
input sequence.

## API inventory

| API | Category | Retained information | Configuration |
| --- | --- | --- | --- |
| `AccelStats::calculate` | Stateless window calculation | None | Step detector thresholds are fixed in code |
| `AccelStats::has_signal` | Stateless predicate | None | None |
| `RoadContext::from_nearest` | Stateless map-context conversion | None | None |
| `TrafficControl::from_nearest` / `holds_traffic` | Stateless conversion/predicate | None | Radius is an argument to `holds_traffic` |
| `classify` | Stateless one-fix decision tree | None | Decision thresholds are fixed constants/code |
| `classify_with_history` | Stateless, history-assisted | None; caller supplies bearing and previous stable state | Decision thresholds are fixed constants/code |
| `classify_accel_only` | Stateless one-window decision table | None | Thresholds are fixed in code |
| `MotionClassifier` | Stateful speed smoother and speed-band classifier | Last accepted fix, bounded speed window, current band, pending dwell | `MotionConfig`, Rust API only |
| `VoteDebouncer` | Stateful vote stabilizer | Vote window, committed state, pending transition/time, last driving vote | `DebounceConfig` |
| `significant_shifts` | Stateless batch analysis | None after return; consumes the supplied series | `ShiftConfig` |
| `t_two_sided_p` | Stateless statistical helper | None | Degrees of freedom is an argument |
| Wasm `MovementTracker` | Stateful composition | A `MotionClassifier`, `VoteDebouncer`, and last raw vote | Debounce config is configurable; speed config is fixed to defaults |
| UniFFI `VoteDebouncer` | Stateful Android/Swift/Python wrapper | Core debouncer behind a mutex | Full `DebounceConfig` at construction |
| `AdaptiveSampler` | Stateful sampling policy | Last evidence/state, current advice, burst/downshift timing, capabilities, intent, applied acknowledgement | `SamplingConfig` plus runtime capabilities/intent |
| `AdaptiveMotionSession` | Stateful complete pipeline | `MotionClassifier`, `VoteDebouncer`, `AdaptiveSampler`, last vote/evidence | `AdaptiveMotionConfig`; Rust, UniFFI, and Wasm |

## Stateless components

### `AccelStats::calculate`

Input:

- raw accelerometer `x`, `y`, and `z` axes in m/s²;
- sample rate in Hz.

Output:

- magnitude variance;
- mean magnitude;
- dominant periodic frequency;
- estimated step count;
- window duration.

It computes one window and remembers nothing. Axes of different lengths are
truncated to the shortest length. Empty input or a zero sample rate returns
`AccelStats::EMPTY`.

Cadence is detected by autocorrelation, not by a stateful step counter. The
detector detrends the magnitude series, looks for periodicity in the 0.5–4 Hz
band, and requires normalized correlation of at least 0.4. These detector
constants are currently fixed, not fields in a configuration record.

`AccelStats::has_signal` is also stateless. It distinguishes a window with some
variance/cadence/steps from the all-empty representation, but it cannot tell an
absent sensor from an exactly motionless, explicitly measured window by itself.
That distinction must remain in the surrounding `Option<AccelStats>`.

### `RoadContext` and `TrafficControl`

`RoadContext::from_nearest` converts a core nearest-road answer plus decoded
road geometry into:

- road class;
- distance to the road;
- local road bearing at the snapped segment.

It performs no lookup itself and stores no track history.

`TrafficControl::from_nearest` copies distance and intersection type from a
nearest-intersection answer. `holds_traffic(radius_m)` is a pure predicate:
signals, stops, and give-way controls count when they are within the supplied
radius; roundabouts and untyped junctions do not.

### `classify`

`classify` is the main stateless per-fix decision tree. It consumes optional:

- instantaneous speed;
- GPS horizontal accuracy;
- nearest-road context;
- accelerometer summary.

It returns one `Vote { movement, confidence }`. It never retains the vote and
never returns `Unknown`; `Unknown` belongs to stateful objects before they have
committed.

The decision order is significant:

1. Poor GPS position falls back to accelerometer evidence, except a finite
   speed above the driving floor can still prove vehicle movement.
2. Road type, distance, speed, and optional cadence provide strong priors.
3. Speed bands provide a fallback.
4. Accelerometer-only classification handles what remains.

Calling it independently for every fix is valid, but the output will be a noisy
vote stream rather than a stable trip state. Stability belongs to
`VoteDebouncer`.

### `classify_with_history`

This function is still stateless. It adds two explicit inputs:

- GPS travel bearing;
- previous committed `MovementType`.

Those inputs enable road-alignment evidence and a per-fix “still driving” prior,
but the function does not remember either one. The caller owns the history and
can reproduce any result from the recorded arguments.

Use it when the application already has a stable state from `VoteDebouncer` and
a bearing derived by the platform or trace logic. Use plain `classify` for a
true one-shot query or when those inputs are unavailable.

### `classify_accel_only`

This is a fixed, first-match decision table over one `AccelStats` value. It has
no GPS, map, clock, or retained state. It can return Stationary, Walking,
Running, or Driving votes, but confidence is deliberately modest for ambiguous
accelerometer-only cases.

### `significant_shifts`

`significant_shifts(samples, config)` is stateless batch analysis. It consumes
the complete supplied `(timestamp, speed)` series and returns statistically
supported boundaries.

It is not a movement classifier:

- it has no Stationary/Walking/Running/Driving vocabulary;
- timestamps are carried to results but statistics operate on adjacent sample
  windows, not elapsed-time windows;
- irregular sample cadence is accepted;
- non-finite speed samples are skipped within each window;
- Welch’s t-test handles unequal variance;
- Bonferroni correction accounts for testing many candidate indices;
- nearby detections are thinned to the strongest one.

Calling it twice with the same series and config returns the same shifts. It is
useful after recording, in a labeling tool, or as a second opinion on committed
classifier transitions.

## Stateful components

### `MotionClassifier`

`MotionClassifier` accepts `TimedFix` values and retains:

- the last accepted fix;
- a bounded window of effective speeds;
- the current speed band;
- a pending band and dwell count.

For each fix it:

1. Rejects fixes beyond its accuracy gate without advancing history.
2. Prefers a finite, non-negative platform speed.
3. Otherwise derives speed from distance and monotonic elapsed time.
4. Averages the bounded speed window.
5. Maps the mean to Stationary, Walking, or Driving.
6. Requires the configured number of agreeing samples before changing state.

It never produces Running because speed alone cannot reliably distinguish a
runner from a cyclist or slow vehicle. Running belongs to accelerometer-based
`classify` votes.

The first position-only fix cannot produce speed because there is no previous
point. A duplicate or backwards timestamp produces no derived speed. A gap
larger than `max_gap_ms` clears the speed window and pending transition; a valid
platform speed can seed the fresh window, while a speed derived across the gap
is discarded. The previously committed state survives this automatic gap
cleanup.

The composed stateful sessions also treat that speedless fix as **no vote**
when no accelerometer window is present. They report an Unknown vote at zero
confidence and hold the committed movement, rather than turning missing motion
evidence into Stationary and letting the wall-clock gap satisfy its debounce.

Public state operations:

- `state()` reads the current band.
- `smoothed_speed_mps()` reads the mean of the current speed window.
- `band_for(value)` applies this instance’s configured thresholds without
  mutating its history.
- `reset()` clears all history and returns the state to `Unknown`, while
  retaining the existing config.

The config cannot be replaced on an existing instance. Construct a new
instance to change it.

### `VoteDebouncer`

`VoteDebouncer` turns raw `Vote.movement` values into a stable committed state.
It retains:

- a bounded majority window;
- the current committed movement;
- a pending transition and consecutive-majority count;
- when that pending transition began;
- the time of the most recent raw Driving vote.

Important behavior:

- Vote confidence is not used. A `0.4` and `0.95` vote for the same movement
  have equal weight in the majority window.
- A strict majority of the current window is required. During startup, the
  partially filled window can already have a majority.
- A majority different from the current state must satisfy both
  `min_continuous` and the applicable time latency.
- Transitions into Driving use `rapid_latency_ms`; all others use
  `default_latency_ms`.
- Any raw Driving vote refreshes the vehicle-sticky timestamp.
- When the committed state is Driving, a Stationary majority is held during
  `vehicle_sticky_ms` so a traffic light does not become a false arrival.
- A nearby signal, stop, or give-way extends that hold to at least
  `signal_sticky_ms`.
- Walking or Running transitions are not blocked by the vehicle-sticky rule.
- `clear_vehicle_sticky()` lets stronger external evidence—such as being
  confidently inside a destination building—drop the driving hold.

`tick(vote, now_ms)` supplies no traffic-control context.
`tick_at(vote, now_ms, control)` can extend the sticky period using the map.
Both require a caller-supplied monotonic clock. The debouncer has no system
clock, making live and replayed behavior deterministic.

There is no core `reset()` or reconfigure method for `VoteDebouncer`. Create a
new object to begin a completely independent track or change its config.
`clear_vehicle_sticky()` clears only that guard; it does not clear the vote
window, current state, or pending transition.

### Wasm `MovementTracker`

`MovementTracker` is the older Wasm-only integrated pipeline. It remains for
compatibility alongside the cross-binding `AdaptiveMotionSession`. It owns:

- `MotionClassifier` for derived/smoothed fallback speed;
- `VoteDebouncer` for stable state;
- the last raw vote for its returned update.

On `push`, platform speed wins. When platform speed is absent, the tracker uses
the motion classifier’s smoothed/derived speed, calls the stateless `classify`,
and feeds that vote to `VoteDebouncer::tick_at`.

Current limitations are worth making explicit:

- only `DebounceConfig` is accepted by the constructor;
- `MotionConfig` is fixed to its defaults;
- it calls `classify`, not `classify_with_history`, so it does not use the new
  previous-stable or GPS-bearing inputs;
- there is no reset/reconfigure method—construct a new tracker;
- the stable movement and smoothed speed are readable, but internal windows are
  not serialized for process restoration.

### `AdaptiveSampler` and `AdaptiveMotionSession`

`AdaptiveSampler` is the hardware-neutral policy layer. It retains the current
advice, last evidence and movement, application intent, capability limits,
burst start, downshift hold, generation, and the most recent
`AppliedSampling` acknowledgement. It never starts a timer or calls a sensor.

`AdaptiveMotionSession` composes that policy with `MotionClassifier`,
`classify_with_history`, and `VoteDebouncer`. Each `observe` call returns:

- raw vote and committed movement;
- smoothed speed and traffic-control evidence;
- `SamplingAdvice`;
- `sampling_changed`, which means an adapter may need to reconfigure hardware.

`tick(now_ms)` reevaluates an advice deadline without fabricating a sensor
reading. The caller must schedule that tick from `reevaluate_after_ms`.
`set_capabilities` clamps or disables impossible requests immediately;
`set_intent` selects Background, Tracking, or Navigation policy;
`report_applied_sampling` records what the host actually configured. A reset
clears classification and policy history while retaining config, capabilities,
and intent.

The hook is a returned record rather than a Rust-to-host callback. UniFFI,
Wasm, native Rust, C-style wrappers, services, and desktop adapters can all
consume the same synchronous result, then expose a Kotlin `Flow`, Swift
delegate, JavaScript event, channel, listener, or IPC message locally. This
avoids imposing callback lifetime, reentrancy, and threading rules across every
FFI boundary.

## Configuration reference

### `MotionConfig` — stateful speed smoothing

Available directly in Rust and as part of `AdaptiveMotionConfig` through
UniFFI and Wasm. The older Wasm `MovementTracker` still fixes it to defaults.

| Field | Default | Effect |
| --- | ---: | --- |
| `stationary_max_mps` | `0.5` | Smoothed speed at or below this is Stationary |
| `driving_min_mps` | `5.0` | Smoothed speed at or above this is Driving |
| `smoothing_window` | `5` samples | Number of recent effective speeds averaged |
| `min_dwell_samples` | `2` | Consecutive target bands required before committing |
| `accuracy_gate_m` | `50.0` m | Worse/non-finite fixes are ignored completely |
| `max_gap_ms` | `30_000` ms | Larger gaps clear smoothing/pending history |

`smoothing_window` and `min_dwell_samples` are internally clamped to at least
one. Other fields are not comprehensively validated at construction; callers
should provide finite, ordered, non-negative thresholds.

### `DebounceConfig` — stable rich movement state

Available in Rust and fully represented through UniFFI. Wasm accepts a partial
object and fills omitted fields from defaults.

| Field | Default | Responsive preset | Effect |
| --- | ---: | ---: | --- |
| `majority_window` | `5` votes | `3` | Number of raw movement votes retained |
| `rapid_latency_ms` | `15_000` | `3_000` | Minimum elapsed time before entering Driving |
| `default_latency_ms` | `60_000` | `3_000` | Minimum elapsed time for other transitions |
| `vehicle_sticky_ms` | `150_000` | `90_000` | Driving→Stationary suppression after a Driving vote |
| `signal_sticky_ms` | `300_000` | `300_000` | Sticky period at a mapped holding control |
| `signal_radius_m` | `25.0` m | `25.0` m | How close that control must be |
| `min_continuous` | `3` | `3` | Consecutive agreeing majorities required |

`DebounceConfig::responsive()` is a Rust preset, not the default. It detects
many more real transitions in the mined-transition corpus, but also produces
more flapping on at least one real trail trace. It is appropriate when missed
arrivals cost more than spurious transitions; defaults favor stability.

The responsive factory is not currently exported through UniFFI or Wasm.
Android can construct the equivalent record explicitly, but exposing a named
preset would avoid duplicating these values.

`majority_window` and `min_continuous` are internally clamped to at least one.
Zero latency is valid for tests or immediate transitions. A signal sticky value
shorter than the normal vehicle sticky never shortens the hold—the larger value
is used. Other semantic validation is the caller’s responsibility.

### `ShiftConfig` — offline change-point analysis

Available in Rust and Wasm. It is not currently exposed through UniFFI.

| Field | Default | Effect |
| --- | ---: | --- |
| `window` | `12` samples per side | Sensitivity/localization tradeoff |
| `alpha` | `0.01` | Family-wise significance before Bonferroni correction |
| `min_separation` | `8` samples | Minimum spacing after strongest-hit thinning |
| `min_delta_mps` | `0.4` m/s | Minimum meaningful change in mean speed |

`window` is clamped to at least two and `min_separation` to at least one. The
remaining values are not rejected by a validating constructor; use a finite
`alpha` in `(0, 1)` and a finite non-negative effect-size threshold.

### `SamplingConfig` — adaptive collection policy

Available in Rust, UniFFI, and Wasm through `AdaptiveMotionConfig`. Defaults
request a 1-second location interval and 50 Hz accelerometer burst while
initializing or resolving a transition. Stable profiles request approximately:

| State | Location | Accelerometer |
| --- | --- | --- |
| Stationary | Passive, 60 s / 25 m | Passive wakeup, or 5 Hz when wakeup is unavailable |
| Walking | Balanced, 5 s / 5 m | Balanced, 20 Hz |
| Running | High, 2 s / 3 m | High, 25 Hz |
| Driving | High, 2 s / 8 m | Low, 10 Hz |

The default accelerometer window is 4 seconds. A transition burst is capped at
10 seconds, advice is reevaluated within 10 seconds, and a 15-second hold
prevents an immediate power downshift after an escalation. The confidence gate
is `0.60`. Every interval, rate, distance, duration, and gate is configurable.

`SamplingCapabilities` can disable location or acceleration, replace passive
location with low-rate active collection, enforce the host's minimum location
interval, and cap accelerometer frequency. Capability restrictions are marked
in the advice instead of silently promising unavailable data.

### Fixed decision-tree constants

These are exposed for inspection but are not runtime config:

| Constant | Value | Meaning |
| --- | ---: | --- |
| `STATIONARY_CEILING_MPS` | `0.5` m/s | At or below this, an established walk may transition to Stationary; above it, visible displacement preserves Walking when sensor/map context is absent |
| `WALKING_CEILING_MPS` | `2.2` m/s | Above this (and below the vehicle floor), the stateless speed fallback votes Walking outright; at or below it, context/acceleration normally decides |
| `DRIVING_FLOOR_MPS` | `8.9` m/s | A finite speed above this is decisive vehicle evidence |
| `GPS_ACCURACY_GATE_M` | `30.0` m | Worse position suppresses normal road/speed priors |
| `RUNNING_SPEED_HINT_MPS` | `2.6` m/s | UI/labeling hint only; never read by the classifier |

Several road, alignment, cadence, and accelerometer thresholds also remain
fixed inside the decision tree. Examples include road-distance gates, bearing
alignment ranges, the 1–3 Hz gait band, minimum step/variance evidence, and the
accelerometer-only table. Changing those currently requires a code change and
new corpus validation; `MotionConfig` does not affect them.

## Runtime and binding matrix

| Capability | Rust | Android/UniFFI | Wasm |
| --- | --- | --- | --- |
| Calculate `AccelStats` | Yes | `accel_stats_from_samples` | `accel_stats` |
| One-fix `classify` | Yes | `classify_movement` | Only inside `MovementTracker` |
| `classify_with_history` | Yes | `classify_movement_with_history` | No |
| `classify_accel_only` | Yes | `classify_movement_accel_only` | Only through tracker/classifier flow |
| `MotionClassifier` | Public | Not exposed | Internal to `MovementTracker` |
| Custom `MotionConfig` | Yes | Through adaptive session | Through adaptive session |
| `VoteDebouncer` | Public | Public opaque object | Internal to `MovementTracker` |
| Custom `DebounceConfig` | Yes | Yes | Yes, partial object accepted |
| Responsive preset by name | Yes | No | No |
| `significant_shifts` | Yes | No | Yes |
| Custom `ShiftConfig` | Yes | No | Yes |
| Adaptive sampling advice | Yes | Yes | Yes |
| Capability/applied-policy feedback | Yes | Yes | Yes |
| Background/tracking/navigation intent | Yes | Yes | Yes |
| Full integrated adaptive tracker | `AdaptiveMotionSession` | `AdaptiveMotionSession` | `AdaptiveMotionSession` |
| Legacy integrated tracker | Compose manually | Compose manually | `MovementTracker` |
| Reset adaptive session | Yes | Yes | Yes |

## Recommended Android composition

Android can continue composing the lower-level functions, but new integrations
should keep one UniFFI `AdaptiveMotionSession` per active capture session:

```text
location/sensor callback
    -> AccelStats (when a sensor window exists)
    -> PTiles nearest road + nearest traffic control
    -> adaptiveSession.observe(MotionObservation(...))
    -> AdaptiveMotionUpdate
    -> if samplingChanged, translate SamplingAdvice into Android APIs
    -> schedule tick from reevaluateAfterMs
    -> reportAppliedSampling(actual configuration)
```

Operational rules:

- Keep the `AdaptiveMotionSession` instance for the lifetime of one capture
  session.
- Use a monotonic elapsed-realtime clock, not wall-clock epoch time.
- Call `reset` when starting an unrelated track; recreate the session to change
  construction-time classifier tuning.
- Record both the raw `Vote` and committed state; they answer different
  questions and make later debugging possible.
- Pass absent speed, accuracy, accelerometer, or map context as absent—not as a
  zero measurement.
- Use the indoor/outdoor estimate as additional application evidence, not as a
  replacement motion class. A confident arrival inside a building may justify
  `clear_vehicle_sticky()` before accepting Stationary.
- Treat advice as a request. Android permissions, lifecycle, battery saver, and
  OS throttling remain authoritative; report what was actually applied.

## Configuration lifecycle and persistence

All config records are copied into their stateful object at construction. They
are not live references, and changing the original record later has no effect.

No stateful object exposes a serializable snapshot of its internal
history. For Android process death, there are three options:

1. Recreate state and accept a short warm-up period.
2. Replay a bounded tail of recorded fixes/votes using their monotonic-relative
   timing.
3. Add explicit, versioned state snapshot/restore APIs.

Option 1 is simplest. Option 2 is deterministic but requires careful timestamp
rebasing after reboot. Option 3 is appropriate only when continuity across
process death materially improves the product; internal state then becomes a
versioned persistence contract.

## Common mistakes

- Calling raw `classify` output the stable movement state.
- Running both `MotionClassifier` and `VoteDebouncer` as independent final
  classifiers and expecting them to agree; one is speed-only, the other
  stabilizes rich votes.
- Assuming `classify_with_history` stores history because of its name.
- Treating `Vote.confidence` as a debouncer weight; it currently is not used.
- Passing wall-clock time which can jump backwards into a monotonic state
  machine.
- Reusing one debouncer across unrelated trips.
- Treating absent sensor fields as zero-valued measurements.
- Assuming `RUNNING_SPEED_HINT_MPS` can make the classifier return Running.
- Tuning `MotionConfig` and expecting the stateless road/cadence decision tree
  to change.
- Changing debounce latency without measuring both missed transitions and
  flapping on representative traces.
- Using `significant_shifts` as segment labels; it finds boundaries, not the
  movement types between them.
- Treating `SamplingAdvice` as a guarantee rather than a capability-clamped
  request that the host may further restrict.
- Waiting for a callback from Rust: the portable hook is the returned update;
  each adapter emits its own native event when `sampling_changed` is true.

## Useful future API additions

- Export `DebounceConfig::responsive()` by name through UniFFI and Wasm.
- Add adapter examples that map advice to Android, Apple, browser, and desktop
  sensor services without putting those dependencies in `ptiles-motion`.
- Feed route deviation, indoor/outdoor state, and arrival evidence into the
  sampling policy as optional generic signals after their behavior is measured.
- Expose `significant_shifts` and `ShiftConfig` through UniFFI for on-device
  trace diagnostics.
- Add optional confidence-weighted debouncing only after corpus evidence shows
  it improves results; changing vote weights alters transition semantics.
- Add validated config constructors which reject non-finite, negative, or
  internally inconsistent values.
- Add versioned state snapshot/restore only if Android process-continuity tests
  justify the persistence contract.
