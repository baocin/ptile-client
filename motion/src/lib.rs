//! ptiles-motion: stateful GPS motion classification (stationary / walking /
//! driving) layered on top of `ptiles-core`'s stateless single-fix surface.
//!
//! `ptiles-core` is deliberately stateless: `score_candidates` takes one
//! `Fix` and its `Fix` carries no timestamp (see `core/src/scoring.rs`). Motion
//! classification is inherently *temporal* — it needs a sequence of
//! timestamped fixes and retained state — so it lives here in its own crate
//! rather than distorting core's contract. The one thing this crate feeds back
//! into core is a smoothed `speed_mps`: populate `Fix.speed_mps` from
//! [`MotionClassifier::smoothed_speed_mps`] before calling
//! `score_candidates` and its existing binary road/stationary gate
//! (`road_speed_gate_mps`) gets a denoised speed instead of a single noisy
//! instantaneous reading. This crate does NOT change any core scoring
//! semantics.
//!
//! `no_std + alloc`, matching core. History is a bounded `VecDeque`; no `std`
//! collections, no clock (the caller supplies monotonic timestamps).

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::collections::VecDeque;

use ptiles_core::{haversine_distance_m, Fix};

// Re-exported because a `TimedFix` cannot be built without it: a caller that
// only wants motion classification should not have to name ptiles-core.
pub use ptiles_core::Fix as CoreFix;

pub mod movement;
pub mod shifts;
pub use shifts::{significant_shifts, Shift, ShiftConfig};
pub use movement::{
    AccelStats, DebounceConfig, MovementType, RoadContext, TrafficControl, Vote, VoteDebouncer,
    classify, classify_accel_only, DRIVING_FLOOR_MPS, GPS_ACCURACY_GATE_M,
    RUNNING_SPEED_HINT_MPS, WALKING_CEILING_MPS,
};

/// A [`Fix`] stamped with a monotonic millisecond timestamp. Core's `Fix` has
/// no time field (it's stateless); motion needs one to derive speed and gate
/// stale samples, so the timestamp is attached here rather than in core.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimedFix {
    pub fix: Fix,
    /// Monotonic timestamp in milliseconds. Must be non-decreasing across a
    /// classifier's `push` calls; a decrease or a gap larger than
    /// [`MotionConfig::max_gap_ms`] resets the smoothing window.
    pub t_ms: u64,
}

impl TimedFix {
    pub fn new(fix: Fix, t_ms: u64) -> Self {
        TimedFix { fix, t_ms }
    }
}

/// Tunable thresholds for [`MotionClassifier`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MotionConfig {
    /// Smoothed speed at or below this (m/s) is `Stationary`. Default 0.5.
    pub stationary_max_mps: f64,
    /// Smoothed speed at or above this (m/s) is `Driving`; between the two
    /// thresholds is `Walking`. Default 5.0.
    pub driving_min_mps: f64,
    /// Number of most-recent effective speeds averaged into the smoothed
    /// speed. Larger = steadier but laggier. Default 5.
    pub smoothing_window: usize,
    /// Consecutive smoothed samples that must agree on a new band before the
    /// state actually switches — debounces single-sample outliers. Default 2.
    pub min_dwell_samples: u32,
    /// Fixes with `horizontal_accuracy_m` worse than this (m), or non-finite,
    /// are ignored (state unchanged). Default 50.0.
    pub accuracy_gate_m: f64,
    /// A gap larger than this (ms) between consecutive fixes makes any
    /// position-derived speed meaningless and the buffered history stale, so
    /// the smoothing window is reset. Default 30_000 (30 s).
    pub max_gap_ms: u64,
}

impl Default for MotionConfig {
    fn default() -> Self {
        MotionConfig {
            stationary_max_mps: 0.5,
            driving_min_mps: 5.0,
            smoothing_window: 5,
            min_dwell_samples: 2,
            accuracy_gate_m: 50.0,
            max_gap_ms: 30_000,
        }
    }
}

/// Stateful classifier: feed it timestamped fixes with [`push`] and read back
/// the current [`MovementType`].
///
/// [`push`]: MotionClassifier::push
#[derive(Clone, Debug)]
pub struct MotionClassifier {
    cfg: MotionConfig,
    /// Recent effective speeds (m/s), newest at the back, bounded to
    /// `cfg.smoothing_window`.
    speeds: VecDeque<f64>,
    /// Last accepted fix, for position-derived speed on the next sample.
    last: Option<TimedFix>,
    state: MovementType,
    /// Candidate band and how many consecutive samples have agreed on it.
    pending: Option<(MovementType, u32)>,
}

impl MotionClassifier {
    pub fn new(cfg: MotionConfig) -> Self {
        MotionClassifier {
            cfg,
            speeds: VecDeque::new(),
            last: None,
            state: MovementType::Unknown,
            pending: None,
        }
    }

    /// Current classification.
    pub fn state(&self) -> MovementType {
        self.state
    }

    /// Mean of the current smoothing window, or `None` if empty. Suitable for
    /// populating `Fix.speed_mps` before a `score_candidates` call.
    pub fn smoothed_speed_mps(&self) -> Option<f64> {
        if self.speeds.is_empty() {
            return None;
        }
        let sum: f64 = self.speeds.iter().copied().sum();
        Some(sum / self.speeds.len() as f64)
    }

    /// Reset smoothing state (window + pending), keeping config. State is set
    /// to `Unknown`. Called internally on a time gap; also usable by callers
    /// starting a new track.
    pub fn reset(&mut self) {
        self.speeds.clear();
        self.last = None;
        self.pending = None;
        self.state = MovementType::Unknown;
    }

    /// Ingest one fix and return the (possibly updated) state.
    pub fn push(&mut self, f: TimedFix) -> MovementType {
        // 1. Accuracy gate: an imprecise fix tells us little and would inject
        //    a spurious large position delta — ignore it entirely.
        let acc = f.fix.horizontal_accuracy_m;
        if !acc.is_finite() || acc > self.cfg.accuracy_gate_m {
            return self.state;
        }

        // 2. Determine an effective speed for this sample.
        let effective = self.effective_speed(&f);
        // Always advance `last` to the most recent accepted fix.
        self.last = Some(f);
        let Some(speed) = effective else {
            // No usable speed this sample (first fix, or a gap reset) — keep
            // state, wait for the next.
            return self.state;
        };

        // 3. Push into the bounded smoothing window.
        self.speeds.push_back(speed);
        while self.speeds.len() > self.cfg.smoothing_window.max(1) {
            self.speeds.pop_front();
        }

        // 4. Classify the smoothed speed, with dwell-based debouncing.
        let smoothed = self.smoothed_speed_mps().unwrap_or(speed);
        let target = self.band(smoothed);
        self.apply_transition(target);
        self.state
    }

    /// Effective speed for a fix: prefer a valid platform `speed_mps`, else
    /// derive from the displacement since the last accepted fix. Returns
    /// `None` when neither is available or the time delta is unusable (first
    /// fix, non-monotonic time, or a derived speed across a gap beyond
    /// `max_gap_ms`).
    ///
    /// The gap is checked *before* the platform-speed shortcut, not after.
    /// Staleness is a property of the interval since the last fix, not of
    /// where this fix's speed came from: checking it second meant a track fed
    /// platform speeds never reset its window, so after an hour parked the
    /// smoothed speed still averaged in speeds from the trip before.
    fn effective_speed(&mut self, f: &TimedFix) -> Option<f64> {
        let dt_ms = self.last.and_then(|prev| f.t_ms.checked_sub(prev.t_ms));
        if dt_ms.is_some_and(|dt| dt > self.cfg.max_gap_ms) {
            // Stale: the buffered speeds describe an older trip.
            self.speeds.clear();
            self.pending = None;
            // A platform speed still describes *this instant*, so it seeds the
            // fresh window; only a position-derived speed would be measuring
            // across the gap and is dropped.
            return platform_speed(f);
        }
        if let Some(s) = platform_speed(f) {
            return Some(s);
        }
        let prev = self.last?;
        match dt_ms {
            // Non-monotonic (time went backwards or stood still): unusable.
            None | Some(0) => None,
            Some(dt) => {
                let meters = haversine_distance_m(
                    prev.fix.lat,
                    prev.fix.lon,
                    f.fix.lat,
                    f.fix.lon,
                );
                if !meters.is_finite() {
                    return None;
                }
                Some(meters / (dt as f64 / 1000.0))
            }
        }
    }

    /// Which band a smoothed speed falls in, before any debouncing.
    ///
    /// Public because a UI that draws the speed bands must ask the classifier
    /// rather than keep its own copy of the thresholds -- that copy is how a
    /// chart ends up disagreeing with the labels beside it.
    pub fn band_for(&self, smoothed_mps: f64) -> MovementType {
        self.band(smoothed_mps)
    }

    /// Raw band for a smoothed speed, before debouncing (see also
    /// [`platform_speed`] for what counts as a usable reported speed).
    fn band(&self, v: f64) -> MovementType {
        if v <= self.cfg.stationary_max_mps {
            MovementType::Stationary
        } else if v >= self.cfg.driving_min_mps {
            MovementType::Driving
        } else {
            MovementType::Walking
        }
    }

    /// Dwell-based state machine: a new band must persist for
    /// `min_dwell_samples` consecutive samples before it becomes the state.
    fn apply_transition(&mut self, target: MovementType) {
        if target == self.state {
            self.pending = None;
            return;
        }
        let dwell = self.cfg.min_dwell_samples.max(1);
        let count = match self.pending {
            Some((band, n)) if band == target => n + 1,
            _ => 1,
        };
        if count >= dwell {
            self.state = target;
            self.pending = None;
        } else {
            self.pending = Some((target, count));
        }
    }
}

/// The fix's reported speed, if it is one: finite and non-negative. A
/// negative, NaN or infinite reading is a driver artefact, not a measurement,
/// and must fall through to the position-derived path rather than be banded.
fn platform_speed(f: &TimedFix) -> Option<f64> {
    match f.fix.speed_mps {
        Some(s) if s.is_finite() && s >= 0.0 => Some(s),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fix(lat: f64, lon: f64, speed_mps: Option<f64>) -> Fix {
        Fix { lat, lon, horizontal_accuracy_m: 5.0, speed_mps }
    }

    /// Feed `n` fixes at the same spot with an explicit platform speed, 1 s
    /// apart, starting at `t0`. Returns the final state.
    fn feed_speed(c: &mut MotionClassifier, speed: f64, n: usize, t0: u64) -> MovementType {
        let mut last = c.state();
        for i in 0..n {
            last = c.push(TimedFix::new(fix(36.16, -86.79, Some(speed)), t0 + i as u64 * 1000));
        }
        last
    }

    /// A fix `n` metres-ish east of the base point: at lat 36.16 one degree of
    /// longitude is ~899 m, so 1e-5 deg is ~9 cm.
    fn moved_fix(lon_steps: i64, speed_mps: Option<f64>) -> Fix {
        fix(36.16, -86.79 + lon_steps as f64 * 0.00001, speed_mps)
    }

    #[test]
    fn a_gap_resets_the_window_even_when_the_platform_reports_speed() {
        // The bug this pins: the gap check used to sit *after* the
        // platform-speed shortcut, so a track fed platform speeds never reset.
        // Drive at 12 m/s, park for 90 s (> max_gap_ms), then report 0.5 m/s.
        let mut c = MotionClassifier::new(MotionConfig::default());
        feed_speed(&mut c, 12.0, 5, 0);
        assert_eq!(c.smoothed_speed_mps(), Some(12.0));
        c.push(TimedFix::new(fix(36.16, -86.79, Some(0.5)), 4_000 + 90_000));
        assert_eq!(
            c.smoothed_speed_mps(),
            Some(0.5),
            "the window must hold only the post-gap sample, not an average with the old trip"
        );
    }

    #[test]
    fn a_gap_still_drops_a_derived_speed_measured_across_it() {
        // The other half: with no platform speed there is nothing to seed the
        // fresh window with, so the gap leaves it empty rather than dividing a
        // 90 s displacement into a speed.
        let mut c = MotionClassifier::new(MotionConfig::default());
        for i in 0..6 {
            c.push(TimedFix::new(moved_fix(i as i64 * 10, None), i * 1000));
        }
        assert!(c.smoothed_speed_mps().is_some());
        c.push(TimedFix::new(moved_fix(1000, None), 5_000 + 90_000));
        assert_eq!(c.smoothed_speed_mps(), None);
    }

    #[test]
    fn smoothing_window_is_bounded_to_the_configured_size() {
        // 15 slow samples then 5 fast ones: with a window of 5 the slow half
        // must be fully evicted, not averaged in forever.
        let mut c = MotionClassifier::new(MotionConfig::default());
        feed_speed(&mut c, 1.0, 15, 0);
        feed_speed(&mut c, 11.0, 5, 15_000);
        assert_eq!(c.smoothed_speed_mps(), Some(11.0));
        assert_eq!(c.state(), MovementType::Driving);
    }

    #[test]
    fn accuracy_gate_boundary_is_inclusive() {
        // The gate rejects `> accuracy_gate_m`; exactly at it is accepted.
        let mut c = MotionClassifier::new(MotionConfig::default());
        let at_gate = Fix {
            lat: 36.16,
            lon: -86.79,
            horizontal_accuracy_m: MotionConfig::default().accuracy_gate_m,
            speed_mps: Some(12.0),
        };
        for i in 0..3 {
            c.push(TimedFix::new(at_gate, i * 1000));
        }
        assert_eq!(c.state(), MovementType::Driving);
    }

    #[test]
    fn invalid_platform_speed_falls_back_to_positions() {
        // Negative and NaN platform speeds are ignored, not banded. Both of
        // these would read as Stationary if the value were trusted; the real
        // motion is ~9 m/s of displacement.
        for bogus in [Some(-5.0), Some(f64::NAN), Some(f64::INFINITY)] {
            let mut c = MotionClassifier::new(MotionConfig::default());
            for i in 0..6 {
                c.push(TimedFix::new(moved_fix(i as i64 * 10, bogus), i * 1000));
            }
            let smoothed = c.smoothed_speed_mps().expect("derived speed");
            assert!(smoothed > 5.0, "smoothed {smoothed} for platform speed {bogus:?}");
            assert_eq!(c.state(), MovementType::Driving);
        }
    }

    #[test]
    fn gap_exactly_at_the_limit_still_derives_speed() {
        // `max_gap_ms` resets only on a *larger* gap; a fix landing exactly on
        // the limit is still usable.
        let cfg = MotionConfig::default();
        let mut c = MotionClassifier::new(cfg);
        c.push(TimedFix::new(moved_fix(0, None), 0));
        c.push(TimedFix::new(moved_fix(300, None), cfg.max_gap_ms));
        // 300 steps of 1e-5 deg lon at lat 36.16 is ~270 m, over 30 s: ~9 m/s.
        let smoothed = c.smoothed_speed_mps().expect("speed at the gap limit");
        assert!((8.0..10.0).contains(&smoothed), "~9 m/s expected, got {smoothed}");
    }

    #[test]
    fn zero_smoothing_window_is_clamped_to_one() {
        // A window of 0 would divide by zero on the mean; it clamps to 1, i.e.
        // no smoothing at all.
        let cfg = MotionConfig { smoothing_window: 0, ..MotionConfig::default() };
        let mut c = MotionClassifier::new(cfg);
        feed_speed(&mut c, 12.0, 3, 0);
        assert_eq!(c.smoothed_speed_mps(), Some(12.0));
        assert_eq!(c.state(), MovementType::Driving);
    }

    #[test]
    fn dwell_of_one_commits_immediately() {
        let cfg = MotionConfig { min_dwell_samples: 1, smoothing_window: 1, ..MotionConfig::default() };
        let mut c = MotionClassifier::new(cfg);
        assert_eq!(feed_speed(&mut c, 12.0, 1, 0), MovementType::Driving);
    }

    #[test]
    fn band_boundaries_are_inclusive_at_both_ends() {
        // `<= stationary_max` and `>= driving_min` — the thresholds themselves
        // belong to the outer bands, unlike the stateless classifier's `>`.
        let cfg = MotionConfig { min_dwell_samples: 1, smoothing_window: 1, ..MotionConfig::default() };
        let mut c = MotionClassifier::new(cfg);
        assert_eq!(feed_speed(&mut c, cfg.stationary_max_mps, 1, 0), MovementType::Stationary);
        let mut c = MotionClassifier::new(cfg);
        assert_eq!(feed_speed(&mut c, cfg.driving_min_mps, 1, 0), MovementType::Driving);
        // Just inside either threshold is the walking band.
        let mut c = MotionClassifier::new(cfg);
        assert_eq!(feed_speed(&mut c, cfg.stationary_max_mps + 0.01, 1, 0), MovementType::Walking);
    }

    #[test]
    fn state_survives_a_reset_and_reuse() {
        // After a reset the classifier must behave like a fresh one, including
        // needing a second fix before any derived speed exists.
        let mut c = MotionClassifier::new(MotionConfig::default());
        feed_speed(&mut c, 12.0, 5, 0);
        c.reset();
        c.push(TimedFix::new(moved_fix(0, None), 100_000));
        assert_eq!(c.smoothed_speed_mps(), None, "no previous fix to measure against");
        for i in 1..6 {
            c.push(TimedFix::new(moved_fix(i as i64 * 10, None), 100_000 + i * 1000));
        }
        assert_eq!(c.state(), MovementType::Driving);
    }

    #[test]
    fn repeated_timestamps_are_ignored() {
        // Two fixes at the same millisecond would divide by zero.
        let mut c = MotionClassifier::new(MotionConfig::default());
        c.push(TimedFix::new(moved_fix(0, None), 5000));
        let s = c.push(TimedFix::new(moved_fix(100, None), 5000));
        assert_eq!(s, MovementType::Unknown);
        assert_eq!(c.smoothed_speed_mps(), None);
    }

    #[test]
    fn stale_gap_clears_the_window_but_keeps_the_state() {
        // A long gap discards the buffered speeds (they describe an older
        // trip) yet leaves the last known state — the caller has no better
        // answer until fresh fixes arrive.
        let mut c = MotionClassifier::new(MotionConfig::default());
        feed_speed(&mut c, 12.0, 5, 0);
        assert_eq!(c.state(), MovementType::Driving);
        c.push(TimedFix::new(moved_fix(0, None), 5_000 + 90_000));
        assert_eq!(c.smoothed_speed_mps(), None);
        assert_eq!(c.state(), MovementType::Driving, "state persists across a gap");
    }

    #[test]
    fn empty_and_single_fix_are_unknown() {
        let mut c = MotionClassifier::new(MotionConfig::default());
        assert_eq!(c.state(), MovementType::Unknown);
        // A single fix with no platform speed can't derive one: stays Unknown.
        c.push(TimedFix::new(fix(36.16, -86.79, None), 1000));
        assert_eq!(c.state(), MovementType::Unknown);
        assert_eq!(c.smoothed_speed_mps(), None);
    }

    #[test]
    fn platform_speed_classifies_each_band() {
        let mut c = MotionClassifier::new(MotionConfig::default());
        assert_eq!(feed_speed(&mut c, 0.0, 3, 0), MovementType::Stationary);

        let mut c = MotionClassifier::new(MotionConfig::default());
        assert_eq!(feed_speed(&mut c, 1.4, 3, 0), MovementType::Walking);

        let mut c = MotionClassifier::new(MotionConfig::default());
        assert_eq!(feed_speed(&mut c, 15.0, 3, 0), MovementType::Driving);
    }

    #[test]
    fn full_transition_sequence_idle_walk_drive_stop() {
        let mut c = MotionClassifier::new(MotionConfig::default());
        let mut t = 0u64;
        // Idle.
        for _ in 0..3 {
            c.push(TimedFix::new(fix(36.16, -86.79, Some(0.0)), t));
            t += 1000;
        }
        assert_eq!(c.state(), MovementType::Stationary);
        // Start walking.
        for _ in 0..4 {
            c.push(TimedFix::new(fix(36.16, -86.79, Some(1.5)), t));
            t += 1000;
        }
        assert_eq!(c.state(), MovementType::Walking);
        // Get in a car.
        for _ in 0..6 {
            c.push(TimedFix::new(fix(36.16, -86.79, Some(13.0)), t));
            t += 1000;
        }
        assert_eq!(c.state(), MovementType::Driving);
        // Park and stop.
        for _ in 0..6 {
            c.push(TimedFix::new(fix(36.16, -86.79, Some(0.0)), t));
            t += 1000;
        }
        assert_eq!(c.state(), MovementType::Stationary);
    }

    #[test]
    fn single_outlier_does_not_flip_state() {
        // Settle into Stationary, then one lone fast sample: min_dwell=2 and
        // the smoothing window both prevent a flip to Driving.
        let mut c = MotionClassifier::new(MotionConfig::default());
        feed_speed(&mut c, 0.0, 5, 0);
        assert_eq!(c.state(), MovementType::Stationary);
        c.push(TimedFix::new(fix(36.16, -86.79, Some(40.0)), 5000));
        assert_eq!(c.state(), MovementType::Stationary, "one outlier must not flip state");
    }

    #[test]
    fn speed_derived_from_positions_when_absent() {
        // No platform speed; ~13.9 m/s eastward => Driving. At lat 36.16,
        // 1e-4 deg lon ~= 9.0 m; step 0.00015 deg/s ~= 13.5 m/s.
        let cfg = MotionConfig::default();
        let mut c = MotionClassifier::new(cfg);
        let mut lon = -86.79;
        let mut t = 0u64;
        let mut state = MovementType::Unknown;
        for _ in 0..6 {
            state = c.push(TimedFix::new(fix(36.16, lon, None), t));
            lon += 0.00015;
            t += 1000;
        }
        assert_eq!(state, MovementType::Driving);
        assert!(c.smoothed_speed_mps().unwrap() > 5.0);
    }

    #[test]
    fn low_accuracy_fixes_are_ignored() {
        let mut c = MotionClassifier::new(MotionConfig::default());
        feed_speed(&mut c, 0.0, 3, 0);
        assert_eq!(c.state(), MovementType::Stationary);
        // A wildly inaccurate fix (200 m > 50 m gate) must not update state,
        // even though its platform speed says Driving.
        let before = c.state();
        let bad = Fix { lat: 36.16, lon: -86.79, horizontal_accuracy_m: 200.0, speed_mps: Some(30.0) };
        c.push(TimedFix::new(bad, 4000));
        assert_eq!(c.state(), before);
        // Non-finite accuracy is likewise ignored.
        let nan_acc = Fix { lat: 36.16, lon: -86.79, horizontal_accuracy_m: f64::NAN, speed_mps: Some(30.0) };
        c.push(TimedFix::new(nan_acc, 5000));
        assert_eq!(c.state(), before);
    }

    #[test]
    fn large_time_gap_resets_smoothing() {
        let mut c = MotionClassifier::new(MotionConfig::default());
        // Establish a derived-speed history while driving.
        let mut lon = -86.79;
        let mut t = 0u64;
        for _ in 0..6 {
            c.push(TimedFix::new(fix(36.16, lon, None), t));
            lon += 0.00015;
            t += 1000;
        }
        assert_eq!(c.state(), MovementType::Driving);
        // A 60 s gap (> max_gap_ms) then a nearby fix: the derived speed is
        // discarded and the window reset, so smoothed speed drops to empty.
        c.push(TimedFix::new(fix(36.16, lon, None), t + 60_000));
        assert_eq!(c.smoothed_speed_mps(), None, "window should reset after a big gap");
    }

    #[test]
    fn non_monotonic_time_yields_no_derived_speed() {
        let mut c = MotionClassifier::new(MotionConfig::default());
        c.push(TimedFix::new(fix(36.16, -86.79, None), 5000));
        // Time goes backwards: cannot derive speed, state stays Unknown.
        let s = c.push(TimedFix::new(fix(36.161, -86.79, None), 4000));
        assert_eq!(s, MovementType::Unknown);
        assert_eq!(c.smoothed_speed_mps(), None);
    }

    #[test]
    fn reset_clears_state() {
        let mut c = MotionClassifier::new(MotionConfig::default());
        feed_speed(&mut c, 12.0, 5, 0);
        assert_eq!(c.state(), MovementType::Driving);
        c.reset();
        assert_eq!(c.state(), MovementType::Unknown);
        assert_eq!(c.smoothed_speed_mps(), None);
    }
}
