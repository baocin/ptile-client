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

use ptiles_core::math::sqrt;
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
}

impl RoadContext {
    /// Build the prior from a `ptiles_core::nearest_road` hit and the roads
    /// slice it indexes into. Returns `None` if the index is out of range.
    pub fn from_nearest(roads: &[RoadSegment], near: &NearestRoad) -> Option<RoadContext> {
        let road = roads.get(near.road_index)?;
        Some(RoadContext {
            road_class: road.road_class.clone(),
            distance_m: near.distance_m,
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
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct AccelStats {
    /// Variance of the magnitude series, (m/s^2)^2.
    pub variance: f64,
    pub mean_magnitude: f64,
    /// Step cadence, Hz.
    pub dominant_frequency: f64,
    pub step_count: u32,
    /// Window length, seconds.
    pub window_duration_s: f64,
}

impl AccelStats {
    /// All-zero stats: what a caller with no accelerometer passes.
    pub const EMPTY: AccelStats = AccelStats {
        variance: 0.0,
        mean_magnitude: 0.0,
        dominant_frequency: 0.0,
        step_count: 0,
        window_duration_s: 0.0,
    };

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
            mean_magnitude: mean,
            dominant_frequency,
            step_count,
            window_duration_s: n as f64 / sample_rate_hz as f64,
        }
    }
}

// ponytail: simplified step detector — prominent local maxima above
// (mean + 0.5*std) with a refractory gap, instead of the FIR-lowpass +
// autocorrelation of MDT's step_detection.rs. Cadence = peaks / seconds. Good
// enough for a fallback (GPS speed is the primary signal); upgrade to
// autocorrelation only if accel-only misclassification is actually observed.
fn detect_steps(
    magnitudes: &VecDeque<f64>,
    mean: f64,
    variance: f64,
    sample_rate_hz: u32,
) -> (u32, f64) {
    let n = magnitudes.len();
    if n < 3 {
        return (0, 0.0);
    }
    let threshold = mean + 0.5 * sqrt(variance);
    // Refractory: no two steps within ~0.25 s (max plausible cadence ~4 Hz).
    let min_gap = (sample_rate_hz / 4).max(1) as isize;
    let mut steps: u32 = 0;
    let mut last_peak: isize = -min_gap;
    for i in 1..n - 1 {
        let m = magnitudes[i];
        let is_peak = m > magnitudes[i - 1] && m >= magnitudes[i + 1] && m > threshold;
        if is_peak && i as isize - last_peak >= min_gap {
            steps += 1;
            last_peak = i as isize;
        }
    }
    let seconds = n as f64 / sample_rate_hz as f64;
    let frequency = if seconds > 0.0 {
        steps as f64 / seconds
    } else {
        0.0
    };
    (steps, frequency)
}

/// Walking/driving speed split, m/s (~5 mph).
pub const WALKING_CEILING_MPS: f64 = 2.2;
/// Definitely-a-vehicle speed, m/s (~20 mph).
pub const DRIVING_FLOOR_MPS: f64 = 8.9;
/// Above this horizontal accuracy (m) GPS is not trusted at all.
pub const GPS_ACCURACY_GATE_M: f64 = 30.0;

/// Stateless single-fix classification. Order: GPS-accuracy gate (bad fix =>
/// accel only) -> road-context priors -> speed-only bands -> accel-only.
///
/// `inst_speed_mps` / `gps_accuracy_m` are `None` when the platform doesn't
/// report them; `nearest_road` is `None` when no road tile answer is available.
pub fn classify(
    inst_speed_mps: Option<f64>,
    gps_accuracy_m: Option<f64>,
    nearest_road: Option<&RoadContext>,
    accel: &AccelStats,
) -> Vote {
    // Poor GPS: trust the accelerometer only.
    if gps_accuracy_m.is_some_and(|a| !a.is_finite() || a > GPS_ACCURACY_GATE_M) {
        return classify_accel_only(accel);
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
        RoadContext { road_class: class.to_string(), distance_m }
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
        assert_eq!(
            RoadContext::from_nearest(&roads, &near),
            Some(road("footway", 3.0))
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
        assert!((s.window_duration_s - 4.0).abs() < 1e-9);
    }

    #[test]
    fn accel_stats_uses_the_shortest_axis() {
        // Mismatched axis lengths: the window is min(len), not max — a short
        // axis must not read past its end or inflate the duration.
        let long = alloc::vec![1.0f32; 100];
        let short = alloc::vec![1.0f32; 50];
        let s = AccelStats::calculate(&long, &short, &long, 50);
        assert_eq!(s.window_duration_s, 1.0);
        // magnitude = sqrt(1+1+1) for every sample.
        assert!((s.mean_magnitude - sqrt(3.0)).abs() < 1e-6);
        assert!(s.variance < 1e-9);
    }

    #[test]
    fn accel_stats_needs_three_samples_for_a_peak() {
        // A local maximum needs a neighbour on each side; 2 samples can't have
        // one, but mean/variance are still meaningful.
        let s = AccelStats::calculate(&[9.0, 12.0], &[0.0, 0.0], &[0.0, 0.0], 50);
        assert_eq!(s.step_count, 0);
        assert_eq!(s.dominant_frequency, 0.0);
        assert!(s.mean_magnitude > 10.0);
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
    fn bad_gps_accuracy_falls_back_to_accel() {
        let walking = accel_window(2.0, 1.5, 9.8, 50, 4.0);
        // Speed says Driving, but a 100 m fix is not trusted: accel wins.
        let v = classify(Some(20.0), Some(100.0), None, &walking);
        assert_eq!(v.movement, MovementType::Walking);
        // Non-finite accuracy is equally untrusted.
        let v = classify(Some(20.0), Some(f64::NAN), None, &walking);
        assert_eq!(v.movement, MovementType::Walking);
    }

    #[test]
    fn speed_only_bands() {
        let e = AccelStats::EMPTY;
        assert_eq!(classify(Some(15.0), Some(5.0), None, &e).movement, MovementType::Driving);
        assert_eq!(classify(Some(3.0), Some(5.0), None, &e).movement, MovementType::Walking);
        // Below the walking ceiling with no accel signal: stationary.
        assert_eq!(classify(Some(1.0), Some(5.0), None, &e).movement, MovementType::Stationary);
        // No speed at all: accel-only.
        assert_eq!(classify(None, Some(5.0), None, &e).movement, MovementType::Stationary);
    }

    #[test]
    fn threshold_boundaries_are_exclusive() {
        let e = AccelStats::EMPTY;
        // The accuracy gate is `> 30`: exactly 30 m is still trusted GPS.
        assert_eq!(
            classify(Some(15.0), Some(GPS_ACCURACY_GATE_M), None, &e).movement,
            MovementType::Driving
        );
        // Speed bands are `>` too: exactly at a threshold stays in the band below.
        assert_eq!(
            classify(Some(DRIVING_FLOOR_MPS), Some(5.0), None, &e).movement,
            MovementType::Walking
        );
        assert_eq!(
            classify(Some(WALKING_CEILING_MPS), Some(5.0), None, &e).movement,
            MovementType::Stationary
        );
    }

    #[test]
    fn missing_accuracy_still_uses_speed() {
        // Accuracy `None` means "unreported", not "bad" — the gate only fires
        // on a number worse than the threshold.
        let e = AccelStats::EMPTY;
        assert_eq!(classify(Some(15.0), None, None, &e).movement, MovementType::Driving);
        assert_eq!(
            classify(Some(3.0), None, Some(&road("footway", 2.0)), &e).movement,
            MovementType::Walking
        );
    }

    #[test]
    fn nonsense_speed_falls_through_to_accel() {
        // A negative platform speed is not evidence of anything; neither band
        // may claim it.
        let e = AccelStats::EMPTY;
        assert_eq!(classify(Some(-5.0), Some(5.0), None, &e).movement, MovementType::Stationary);
        // Road priors need a speed, so a road hit with no speed is inert.
        assert_eq!(
            classify(None, Some(5.0), Some(&road("motorway", 2.0)), &e).movement,
            MovementType::Stationary
        );
    }

    #[test]
    fn road_priors_beat_the_speed_bands() {
        let e = AccelStats::EMPTY;
        // 3 m/s on a motorway is a slow-moving car, not a walk.
        let v = classify(Some(3.0), Some(5.0), Some(&road("motorway", 4.0)), &e);
        assert_eq!(v.movement, MovementType::Driving);
        assert!(v.confidence > 0.9);
        // Same speed on a footway is a run/walk, not a car.
        let v = classify(Some(3.0), Some(5.0), Some(&road("footway", 2.0)), &e);
        assert_eq!(v.movement, MovementType::Walking);
        // Residential street at 3 m/s: vehicular prior.
        assert_eq!(
            classify(Some(3.0), Some(5.0), Some(&road("residential", 6.0)), &e).movement,
            MovementType::Driving
        );
        // Far from any road at walking pace: walking, whatever the accel says.
        assert_eq!(
            classify(Some(1.5), Some(5.0), Some(&road("residential", 120.0)), &e).movement,
            MovementType::Walking
        );
    }

    #[test]
    fn road_prior_edges_and_unknown_classes() {
        let e = AccelStats::EMPTY;
        // Ramps ("*_link") count as highway.
        assert_eq!(
            classify(Some(3.0), Some(5.0), Some(&road("motorway_link", 4.0)), &e).movement,
            MovementType::Driving
        );
        // Footway priors need speed > 1.1: exactly 1.1 falls through to the
        // speed bands, which at that speed say nothing, so accel decides.
        assert_eq!(
            classify(Some(1.1), Some(5.0), Some(&road("footway", 2.0)), &e).movement,
            MovementType::Stationary
        );
        // Distance bounds are exclusive: 5 m off a footway is too far, 10 m
        // off a residential street likewise.
        assert_eq!(
            classify(Some(1.5), Some(5.0), Some(&road("footway", 5.0)), &e).movement,
            MovementType::Stationary
        );
        assert_eq!(
            classify(Some(3.0), Some(5.0), Some(&road("residential", 10.0)), &e).movement,
            MovementType::Walking,
            "no vehicular prior at 10 m, so the speed band decides"
        );
        // An unmapped-for-us class (track, cycleway) has no prior at all.
        assert_eq!(
            classify(Some(3.0), Some(5.0), Some(&road("track", 2.0)), &e).movement,
            MovementType::Walking
        );
        // Off-road walking window is inclusive at both ends.
        for speed in [0.5, 2.2] {
            assert_eq!(
                classify(Some(speed), Some(5.0), Some(&road("residential", 120.0)), &e).movement,
                MovementType::Walking,
                "{speed} m/s far from any road is a walk"
            );
        }
        // Below it, the off-road prior does not fire.
        assert_eq!(
            classify(Some(0.4), Some(5.0), Some(&road("residential", 120.0)), &e).movement,
            MovementType::Stationary
        );
    }

    #[test]
    fn matched_road_branch_does_not_fall_into_later_branches() {
        // 120 m from a motorway at 1.5 m/s: the highway branch does not match
        // (too far), so the off-road walking branch gets its turn.
        let e = AccelStats::EMPTY;
        assert_eq!(
            classify(Some(1.5), Some(5.0), Some(&road("motorway", 120.0)), &e).movement,
            MovementType::Walking
        );
    }

    #[test]
    fn walking_cadence_overrides_a_bad_motorway_snap() {
        // On the sidewalk beside a highway: 7 m off, real step cadence. The
        // motorway prior must not claim this as Driving.
        let walking = accel_window(2.0, 1.5, 9.8, 50, 4.0);
        let v = classify(Some(3.0), Some(5.0), Some(&road("motorway", 7.0)), &walking);
        assert_eq!(v.movement, MovementType::Walking);
        // Inside 5 m the counter-signal does not apply — snap is trusted.
        let v = classify(Some(3.0), Some(5.0), Some(&road("motorway", 3.0)), &walking);
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
            classify(Some(3.0), Some(5.0), Some(&road("motorway", 7.0)), &weak).movement,
            MovementType::Driving
        );
        // Plenty of steps but a 4 Hz cadence is outside the 1..=3 Hz stride
        // band, so it is not walking evidence either.
        let too_fast = AccelStats { dominant_frequency: 4.0, step_count: 20, ..AccelStats::EMPTY };
        assert_eq!(
            classify(Some(3.0), Some(5.0), Some(&road("motorway", 7.0)), &too_fast).movement,
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
        let v = classify(Some(12.0), Some(5.0), Some(&road("motorway", 7.0)), &walking);
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
        let mut now = feed(&mut d, MovementType::Walking, 80, 0);
        now = feed(&mut d, MovementType::Driving, 4, now); // 4 votes, 10 s apart below
        assert_eq!(d.current(), MovementType::Walking, "not yet: 3 s of evidence");
        // Same four votes spread over 30 s, then Stationary votes: Driving is
        // still the window majority long enough to clear the 15 s latency.
        let mut d = VoteDebouncer::new(DebounceConfig::default());
        now = feed(&mut d, MovementType::Walking, 80, 0);
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
