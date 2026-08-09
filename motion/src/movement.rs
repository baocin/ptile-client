//! Per-fix movement classification, ported from the Rookery Android client's
//! `com.rookery.rook.movement` package (itself a port of MDT's
//! `timeline-core/src/machine/capture/movement.rs`).
//!
//! Three pieces, all framework-free and `no_std`:
//! - [`AccelStats`] — magnitude variance + step cadence over an accelerometer
//!   window (the signal used when GPS is useless).
//! - [`classify`] — stateless decision tree over one fix:
//!   GPS-accuracy gate -> road-context priors -> speed-only -> accel-only.
//! - [`VoteDebouncer`] — turns the noisy per-fix vote stream into a stable
//!   [`MovementType`] with CHRE-style latencies and a vehicle-sticky guard.
//!
//! Differences from the Kotlin original:
//! - `RoadContext` is live here, not dormant: the browser/FFI callers have
//!   ptiles road tiles, so [`RoadContext::from_nearest`] converts a
//!   `nearest_road` hit straight into the prior. Its `snappedLat/snappedLon`
//!   fields are dropped — `classify` never read them, and callers that want
//!   the snap already have it from `nearest_road`.
//! - Still omitted (same as Kotlin): the gridlock stationary-fraction
//!   override and the trailing 5-minute motion features. Both need a GPS
//!   trailing window nobody collects yet.

use alloc::collections::VecDeque;
use alloc::string::String;

/// `f64::abs` is core, but naming it once keeps the no_std intent obvious.
#[inline]
fn libm_fabs(x: f64) -> f64 {
    if x < 0.0 { -x } else { x }
}

use ptiles_core::haversine_distance_m;
use ptiles_core::math::{atan2, cos, sin, sqrt};
use ptiles_core::{NearestIntersection, NearestRoad, RoadSegment};

/// Coarse movement state. `Unknown` is the initial state only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
pub enum MovementType {
    Unknown,
    Stationary,
    Walking,
    Running,
    Driving,
}

impl MovementType {
    /// Lowercase wire name, matching the serde representation.
    pub fn as_str(self) -> &'static str {
        match self {
            MovementType::Unknown => "unknown",
            MovementType::Stationary => "stationary",
            MovementType::Walking => "walking",
            MovementType::Running => "running",
            MovementType::Driving => "driving",
        }
    }
}

/// One classifier output: a type plus how much the evidence is worth.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Vote {
    pub movement: MovementType,
    pub confidence: f64,
}

/// Nearest-road prior for a fix: what kind of way it is and how far off it we
/// are. This is the map half of the classifier — it's what separates "stopped
/// at a light in a traffic lane" from "standing on the sidewalk".
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RoadContext {
    /// OSM `highway` tag: "motorway", "footway", "residential", ...
    pub road_class: String,
    /// Fix to nearest road, meters.
    pub distance_m: f64,
    /// Bearing of the road at the snapped point, degrees. `None` when the
    /// caller cannot compute it -- which is different from a road running due
    /// north, so it is not defaulted to zero.
    pub bearing: Option<f64>,
}

/// Heading of the polyline at the point nearest `snapped`, in degrees.
///
/// Uses the segment whose endpoint is closest to the snap rather than the whole
/// way's end-to-end heading: a road that curves through 90 degrees has no single
/// bearing, and the one that matters is where the fix actually is.
fn bearing_at(coords: &[[f64; 2]], snapped: (f64, f64)) -> Option<f64> {
    if coords.len() < 2 {
        return None;
    }
    let (slat, slon) = snapped;
    let mut best = 0usize;
    let mut best_d = f64::MAX;
    for (i, c) in coords.iter().enumerate() {
        let d = haversine_distance_m(slat, slon, c[1], c[0]);
        if d < best_d {
            best_d = d;
            best = i;
        }
    }
    let (a, b) = if best + 1 < coords.len() {
        (coords[best], coords[best + 1])
    } else {
        (coords[best - 1], coords[best])
    };
    let (lat1, lon1) = (a[1].to_radians(), a[0].to_radians());
    let (lat2, lon2) = (b[1].to_radians(), b[0].to_radians());
    let dlon = lon2 - lon1;
    let y = sin(dlon) * cos(lat2);
    let x = cos(lat1) * sin(lat2) - sin(lat1) * cos(lat2) * cos(dlon);
    let deg = atan2(y, x).to_degrees();
    Some((deg + 360.0) % 360.0)
}

impl RoadContext {
    /// Build the prior from a `ptiles_core::nearest_road` hit and the roads
    /// slice it indexes into. Returns `None` if the index is out of range.
    pub fn from_nearest(roads: &[RoadSegment], near: &NearestRoad) -> Option<RoadContext> {
        let road = roads.get(near.road_index)?;
        Some(RoadContext {
            road_class: road.road_class.clone(),
            distance_m: near.distance_m,
            // The snapped segment's heading. `nearest_road` reports which
            // segment was hit but not its direction, so this is derived from
            // the two vertices bracketing the snap.
            bearing: bearing_at(&road.coords, near.snapped),
        })
    }
}

/// Nearest mapped traffic control to a fix: a signal, stop, give-way or
/// roundabout node from the roads layer's intersection table. Deserializes
/// straight from a `nearest_intersection` result.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TrafficControl {
    /// Fix to the intersection node, meters.
    pub distance_m: f64,
    /// 1 = traffic_signals, 2 = stop, 3 = give_way, 4 = roundabout
    /// (0/other = untyped junction).
    pub intersection_type: u8,
}

impl TrafficControl {
    /// From a `ptiles_core::nearest_intersection` hit.
    pub fn from_nearest(near: &NearestIntersection) -> TrafficControl {
        TrafficControl {
            distance_m: near.distance_m,
            intersection_type: near.intersection_type,
        }
    }

    /// Whether this is the kind of node a vehicle *waits* at, within
    /// `radius_m`. Signals, stops and give-ways queue traffic; a roundabout
    /// (4) and an untyped junction (0) do not hold you for minutes, so they
    /// get no extension.
    pub fn holds_traffic(&self, radius_m: f64) -> bool {
        matches!(self.intersection_type, 1 | 2 | 3)
            && self.distance_m.is_finite()
            && self.distance_m <= radius_m
    }
}

/// Accelerometer window summary. Feeds the accel-only fallback.
///
/// `mean_magnitude` and `window_duration_s` are `Option` because real producers
/// omit them: the Rookery Android GPX exporter sends variance, cadence and step
/// count only (see `label-gpx/SCHEMA.md` and `ANDROID_INTEGRATION.md`). Filling
/// them with `0.0` would make a genuine three-field reading indistinguishable
/// from [`AccelStats::EMPTY`], i.e. from having no accelerometer at all -- so
/// the absence is in the type, and any future rule that wants mean magnitude has
/// to decide what to do when it is missing instead of silently reading a zero.
///
/// The other three are plain numbers on purpose: every producer sends them, and
/// `0` is a meaningful *reading* for each (a still phone, no cadence, no steps).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct AccelStats {
    /// Variance of the magnitude series, (m/s^2)^2.
    pub variance: f64,
    /// Mean magnitude, m/s^2. `None` when the producer does not report it.
    pub mean_magnitude: Option<f64>,
    /// Step cadence, Hz.
    pub dominant_frequency: f64,
    pub step_count: u32,
    /// Window length, seconds. `None` when the producer does not report it.
    pub window_duration_s: Option<f64>,
}

impl AccelStats {
    /// No accelerometer at all: no variance, no cadence, no steps, and nothing
    /// reported for the two optional fields either.
    pub const EMPTY: AccelStats = AccelStats {
        variance: 0.0,
        mean_magnitude: None,
        dominant_frequency: 0.0,
        step_count: 0,
        window_duration_s: None,
    };

    /// Whether this window carries any accelerometer signal at all.
    ///
    /// `EMPTY` is what a caller with no sensor passes, and it votes `Stationary`
    /// through the accel table -- which is a claim about the world made from no
    /// evidence. Callers that care about the difference (a UI that would rather
    /// say "unknown", a fixture builder deciding whether a field is worth
    /// writing) ask this instead of comparing against `EMPTY`, which would also
    /// be false for a phone that is genuinely, exactly still.
    pub fn has_signal(&self) -> bool {
        self.variance > 0.0 || self.dominant_frequency > 0.0 || self.step_count > 0
    }

    /// magnitude = sqrt(x^2+y^2+z^2) per sample; mean + variance of that
    /// series; cadence from peak detection. Extra samples in the longer axes
    /// are ignored (the window is `min(len)`).
    pub fn calculate(x: &[f32], y: &[f32], z: &[f32], sample_rate_hz: u32) -> AccelStats {
        let n = x.len().min(y.len()).min(z.len());
        if n == 0 || sample_rate_hz == 0 {
            return AccelStats::EMPTY;
        }
        let mut magnitudes = VecDeque::with_capacity(n);
        for i in 0..n {
            let (xi, yi, zi) = (x[i] as f64, y[i] as f64, z[i] as f64);
            magnitudes.push_back(sqrt(xi * xi + yi * yi + zi * zi));
        }

        let mean = magnitudes.iter().sum::<f64>() / n as f64;
        let variance = magnitudes
            .iter()
            .map(|m| {
                let d = m - mean;
                d * d
            })
            .sum::<f64>()
            / n as f64;

        let (step_count, dominant_frequency) =
            detect_steps(&magnitudes, mean, variance, sample_rate_hz);
        AccelStats {
            variance,
            // Computed here, so reported -- unlike a wire format that omits them.
            mean_magnitude: Some(mean),
            dominant_frequency,
            step_count,
            window_duration_s: Some(n as f64 / sample_rate_hz as f64),
        }
    }
}

// ponytail: simplified step detector — prominent local maxima above
// (mean + 0.5*std) with a refractory gap, instead of the FIR-lowpass +
// autocorrelation of MDT's step_detection.rs. Cadence = peaks / seconds. Good
// enough for a fallback (GPS speed is the primary signal); upgrade to
// autocorrelation only if accel-only misclassification is actually observed.
/// Step cadence by AUTOCORRELATION, not peak counting.
///
/// Peak counting thresholds the magnitude series and counts crossings with a
/// refractory gap. At rest that is not a cadence detector, it is a jitter
/// detector: random resting noise produces maxima above `mean + 0.5*sd` often
/// enough that roughly a third of stationary windows reported a walking
/// cadence, and the accel-only table reads a cadence as Walking. A phone on a
/// desk was Walking.
///
/// Autocorrelation asks a different question -- is this signal PERIODIC -- and
/// noise is not. Detrend by the mean to drop the ~9.8 m/s^2 gravity DC term,
/// then take the strongest normalised peak in the 0.5-4 Hz stride band. A real
/// gait peaks sharply (r ~ 0.4-0.9) at its stride period; jitter has no peak at
/// all (r ~ 0), so it returns 0 Hz and the table reads Stationary, which is the
/// truth.
///
/// Biased normalisation and integer-lag resolution, deliberately: no parabolic
/// interpolation, no FIR band-pass. Good to about +/-0.2 Hz over a 4 s window,
/// which is far inside the gaps between the walk/run/stationary thresholds.
fn detect_steps(
    magnitudes: &VecDeque<f64>,
    mean: f64,
    _variance: f64,
    sample_rate_hz: u32,
) -> (u32, f64) {
    const MIN_SAMPLES: usize = 8;
    const ENERGY_EPSILON: f64 = 1e-4;
    /// Slowest stride treated as periodic.
    const MIN_CADENCE_HZ: f64 = 0.5;
    /// Fastest plausible step cadence.
    const MAX_CADENCE_HZ: f64 = 4.0;
    /// Minimum normalised autocorrelation before this counts as a cadence.
    const PERIODICITY_MIN: f64 = 0.4;

    let n = magnitudes.len();
    if n < MIN_SAMPLES {
        return (0, 0.0);
    }
    let detrended: alloc::vec::Vec<f64> = magnitudes.iter().map(|m| m - mean).collect();
    let energy: f64 = detrended.iter().map(|v| v * v).sum();
    if energy < ENERGY_EPSILON {
        return (0, 0.0); // flat: no motion at all
    }

    let rate = sample_rate_hz as f64;
    let min_lag = ((rate / MAX_CADENCE_HZ).round() as usize).max(2);
    let max_lag = ((rate / MIN_CADENCE_HZ).round() as usize).min(n - 1);
    if max_lag <= min_lag {
        return (0, 0.0);
    }

    let mut best_lag = 0usize;
    let mut best_r = 0.0f64;
    for lag in min_lag..=max_lag {
        let mut acc = 0.0;
        for i in 0..(n - lag) {
            acc += detrended[i] * detrended[i + lag];
        }
        let r = acc / energy; // normalised, r(0) = 1
        if r > best_r {
            best_r = r;
            best_lag = lag;
        }
    }
    if best_lag == 0 || best_r < PERIODICITY_MIN {
        return (0, 0.0); // periodic enough to be a gait? no.
    }

    let frequency = rate / best_lag as f64;
    let window_seconds = n as f64 / rate;
    let steps = (frequency * window_seconds).round().max(0.0) as u32;
    (steps, frequency)
}

/// Walking/driving speed split, m/s (~5 mph).
pub const WALKING_CEILING_MPS: f64 = 2.2;
/// Definitely-a-vehicle speed, m/s (~20 mph).
pub const DRIVING_FLOOR_MPS: f64 = 8.9;
/// Above this horizontal accuracy (m) GPS is not trusted at all.
pub const GPS_ACCURACY_GATE_M: f64 = 30.0;

/// Where a human would put the walking/running line on a speed axis, m/s
/// (~5.8 mph).
///
/// **This is a labelling aid, not a classifier threshold.** Nothing in
/// [`classify`] or [`classify_accel_only`] reads it, and it never will:
/// [`MovementType::Running`] is inferred from accelerometer cadence, because
/// speed alone cannot tell a runner from a slow cyclist or a car in a car park.
/// It exists so a tool that asks a *person* to mark up a speed chart has one
/// documented number to draw the line at, instead of each such tool inventing
/// its own. Treat it as a default a UI may expose for tuning, and never as
/// evidence about what a trace was doing.
pub const RUNNING_SPEED_HINT_MPS: f64 = 2.6;

/// Stateless single-fix classification. Order: GPS-accuracy gate (bad fix =>
/// accel only) -> road-context priors -> speed-only bands -> accel-only.
///
/// Every input is optional because every one of them is genuinely missing on
/// some real fix: `inst_speed_mps` and `gps_accuracy_m` when the platform does
/// not report them, `nearest_road` when no tile answer is available, `accel`
/// when there is no accelerometer window for this fix. `None` accel and
/// [`AccelStats::EMPTY`] classify identically today -- both fall to the table's
/// catch-all -- but they are different facts, and only one of them can be
/// mistaken for a measurement.
pub fn classify(
    inst_speed_mps: Option<f64>,
    gps_accuracy_m: Option<f64>,
    nearest_road: Option<&RoadContext>,
    accel: Option<&AccelStats>,
) -> Vote {
    classify_with_history(
        inst_speed_mps,
        gps_accuracy_m,
        nearest_road,
        accel,
        None,
        MovementType::Unknown,
    )
}

/// [`classify`] plus the two inputs a caller can only supply if it is tracking
/// a sequence: which way the fix is travelling, and what the last committed
/// state was.
///
/// Split from `classify` rather than added to it so a one-shot caller -- a GPX
/// replay, a single-point query -- keeps a four-argument call and gets exactly
/// the old behaviour: `None` bearing makes the alignment test inert, and an
/// `Unknown` previous state makes the driving-sticky inert.
///
/// Both branches exist because distance to a road cannot separate a car from a
/// pedestrian on the pavement, and a single sample cannot tell a car at a red
/// light from a parked one. Direction and history can.
pub fn classify_with_history(
    inst_speed_mps: Option<f64>,
    gps_accuracy_m: Option<f64>,
    nearest_road: Option<&RoadContext>,
    accel: Option<&AccelStats>,
    gps_bearing: Option<f64>,
    previous_stable: MovementType,
) -> Vote {
    let accel = accel.unwrap_or(&AccelStats::EMPTY);
    // Poor GPS: the POSITION is uncertain. That is not a reason to throw away a
    // speed that no pedestrian can produce.
    //
    // Falling straight to the accelerometer here misreads vehicles badly. In a
    // tunnel or an urban canyon accuracy degrades while the vehicle keeps
    // moving, and the accelerometer hears engine and road vibration as a 1-3 Hz
    // cadence with a plausible step count -- which is exactly the walking row of
    // `classify_accel_only`. One recording produced 81 such rows, reporting
    // Walking at up to 56 mph with accuracy drifting from 31 m to 314 m.
    //
    // So an uncertain fix still gets to veto everything EXCEPT a speed clearing
    // the driving floor. The bar is deliberately the floor rather than mere
    // existence: a bad fix can manufacture a small bogus speed, and this is not
    // a licence to trust it generally. Confidence is lower than the same call on
    // a good fix (0.90) because the reading it rests on is less certain.
    if gps_accuracy_m.is_some_and(|a| !a.is_finite() || a > GPS_ACCURACY_GATE_M) {
        if inst_speed_mps.is_some_and(|s| s.is_finite() && s > DRIVING_FLOOR_MPS) {
            return Vote { movement: MovementType::Driving, confidence: 0.85 };
        }
        return classify_accel_only(accel);
    }

    // Travelling along a road, rather than merely beside one.
    //
    // Distance alone cannot separate a car from a pedestrian on the pavement --
    // both are within a few metres of the centreline. Direction can: a fix
    // moving parallel to a vehicular way at speed is in a vehicle, and one
    // crossing it perpendicular at speed is too (a car on an intersecting
    // street), while a walker's heading wanders relative to the road.
    if let (Some(road), Some(bearing), Some(speed), Some(gps_bearing)) =
        (nearest_road, nearest_road.and_then(|r| r.bearing), inst_speed_mps, gps_bearing)
    {
        if speed > 2.0 && road.distance_m < 15.0 {
            // Modulo 180: a road has an axis, not a direction. Travelling
            // "backwards" along it is the same alignment.
            let diff = libm_fabs(road.bearing.unwrap_or(bearing) - gps_bearing) % 180.0;
            let aligned = diff < 25.0 || diff > 155.0;
            let perpendicular = (70.0..=110.0).contains(&diff);
            let cls = road.road_class.as_str();
            if aligned && (is_highway(cls) || is_vehicular(cls)) {
                return Vote { movement: MovementType::Driving, confidence: 0.95 };
            }
            if perpendicular && speed > 5.0 && (is_highway(cls) || is_vehicular(cls)) {
                return Vote { movement: MovementType::Driving, confidence: 0.90 };
            }
        }
    }

    if let (Some(road), Some(speed)) = (nearest_road, inst_speed_mps) {
        let d = road.distance_m;
        let cls = road.road_class.as_str();
        if is_highway(cls) && d < 10.0 && speed > 2.2 {
            // Counter-signal: a bit off the road AND a walking cadence means
            // the snap was wrong — fall through to speed-only.
            let walking_cadence = (1.0..=3.0).contains(&accel.dominant_frequency)
                && accel.step_count > 4;
            if !(d > 5.0 && walking_cadence) {
                return Vote { movement: MovementType::Driving, confidence: 0.95 };
            }
        } else if is_footpath(cls) && d < 5.0 && speed > 1.1 {
            return Vote { movement: MovementType::Walking, confidence: 0.90 };
        } else if is_vehicular(cls) && d < 10.0 && speed > 2.2 {
            return Vote { movement: MovementType::Driving, confidence: 0.85 };
        } else if d > 50.0 && (0.5..=2.2).contains(&speed) {
            return Vote { movement: MovementType::Walking, confidence: 0.90 };
        }
    }

    // Still moving, just off Driving, and no walking cadence: a drive-thru, a
    // car park, a red light. Held as Driving because the alternative -- flipping
    // to Stationary at every stop -- turns one journey into a string of false
    // arrivals. A genuine walking cadence breaks it immediately rather than
    // waiting the speed out, which is what makes driving->walking responsive.
    if previous_stable == MovementType::Driving {
        let step_cadence = (1.0..=3.0).contains(&accel.dominant_frequency)
            && accel.step_count > 3
            && accel.variance > 0.01;
        if inst_speed_mps.is_some_and(|s| s > 0.3) && !step_cadence {
            return Vote { movement: MovementType::Driving, confidence: 0.75 };
        }
    }

    if let Some(speed) = inst_speed_mps {
        if speed > DRIVING_FLOOR_MPS {
            return Vote { movement: MovementType::Driving, confidence: 0.90 };
        }
        if speed > WALKING_CEILING_MPS {
            return Vote { movement: MovementType::Walking, confidence: 0.85 };
        }
    }

    classify_accel_only(accel)
}

/// Accel-only table — first match wins, top to bottom.
pub fn classify_accel_only(s: &AccelStats) -> Vote {
    let f = s.dominant_frequency;
    let v = s.variance;
    let (movement, confidence) = if f > 2.5 && v > 0.3 {
        (MovementType::Running, 0.50)
    } else if f > 1.0 && v > 0.01 {
        (MovementType::Walking, 0.60)
    } else if s.step_count > 0 && v > 0.02 {
        (MovementType::Walking, 0.40)
    } else if f < 1.0 && v < 1.0 {
        (MovementType::Stationary, 0.70)
    } else if f < 1.0 && (1.0..5.0).contains(&v) {
        (MovementType::Driving, 0.40)
    } else {
        (MovementType::Stationary, 0.85)
    };
    Vote { movement, confidence }
}

fn is_highway(c: &str) -> bool {
    c == "motorway" || c == "trunk" || c.ends_with("_link")
}

fn is_footpath(c: &str) -> bool {
    c == "footway" || c == "path" || c == "pedestrian" || c == "steps"
}

fn is_vehicular(c: &str) -> bool {
    c == "residential" || c == "unclassified" || c == "service"
}

/// Tunables for [`VoteDebouncer`]. Defaults are the reverse-engineered Google
/// CHRE activity-recognition parameters the Kotlin original shipped with.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct DebounceConfig {
    /// Votes kept in the majority window.
    pub majority_window: usize,
    /// Latency into `Driving`, ms.
    pub rapid_latency_ms: u64,
    /// Latency for every other transition, ms.
    pub default_latency_ms: u64,
    /// After a `Driving` vote, suppress a flip to `Stationary` for this long
    /// (ms) — a red light is not an arrival.
    pub vehicle_sticky_ms: u64,
    /// Sticky window (ms) used instead of `vehicle_sticky_ms` while the fix
    /// sits at a mapped traffic control. A long light plus a queue can hold a
    /// car well past 150 s, and the map says so — 5 min by default.
    pub signal_sticky_ms: u64,
    /// How close (m) a traffic control has to be to count as "waiting at it".
    /// Roughly one intersection's worth of queue.
    pub signal_radius_m: f64,
    /// Consecutive agreeing majorities required before a transition commits.
    pub min_continuous: u32,
}

impl Default for DebounceConfig {
    fn default() -> Self {
        DebounceConfig {
            majority_window: 5,
            rapid_latency_ms: 15_000,
            default_latency_ms: 60_000,
            vehicle_sticky_ms: 150_000,
            signal_sticky_ms: 300_000,
            signal_radius_m: 25.0,
            min_continuous: 3,
        }
    }
}

/// Stabilizes a [`Vote`] stream into [`MovementType`] transitions: a majority
/// window, per-direction latency, a minimum run of agreeing votes, and the
/// vehicle-sticky guard.
#[derive(Clone, Debug)]
pub struct VoteDebouncer {
    cfg: DebounceConfig,
    window: VecDeque<MovementType>,
    current: MovementType,
    pending: Option<(MovementType, u32)>,
    pending_since_ms: u64,
    last_driving_vote_ms: Option<u64>,
}

impl VoteDebouncer {
    pub fn new(cfg: DebounceConfig) -> Self {
        VoteDebouncer {
            cfg,
            window: VecDeque::new(),
            current: MovementType::Unknown,
            pending: None,
            pending_since_ms: 0,
            last_driving_vote_ms: None,
        }
    }

    pub fn current(&self) -> MovementType {
        self.current
    }

    pub fn config(&self) -> DebounceConfig {
        self.cfg
    }

    /// Feed one vote with no map context; returns the debounced stable type.
    /// `now_ms` is a monotonic clock.
    pub fn tick(&mut self, vote: &Vote, now_ms: u64) -> MovementType {
        self.tick_at(vote, now_ms, None)
    }

    /// Feed one vote plus the nearest mapped traffic control to the fix.
    ///
    /// The control only ever *extends* the vehicle-sticky window
    /// (`signal_sticky_ms` instead of `vehicle_sticky_ms`), and only while the
    /// fix is still at it — which is the whole point: a car idling at a signal
    /// looks identical to a parked car, and only the map can tell them apart.
    /// It never suppresses a transition the plain [`tick`] would have allowed.
    ///
    /// [`tick`]: VoteDebouncer::tick
    /// Drop the vehicle-sticky guard so the next Stationary majority commits
    /// without waiting `vehicle_sticky_ms` out.
    ///
    /// For a caller holding evidence the sticky no longer applies -- the fix is
    /// inside a known place, say. A red light is not inside your house, so
    /// waiting 90 s there buys nothing and delays the arrival that matters.
    pub fn clear_vehicle_sticky(&mut self) {
        self.last_driving_vote_ms = None;
    }

    pub fn tick_at(
        &mut self,
        vote: &Vote,
        now_ms: u64,
        control: Option<&TrafficControl>,
    ) -> MovementType {
        self.window.push_back(vote.movement);
        while self.window.len() > self.cfg.majority_window.max(1) {
            self.window.pop_front();
        }
        if vote.movement == MovementType::Driving {
            self.last_driving_vote_ms = Some(now_ms);
        }

        let Some(majority) = self.majority() else {
            return self.current;
        };

        if majority == self.current {
            // Settled: drop any half-formed transition.
            self.pending = None;
            return self.current;
        }

        // Accumulate the pending transition (whether or not sticky suppresses it).
        let count = match self.pending {
            Some((t, n)) if t == majority => n + 1,
            _ => {
                self.pending_since_ms = now_ms;
                1
            }
        };
        self.pending = Some((majority, count));

        // Vehicle sticky: fresh off Driving, ignore a flip to Stationary. At a
        // signal/stop/give-way the window is the longer signal one.
        let at_control = control.is_some_and(|c| c.holds_traffic(self.cfg.signal_radius_m));
        let sticky_ms = if at_control {
            self.cfg.signal_sticky_ms.max(self.cfg.vehicle_sticky_ms)
        } else {
            self.cfg.vehicle_sticky_ms
        };
        let sticky = self.current == MovementType::Driving
            && majority == MovementType::Stationary
            && self
                .last_driving_vote_ms
                .is_some_and(|t| now_ms.saturating_sub(t) < sticky_ms);
        if sticky {
            return self.current;
        }

        let latency = if majority == MovementType::Driving {
            self.cfg.rapid_latency_ms
        } else {
            self.cfg.default_latency_ms
        };
        if count >= self.cfg.min_continuous.max(1)
            && now_ms.saturating_sub(self.pending_since_ms) >= latency
        {
            self.current = majority;
            self.pending = None;
        }
        self.current
    }

    /// Majority type in the window, or `None` when no type holds
    /// `len/2 + 1` votes.
    fn majority(&self) -> Option<MovementType> {
        if self.window.is_empty() {
            return None;
        }
        let threshold = self.window.len() / 2 + 1;
        self.window.iter().copied().find(|candidate| {
            self.window.iter().filter(|t| *t == candidate).count() >= threshold
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec::Vec;

    fn road(class: &str, distance_m: f64) -> RoadContext {
        RoadContext { road_class: class.to_string(), distance_m, bearing: None }
    }

    /// Synthetic accel window: constant `dc` magnitude plus a sine of
    /// `step_hz` and amplitude `amp`, sampled at `rate` Hz for `secs`.
    fn accel_window(step_hz: f64, amp: f64, dc: f64, rate: u32, secs: f64) -> AccelStats {
        let n = (rate as f64 * secs) as usize;
        let mut x: Vec<f32> = Vec::with_capacity(n);
        for i in 0..n {
            let t = i as f64 / rate as f64;
            // sin without std: tiny Taylor-free approach — use core's f64 via
            // ptiles_core math (sin is exported there for exactly this).
            let phase = 2.0 * core::f64::consts::PI * step_hz * t;
            x.push((dc + amp * ptiles_core::math::sin(phase)) as f32);
        }
        let zeros = alloc::vec![0.0f32; n];
        AccelStats::calculate(&x, &zeros, &zeros, rate)
    }

    #[test]
    fn road_context_from_nearest_road() {
        let roads = alloc::vec![RoadSegment {
            osm_id: 1,
            road_class: "footway".to_string(),
            coords: alloc::vec![[-86.79, 36.16], [-86.789, 36.161]],
            name: None,
            ref_tag: None,
            oneway: None,
            speed_limit_kmh: None,
            lanes: None,
            surface: None,
            bridge_tunnel: None,
        }];
        let near = NearestRoad {
            road_index: 0,
            segment_index: 0,
            snapped: (36.16, -86.79),
            distance_m: 3.0,
        };
        let ctx = RoadContext::from_nearest(&roads, &near).expect("in-range index");
        assert_eq!(ctx.road_class, "footway");
        assert_eq!(ctx.distance_m, 3.0);
        // The segment runs north-east, so the bearing is derived rather than
        // absent -- that is the whole point of carrying it: a fix travelling
        // ALONG a road is far more likely to be in a vehicle than one crossing
        // it, and only a bearing can tell those apart.
        let bearing = ctx.bearing.expect("a two-vertex segment has a heading");
        assert!(
            (30.0..50.0).contains(&bearing),
            "north-east segment should bear ~39 deg, got {bearing}"
        );
        // Out-of-range index yields None instead of panicking.
        let bogus = NearestRoad { road_index: 7, ..near };
        assert_eq!(RoadContext::from_nearest(&roads, &bogus), None);
    }

    #[test]
    fn accel_stats_finds_walking_cadence() {
        // 2 Hz stride, 4 s window at 50 Hz => ~2 Hz dominant frequency.
        let s = accel_window(2.0, 1.5, 9.8, 50, 4.0);
        assert!(
            (s.dominant_frequency - 2.0).abs() < 0.3,
            "dominant {} should be ~2 Hz",
            s.dominant_frequency
        );
        assert!(s.step_count >= 7, "step_count {}", s.step_count);
        assert!(s.variance > 0.5, "variance {}", s.variance);
        assert!((s.window_duration_s.unwrap() - 4.0).abs() < 1e-9);
    }

    #[test]
    fn missing_accel_fields_stay_missing() {
        // Robustness against real producers: the Rookery Android exporter sends
        // variance, cadence and step count and omits the other two. That has to
        // stay distinguishable from "no accelerometer", or a partial reading is
        // silently half-interpreted as an absent one.
        let partial = AccelStats {
            variance: 0.02,
            dominant_frequency: 1.8,
            step_count: 7,
            ..AccelStats::EMPTY
        };
        assert_eq!(partial.mean_magnitude, None);
        assert_eq!(partial.window_duration_s, None);
        assert!(partial.has_signal());
        assert!(!AccelStats::EMPTY.has_signal());
        // A phone lying perfectly still is a *reading*, not an absence -- which
        // is why has_signal() is not "is this EMPTY".
        let still = AccelStats {
            mean_magnitude: Some(9.81),
            window_duration_s: Some(4.0),
            ..AccelStats::EMPTY
        };
        assert!(!still.has_signal(), "no variance, no cadence, no steps");
        assert_ne!(still, AccelStats::EMPTY, "but it did report a mean and a window");
        // And a reported zero is not the same value as nothing reported.
        let zeroed = AccelStats { mean_magnitude: Some(0.0), ..AccelStats::EMPTY };
        assert_ne!(zeroed, AccelStats::EMPTY);
    }

    #[test]
    fn no_accel_window_classifies_like_an_empty_one() {
        // `None` and `EMPTY` agree today: both fall to the table's catch-all.
        // Pinned so that if a future rule starts reading mean magnitude, this
        // test fails and forces a deliberate decision about the missing case
        // rather than letting a 0.0 default answer it.
        let e = AccelStats::EMPTY;
        for speed in [None, Some(0.0), Some(1.0), Some(12.0)] {
            assert_eq!(
                classify(speed, Some(5.0), None, None),
                classify(speed, Some(5.0), None, Some(&e)),
                "speed {speed:?}"
            );
        }
        assert_eq!(
            classify(None, None, None, None).movement,
            MovementType::Stationary
        );
    }

    #[test]
    fn accel_stats_uses_the_shortest_axis() {
        // Mismatched axis lengths: the window is min(len), not max — a short
        // axis must not read past its end or inflate the duration.
        let long = alloc::vec![1.0f32; 100];
        let short = alloc::vec![1.0f32; 50];
        let s = AccelStats::calculate(&long, &short, &long, 50);
        assert_eq!(s.window_duration_s, Some(1.0));
        // magnitude = sqrt(1+1+1) for every sample.
        assert!((s.mean_magnitude.unwrap() - sqrt(3.0)).abs() < 1e-6);
        assert!(s.variance < 1e-9);
    }

    #[test]
    fn accel_stats_needs_three_samples_for_a_peak() {
        // A local maximum needs a neighbour on each side; 2 samples can't have
        // one, but mean/variance are still meaningful.
        let s = AccelStats::calculate(&[9.0, 12.0], &[0.0, 0.0], &[0.0, 0.0], 50);
        assert_eq!(s.step_count, 0);
        assert_eq!(s.dominant_frequency, 0.0);
        assert!(s.mean_magnitude.unwrap() > 10.0);
        assert!(s.variance > 1.0);
    }

    #[test]
    fn accel_cadence_is_capped_by_the_refractory_gap() {
        // 10 Hz vibration (a car, not a stride) at 50 Hz: peaks land every 5
        // samples but the 0.25 s refractory admits at most ~4/s, so the
        // reported cadence cannot claim an impossible stride rate.
        let s = accel_window(10.0, 1.5, 9.8, 50, 4.0);
        assert!(
            s.dominant_frequency <= 4.5,
            "cadence {} must stay inside the refractory cap",
            s.dominant_frequency
        );
        assert!(s.variance > 0.5, "the vibration is still visible in variance");
    }

    #[test]
    fn non_finite_accel_samples_read_as_stationary() {
        // A driver-level glitch (NaN sample) makes variance NaN. Every accel
        // threshold is a `>`/`<` compare, so NaN fails them all and lands on
        // the final catch-all rather than inventing motion.
        let s = AccelStats::calculate(&[9.8, f32::NAN, 9.8, 9.8], &[0.0; 4], &[0.0; 4], 50);
        assert!(s.variance.is_nan());
        assert_eq!(s.step_count, 0);
        let v = classify_accel_only(&s);
        assert_eq!(v.movement, MovementType::Stationary);
        assert_eq!(v.confidence, 0.85);
    }

    #[test]
    fn accel_stats_empty_and_still() {
        assert_eq!(AccelStats::calculate(&[], &[], &[], 50), AccelStats::EMPTY);
        assert_eq!(
            AccelStats::calculate(&[1.0], &[1.0], &[1.0], 0),
            AccelStats::EMPTY
        );
        // Dead-still phone: no variance, no steps.
        let still = AccelStats::calculate(&[9.8; 100], &[0.0; 100], &[0.0; 100], 50);
        assert_eq!(still.step_count, 0);
        assert!(still.variance < 1e-9);
        assert_eq!(
            classify_accel_only(&still).movement,
            MovementType::Stationary
        );
    }

    #[test]
    fn bad_gps_accuracy_falls_back_to_accel_when_speed_is_not_decisive() {
        let walking = accel_window(2.0, 1.5, 9.8, 50, 4.0);
        // A 100 m fix cannot settle a contest the accelerometer can: at 5 m/s the
        // reading is consistent with a fast walk, so the cadence wins.
        let v = classify(Some(5.0), Some(100.0), None, Some(&walking));
        assert_eq!(v.movement, MovementType::Walking);
        // No speed at all is the same situation with less information.
        let v = classify(None, Some(100.0), None, Some(&walking));
        assert_eq!(v.movement, MovementType::Walking);
    }

    fn road_with_bearing(class: &str, distance_m: f64, bearing: f64) -> RoadContext {
        RoadContext { road_class: class.to_string(), distance_m, bearing: Some(bearing) }
    }

    #[test]
    fn travelling_along_a_road_reads_as_driving() {
        // Distance alone cannot separate a car from someone on the pavement --
        // both sit metres from the centreline. Heading can.
        let e = AccelStats::EMPTY;
        // `residential`, not `primary`: is_vehicular covers residential/unclassified/
        // service and is_highway covers motorway/trunk/_link, so the major-road
        // classes fall through both. That gap is pre-existing and shared with the
        // Kotlin original, so it is not silently changed here.
        let road = road_with_bearing("residential", 8.0, 90.0);
        let v = classify_with_history(Some(6.0), Some(5.0), Some(&road), Some(&e), Some(92.0), MovementType::Unknown);
        assert_eq!(v.movement, MovementType::Driving);

        // A road is an axis, not a direction: travelling "backwards" along it is
        // the same alignment.
        let v = classify_with_history(Some(6.0), Some(5.0), Some(&road), Some(&e), Some(271.0), MovementType::Unknown);
        assert_eq!(v.movement, MovementType::Driving);
    }

    #[test]
    fn crossing_a_road_at_speed_is_also_driving() {
        // A car on an intersecting street, not a pedestrian on a crossing --
        // hence the higher speed bar for the perpendicular case.
        let e = AccelStats::EMPTY;
        let road = road_with_bearing("residential", 8.0, 0.0);
        let v = classify_with_history(Some(7.0), Some(5.0), Some(&road), Some(&e), Some(90.0), MovementType::Unknown);
        assert_eq!(v.movement, MovementType::Driving);
    }

    #[test]
    fn a_bearing_without_a_road_bearing_changes_nothing() {
        // The four-argument classify must keep its old behaviour exactly, which
        // is what lets one-shot callers stay on it.
        let e = AccelStats::EMPTY;
        let road = RoadContext { road_class: "residential".to_string(), distance_m: 8.0, bearing: None };
        assert_eq!(
            classify_with_history(Some(3.0), Some(5.0), Some(&road), Some(&e), Some(90.0), MovementType::Unknown),
            classify(Some(3.0), Some(5.0), Some(&road), Some(&e)),
        );
    }

    #[test]
    fn driving_is_held_through_a_stop_but_released_by_a_walking_cadence() {
        // A car at a light looks identical to a parked one in a single sample.
        // Flipping to Stationary at every red turns one journey into a string of
        // false arrivals.
        let still = AccelStats::EMPTY;
        let v = classify_with_history(Some(1.0), Some(5.0), None, Some(&still), None, MovementType::Driving);
        assert_eq!(v.movement, MovementType::Driving);

        // But a real gait breaks out immediately rather than waiting the speed
        // out -- that is what makes driving -> walking responsive.
        let walking = accel_window(2.0, 1.5, 9.8, 50, 4.0);
        let v = classify_with_history(Some(1.0), Some(5.0), None, Some(&walking), None, MovementType::Driving);
        assert_ne!(v.movement, MovementType::Driving);

        // And a genuinely stopped vehicle is not held forever.
        let v = classify_with_history(Some(0.1), Some(5.0), None, Some(&still), None, MovementType::Driving);
        assert_ne!(v.movement, MovementType::Driving);
    }

    #[test]
    fn resting_jitter_is_not_a_cadence() {
        // The reason for autocorrelation over peak counting: a phone at rest
        // produces maxima above mean + 0.5*sd often enough that the old counter
        // called roughly a third of stationary windows Walking.
        let mut x = alloc::vec::Vec::new();
        let (mut y, mut z) = (alloc::vec::Vec::new(), alloc::vec::Vec::new());
        // A modular sequence is itself periodic -- i*7919 % 97 repeats every 97
        // samples, which at 50 Hz is 0.51 Hz and lands squarely inside the
        // cadence band. Use an LCG instead, whose period is far longer than the
        // window.
        let mut seed: u32 = 12345;
        for _ in 0..200 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let n = ((seed >> 16) as f32 / 65535.0 - 0.5) / 30.0;
            x.push(n);
            y.push(-n * 0.7);
            z.push(9.8 + n * 0.5);
        }
        let s = AccelStats::calculate(&x, &y, &z, 50);
        assert_eq!(s.step_count, 0, "aperiodic jitter is not steps");
        assert_eq!(s.dominant_frequency, 0.0);
        assert_eq!(classify_accel_only(&s).movement, MovementType::Stationary);
    }

    #[test]
    fn clearing_the_sticky_lets_stationary_commit() {
        // Explicit config, not defaults: default_latency_ms is 60 s here and the
        // Kotlin original deliberately runs 15 s ("too slow for walking"), so a
        // test on defaults would be measuring the wrong tuning and take a
        // simulated minute to say so.
        let cfg = DebounceConfig { default_latency_ms: 15_000, ..DebounceConfig::default() };
        let mut d = VoteDebouncer::new(cfg);
        let driving = Vote { movement: MovementType::Driving, confidence: 0.9 };
        let stationary = Vote { movement: MovementType::Stationary, confidence: 0.9 };
        let mut t = 0u64;
        // Steps must clear rapid_latency_ms (15 s) as well as min_continuous,
        // or the transition never commits and the test measures nothing.
        for _ in 0..10 { d.tick(&driving, t); t += 5_000; }
        assert_eq!(d.current(), MovementType::Driving);
        d.clear_vehicle_sticky();
        for _ in 0..10 { d.tick(&stationary, t); t += 5_000; }
        // 50 s of Stationary majority against a 15 s latency: if the cleared sticky
        // still blocked it, this is where it would show.
        assert_eq!(d.current(), MovementType::Stationary, "cleared sticky should not block the transition");
    }

    #[test]
    fn bad_gps_accuracy_does_not_discard_a_decisive_speed() {
        // The failure this guard exists for. In a tunnel or an urban canyon
        // accuracy degrades while the vehicle keeps moving, and the accelerometer
        // hears engine and road vibration as a walking cadence. One recording
        // produced 81 rows of Walking at up to 56 mph, accuracy drifting 31 m to
        // 314 m. An uncertain POSITION is not evidence that 20 m/s was walked.
        let walking = accel_window(2.0, 1.5, 9.8, 50, 4.0);
        let v = classify(Some(20.0), Some(100.0), None, Some(&walking));
        assert_eq!(v.movement, MovementType::Driving);
        // Lower than the same call on a good fix (0.90): the reading it rests on
        // is less certain, and the vote is weighted accordingly.
        assert!(v.confidence < 0.90);

        // Unknown accuracy is untrusted the same way, and for the same reason.
        let v = classify(Some(20.0), Some(f64::NAN), None, Some(&walking));
        assert_eq!(v.movement, MovementType::Driving);

        // The bar is the driving floor, not merely having a speed -- a bad fix can
        // manufacture a small bogus one, and that is still not trusted.
        let v = classify(Some(DRIVING_FLOOR_MPS - 0.1), Some(100.0), None, Some(&walking));
        assert_eq!(v.movement, MovementType::Walking);
        // A non-finite speed is not a speed.
        let v = classify(Some(f64::INFINITY), Some(100.0), None, Some(&walking));
        assert_eq!(v.movement, MovementType::Walking);
    }

    #[test]
    fn speed_only_bands() {
        let e = AccelStats::EMPTY;
        assert_eq!(classify(Some(15.0), Some(5.0), None, Some(&e)).movement, MovementType::Driving);
        assert_eq!(classify(Some(3.0), Some(5.0), None, Some(&e)).movement, MovementType::Walking);
        // Below the walking ceiling with no accel signal: stationary.
        assert_eq!(classify(Some(1.0), Some(5.0), None, Some(&e)).movement, MovementType::Stationary);
        // No speed at all: accel-only.
        assert_eq!(classify(None, Some(5.0), None, Some(&e)).movement, MovementType::Stationary);
    }

    #[test]
    fn threshold_boundaries_are_exclusive() {
        let e = AccelStats::EMPTY;
        // The accuracy gate is `> 30`: exactly 30 m is still trusted GPS.
        assert_eq!(
            classify(Some(15.0), Some(GPS_ACCURACY_GATE_M), None, Some(&e)).movement,
            MovementType::Driving
        );
        // Speed bands are `>` too: exactly at a threshold stays in the band below.
        assert_eq!(
            classify(Some(DRIVING_FLOOR_MPS), Some(5.0), None, Some(&e)).movement,
            MovementType::Walking
        );
        assert_eq!(
            classify(Some(WALKING_CEILING_MPS), Some(5.0), None, Some(&e)).movement,
            MovementType::Stationary
        );
    }

    #[test]
    fn missing_accuracy_still_uses_speed() {
        // Accuracy `None` means "unreported", not "bad" — the gate only fires
        // on a number worse than the threshold.
        let e = AccelStats::EMPTY;
        assert_eq!(classify(Some(15.0), None, None, Some(&e)).movement, MovementType::Driving);
        assert_eq!(
            classify(Some(3.0), None, Some(&road("footway", 2.0)), Some(&e)).movement,
            MovementType::Walking
        );
    }

    #[test]
    fn nonsense_speed_falls_through_to_accel() {
        // A negative platform speed is not evidence of anything; neither band
        // may claim it.
        let e = AccelStats::EMPTY;
        assert_eq!(classify(Some(-5.0), Some(5.0), None, Some(&e)).movement, MovementType::Stationary);
        // Road priors need a speed, so a road hit with no speed is inert.
        assert_eq!(
            classify(None, Some(5.0), Some(&road("motorway", 2.0)), Some(&e)).movement,
            MovementType::Stationary
        );
    }

    #[test]
    fn road_priors_beat_the_speed_bands() {
        let e = AccelStats::EMPTY;
        // 3 m/s on a motorway is a slow-moving car, not a walk.
        let v = classify(Some(3.0), Some(5.0), Some(&road("motorway", 4.0)), Some(&e));
        assert_eq!(v.movement, MovementType::Driving);
        assert!(v.confidence > 0.9);
        // Same speed on a footway is a run/walk, not a car.
        let v = classify(Some(3.0), Some(5.0), Some(&road("footway", 2.0)), Some(&e));
        assert_eq!(v.movement, MovementType::Walking);
        // Residential street at 3 m/s: vehicular prior.
        assert_eq!(
            classify(Some(3.0), Some(5.0), Some(&road("residential", 6.0)), Some(&e)).movement,
            MovementType::Driving
        );
        // Far from any road at walking pace: walking, whatever the accel says.
        assert_eq!(
            classify(Some(1.5), Some(5.0), Some(&road("residential", 120.0)), Some(&e)).movement,
            MovementType::Walking
        );
    }

    #[test]
    fn road_prior_edges_and_unknown_classes() {
        let e = AccelStats::EMPTY;
        // Ramps ("*_link") count as highway.
        assert_eq!(
            classify(Some(3.0), Some(5.0), Some(&road("motorway_link", 4.0)), Some(&e)).movement,
            MovementType::Driving
        );
        // Footway priors need speed > 1.1: exactly 1.1 falls through to the
        // speed bands, which at that speed say nothing, so accel decides.
        assert_eq!(
            classify(Some(1.1), Some(5.0), Some(&road("footway", 2.0)), Some(&e)).movement,
            MovementType::Stationary
        );
        // Distance bounds are exclusive: 5 m off a footway is too far, 10 m
        // off a residential street likewise.
        assert_eq!(
            classify(Some(1.5), Some(5.0), Some(&road("footway", 5.0)), Some(&e)).movement,
            MovementType::Stationary
        );
        assert_eq!(
            classify(Some(3.0), Some(5.0), Some(&road("residential", 10.0)), Some(&e)).movement,
            MovementType::Walking,
            "no vehicular prior at 10 m, so the speed band decides"
        );
        // An unmapped-for-us class (track, cycleway) has no prior at all.
        assert_eq!(
            classify(Some(3.0), Some(5.0), Some(&road("track", 2.0)), Some(&e)).movement,
            MovementType::Walking
        );
        // Off-road walking window is inclusive at both ends.
        for speed in [0.5, 2.2] {
            assert_eq!(
                classify(Some(speed), Some(5.0), Some(&road("residential", 120.0)), Some(&e)).movement,
                MovementType::Walking,
                "{speed} m/s far from any road is a walk"
            );
        }
        // Below it, the off-road prior does not fire.
        assert_eq!(
            classify(Some(0.4), Some(5.0), Some(&road("residential", 120.0)), Some(&e)).movement,
            MovementType::Stationary
        );
    }

    #[test]
    fn matched_road_branch_does_not_fall_into_later_branches() {
        // 120 m from a motorway at 1.5 m/s: the highway branch does not match
        // (too far), so the off-road walking branch gets its turn.
        let e = AccelStats::EMPTY;
        assert_eq!(
            classify(Some(1.5), Some(5.0), Some(&road("motorway", 120.0)), Some(&e)).movement,
            MovementType::Walking
        );
    }

    #[test]
    fn walking_cadence_overrides_a_bad_motorway_snap() {
        // On the sidewalk beside a highway: 7 m off, real step cadence. The
        // motorway prior must not claim this as Driving.
        let walking = accel_window(2.0, 1.5, 9.8, 50, 4.0);
        let v = classify(Some(3.0), Some(5.0), Some(&road("motorway", 7.0)), Some(&walking));
        assert_eq!(v.movement, MovementType::Walking);
        // Inside 5 m the counter-signal does not apply — snap is trusted.
        let v = classify(Some(3.0), Some(5.0), Some(&road("motorway", 3.0)), Some(&walking));
        assert_eq!(v.movement, MovementType::Driving);
    }

    #[test]
    fn accel_only_running_and_vehicle_vibration() {
        let running = AccelStats { dominant_frequency: 3.0, variance: 0.5, ..AccelStats::EMPTY };
        assert_eq!(classify_accel_only(&running).movement, MovementType::Running);
        let car = AccelStats { dominant_frequency: 0.4, variance: 2.0, ..AccelStats::EMPTY };
        assert_eq!(classify_accel_only(&car).movement, MovementType::Driving);
    }

    #[test]
    fn counter_signal_needs_both_distance_and_cadence() {
        // 7 m off a motorway but only 4 steps (not > 4): the counter-signal
        // does not fire, so the snap is trusted.
        let weak = AccelStats { dominant_frequency: 2.0, step_count: 4, ..AccelStats::EMPTY };
        assert_eq!(
            classify(Some(3.0), Some(5.0), Some(&road("motorway", 7.0)), Some(&weak)).movement,
            MovementType::Driving
        );
        // Plenty of steps but a 4 Hz cadence is outside the 1..=3 Hz stride
        // band, so it is not walking evidence either.
        let too_fast = AccelStats { dominant_frequency: 4.0, step_count: 20, ..AccelStats::EMPTY };
        assert_eq!(
            classify(Some(3.0), Some(5.0), Some(&road("motorway", 7.0)), Some(&too_fast)).movement,
            MovementType::Driving
        );
    }

    #[test]
    fn counter_signal_falls_through_to_the_speed_bands_not_the_other_priors() {
        // Sidewalk cadence beside a motorway at 12 m/s: the counter-signal
        // rejects the 0.95 highway prior, and the *speed band* answers next —
        // Driving at the band's 0.90, which is how you can tell which code
        // path produced it.
        let walking = AccelStats { dominant_frequency: 2.0, step_count: 20, variance: 1.0, ..AccelStats::EMPTY };
        let v = classify(Some(12.0), Some(5.0), Some(&road("motorway", 7.0)), Some(&walking));
        assert_eq!(v.movement, MovementType::Driving);
        assert_eq!(v.confidence, 0.90, "band confidence, not the 0.95 road prior");
    }

    #[test]
    fn accel_only_table_order() {
        // Running needs BOTH f > 2.5 and v > 0.3; at the boundary it is walking.
        let boundary = AccelStats { dominant_frequency: 2.5, variance: 0.3, ..AccelStats::EMPTY };
        assert_eq!(classify_accel_only(&boundary).movement, MovementType::Walking);
        // Fast cadence, tiny variance (phone on a vibrating surface): the
        // running row misses on variance, the walking row catches it.
        let jitter = AccelStats { dominant_frequency: 3.0, variance: 0.02, ..AccelStats::EMPTY };
        assert_eq!(classify_accel_only(&jitter).movement, MovementType::Walking);
        // Steps counted but no usable cadence: the low-confidence walk row.
        let steps_only = AccelStats { dominant_frequency: 0.5, step_count: 3, variance: 0.05, ..AccelStats::EMPTY };
        let v = classify_accel_only(&steps_only);
        assert_eq!(v.movement, MovementType::Walking);
        assert_eq!(v.confidence, 0.40);
        // Variance exactly 1.0 leaves the stationary row and enters vehicle
        // vibration; 5.0 is past vehicle range and hits the catch-all.
        assert_eq!(
            classify_accel_only(&AccelStats { variance: 1.0, ..AccelStats::EMPTY }).movement,
            MovementType::Driving
        );
        assert_eq!(
            classify_accel_only(&AccelStats { variance: 5.0, ..AccelStats::EMPTY }).movement,
            MovementType::Stationary
        );
    }

    fn vote(t: MovementType) -> Vote {
        Vote { movement: t, confidence: 1.0 }
    }

    /// Feed `n` identical votes, one per second from `t0`. Returns end time.
    fn feed(d: &mut VoteDebouncer, t: MovementType, n: u64, t0: u64) -> u64 {
        let mut now = t0;
        for _ in 0..n {
            d.tick(&vote(t), now);
            now += 1000;
        }
        now
    }

    #[test]
    fn debouncer_needs_majority_run_and_latency() {
        let mut d = VoteDebouncer::new(DebounceConfig::default());
        assert_eq!(d.current(), MovementType::Unknown);
        // Driving has the 15 s rapid latency: 10 s of votes is not enough.
        let t = feed(&mut d, MovementType::Driving, 10, 0);
        assert_eq!(d.current(), MovementType::Unknown);
        // Past 15 s it commits.
        feed(&mut d, MovementType::Driving, 8, t);
        assert_eq!(d.current(), MovementType::Driving);
    }

    #[test]
    fn single_stray_vote_never_transitions() {
        let mut d = VoteDebouncer::new(DebounceConfig::default());
        let t = feed(&mut d, MovementType::Walking, 80, 0);
        assert_eq!(d.current(), MovementType::Walking);
        d.tick(&vote(MovementType::Driving), t);
        assert_eq!(d.current(), MovementType::Walking, "one vote must not flip");
    }

    #[test]
    fn vehicle_sticky_survives_a_red_light() {
        let mut d = VoteDebouncer::new(DebounceConfig::default());
        let t = feed(&mut d, MovementType::Driving, 20, 0);
        assert_eq!(d.current(), MovementType::Driving);
        // 100 s stopped at a light (< 150 s sticky): still Driving.
        let t = feed(&mut d, MovementType::Stationary, 100, t);
        assert_eq!(d.current(), MovementType::Driving, "red light is not an arrival");
        // Keep standing still past the sticky window plus the 60 s default
        // latency: now it is a real arrival.
        feed(&mut d, MovementType::Stationary, 120, t);
        assert_eq!(d.current(), MovementType::Stationary);
    }

    #[test]
    fn first_transition_out_of_unknown_pays_the_default_latency() {
        // Unknown -> Stationary is not a "rapid" transition: 60 s, not 15 s.
        let mut d = VoteDebouncer::new(DebounceConfig::default());
        let t = feed(&mut d, MovementType::Stationary, 40, 0);
        assert_eq!(d.current(), MovementType::Unknown);
        feed(&mut d, MovementType::Stationary, 30, t);
        assert_eq!(d.current(), MovementType::Stationary);
    }

    #[test]
    fn a_flapping_majority_never_commits() {
        // Alternating Driving/Stationary 10 s apart: the 5-slot window does
        // produce a 3-vote majority every tick, but it is a *different* one
        // each time, so the pending run resets to 1 and `min_continuous = 3`
        // is never reached however long the flapping goes on.
        let mut d = VoteDebouncer::new(DebounceConfig::default());
        let mut now = feed(&mut d, MovementType::Walking, 80, 0);
        for i in 0..12 {
            let t = if i % 2 == 0 { MovementType::Driving } else { MovementType::Stationary };
            d.tick(&vote(t), now);
            now += 10_000;
        }
        assert_eq!(d.current(), MovementType::Walking, "flapping is not evidence");
    }

    /// The other half of the story: a majority that *outlives* its own block.
    /// 4 Driving votes in a 5-slot window keep the Driving majority alive into
    /// the following votes, so a transition can commit a tick or two after the
    /// evidence stopped arriving. That is the window doing its job, not a bug —
    /// pinned here so a future window change has to acknowledge it.
    #[test]
    fn a_majority_outlives_the_votes_that_built_it() {
        let mut d = VoteDebouncer::new(DebounceConfig::default());
        let t = feed(&mut d, MovementType::Walking, 80, 0);
        feed(&mut d, MovementType::Driving, 4, t); // 4 votes, 1 s apart
        assert_eq!(d.current(), MovementType::Walking, "not yet: 3 s of evidence");
        // Same four votes spread over 30 s, then Stationary votes: Driving is
        // still the window majority long enough to clear the 15 s latency.
        let mut d = VoteDebouncer::new(DebounceConfig::default());
        let mut now = feed(&mut d, MovementType::Walking, 80, 0);
        for _ in 0..4 {
            d.tick(&vote(MovementType::Driving), now);
            now += 10_000;
        }
        d.tick(&vote(MovementType::Stationary), now);
        assert_eq!(d.current(), MovementType::Driving);
    }

    #[test]
    fn returning_to_the_current_state_restarts_the_latency_clock() {
        let mut d = VoteDebouncer::new(DebounceConfig::default());
        let mut now = feed(&mut d, MovementType::Walking, 80, 0);
        // 14 s of Driving: pending, but one second short of the 15 s latency.
        now = feed(&mut d, MovementType::Driving, 14, now);
        assert_eq!(d.current(), MovementType::Walking);
        // Back to Walking clears the pending transition...
        now = feed(&mut d, MovementType::Walking, 5, now);
        // ...so another 14 s of Driving is again not enough, even though 33 s
        // of wall-clock have passed since Driving was first seen.
        now = feed(&mut d, MovementType::Driving, 14, now);
        assert_eq!(d.current(), MovementType::Walking);
        // A full uninterrupted 15 s does commit.
        feed(&mut d, MovementType::Driving, 5, now);
        assert_eq!(d.current(), MovementType::Driving);
    }

    #[test]
    fn zero_and_one_sized_configs_are_clamped() {
        // majority_window/min_continuous of 0 would mean "no window" and "no
        // run required"; both clamp to 1 rather than dividing by zero or
        // committing on nothing.
        let cfg = DebounceConfig {
            majority_window: 0,
            min_continuous: 0,
            rapid_latency_ms: 0,
            default_latency_ms: 0,
            ..DebounceConfig::default()
        };
        let mut d = VoteDebouncer::new(cfg);
        d.tick(&vote(MovementType::Walking), 0);
        assert_eq!(d.current(), MovementType::Walking, "one vote, zero latency, commits");
    }

    #[test]
    fn backwards_clock_does_not_panic_or_commit() {
        // A non-monotonic clock (NTP step, caller bug) must not underflow the
        // elapsed-time math. Latency reads as 0 elapsed, so nothing commits.
        let mut d = VoteDebouncer::new(DebounceConfig::default());
        let mut now = 500_000u64;
        for _ in 0..20 {
            d.tick(&vote(MovementType::Driving), now);
            now = now.saturating_sub(10_000);
        }
        assert_eq!(d.current(), MovementType::Unknown);
    }

    #[test]
    fn sticky_only_guards_the_driving_to_stationary_edge() {
        // Walking -> Stationary is not a vehicle stop, so no sticky applies.
        let mut d = VoteDebouncer::new(DebounceConfig::default());
        let t = feed(&mut d, MovementType::Walking, 80, 0);
        feed(&mut d, MovementType::Stationary, 70, t);
        assert_eq!(d.current(), MovementType::Stationary);
    }

    #[test]
    fn a_driving_vote_refreshes_the_sticky_window() {
        // Crawling in traffic: mostly stopped, but a Driving vote every 100 s.
        // Each one re-arms the 150 s sticky, so the trip never reads as an
        // arrival however long the jam lasts.
        let mut d = VoteDebouncer::new(DebounceConfig::default());
        let mut now = feed(&mut d, MovementType::Driving, 20, 0);
        for _ in 0..5 {
            now = feed(&mut d, MovementType::Stationary, 100, now);
            now = feed(&mut d, MovementType::Driving, 5, now);
        }
        assert_eq!(d.current(), MovementType::Driving);
        // Stop voting Driving and the window finally expires.
        feed(&mut d, MovementType::Stationary, 250, now);
        assert_eq!(d.current(), MovementType::Stationary);
    }

    #[test]
    fn a_partial_window_can_still_reach_a_majority() {
        // Two votes in a 5-slot window: threshold is len/2+1 = 2, so the
        // second agreeing vote already carries the window. (It still has to
        // clear the latency, which is what keeps this from being twitchy.)
        let mut d = VoteDebouncer::new(DebounceConfig {
            min_continuous: 1,
            rapid_latency_ms: 0,
            ..DebounceConfig::default()
        });
        d.tick(&vote(MovementType::Driving), 0);
        assert_eq!(d.current(), MovementType::Driving);
    }

    fn control(intersection_type: u8, distance_m: f64) -> TrafficControl {
        TrafficControl { distance_m, intersection_type }
    }

    /// Feed `n` votes one second apart, all with the same traffic control.
    fn feed_at(
        d: &mut VoteDebouncer,
        t: MovementType,
        n: u64,
        t0: u64,
        c: Option<&TrafficControl>,
    ) -> u64 {
        let mut now = t0;
        for _ in 0..n {
            d.tick_at(&vote(t), now, c);
            now += 1000;
        }
        now
    }

    #[test]
    fn traffic_control_extends_the_sticky_window() {
        let signal = control(1, 8.0);
        let mut d = VoteDebouncer::new(DebounceConfig::default());
        let t = feed_at(&mut d, MovementType::Driving, 20, 0, Some(&signal));
        assert_eq!(d.current(), MovementType::Driving);
        // 200 s stopped: past the 150 s vehicle sticky, inside the 300 s
        // signal sticky. Plain tick() would already have called this an
        // arrival; the map says it's a long light.
        let t_signal = feed_at(&mut d, MovementType::Stationary, 200, t, Some(&signal));
        assert_eq!(d.current(), MovementType::Driving);
        // Past the signal window it still commits — the guard delays, never blocks.
        feed_at(&mut d, MovementType::Stationary, 150, t_signal, Some(&signal));
        assert_eq!(d.current(), MovementType::Stationary);

        // Same stream without the control: the plain 150 s window expires and
        // the arrival lands inside the first 200 s.
        let mut plain = VoteDebouncer::new(DebounceConfig::default());
        let t = feed_at(&mut plain, MovementType::Driving, 20, 0, None);
        feed_at(&mut plain, MovementType::Stationary, 200, t, None);
        assert_eq!(plain.current(), MovementType::Stationary);
    }

    #[test]
    fn only_queueing_controls_within_radius_extend() {
        // Signals/stop/give-way hold traffic; roundabouts and untyped nodes
        // don't, and neither does a node 200 m down the block.
        assert!(control(1, 10.0).holds_traffic(25.0));
        assert!(control(2, 24.9).holds_traffic(25.0));
        assert!(control(3, 0.0).holds_traffic(25.0));
        assert!(!control(4, 5.0).holds_traffic(25.0), "roundabout does not queue");
        assert!(!control(0, 5.0).holds_traffic(25.0), "untyped junction");
        assert!(!control(1, 200.0).holds_traffic(25.0), "too far to be waiting at it");
        assert!(!control(1, f64::NAN).holds_traffic(25.0));

        // A far-away signal must behave exactly like no control at all.
        let far = control(1, 200.0);
        let mut d = VoteDebouncer::new(DebounceConfig::default());
        let t = feed_at(&mut d, MovementType::Driving, 20, 0, Some(&far));
        feed_at(&mut d, MovementType::Stationary, 200, t, Some(&far));
        assert_eq!(d.current(), MovementType::Stationary);
    }

    #[test]
    fn signal_sticky_never_shortens_the_vehicle_window() {
        // A config whose signal window is *shorter* than the vehicle one must
        // not make arrivals at intersections commit sooner — the control can
        // only extend.
        let cfg = DebounceConfig { signal_sticky_ms: 10_000, ..DebounceConfig::default() };
        let signal = control(1, 5.0);
        let mut d = VoteDebouncer::new(cfg);
        let t = feed_at(&mut d, MovementType::Driving, 20, 0, Some(&signal));
        feed_at(&mut d, MovementType::Stationary, 100, t, Some(&signal));
        assert_eq!(d.current(), MovementType::Driving, "still inside the 150 s vehicle window");
    }

    #[test]
    fn leaving_the_intersection_drops_back_to_the_short_window() {
        // Waiting at the light, then the fixes move off it (parked mid-block).
        // The extension applies per fix, so once the control is gone the plain
        // 150 s window governs and the arrival lands.
        let signal = control(1, 5.0);
        let mut d = VoteDebouncer::new(DebounceConfig::default());
        let t = feed_at(&mut d, MovementType::Driving, 20, 0, Some(&signal));
        let t = feed_at(&mut d, MovementType::Stationary, 100, t, Some(&signal));
        assert_eq!(d.current(), MovementType::Driving);
        feed_at(&mut d, MovementType::Stationary, 120, t, None);
        assert_eq!(d.current(), MovementType::Stationary);
    }

    #[test]
    fn traffic_control_does_not_delay_a_walking_transition() {
        // Sticky only ever guards Driving -> Stationary. Parking at a signal
        // and walking off must still transition on the normal latency.
        let signal = control(1, 5.0);
        let mut d = VoteDebouncer::new(DebounceConfig::default());
        let t = feed_at(&mut d, MovementType::Driving, 20, 0, Some(&signal));
        feed_at(&mut d, MovementType::Walking, 80, t, Some(&signal));
        assert_eq!(d.current(), MovementType::Walking);
    }

    #[test]
    fn traffic_control_from_nearest_intersection() {
        let near = NearestIntersection { index: 3, distance_m: 12.5, intersection_type: 1 };
        let c = TrafficControl::from_nearest(&near);
        assert_eq!(c, control(1, 12.5));
        assert!(c.holds_traffic(25.0));
    }

    #[test]
    fn split_window_has_no_majority() {
        // 5-slot window, alternating votes: no type reaches 3, so no change.
        let mut d = VoteDebouncer::new(DebounceConfig::default());
        let mut now = 0u64;
        for i in 0..40 {
            let t = if i % 2 == 0 { MovementType::Walking } else { MovementType::Driving };
            d.tick(&vote(t), now);
            now += 1000;
        }
        // Alternating 5-windows do contain a 3-majority every other tick, so
        // the guarantee here is only that nothing commits without a run: the
        // pending count resets each time the majority flips.
        assert_eq!(d.current(), MovementType::Unknown);
    }
}
