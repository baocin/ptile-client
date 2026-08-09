//! Platform-neutral adaptive sensor sampling.
//!
//! This module decides *what evidence should be collected next*. It never
//! opens a sensor, owns a clock, starts a thread, or calls a platform service.
//! A host feeds observations into [`AdaptiveMotionSession`] and translates the
//! returned [`SamplingAdvice`] into Core Location, Android location/sensor
//! APIs, browser APIs, a desktop service, or any other adapter.

use crate::{
    classify_with_history, AccelStats, DebounceConfig, MotionClassifier, MotionConfig,
    MovementType, RoadContext, TimedFix, TrafficControl, Vote, VoteDebouncer,
};
use ptiles_core::Fix;

/// Relative sampling intensity. The names describe intent, not a particular
/// platform API or accuracy constant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum SamplingLevel {
    Off,
    Passive,
    Low,
    Balanced,
    High,
    Burst,
}

/// Why the current advice was selected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
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

/// The application's purpose for collecting motion. This is deliberately
/// generic: all adapters can express these intents even though their sensor
/// APIs differ.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum SamplingIntent {
    /// Power-sensitive background classification.
    #[default]
    Background,
    /// Recording a track or workout.
    Tracking,
    /// Active turn-by-turn or off-route navigation.
    Navigation,
}

/// What the motion engine would like the host to collect next.
///
/// Intervals and rates are requests. The host remains authoritative: it may
/// clamp or reject them because of permissions, battery policy, lifecycle, or
/// hardware limits, then report the result through [`AppliedSampling`].
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SamplingAdvice {
    pub location_level: SamplingLevel,
    pub location_interval_ms: Option<u32>,
    pub location_min_distance_m: Option<f64>,
    pub accelerometer_level: SamplingLevel,
    pub accelerometer_hz: Option<u32>,
    pub accelerometer_window_ms: Option<u32>,
    /// Maximum time to keep a burst before reevaluating it.
    pub burst_duration_ms: Option<u32>,
    /// The host should call `tick` no later than this, even if no sensor event
    /// arrives first.
    pub reevaluate_after_ms: u32,
    pub reason: SamplingReason,
    /// Increments only when hardware-relevant settings change.
    pub generation: u32,
    pub limited_by_capabilities: bool,
}

impl SamplingAdvice {
    fn same_hardware_request(&self, other: &Self) -> bool {
        self.location_level == other.location_level
            && self.location_interval_ms == other.location_interval_ms
            && self.location_min_distance_m == other.location_min_distance_m
            && self.accelerometer_level == other.accelerometer_level
            && self.accelerometer_hz == other.accelerometer_hz
            && self.accelerometer_window_ms == other.accelerometer_window_ms
            && self.burst_duration_ms == other.burst_duration_ms
    }

    fn intensity(&self) -> SamplingLevel {
        self.location_level.max(self.accelerometer_level)
    }
}

/// Sensor features and rate limits available to a particular host.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
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
        Self {
            location_available: true,
            accelerometer_available: true,
            supports_passive_location: true,
            supports_motion_wakeup: true,
            minimum_location_interval_ms: None,
            maximum_accelerometer_hz: None,
        }
    }
}

/// What the host actually configured after interpreting an advice record.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AppliedSampling {
    pub location_level: SamplingLevel,
    pub location_interval_ms: Option<u32>,
    pub accelerometer_level: SamplingLevel,
    pub accelerometer_hz: Option<u32>,
    /// Advice generation this acknowledges.
    pub generation: u32,
}

/// Cross-platform policy tuning. Rates are intentionally centralized here so
/// adapters do not grow their own copies of the policy.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
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
        Self {
            uncertain_location_interval_ms: 1_000,
            stationary_location_interval_ms: 60_000,
            walking_location_interval_ms: 5_000,
            running_location_interval_ms: 2_000,
            driving_location_interval_ms: 2_000,
            stationary_min_distance_m: 25.0,
            walking_min_distance_m: 5.0,
            running_min_distance_m: 3.0,
            driving_min_distance_m: 8.0,
            uncertain_accelerometer_hz: 50,
            stationary_accelerometer_hz: 5,
            walking_accelerometer_hz: 20,
            running_accelerometer_hz: 25,
            driving_accelerometer_hz: 10,
            accelerometer_window_ms: 4_000,
            transition_burst_ms: 10_000,
            downshift_hold_ms: 15_000,
            advice_ttl_ms: 10_000,
            confidence_gate: 0.60,
        }
    }
}

/// Configuration for the composed classifier and sampling controller.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct AdaptiveMotionConfig {
    pub motion: MotionConfig,
    pub debounce: DebounceConfig,
    pub sampling: SamplingConfig,
}

/// One optional platform location observation.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct LocationSample {
    pub lat: f64,
    pub lon: f64,
    pub horizontal_accuracy_m: Option<f64>,
    pub speed_mps: Option<f64>,
    pub bearing_degrees: Option<f64>,
}

/// All evidence available at one caller-supplied monotonic timestamp.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MotionObservation {
    pub t_ms: u64,
    pub location: Option<LocationSample>,
    pub accelerometer: Option<AccelStats>,
    pub road: Option<RoadContext>,
    pub traffic_control: Option<TrafficControl>,
}

/// Result of observing evidence or reevaluating a deadline.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AdaptiveMotionUpdate {
    pub movement: MovementType,
    pub vote: Vote,
    pub smoothed_speed_mps: Option<f64>,
    pub at_traffic_control: bool,
    pub sampling: SamplingAdvice,
    /// True only when the host may need to reconfigure hardware.
    pub sampling_changed: bool,
}

#[derive(Clone, Copy, Debug)]
struct Evidence {
    has_location: bool,
    has_accelerometer: bool,
    poor_location_accuracy: bool,
}

/// Stateful policy mechanism. It is useful independently when an application
/// already owns its classifier and only wants sampling recommendations.
#[derive(Clone, Debug)]
pub struct AdaptiveSampler {
    cfg: SamplingConfig,
    capabilities: SamplingCapabilities,
    intent: SamplingIntent,
    advice: SamplingAdvice,
    last_increase_ms: u64,
    burst_started_ms: Option<u64>,
    burst_exhausted: bool,
    last_applied: Option<AppliedSampling>,
    last_movement: MovementType,
    last_vote: Vote,
    last_evidence: Evidence,
}

impl AdaptiveSampler {
    pub fn new(cfg: SamplingConfig, capabilities: SamplingCapabilities) -> Self {
        let mut sampler = Self {
            cfg,
            capabilities,
            intent: SamplingIntent::Background,
            advice: SamplingAdvice {
                location_level: SamplingLevel::Off,
                location_interval_ms: None,
                location_min_distance_m: None,
                accelerometer_level: SamplingLevel::Off,
                accelerometer_hz: None,
                accelerometer_window_ms: None,
                burst_duration_ms: None,
                reevaluate_after_ms: cfg.advice_ttl_ms.max(1),
                reason: SamplingReason::Initializing,
                generation: 0,
                limited_by_capabilities: false,
            },
            last_increase_ms: 0,
            // Construction has no caller clock. The first `observe`/`tick`
            // starts this deadline in the caller's monotonic time domain.
            burst_started_ms: None,
            burst_exhausted: false,
            last_applied: None,
            last_movement: MovementType::Unknown,
            last_vote: Vote {
                movement: MovementType::Unknown,
                confidence: 0.0,
            },
            last_evidence: Evidence {
                has_location: false,
                has_accelerometer: false,
                poor_location_accuracy: false,
            },
        };
        let desired = sampler.profile(MovementType::Unknown, SamplingReason::Initializing);
        sampler.advice = sampler.limit(desired);
        sampler.advice.generation = 1;
        sampler
    }

    pub fn current_advice(&self) -> SamplingAdvice {
        self.advice
    }

    pub fn capabilities(&self) -> SamplingCapabilities {
        self.capabilities
    }

    pub fn set_capabilities(&mut self, capabilities: SamplingCapabilities, now_ms: u64) -> bool {
        if !self.capabilities.accelerometer_available && capabilities.accelerometer_available {
            self.burst_exhausted = false;
            self.burst_started_ms = None;
        }
        self.capabilities = capabilities;
        self.update_policy(
            self.last_movement,
            self.last_vote,
            self.last_evidence,
            now_ms,
            false,
        )
    }

    pub fn intent(&self) -> SamplingIntent {
        self.intent
    }

    pub fn set_intent(&mut self, intent: SamplingIntent, now_ms: u64) -> bool {
        self.intent = intent;
        self.update(
            self.last_movement,
            self.last_vote,
            self.last_evidence,
            now_ms,
        )
    }

    pub fn report_applied(&mut self, applied: AppliedSampling) {
        self.last_applied = Some(applied);
    }

    pub fn last_applied(&self) -> Option<AppliedSampling> {
        self.last_applied
    }

    fn update(
        &mut self,
        movement: MovementType,
        vote: Vote,
        evidence: Evidence,
        now_ms: u64,
    ) -> bool {
        self.update_policy(movement, vote, evidence, now_ms, true)
    }

    fn update_policy(
        &mut self,
        movement: MovementType,
        vote: Vote,
        evidence: Evidence,
        now_ms: u64,
        hold_downshift: bool,
    ) -> bool {
        self.last_movement = movement;
        self.last_vote = vote;
        self.last_evidence = evidence;
        let reason = if !evidence.has_location && !evidence.has_accelerometer {
            SamplingReason::Initializing
        } else if evidence.poor_location_accuracy && !evidence.has_accelerometer {
            SamplingReason::PoorLocationAccuracy
        } else if movement == MovementType::Unknown {
            SamplingReason::Initializing
        } else if vote.movement != movement {
            SamplingReason::PendingTransition
        } else if vote.confidence < self.cfg.confidence_gate {
            SamplingReason::LowConfidence
        } else if !evidence.has_location {
            SamplingReason::MissingLocation
        } else if !evidence.has_accelerometer
            && matches!(movement, MovementType::Walking | MovementType::Running)
        {
            SamplingReason::MissingAccelerometer
        } else {
            stable_reason(movement)
        };

        let desired_movement = if matches!(
            reason,
            SamplingReason::Initializing
                | SamplingReason::PendingTransition
                | SamplingReason::LowConfidence
                | SamplingReason::PoorLocationAccuracy
                | SamplingReason::MissingLocation
                | SamplingReason::MissingAccelerometer
        ) {
            MovementType::Unknown
        } else {
            movement
        };
        let desired = self.profile(desired_movement, reason);
        let desired = self.apply_intent(desired);
        let requests_burst = desired.accelerometer_level == SamplingLevel::Burst;
        if !requests_burst {
            self.burst_exhausted = false;
            self.burst_started_ms = None;
        } else if !self.burst_exhausted && self.burst_started_ms.is_none() {
            self.burst_started_ms = Some(now_ms);
        }
        let burst_expired = requests_burst
            && !self.burst_exhausted
            && self.burst_started_ms.is_some_and(|started| {
                now_ms.saturating_sub(started) >= self.cfg.transition_burst_ms as u64
            });
        if burst_expired {
            self.burst_exhausted = true;
        }
        let desired = self.cap_burst(desired, now_ms);
        let desired = self.limit(desired);
        self.commit(desired, now_ms, hold_downshift && !burst_expired)
    }

    fn cap_burst(&self, mut advice: SamplingAdvice, now_ms: u64) -> SamplingAdvice {
        if advice.accelerometer_level == SamplingLevel::Burst
            && (self.burst_exhausted
                || self.burst_started_ms.is_some_and(|started| {
                    now_ms.saturating_sub(started) >= self.cfg.transition_burst_ms as u64
                }))
        {
            advice.accelerometer_level = SamplingLevel::High;
            advice.burst_duration_ms = None;
            advice.reevaluate_after_ms = self.cfg.advice_ttl_ms.max(1);
        }
        advice
    }

    fn profile(&self, movement: MovementType, reason: SamplingReason) -> SamplingAdvice {
        let (location_level, location_interval_ms, min_distance, accel_level, accel_hz, burst) =
            match movement {
                MovementType::Stationary => (
                    SamplingLevel::Passive,
                    self.cfg.stationary_location_interval_ms,
                    self.cfg.stationary_min_distance_m,
                    if self.capabilities.supports_motion_wakeup {
                        SamplingLevel::Passive
                    } else {
                        SamplingLevel::Low
                    },
                    self.cfg.stationary_accelerometer_hz,
                    None,
                ),
                MovementType::Walking => (
                    SamplingLevel::Balanced,
                    self.cfg.walking_location_interval_ms,
                    self.cfg.walking_min_distance_m,
                    SamplingLevel::Balanced,
                    self.cfg.walking_accelerometer_hz,
                    None,
                ),
                MovementType::Running => (
                    SamplingLevel::High,
                    self.cfg.running_location_interval_ms,
                    self.cfg.running_min_distance_m,
                    SamplingLevel::High,
                    self.cfg.running_accelerometer_hz,
                    None,
                ),
                MovementType::Driving => (
                    SamplingLevel::High,
                    self.cfg.driving_location_interval_ms,
                    self.cfg.driving_min_distance_m,
                    SamplingLevel::Low,
                    self.cfg.driving_accelerometer_hz,
                    None,
                ),
                MovementType::Unknown => (
                    SamplingLevel::High,
                    self.cfg.uncertain_location_interval_ms,
                    0.0,
                    SamplingLevel::Burst,
                    self.cfg.uncertain_accelerometer_hz,
                    Some(self.cfg.transition_burst_ms),
                ),
            };
        SamplingAdvice {
            location_level,
            location_interval_ms: Some(location_interval_ms.max(1)),
            location_min_distance_m: Some(min_distance),
            accelerometer_level: accel_level,
            accelerometer_hz: Some(accel_hz.max(1)),
            accelerometer_window_ms: Some(self.cfg.accelerometer_window_ms.max(1)),
            burst_duration_ms: burst,
            reevaluate_after_ms: burst
                .unwrap_or(self.cfg.advice_ttl_ms)
                .min(self.cfg.advice_ttl_ms.max(1))
                .max(1),
            reason,
            generation: self.advice.generation,
            limited_by_capabilities: false,
        }
    }

    fn apply_intent(&self, mut advice: SamplingAdvice) -> SamplingAdvice {
        match self.intent {
            SamplingIntent::Background => {}
            SamplingIntent::Tracking => {
                advice.location_level = advice.location_level.max(SamplingLevel::Balanced);
                advice.location_interval_ms =
                    Some(advice.location_interval_ms.unwrap_or(5_000).min(5_000));
                advice.accelerometer_level = advice.accelerometer_level.max(SamplingLevel::Low);
            }
            SamplingIntent::Navigation => {
                advice.location_level = advice.location_level.max(SamplingLevel::High);
                advice.location_interval_ms =
                    Some(advice.location_interval_ms.unwrap_or(2_000).min(2_000));
                advice.location_min_distance_m =
                    Some(advice.location_min_distance_m.unwrap_or(5.0).min(5.0));
            }
        }
        advice
    }

    fn limit(&self, mut advice: SamplingAdvice) -> SamplingAdvice {
        let mut limited = false;
        if !self.capabilities.location_available {
            limited |= advice.location_level != SamplingLevel::Off;
            advice.location_level = SamplingLevel::Off;
            advice.location_interval_ms = None;
            advice.location_min_distance_m = None;
        } else {
            if advice.location_level == SamplingLevel::Passive
                && !self.capabilities.supports_passive_location
            {
                advice.location_level = SamplingLevel::Low;
                limited = true;
            }
            if let (Some(requested), Some(minimum)) = (
                advice.location_interval_ms,
                self.capabilities.minimum_location_interval_ms,
            ) {
                if requested < minimum {
                    advice.location_interval_ms = Some(minimum);
                    limited = true;
                }
            }
        }
        if !self.capabilities.accelerometer_available {
            limited |= advice.accelerometer_level != SamplingLevel::Off;
            advice.accelerometer_level = SamplingLevel::Off;
            advice.accelerometer_hz = None;
            advice.accelerometer_window_ms = None;
            advice.burst_duration_ms = None;
        } else if let (Some(requested), Some(maximum)) = (
            advice.accelerometer_hz,
            self.capabilities.maximum_accelerometer_hz,
        ) {
            if requested > maximum {
                advice.accelerometer_hz = Some(maximum);
                limited = true;
            }
        }
        if limited {
            advice.limited_by_capabilities = true;
            advice.reason = SamplingReason::CapabilityLimited;
        }
        advice
    }

    fn commit(&mut self, mut desired: SamplingAdvice, now_ms: u64, hold_downshift: bool) -> bool {
        let old_intensity = self.advice.intensity();
        let new_intensity = desired.intensity();
        if hold_downshift
            && new_intensity < old_intensity
            && now_ms.saturating_sub(self.last_increase_ms) < self.cfg.downshift_hold_ms as u64
        {
            let remaining =
                self.cfg.downshift_hold_ms as u64 - now_ms.saturating_sub(self.last_increase_ms);
            self.advice.reevaluate_after_ms = remaining.min(u32::MAX as u64) as u32;
            self.advice.reason = desired.reason;
            return false;
        }
        if new_intensity > old_intensity {
            self.last_increase_ms = now_ms;
        }
        let changed = !self.advice.same_hardware_request(&desired);
        if desired.accelerometer_level == SamplingLevel::Burst
            && self.advice.accelerometer_level != SamplingLevel::Burst
        {
            self.burst_started_ms = Some(now_ms);
        } else if desired.accelerometer_level != SamplingLevel::Burst {
            self.burst_started_ms = None;
        }
        if changed {
            desired.generation = self.advice.generation.wrapping_add(1).max(1);
        } else {
            desired.generation = self.advice.generation;
        }
        self.advice = desired;
        changed
    }
}

fn stable_reason(movement: MovementType) -> SamplingReason {
    match movement {
        MovementType::Unknown => SamplingReason::Initializing,
        MovementType::Stationary => SamplingReason::StableStationary,
        MovementType::Walking => SamplingReason::StableWalking,
        MovementType::Running => SamplingReason::StableRunning,
        MovementType::Driving => SamplingReason::StableDriving,
    }
}

/// Complete portable motion pipeline: speed smoothing, per-observation vote,
/// transition debouncing, and adaptive sensor advice.
#[derive(Clone, Debug)]
pub struct AdaptiveMotionSession {
    cfg: AdaptiveMotionConfig,
    speed: MotionClassifier,
    debouncer: VoteDebouncer,
    sampler: AdaptiveSampler,
    last_vote: Vote,
    last_evidence: Evidence,
    last_at_traffic_control: bool,
}

impl AdaptiveMotionSession {
    pub fn new(config: AdaptiveMotionConfig, capabilities: SamplingCapabilities) -> Self {
        Self {
            cfg: config,
            speed: MotionClassifier::new(config.motion),
            debouncer: VoteDebouncer::new(config.debounce),
            sampler: AdaptiveSampler::new(config.sampling, capabilities),
            last_vote: Vote {
                movement: MovementType::Unknown,
                confidence: 0.0,
            },
            last_evidence: Evidence {
                has_location: false,
                has_accelerometer: false,
                poor_location_accuracy: false,
            },
            last_at_traffic_control: false,
        }
    }

    pub fn observe(&mut self, observation: MotionObservation) -> AdaptiveMotionUpdate {
        let location = observation.location;
        if let Some(sample) = location.filter(valid_location) {
            self.speed.push(TimedFix::new(
                Fix {
                    lat: sample.lat,
                    lon: sample.lon,
                    horizontal_accuracy_m: sample.horizontal_accuracy_m.unwrap_or(f64::INFINITY),
                    speed_mps: sample.speed_mps,
                },
                observation.t_ms,
            ));
        }
        let effective_speed = location
            .and_then(|sample| sample.speed_mps.filter(|s| s.is_finite() && *s >= 0.0))
            .or_else(|| self.speed.smoothed_speed_mps());
        let accuracy = location.and_then(|sample| sample.horizontal_accuracy_m);
        let bearing = location.and_then(|sample| sample.bearing_degrees);

        let no_evidence =
            location.is_none() && observation.accelerometer.is_none() && observation.road.is_none();
        self.last_vote = if no_evidence {
            Vote {
                movement: MovementType::Unknown,
                confidence: 0.0,
            }
        } else {
            classify_with_history(
                effective_speed,
                accuracy,
                observation.road.as_ref(),
                observation.accelerometer.as_ref(),
                bearing,
                self.debouncer.current(),
            )
        };
        let movement = self.debouncer.tick_at(
            &self.last_vote,
            observation.t_ms,
            observation.traffic_control.as_ref(),
        );
        self.last_at_traffic_control = observation
            .traffic_control
            .is_some_and(|control| control.holds_traffic(self.cfg.debounce.signal_radius_m));
        self.last_evidence = Evidence {
            has_location: location.is_some(),
            has_accelerometer: observation.accelerometer.is_some(),
            poor_location_accuracy: accuracy
                .is_some_and(|a| !a.is_finite() || a > self.cfg.motion.accuracy_gate_m),
        };
        let changed = self.sampler.update(
            movement,
            self.last_vote,
            self.last_evidence,
            observation.t_ms,
        );
        self.update(changed)
    }

    /// Reevaluate the current policy when its deadline fires, without
    /// inventing a sensor observation.
    pub fn tick(&mut self, now_ms: u64) -> AdaptiveMotionUpdate {
        let changed = self.sampler.update(
            self.debouncer.current(),
            self.last_vote,
            self.last_evidence,
            now_ms,
        );
        self.update(changed)
    }

    pub fn current_advice(&self) -> SamplingAdvice {
        self.sampler.current_advice()
    }

    pub fn movement(&self) -> MovementType {
        self.debouncer.current()
    }

    pub fn set_capabilities(&mut self, capabilities: SamplingCapabilities, now_ms: u64) -> bool {
        self.sampler.set_capabilities(capabilities, now_ms)
    }

    pub fn set_intent(&mut self, intent: SamplingIntent, now_ms: u64) -> bool {
        self.sampler.set_intent(intent, now_ms)
    }

    pub fn report_applied_sampling(&mut self, applied: AppliedSampling) {
        self.sampler.report_applied(applied);
    }

    pub fn last_applied_sampling(&self) -> Option<AppliedSampling> {
        self.sampler.last_applied()
    }

    pub fn reset(&mut self) {
        let capabilities = self.sampler.capabilities();
        let intent = self.sampler.intent();
        *self = Self::new(self.cfg, capabilities);
        self.sampler.intent = intent;
    }

    fn update(&self, sampling_changed: bool) -> AdaptiveMotionUpdate {
        AdaptiveMotionUpdate {
            movement: self.debouncer.current(),
            vote: self.last_vote,
            smoothed_speed_mps: self.speed.smoothed_speed_mps(),
            at_traffic_control: self.last_at_traffic_control,
            sampling: self.sampler.current_advice(),
            sampling_changed,
        }
    }
}

fn valid_location(sample: &LocationSample) -> bool {
    sample.lat.is_finite()
        && sample.lon.is_finite()
        && (-90.0..=90.0).contains(&sample.lat)
        && (-180.0..=180.0).contains(&sample.lon)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn location(speed_mps: Option<f64>, accuracy: f64) -> LocationSample {
        LocationSample {
            lat: 36.1627,
            lon: -86.7816,
            horizontal_accuracy_m: Some(accuracy),
            speed_mps,
            bearing_degrees: Some(90.0),
        }
    }

    fn observation(t_ms: u64, speed_mps: Option<f64>) -> MotionObservation {
        MotionObservation {
            t_ms,
            location: Some(location(speed_mps, 5.0)),
            accelerometer: Some(AccelStats {
                variance: 0.01,
                mean_magnitude: Some(9.81),
                dominant_frequency: 0.0,
                step_count: 0,
                window_duration_s: Some(4.0),
            }),
            road: None,
            traffic_control: None,
        }
    }

    #[test]
    fn initial_advice_requests_evidence() {
        let session = AdaptiveMotionSession::new(
            AdaptiveMotionConfig::default(),
            SamplingCapabilities::default(),
        );
        let advice = session.current_advice();
        assert_eq!(advice.location_level, SamplingLevel::High);
        assert_eq!(advice.accelerometer_level, SamplingLevel::Burst);
        assert_eq!(advice.reason, SamplingReason::Initializing);
    }

    #[test]
    fn stable_driving_requests_high_location_not_accel_burst() {
        let cfg = AdaptiveMotionConfig {
            debounce: DebounceConfig {
                majority_window: 1,
                rapid_latency_ms: 0,
                default_latency_ms: 0,
                min_continuous: 1,
                ..DebounceConfig::default()
            },
            sampling: SamplingConfig {
                downshift_hold_ms: 0,
                ..SamplingConfig::default()
            },
            ..AdaptiveMotionConfig::default()
        };
        let mut session = AdaptiveMotionSession::new(cfg, SamplingCapabilities::default());
        let update = session.observe(observation(1_000, Some(18.0)));
        assert_eq!(update.movement, MovementType::Driving);
        assert_eq!(update.sampling.reason, SamplingReason::StableDriving);
        assert_eq!(update.sampling.location_level, SamplingLevel::High);
        assert_eq!(update.sampling.accelerometer_level, SamplingLevel::Low);
        assert!(update.sampling_changed);
    }

    #[test]
    fn disagreement_bursts_and_then_holds_downshift() {
        let mut sampler = AdaptiveSampler::new(
            SamplingConfig {
                downshift_hold_ms: 5_000,
                ..SamplingConfig::default()
            },
            SamplingCapabilities::default(),
        );
        let evidence = Evidence {
            has_location: true,
            has_accelerometer: true,
            poor_location_accuracy: false,
        };
        sampler.update(
            MovementType::Walking,
            Vote {
                movement: MovementType::Running,
                confidence: 0.9,
            },
            evidence,
            1_000,
        );
        assert_eq!(
            sampler.current_advice().accelerometer_level,
            SamplingLevel::Burst
        );
        let changed = sampler.update(
            MovementType::Walking,
            Vote {
                movement: MovementType::Walking,
                confidence: 0.9,
            },
            evidence,
            2_000,
        );
        assert!(!changed);
        assert_eq!(
            sampler.current_advice().accelerometer_level,
            SamplingLevel::Burst
        );
        assert!(sampler.update(
            MovementType::Walking,
            Vote {
                movement: MovementType::Walking,
                confidence: 0.9
            },
            evidence,
            7_000,
        ));
        assert_eq!(
            sampler.current_advice().accelerometer_level,
            SamplingLevel::Balanced
        );
    }

    #[test]
    fn capabilities_clamp_or_disable_requests() {
        let capabilities = SamplingCapabilities {
            location_available: true,
            accelerometer_available: false,
            supports_passive_location: false,
            supports_motion_wakeup: false,
            minimum_location_interval_ms: Some(4_000),
            maximum_accelerometer_hz: None,
        };
        let session = AdaptiveMotionSession::new(AdaptiveMotionConfig::default(), capabilities);
        let advice = session.current_advice();
        assert_eq!(advice.location_interval_ms, Some(4_000));
        assert_eq!(advice.accelerometer_level, SamplingLevel::Off);
        assert_eq!(advice.accelerometer_hz, None);
        assert!(advice.limited_by_capabilities);
        assert_eq!(advice.reason, SamplingReason::CapabilityLimited);
    }

    #[test]
    fn capability_changes_take_effect_immediately_and_can_recover() {
        let mut sampler = AdaptiveSampler::new(
            SamplingConfig::default(),
            SamplingCapabilities {
                location_available: false,
                accelerometer_available: false,
                ..SamplingCapabilities::default()
            },
        );
        assert_eq!(sampler.current_advice().location_level, SamplingLevel::Off);
        assert!(sampler.set_capabilities(SamplingCapabilities::default(), 50_000));
        assert_eq!(sampler.current_advice().location_level, SamplingLevel::High);
        assert_eq!(
            sampler.current_advice().accelerometer_level,
            SamplingLevel::Burst
        );
        assert!(sampler.set_capabilities(
            SamplingCapabilities {
                location_available: false,
                accelerometer_available: false,
                ..SamplingCapabilities::default()
            },
            50_001,
        ));
        assert_eq!(sampler.current_advice().location_level, SamplingLevel::Off);
    }

    #[test]
    fn uncertain_accelerometer_burst_has_a_hard_duration() {
        let mut session = AdaptiveMotionSession::new(
            AdaptiveMotionConfig::default(),
            SamplingCapabilities::default(),
        );
        session.tick(1_000);
        assert_eq!(
            session.current_advice().accelerometer_level,
            SamplingLevel::Burst
        );
        let update = session.tick(11_000);
        assert_eq!(update.sampling.accelerometer_level, SamplingLevel::High);
        assert_eq!(update.sampling.burst_duration_ms, None);
        assert!(update.sampling_changed);
        assert_eq!(
            session.tick(12_000).sampling.accelerometer_level,
            SamplingLevel::High,
            "the same uncertainty episode must not immediately re-arm its burst"
        );
    }

    #[test]
    fn applied_sampling_round_trips_without_owning_hardware() {
        let mut session = AdaptiveMotionSession::new(
            AdaptiveMotionConfig::default(),
            SamplingCapabilities::default(),
        );
        let applied = AppliedSampling {
            location_level: SamplingLevel::Balanced,
            location_interval_ms: Some(5_000),
            accelerometer_level: SamplingLevel::Low,
            accelerometer_hz: Some(10),
            generation: session.current_advice().generation,
        };
        session.report_applied_sampling(applied);
        assert_eq!(session.last_applied_sampling(), Some(applied));
    }

    #[test]
    fn no_observation_is_unknown_instead_of_fake_stationary() {
        let cfg = AdaptiveMotionConfig {
            debounce: DebounceConfig {
                majority_window: 1,
                rapid_latency_ms: 0,
                default_latency_ms: 0,
                min_continuous: 1,
                ..DebounceConfig::default()
            },
            ..AdaptiveMotionConfig::default()
        };
        let mut session = AdaptiveMotionSession::new(cfg, SamplingCapabilities::default());
        let update = session.observe(MotionObservation {
            t_ms: 1,
            location: None,
            accelerometer: None,
            road: None,
            traffic_control: None,
        });
        assert_eq!(update.vote.movement, MovementType::Unknown);
        assert_eq!(update.movement, MovementType::Unknown);
    }
}
