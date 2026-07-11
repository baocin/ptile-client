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

/// Coarse motion state. `Unknown` until enough evidence accumulates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MotionState {
    Unknown,
    Stationary,
    Walking,
    Driving,
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
/// the current [`MotionState`].
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
    state: MotionState,
    /// Candidate band and how many consecutive samples have agreed on it.
    pending: Option<(MotionState, u32)>,
}

impl MotionClassifier {
    pub fn new(cfg: MotionConfig) -> Self {
        MotionClassifier {
            cfg,
            speeds: VecDeque::new(),
            last: None,
            state: MotionState::Unknown,
            pending: None,
        }
    }

    /// Current classification.
    pub fn state(&self) -> MotionState {
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
        self.state = MotionState::Unknown;
    }

    /// Ingest one fix and return the (possibly updated) state.
    pub fn push(&mut self, f: TimedFix) -> MotionState {
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
    /// fix, non-monotonic time, or a gap beyond `max_gap_ms` — the latter also
    /// resets the smoothing window so stale speeds don't linger).
    fn effective_speed(&mut self, f: &TimedFix) -> Option<f64> {
        if let Some(s) = f.fix.speed_mps {
            if s.is_finite() && s >= 0.0 {
                return Some(s);
            }
        }
        let prev = self.last?;
        let dt_ms = f.t_ms.checked_sub(prev.t_ms);
        match dt_ms {
            // Non-monotonic (time went backwards or stood still): unusable.
            None | Some(0) => None,
            Some(dt) if dt > self.cfg.max_gap_ms => {
                // Stale: derived speed would be meaningless and the window is
                // no longer representative. Reset smoothing, keep no speed.
                self.speeds.clear();
                self.pending = None;
                None
            }
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

    /// Raw band for a smoothed speed, before debouncing.
    fn band(&self, v: f64) -> MotionState {
        if v <= self.cfg.stationary_max_mps {
            MotionState::Stationary
        } else if v >= self.cfg.driving_min_mps {
            MotionState::Driving
        } else {
            MotionState::Walking
        }
    }

    /// Dwell-based state machine: a new band must persist for
    /// `min_dwell_samples` consecutive samples before it becomes the state.
    fn apply_transition(&mut self, target: MotionState) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn fix(lat: f64, lon: f64, speed_mps: Option<f64>) -> Fix {
        Fix { lat, lon, horizontal_accuracy_m: 5.0, speed_mps }
    }

    /// Feed `n` fixes at the same spot with an explicit platform speed, 1 s
    /// apart, starting at `t0`. Returns the final state.
    fn feed_speed(c: &mut MotionClassifier, speed: f64, n: usize, t0: u64) -> MotionState {
        let mut last = c.state();
        for i in 0..n {
            last = c.push(TimedFix::new(fix(36.16, -86.79, Some(speed)), t0 + i as u64 * 1000));
        }
        last
    }

    #[test]
    fn empty_and_single_fix_are_unknown() {
        let mut c = MotionClassifier::new(MotionConfig::default());
        assert_eq!(c.state(), MotionState::Unknown);
        // A single fix with no platform speed can't derive one: stays Unknown.
        c.push(TimedFix::new(fix(36.16, -86.79, None), 1000));
        assert_eq!(c.state(), MotionState::Unknown);
        assert_eq!(c.smoothed_speed_mps(), None);
    }

    #[test]
    fn platform_speed_classifies_each_band() {
        let mut c = MotionClassifier::new(MotionConfig::default());
        assert_eq!(feed_speed(&mut c, 0.0, 3, 0), MotionState::Stationary);

        let mut c = MotionClassifier::new(MotionConfig::default());
        assert_eq!(feed_speed(&mut c, 1.4, 3, 0), MotionState::Walking);

        let mut c = MotionClassifier::new(MotionConfig::default());
        assert_eq!(feed_speed(&mut c, 15.0, 3, 0), MotionState::Driving);
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
        assert_eq!(c.state(), MotionState::Stationary);
        // Start walking.
        for _ in 0..4 {
            c.push(TimedFix::new(fix(36.16, -86.79, Some(1.5)), t));
            t += 1000;
        }
        assert_eq!(c.state(), MotionState::Walking);
        // Get in a car.
        for _ in 0..6 {
            c.push(TimedFix::new(fix(36.16, -86.79, Some(13.0)), t));
            t += 1000;
        }
        assert_eq!(c.state(), MotionState::Driving);
        // Park and stop.
        for _ in 0..6 {
            c.push(TimedFix::new(fix(36.16, -86.79, Some(0.0)), t));
            t += 1000;
        }
        assert_eq!(c.state(), MotionState::Stationary);
    }

    #[test]
    fn single_outlier_does_not_flip_state() {
        // Settle into Stationary, then one lone fast sample: min_dwell=2 and
        // the smoothing window both prevent a flip to Driving.
        let mut c = MotionClassifier::new(MotionConfig::default());
        feed_speed(&mut c, 0.0, 5, 0);
        assert_eq!(c.state(), MotionState::Stationary);
        c.push(TimedFix::new(fix(36.16, -86.79, Some(40.0)), 5000));
        assert_eq!(c.state(), MotionState::Stationary, "one outlier must not flip state");
    }

    #[test]
    fn speed_derived_from_positions_when_absent() {
        // No platform speed; ~13.9 m/s eastward => Driving. At lat 36.16,
        // 1e-4 deg lon ~= 9.0 m; step 0.00015 deg/s ~= 13.5 m/s.
        let cfg = MotionConfig::default();
        let mut c = MotionClassifier::new(cfg);
        let mut lon = -86.79;
        let mut t = 0u64;
        let mut state = MotionState::Unknown;
        for _ in 0..6 {
            state = c.push(TimedFix::new(fix(36.16, lon, None), t));
            lon += 0.00015;
            t += 1000;
        }
        assert_eq!(state, MotionState::Driving);
        assert!(c.smoothed_speed_mps().unwrap() > 5.0);
    }

    #[test]
    fn low_accuracy_fixes_are_ignored() {
        let mut c = MotionClassifier::new(MotionConfig::default());
        feed_speed(&mut c, 0.0, 3, 0);
        assert_eq!(c.state(), MotionState::Stationary);
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
        assert_eq!(c.state(), MotionState::Driving);
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
        assert_eq!(s, MotionState::Unknown);
        assert_eq!(c.smoothed_speed_mps(), None);
    }

    #[test]
    fn reset_clears_state() {
        let mut c = MotionClassifier::new(MotionConfig::default());
        feed_speed(&mut c, 12.0, 5, 0);
        assert_eq!(c.state(), MotionState::Driving);
        c.reset();
        assert_eq!(c.state(), MotionState::Unknown);
        assert_eq!(c.smoothed_speed_mps(), None);
    }
}
