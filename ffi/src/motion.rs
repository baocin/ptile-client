//! UniFFI bindings over `ptiles-motion`.
//!
//! Movement classification was the one part of this library a mobile caller
//! could see the shape of but not call: `ptiles-motion` has carried
//! `classify`, `AccelStats` and `VoteDebouncer` since it was extracted, and the
//! FFI exposed none of them. So every integration re-implemented the same
//! decision tree in Kotlin or Swift, against a copy of the thresholds, and the
//! copies drifted -- one of them fixed a misclassification the library still
//! had. That is the cost this module removes: one implementation, one set of
//! constants, and a bug fixed once.
//!
//! Shapes are records rather than opaque objects wherever the Rust type is a
//! plain data carrier, so a caller builds an `AccelStats` from its own sensor
//! window without a round trip. `VoteDebouncer` stays an object because it is
//! stateful and its state is the whole point.
//!
//! Deliberately NOT exposed: `MotionClassifier` and `significant_shifts`. Both
//! are useful and neither is what a caller replacing a hand-written classifier
//! needs first; adding them later is additive, and a binding surface is much
//! easier to grow than to shrink.

use std::sync::Arc;
use std::sync::Mutex;

use ptiles_motion::movement::{
    self, AccelStats as CoreAccelStats, DebounceConfig as CoreDebounceConfig,
    MovementType as CoreMovementType, RoadContext as CoreRoadContext,
    TrafficControl as CoreTrafficControl,
    Vote as CoreVote, VoteDebouncer as CoreVoteDebouncer,
};

/// What the classifier thinks is happening.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum MovementType {
    Unknown,
    Stationary,
    Walking,
    Running,
    Driving,
}

impl From<CoreMovementType> for MovementType {
    fn from(value: CoreMovementType) -> Self {
        match value {
            CoreMovementType::Unknown => MovementType::Unknown,
            CoreMovementType::Stationary => MovementType::Stationary,
            CoreMovementType::Walking => MovementType::Walking,
            CoreMovementType::Running => MovementType::Running,
            CoreMovementType::Driving => MovementType::Driving,
        }
    }
}

impl From<MovementType> for CoreMovementType {
    fn from(value: MovementType) -> Self {
        match value {
            MovementType::Unknown => CoreMovementType::Unknown,
            MovementType::Stationary => CoreMovementType::Stationary,
            MovementType::Walking => CoreMovementType::Walking,
            MovementType::Running => CoreMovementType::Running,
            MovementType::Driving => CoreMovementType::Driving,
        }
    }
}

/// One classifier output: a type plus how much the evidence is worth.
#[derive(Clone, Copy, Debug, uniffi::Record)]
pub struct Vote {
    pub movement: MovementType,
    pub confidence: f64,
}

impl From<CoreVote> for Vote {
    fn from(value: CoreVote) -> Self {
        Vote { movement: value.movement.into(), confidence: value.confidence }
    }
}

/// A window of accelerometer statistics.
///
/// `mean_magnitude` and `window_duration_s` are optional because a producer
/// that does not measure them must be able to say so: a zero there is a
/// measurement, and absence is not.
#[derive(Clone, Copy, Debug, uniffi::Record)]
pub struct AccelStats {
    /// Variance of the magnitude series, (m/s^2)^2.
    pub variance: f64,
    /// Mean magnitude, m/s^2.
    pub mean_magnitude: Option<f64>,
    /// Step cadence, Hz.
    pub dominant_frequency: f64,
    pub step_count: u32,
    /// Window length, seconds.
    pub window_duration_s: Option<f64>,
}

impl From<CoreAccelStats> for AccelStats {
    fn from(v: CoreAccelStats) -> Self {
        AccelStats {
            variance: v.variance,
            mean_magnitude: v.mean_magnitude,
            dominant_frequency: v.dominant_frequency,
            step_count: v.step_count,
            window_duration_s: v.window_duration_s,
        }
    }
}

impl From<AccelStats> for CoreAccelStats {
    fn from(v: AccelStats) -> Self {
        CoreAccelStats {
            variance: v.variance,
            mean_magnitude: v.mean_magnitude,
            dominant_frequency: v.dominant_frequency,
            step_count: v.step_count,
            window_duration_s: v.window_duration_s,
        }
    }
}

/// Nearest-road prior for a fix.
#[derive(Clone, Debug, uniffi::Record)]
pub struct RoadContext {
    /// OSM `highway` tag: "motorway", "footway", "residential", ...
    pub road_class: String,
    /// Fix to nearest road, meters.
    pub distance_m: f64,
    /// Bearing of the road at the snapped point, degrees. `None` when the caller
    /// cannot compute it -- which is a different fact from a road running due
    /// north, so it is not defaulted to zero.
    pub bearing: Option<f64>,
}

impl From<RoadContext> for CoreRoadContext {
    fn from(v: RoadContext) -> Self {
        CoreRoadContext { road_class: v.road_class, distance_m: v.distance_m, bearing: v.bearing }
    }
}

/// The nearest mapped node a vehicle might be waiting at.
///
/// Only ever EXTENDS the vehicle-sticky window, and only while the fix is still
/// at it. That is the whole reason it exists: a car idling at a signal looks
/// identical to a parked car, and only the map can tell them apart.
#[derive(Clone, Copy, Debug, uniffi::Record)]
pub struct TrafficControl {
    /// Fix to the intersection node, meters.
    pub distance_m: f64,
    /// 1 = traffic_signals, 2 = stop, 3 = give_way, 4 = roundabout;
    /// 0 or anything else is an untyped junction, which does not hold traffic.
    pub intersection_type: u8,
}

impl From<TrafficControl> for CoreTrafficControl {
    fn from(v: TrafficControl) -> Self {
        CoreTrafficControl { distance_m: v.distance_m, intersection_type: v.intersection_type }
    }
}

/// Thresholds the library classifies against.
///
/// Exposed as data rather than left as Rust constants so a caller stops keeping
/// its own copy. Every one of these was duplicated in at least one integration,
/// which is how a threshold and its meaning drift apart.
#[derive(Clone, Copy, Debug, uniffi::Record)]
pub struct MovementThresholds {
    /// Above this speed (m/s) nothing is walking.
    pub walking_ceiling_mps: f64,
    /// At or above this speed (m/s) a fix reads as driving.
    pub driving_floor_mps: f64,
    /// Above this horizontal accuracy (m) GPS position is not trusted.
    pub gps_accuracy_gate_m: f64,
    /// Where a person would draw the walking/running line on a speed chart.
    /// A labelling aid for UIs, never a classifier threshold.
    pub running_speed_hint_mps: f64,
}

/// The thresholds this build classifies against.
#[uniffi::export]
pub fn movement_thresholds() -> MovementThresholds {
    MovementThresholds {
        walking_ceiling_mps: movement::WALKING_CEILING_MPS,
        driving_floor_mps: movement::DRIVING_FLOOR_MPS,
        gps_accuracy_gate_m: movement::GPS_ACCURACY_GATE_M,
        running_speed_hint_mps: movement::RUNNING_SPEED_HINT_MPS,
    }
}

/// Accelerometer statistics for one window of raw samples.
///
/// Takes the three axes a platform actually reports rather than a pre-computed
/// magnitude series, so the windowing rule stays in one place. Returns the
/// empty window when the axes are empty or disagree in length, or when the
/// sample rate is zero -- all of which mean "no measurement", not "zero".
#[uniffi::export]
pub fn accel_stats_from_samples(x: Vec<f32>, y: Vec<f32>, z: Vec<f32>, sample_rate_hz: u32) -> AccelStats {
    CoreAccelStats::calculate(&x, &y, &z, sample_rate_hz).into()
}

/// Stateless single-fix classification.
///
/// Every input is optional because every one is genuinely missing on some real
/// fix. Note that a poor `gps_accuracy_m` suppresses the road and speed priors
/// but does NOT discard a speed clearing the driving floor: an uncertain
/// position is not evidence that 20 m/s was walked.
#[uniffi::export]
pub fn classify_movement(
    inst_speed_mps: Option<f64>,
    gps_accuracy_m: Option<f64>,
    nearest_road: Option<RoadContext>,
    accel: Option<AccelStats>,
) -> Vote {
    let road = nearest_road.map(CoreRoadContext::from);
    let accel = accel.map(CoreAccelStats::from);
    movement::classify(inst_speed_mps, gps_accuracy_m, road.as_ref(), accel.as_ref()).into()
}

/// [`classify_movement`] plus the two inputs only a caller tracking a sequence
/// can supply: which way the fix is travelling, and the last committed state.
///
/// Separate from `classify_movement` so a one-shot caller keeps the shorter
/// call and identical behaviour -- a `None` bearing makes the alignment test
/// inert and an `Unknown` previous state makes the driving-sticky inert.
#[uniffi::export]
pub fn classify_movement_with_history(
    inst_speed_mps: Option<f64>,
    gps_accuracy_m: Option<f64>,
    nearest_road: Option<RoadContext>,
    accel: Option<AccelStats>,
    gps_bearing: Option<f64>,
    previous_stable: MovementType,
) -> Vote {
    let road = nearest_road.map(CoreRoadContext::from);
    let accel = accel.map(CoreAccelStats::from);
    movement::classify_with_history(
        inst_speed_mps,
        gps_accuracy_m,
        road.as_ref(),
        accel.as_ref(),
        gps_bearing,
        previous_stable.into(),
    )
    .into()
}

/// Classification from the accelerometer alone, ignoring GPS entirely.
#[uniffi::export]
pub fn classify_movement_accel_only(accel: AccelStats) -> Vote {
    movement::classify_accel_only(&CoreAccelStats::from(accel)).into()
}

/// Tuning for [`VoteDebouncer`].
#[derive(Clone, Copy, Debug, uniffi::Record)]
pub struct DebounceConfig {
    /// Votes kept in the majority window.
    pub majority_window: u32,
    /// Latency into `Driving`, ms.
    pub rapid_latency_ms: u64,
    /// Latency for every other transition, ms.
    pub default_latency_ms: u64,
    /// After a `Driving` vote, how long (ms) a flip to `Stationary` is
    /// suppressed -- a red light is not an arrival.
    pub vehicle_sticky_ms: u64,
    /// Sticky window (ms) used instead of `vehicle_sticky_ms` at a mapped
    /// traffic control, where a queue can hold a car far longer.
    pub signal_sticky_ms: u64,
    /// How close (m) a traffic control counts as "waiting at it".
    pub signal_radius_m: f64,
    /// Consecutive agreeing majorities required before a transition commits.
    pub min_continuous: u32,
}

impl Default for DebounceConfig {
    fn default() -> Self {
        CoreDebounceConfig::default().into()
    }
}

impl From<CoreDebounceConfig> for DebounceConfig {
    fn from(v: CoreDebounceConfig) -> Self {
        DebounceConfig {
            majority_window: v.majority_window as u32,
            rapid_latency_ms: v.rapid_latency_ms,
            default_latency_ms: v.default_latency_ms,
            vehicle_sticky_ms: v.vehicle_sticky_ms,
            signal_sticky_ms: v.signal_sticky_ms,
            signal_radius_m: v.signal_radius_m,
            min_continuous: v.min_continuous,
        }
    }
}

impl From<DebounceConfig> for CoreDebounceConfig {
    fn from(v: DebounceConfig) -> Self {
        CoreDebounceConfig {
            majority_window: v.majority_window as usize,
            rapid_latency_ms: v.rapid_latency_ms,
            default_latency_ms: v.default_latency_ms,
            vehicle_sticky_ms: v.vehicle_sticky_ms,
            signal_sticky_ms: v.signal_sticky_ms,
            signal_radius_m: v.signal_radius_m,
            min_continuous: v.min_continuous,
        }
    }
}

/// The library's default debounce tuning.
#[uniffi::export]
pub fn default_debounce_config() -> DebounceConfig {
    DebounceConfig::default()
}

/// Turns a stream of per-fix votes into a stable movement state.
///
/// Opaque and stateful: a majority window, per-transition latency and the
/// vehicle sticky window all depend on what came before, which is exactly the
/// part a caller should not be reimplementing.
///
/// Interior-mutable because UniFFI hands out `Arc<Self>` and a caller ticks it
/// from whichever thread its location callback arrives on.
#[derive(uniffi::Object)]
pub struct VoteDebouncer {
    inner: Mutex<CoreVoteDebouncer>,
}

#[uniffi::export]
impl VoteDebouncer {
    #[uniffi::constructor]
    pub fn new(config: DebounceConfig) -> Arc<Self> {
        Arc::new(VoteDebouncer { inner: Mutex::new(CoreVoteDebouncer::new(config.into())) })
    }

    /// Feed one vote and read back the state after it.
    ///
    /// `now_ms` is caller-supplied and must be monotonic; the library holds no
    /// clock so that replaying a recorded trace produces the same states it
    /// produced live.
    pub fn tick(&self, vote: Vote, now_ms: u64) -> MovementType {
        self.tick_at(vote, now_ms, None)
    }

    /// Feed one vote plus the nearest mapped traffic control to the fix.
    ///
    /// The control only extends the vehicle-sticky window; it never suppresses
    /// a transition plain `tick` would have allowed.
    pub fn tick_at(&self, vote: Vote, now_ms: u64, control: Option<TrafficControl>) -> MovementType {
        let core = CoreVote { movement: vote.movement.into(), confidence: vote.confidence };
        let control = control.map(CoreTrafficControl::from);
        self.inner
            .lock()
            .expect("debouncer lock")
            .tick_at(&core, now_ms, control.as_ref())
            .into()
    }

    /// The committed state, without feeding anything.
    pub fn current(&self) -> MovementType {
        self.inner.lock().expect("debouncer lock").current().into()
    }

    /// Drop the vehicle-sticky guard so the next Stationary majority commits
    /// without waiting the sticky window out.
    ///
    /// For a caller holding evidence the sticky no longer applies -- the fix is
    /// inside a known place, say. A red light is not inside your house.
    pub fn clear_vehicle_sticky(&self) {
        self.inner.lock().expect("debouncer lock").clear_vehicle_sticky();
    }

    /// The tuning this debouncer was built with.
    pub fn config(&self) -> DebounceConfig {
        self.inner.lock().expect("debouncer lock").config().into()
    }
}

