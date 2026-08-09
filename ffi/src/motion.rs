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
use ptiles_motion::{
    AdaptiveMotionConfig as CoreAdaptiveMotionConfig,
    AdaptiveMotionSession as CoreAdaptiveMotionSession,
    AdaptiveMotionUpdate as CoreAdaptiveMotionUpdate, AppliedSampling as CoreAppliedSampling,
    LocationSample as CoreLocationSample, MotionConfig as CoreMotionConfig,
    MotionObservation as CoreMotionObservation, SamplingAdvice as CoreSamplingAdvice,
    SamplingCapabilities as CoreSamplingCapabilities, SamplingConfig as CoreSamplingConfig,
    SamplingIntent as CoreSamplingIntent, SamplingLevel as CoreSamplingLevel,
    SamplingReason as CoreSamplingReason,
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

// --- Adaptive cross-platform sampling -------------------------------------

/// Relative sensor intensity requested by PTiles Motion. Platform adapters map
/// this intent onto their own location and sensor services.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum SamplingLevel {
    Off,
    Passive,
    Low,
    Balanced,
    High,
    Burst,
}

impl From<CoreSamplingLevel> for SamplingLevel {
    fn from(value: CoreSamplingLevel) -> Self {
        match value {
            CoreSamplingLevel::Off => Self::Off,
            CoreSamplingLevel::Passive => Self::Passive,
            CoreSamplingLevel::Low => Self::Low,
            CoreSamplingLevel::Balanced => Self::Balanced,
            CoreSamplingLevel::High => Self::High,
            CoreSamplingLevel::Burst => Self::Burst,
        }
    }
}

impl From<SamplingLevel> for CoreSamplingLevel {
    fn from(value: SamplingLevel) -> Self {
        match value {
            SamplingLevel::Off => Self::Off,
            SamplingLevel::Passive => Self::Passive,
            SamplingLevel::Low => Self::Low,
            SamplingLevel::Balanced => Self::Balanced,
            SamplingLevel::High => Self::High,
            SamplingLevel::Burst => Self::Burst,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum SamplingReason {
    Initializing,
    StableStationary,
    StableWalking,
    StableRunning,
    StableDriving,
    PendingTransition,
    LowConfidence,
    PoorLocationAccuracy,
    MissingLocation,
    MissingAccelerometer,
    CapabilityLimited,
}

impl From<CoreSamplingReason> for SamplingReason {
    fn from(value: CoreSamplingReason) -> Self {
        match value {
            CoreSamplingReason::Initializing => Self::Initializing,
            CoreSamplingReason::StableStationary => Self::StableStationary,
            CoreSamplingReason::StableWalking => Self::StableWalking,
            CoreSamplingReason::StableRunning => Self::StableRunning,
            CoreSamplingReason::StableDriving => Self::StableDriving,
            CoreSamplingReason::PendingTransition => Self::PendingTransition,
            CoreSamplingReason::LowConfidence => Self::LowConfidence,
            CoreSamplingReason::PoorLocationAccuracy => Self::PoorLocationAccuracy,
            CoreSamplingReason::MissingLocation => Self::MissingLocation,
            CoreSamplingReason::MissingAccelerometer => Self::MissingAccelerometer,
            CoreSamplingReason::CapabilityLimited => Self::CapabilityLimited,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum SamplingIntent {
    Background,
    Tracking,
    Navigation,
}

impl From<CoreSamplingIntent> for SamplingIntent {
    fn from(value: CoreSamplingIntent) -> Self {
        match value {
            CoreSamplingIntent::Background => Self::Background,
            CoreSamplingIntent::Tracking => Self::Tracking,
            CoreSamplingIntent::Navigation => Self::Navigation,
        }
    }
}

impl From<SamplingIntent> for CoreSamplingIntent {
    fn from(value: SamplingIntent) -> Self {
        match value {
            SamplingIntent::Background => Self::Background,
            SamplingIntent::Tracking => Self::Tracking,
            SamplingIntent::Navigation => Self::Navigation,
        }
    }
}

#[derive(Clone, Copy, Debug, uniffi::Record)]
pub struct SamplingAdvice {
    pub location_level: SamplingLevel,
    pub location_interval_ms: Option<u32>,
    pub location_min_distance_m: Option<f64>,
    pub accelerometer_level: SamplingLevel,
    pub accelerometer_hz: Option<u32>,
    pub accelerometer_window_ms: Option<u32>,
    pub burst_duration_ms: Option<u32>,
    pub reevaluate_after_ms: u32,
    pub reason: SamplingReason,
    pub generation: u32,
    pub limited_by_capabilities: bool,
}

impl From<CoreSamplingAdvice> for SamplingAdvice {
    fn from(v: CoreSamplingAdvice) -> Self {
        Self {
            location_level: v.location_level.into(),
            location_interval_ms: v.location_interval_ms,
            location_min_distance_m: v.location_min_distance_m,
            accelerometer_level: v.accelerometer_level.into(),
            accelerometer_hz: v.accelerometer_hz,
            accelerometer_window_ms: v.accelerometer_window_ms,
            burst_duration_ms: v.burst_duration_ms,
            reevaluate_after_ms: v.reevaluate_after_ms,
            reason: v.reason.into(),
            generation: v.generation,
            limited_by_capabilities: v.limited_by_capabilities,
        }
    }
}

#[derive(Clone, Copy, Debug, uniffi::Record)]
pub struct SamplingCapabilities {
    pub location_available: bool,
    pub accelerometer_available: bool,
    pub supports_passive_location: bool,
    pub supports_motion_wakeup: bool,
    pub minimum_location_interval_ms: Option<u32>,
    pub maximum_accelerometer_hz: Option<u32>,
}

impl Default for SamplingCapabilities {
    fn default() -> Self {
        CoreSamplingCapabilities::default().into()
    }
}

impl From<CoreSamplingCapabilities> for SamplingCapabilities {
    fn from(v: CoreSamplingCapabilities) -> Self {
        Self {
            location_available: v.location_available,
            accelerometer_available: v.accelerometer_available,
            supports_passive_location: v.supports_passive_location,
            supports_motion_wakeup: v.supports_motion_wakeup,
            minimum_location_interval_ms: v.minimum_location_interval_ms,
            maximum_accelerometer_hz: v.maximum_accelerometer_hz,
        }
    }
}

impl From<SamplingCapabilities> for CoreSamplingCapabilities {
    fn from(v: SamplingCapabilities) -> Self {
        Self {
            location_available: v.location_available,
            accelerometer_available: v.accelerometer_available,
            supports_passive_location: v.supports_passive_location,
            supports_motion_wakeup: v.supports_motion_wakeup,
            minimum_location_interval_ms: v.minimum_location_interval_ms,
            maximum_accelerometer_hz: v.maximum_accelerometer_hz,
        }
    }
}

#[derive(Clone, Copy, Debug, uniffi::Record)]
pub struct AppliedSampling {
    pub location_level: SamplingLevel,
    pub location_interval_ms: Option<u32>,
    pub accelerometer_level: SamplingLevel,
    pub accelerometer_hz: Option<u32>,
    pub generation: u32,
}

impl From<AppliedSampling> for CoreAppliedSampling {
    fn from(v: AppliedSampling) -> Self {
        Self {
            location_level: v.location_level.into(),
            location_interval_ms: v.location_interval_ms,
            accelerometer_level: v.accelerometer_level.into(),
            accelerometer_hz: v.accelerometer_hz,
            generation: v.generation,
        }
    }
}

impl From<CoreAppliedSampling> for AppliedSampling {
    fn from(v: CoreAppliedSampling) -> Self {
        Self {
            location_level: v.location_level.into(),
            location_interval_ms: v.location_interval_ms,
            accelerometer_level: v.accelerometer_level.into(),
            accelerometer_hz: v.accelerometer_hz,
            generation: v.generation,
        }
    }
}

#[derive(Clone, Copy, Debug, uniffi::Record)]
pub struct MotionConfig {
    pub stationary_max_mps: f64,
    pub driving_min_mps: f64,
    pub smoothing_window: u32,
    pub min_dwell_samples: u32,
    pub accuracy_gate_m: f64,
    pub max_gap_ms: u64,
}

impl Default for MotionConfig {
    fn default() -> Self {
        CoreMotionConfig::default().into()
    }
}

impl From<CoreMotionConfig> for MotionConfig {
    fn from(v: CoreMotionConfig) -> Self {
        Self {
            stationary_max_mps: v.stationary_max_mps,
            driving_min_mps: v.driving_min_mps,
            smoothing_window: v.smoothing_window as u32,
            min_dwell_samples: v.min_dwell_samples,
            accuracy_gate_m: v.accuracy_gate_m,
            max_gap_ms: v.max_gap_ms,
        }
    }
}

impl From<MotionConfig> for CoreMotionConfig {
    fn from(v: MotionConfig) -> Self {
        Self {
            stationary_max_mps: v.stationary_max_mps,
            driving_min_mps: v.driving_min_mps,
            smoothing_window: v.smoothing_window as usize,
            min_dwell_samples: v.min_dwell_samples,
            accuracy_gate_m: v.accuracy_gate_m,
            max_gap_ms: v.max_gap_ms,
        }
    }
}

#[derive(Clone, Copy, Debug, uniffi::Record)]
pub struct SamplingConfig {
    pub uncertain_location_interval_ms: u32,
    pub stationary_location_interval_ms: u32,
    pub walking_location_interval_ms: u32,
    pub running_location_interval_ms: u32,
    pub driving_location_interval_ms: u32,
    pub stationary_min_distance_m: f64,
    pub walking_min_distance_m: f64,
    pub running_min_distance_m: f64,
    pub driving_min_distance_m: f64,
    pub uncertain_accelerometer_hz: u32,
    pub stationary_accelerometer_hz: u32,
    pub walking_accelerometer_hz: u32,
    pub running_accelerometer_hz: u32,
    pub driving_accelerometer_hz: u32,
    pub accelerometer_window_ms: u32,
    pub transition_burst_ms: u32,
    pub downshift_hold_ms: u32,
    pub advice_ttl_ms: u32,
    pub confidence_gate: f64,
}

impl Default for SamplingConfig {
    fn default() -> Self {
        CoreSamplingConfig::default().into()
    }
}

impl From<CoreSamplingConfig> for SamplingConfig {
    fn from(v: CoreSamplingConfig) -> Self {
        Self {
            uncertain_location_interval_ms: v.uncertain_location_interval_ms,
            stationary_location_interval_ms: v.stationary_location_interval_ms,
            walking_location_interval_ms: v.walking_location_interval_ms,
            running_location_interval_ms: v.running_location_interval_ms,
            driving_location_interval_ms: v.driving_location_interval_ms,
            stationary_min_distance_m: v.stationary_min_distance_m,
            walking_min_distance_m: v.walking_min_distance_m,
            running_min_distance_m: v.running_min_distance_m,
            driving_min_distance_m: v.driving_min_distance_m,
            uncertain_accelerometer_hz: v.uncertain_accelerometer_hz,
            stationary_accelerometer_hz: v.stationary_accelerometer_hz,
            walking_accelerometer_hz: v.walking_accelerometer_hz,
            running_accelerometer_hz: v.running_accelerometer_hz,
            driving_accelerometer_hz: v.driving_accelerometer_hz,
            accelerometer_window_ms: v.accelerometer_window_ms,
            transition_burst_ms: v.transition_burst_ms,
            downshift_hold_ms: v.downshift_hold_ms,
            advice_ttl_ms: v.advice_ttl_ms,
            confidence_gate: v.confidence_gate,
        }
    }
}

impl From<SamplingConfig> for CoreSamplingConfig {
    fn from(v: SamplingConfig) -> Self {
        Self {
            uncertain_location_interval_ms: v.uncertain_location_interval_ms,
            stationary_location_interval_ms: v.stationary_location_interval_ms,
            walking_location_interval_ms: v.walking_location_interval_ms,
            running_location_interval_ms: v.running_location_interval_ms,
            driving_location_interval_ms: v.driving_location_interval_ms,
            stationary_min_distance_m: v.stationary_min_distance_m,
            walking_min_distance_m: v.walking_min_distance_m,
            running_min_distance_m: v.running_min_distance_m,
            driving_min_distance_m: v.driving_min_distance_m,
            uncertain_accelerometer_hz: v.uncertain_accelerometer_hz,
            stationary_accelerometer_hz: v.stationary_accelerometer_hz,
            walking_accelerometer_hz: v.walking_accelerometer_hz,
            running_accelerometer_hz: v.running_accelerometer_hz,
            driving_accelerometer_hz: v.driving_accelerometer_hz,
            accelerometer_window_ms: v.accelerometer_window_ms,
            transition_burst_ms: v.transition_burst_ms,
            downshift_hold_ms: v.downshift_hold_ms,
            advice_ttl_ms: v.advice_ttl_ms,
            confidence_gate: v.confidence_gate,
        }
    }
}

#[derive(Clone, Copy, Debug, uniffi::Record)]
pub struct AdaptiveMotionConfig {
    pub motion: MotionConfig,
    pub debounce: DebounceConfig,
    pub sampling: SamplingConfig,
}

impl Default for AdaptiveMotionConfig {
    fn default() -> Self {
        CoreAdaptiveMotionConfig::default().into()
    }
}

impl From<CoreAdaptiveMotionConfig> for AdaptiveMotionConfig {
    fn from(v: CoreAdaptiveMotionConfig) -> Self {
        Self { motion: v.motion.into(), debounce: v.debounce.into(), sampling: v.sampling.into() }
    }
}

impl From<AdaptiveMotionConfig> for CoreAdaptiveMotionConfig {
    fn from(v: AdaptiveMotionConfig) -> Self {
        Self { motion: v.motion.into(), debounce: v.debounce.into(), sampling: v.sampling.into() }
    }
}

#[derive(Clone, Copy, Debug, uniffi::Record)]
pub struct LocationSample {
    pub lat: f64,
    pub lon: f64,
    pub horizontal_accuracy_m: Option<f64>,
    pub speed_mps: Option<f64>,
    pub bearing_degrees: Option<f64>,
}

impl From<LocationSample> for CoreLocationSample {
    fn from(v: LocationSample) -> Self {
        Self {
            lat: v.lat,
            lon: v.lon,
            horizontal_accuracy_m: v.horizontal_accuracy_m,
            speed_mps: v.speed_mps,
            bearing_degrees: v.bearing_degrees,
        }
    }
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct MotionObservation {
    pub t_ms: u64,
    pub location: Option<LocationSample>,
    pub accelerometer: Option<AccelStats>,
    pub road: Option<RoadContext>,
    pub traffic_control: Option<TrafficControl>,
}

impl From<MotionObservation> for CoreMotionObservation {
    fn from(v: MotionObservation) -> Self {
        Self {
            t_ms: v.t_ms,
            location: v.location.map(Into::into),
            accelerometer: v.accelerometer.map(Into::into),
            road: v.road.map(Into::into),
            traffic_control: v.traffic_control.map(Into::into),
        }
    }
}

#[derive(Clone, Copy, Debug, uniffi::Record)]
pub struct AdaptiveMotionUpdate {
    pub movement: MovementType,
    pub vote: Vote,
    pub smoothed_speed_mps: Option<f64>,
    pub at_traffic_control: bool,
    pub sampling: SamplingAdvice,
    pub sampling_changed: bool,
}

impl From<CoreAdaptiveMotionUpdate> for AdaptiveMotionUpdate {
    fn from(v: CoreAdaptiveMotionUpdate) -> Self {
        Self {
            movement: v.movement.into(),
            vote: v.vote.into(),
            smoothed_speed_mps: v.smoothed_speed_mps,
            at_traffic_control: v.at_traffic_control,
            sampling: v.sampling.into(),
            sampling_changed: v.sampling_changed,
        }
    }
}

#[uniffi::export]
pub fn default_adaptive_motion_config() -> AdaptiveMotionConfig {
    AdaptiveMotionConfig::default()
}

#[uniffi::export]
pub fn default_sampling_capabilities() -> SamplingCapabilities {
    SamplingCapabilities::default()
}

/// Stateful, hardware-neutral pipeline. Calls return sampling advice; Kotlin,
/// Swift, desktop, or other adapters decide how to apply it and may emit their
/// own native callback/stream after `sampling_changed` becomes true.
#[derive(uniffi::Object)]
pub struct AdaptiveMotionSession {
    inner: Mutex<CoreAdaptiveMotionSession>,
}

#[uniffi::export]
impl AdaptiveMotionSession {
    #[uniffi::constructor]
    pub fn new(config: AdaptiveMotionConfig, capabilities: SamplingCapabilities) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(CoreAdaptiveMotionSession::new(config.into(), capabilities.into())),
        })
    }

    pub fn observe(&self, observation: MotionObservation) -> AdaptiveMotionUpdate {
        self.inner
            .lock()
            .expect("adaptive motion lock")
            .observe(observation.into())
            .into()
    }

    pub fn tick(&self, now_ms: u64) -> AdaptiveMotionUpdate {
        self.inner.lock().expect("adaptive motion lock").tick(now_ms).into()
    }

    pub fn current_advice(&self) -> SamplingAdvice {
        self.inner.lock().expect("adaptive motion lock").current_advice().into()
    }

    pub fn movement(&self) -> MovementType {
        self.inner.lock().expect("adaptive motion lock").movement().into()
    }

    pub fn set_capabilities(&self, capabilities: SamplingCapabilities, now_ms: u64) -> bool {
        self.inner
            .lock()
            .expect("adaptive motion lock")
            .set_capabilities(capabilities.into(), now_ms)
    }

    pub fn set_intent(&self, intent: SamplingIntent, now_ms: u64) -> bool {
        self.inner.lock().expect("adaptive motion lock").set_intent(intent.into(), now_ms)
    }

    pub fn report_applied_sampling(&self, applied: AppliedSampling) {
        self.inner
            .lock()
            .expect("adaptive motion lock")
            .report_applied_sampling(applied.into());
    }

    pub fn last_applied_sampling(&self) -> Option<AppliedSampling> {
        self.inner
            .lock()
            .expect("adaptive motion lock")
            .last_applied_sampling()
            .map(Into::into)
    }

    pub fn reset(&self) {
        self.inner.lock().expect("adaptive motion lock").reset();
    }
}

#[cfg(test)]
mod adaptive_tests {
    use super::*;

    #[test]
    fn uniffi_session_returns_actionable_sampling_advice() {
        let session = AdaptiveMotionSession::new(
            default_adaptive_motion_config(),
            default_sampling_capabilities(),
        );
        let initial = session.current_advice();
        assert_eq!(initial.location_level, SamplingLevel::High);
        assert_eq!(initial.accelerometer_level, SamplingLevel::Burst);

        let update = session.observe(MotionObservation {
            t_ms: 50_000,
            location: Some(LocationSample {
                lat: 36.1627,
                lon: -86.7816,
                horizontal_accuracy_m: Some(5.0),
                speed_mps: Some(15.0),
                bearing_degrees: Some(90.0),
            }),
            accelerometer: None,
            road: None,
            traffic_control: None,
        });
        assert!(update.sampling.location_interval_ms.is_some());
        assert!(update.sampling.generation >= initial.generation);
    }

    #[test]
    fn uniffi_capability_and_applied_feedback_round_trip() {
        let session = AdaptiveMotionSession::new(
            default_adaptive_motion_config(),
            SamplingCapabilities {
                location_available: false,
                accelerometer_available: true,
                supports_passive_location: false,
                supports_motion_wakeup: false,
                minimum_location_interval_ms: None,
                maximum_accelerometer_hz: Some(12),
            },
        );
        let advice = session.current_advice();
        assert_eq!(advice.location_level, SamplingLevel::Off);
        assert_eq!(advice.accelerometer_hz, Some(12));
        let applied = AppliedSampling {
            location_level: SamplingLevel::Off,
            location_interval_ms: None,
            accelerometer_level: SamplingLevel::Low,
            accelerometer_hz: Some(10),
            generation: advice.generation,
        };
        session.report_applied_sampling(applied);
        let stored = session.last_applied_sampling().expect("applied feedback");
        assert_eq!(stored.generation, advice.generation);
        assert_eq!(stored.accelerometer_hz, Some(10));
    }
}
