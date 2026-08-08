//! Where the motion actually changed, with a p-value attached.
//!
//! The classifier's transitions come from thresholds and a debouncer: they say
//! "the speed crossed 8.9 m/s and stayed there", which is a decision, not a
//! measurement. This module answers the different question a person labelling a
//! trace is really asking -- *where does this trace stop being one behaviour and
//! start being another?* -- from the data alone, with no thresholds and no
//! movement vocabulary involved.
//!
//! Method: at every candidate index, Welch's t-test between the `window` samples
//! before it and the `window` after. Welch rather than Student because the two
//! sides routinely have wildly different variance (a parked car has almost none,
//! a drive has a lot), and pooled-variance Student is exactly wrong for that.
//! The two-sided p-value comes from the t distribution with
//! Welch-Satterthwaite degrees of freedom, and the significance level is
//! Bonferroni-corrected by the number of candidates tested, because testing
//! every index and then quoting an uncorrected 0.01 would manufacture
//! "significant" shifts out of noise. Overlapping detections are thinned to the
//! strongest within `min_separation`, so one real change reports once instead of
//! once per window offset.
//!
//! What this is not: a segmentation. It marks boundaries; deciding what lies
//! between them is the classifier's job, and comparing the two is the point --
//! a shift with no transition near it is usually a real change the thresholds
//! missed, and a transition with no shift near it is usually the debouncer
//! reacting to noise.

use alloc::vec::Vec;

use ptiles_core::math::{exp, ln, sqrt};

/// One detected change in the speed series.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Shift {
    /// Index of the first sample *after* the change.
    pub index: usize,
    /// Timestamp of that sample.
    pub t_ms: u64,
    /// Welch's t. Signed: negative means the speed dropped.
    pub t_stat: f64,
    /// Two-sided p-value, uncorrected. Compare against
    /// [`Shift::alpha_corrected`] for the significance decision that was
    /// actually made.
    pub p_value: f64,
    /// The Bonferroni-corrected threshold this shift was accepted at, carried so
    /// a consumer can see the decision rule rather than re-deriving it.
    pub alpha_corrected: f64,
    /// Mean speed (m/s) over the window before and after.
    pub before_mps: f64,
    pub after_mps: f64,
}

impl Shift {
    /// Signed change in mean speed, m/s.
    pub fn delta_mps(&self) -> f64 {
        self.after_mps - self.before_mps
    }
}

/// Tunables for [`significant_shifts`].
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct ShiftConfig {
    /// Samples on each side of a candidate. Smaller finds shorter events and is
    /// noisier; larger is steadier and blurs where the change happened. Default
    /// 12, which at a typical 1-5 s cadence is roughly a half-minute per side.
    pub window: usize,
    /// Family-wise significance level, *before* the Bonferroni correction by the
    /// number of candidates. Default 0.01.
    pub alpha: f64,
    /// Minimum samples between reported shifts; within this distance only the
    /// strongest survives. Default 8.
    pub min_separation: usize,
    /// Ignore changes whose mean speed moves less than this (m/s) however small
    /// the p-value. Statistical significance is not importance: with a long
    /// enough window, 0.05 m/s of GPS drift is detectable and meaningless.
    /// Default 0.4.
    pub min_delta_mps: f64,
}

impl Default for ShiftConfig {
    fn default() -> Self {
        ShiftConfig {
            window: 12,
            alpha: 0.01,
            min_separation: 8,
            min_delta_mps: 0.4,
        }
    }
}

/// Detect significant changes in a `(t_ms, speed_mps)` series.
///
/// Samples must be in time order; the timestamps are carried through untouched
/// and never used for the statistics, so an irregular cadence is fine (which is
/// the normal case -- see the fixtures in `test-fixtures/gpx/`).
///
/// Returns shifts in index order. An empty result means the trace has no change
/// this test can distinguish from noise at the corrected level, which for a
/// steady walk is the correct answer.
pub fn significant_shifts(samples: &[(u64, f64)], cfg: ShiftConfig) -> Vec<Shift> {
    let w = cfg.window.max(2);
    let n = samples.len();
    if n < 2 * w + 1 {
        return Vec::new();
    }
    // Candidates are the indices with a full window on both sides. Bonferroni
    // over exactly that count: the correction has to match the number of tests
    // actually performed, not a round number.
    let candidates = n - 2 * w;
    let alpha_corrected = (cfg.alpha / candidates as f64).max(f64::MIN_POSITIVE);

    let mut found: Vec<Shift> = Vec::new();
    for i in w..(n - w) {
        let before = &samples[i - w..i];
        let after = &samples[i..i + w];
        let (m1, v1) = mean_var(before);
        let (m2, v2) = mean_var(after);
        if (m2 - m1).abs() < cfg.min_delta_mps {
            continue;
        }
        let Some((t, df)) = welch(m1, v1, w, m2, v2, w) else {
            continue;
        };
        let p = t_two_sided_p(t, df);
        if p <= alpha_corrected {
            found.push(Shift {
                index: i,
                t_ms: samples[i].0,
                t_stat: t,
                p_value: p,
                alpha_corrected,
                before_mps: m1,
                after_mps: m2,
            });
        }
    }
    thin(found, cfg.min_separation.max(1))
}

/// Keep the strongest shift in each cluster: a single real change trips the test
/// at every offset where its step is inside the window, so raw output arrives in
/// runs. Strength is |t|, and ties keep the earlier index.
fn thin(mut found: Vec<Shift>, min_separation: usize) -> Vec<Shift> {
    found.sort_by(|a, b| {
        b.t_stat
            .abs()
            .partial_cmp(&a.t_stat.abs())
            .unwrap_or(core::cmp::Ordering::Equal)
            .then(a.index.cmp(&b.index))
    });
    let mut kept: Vec<Shift> = Vec::new();
    for s in found {
        if kept
            .iter()
            .all(|k| k.index.abs_diff(s.index) >= min_separation)
        {
            kept.push(s);
        }
    }
    kept.sort_by_key(|s| s.index);
    kept
}

/// Mean and sample variance (n-1) of the speeds. Non-finite samples are skipped
/// rather than poisoning the window: one NaN speed should cost one sample, not
/// the whole test.
fn mean_var(xs: &[(u64, f64)]) -> (f64, f64) {
    let mut n = 0usize;
    let mut sum = 0.0;
    for &(_, x) in xs {
        if x.is_finite() {
            n += 1;
            sum += x;
        }
    }
    if n == 0 {
        return (f64::NAN, f64::NAN);
    }
    let mean = sum / n as f64;
    if n < 2 {
        return (mean, 0.0);
    }
    let mut ss = 0.0;
    for &(_, x) in xs {
        if x.is_finite() {
            let d = x - mean;
            ss += d * d;
        }
    }
    (mean, ss / (n - 1) as f64)
}

/// Welch's t and its Welch-Satterthwaite degrees of freedom, or `None` when the
/// test is undefined (no spread on either side, or a non-finite input).
///
/// The zero-variance case is real and worth naming: two windows of a phone
/// sitting perfectly still both have variance 0, the denominator is 0, and t is
/// infinite. That is not a detection, it is a division by zero wearing a
/// significant p-value, so it is refused.
fn welch(m1: f64, v1: f64, n1: usize, m2: f64, v2: f64, n2: usize) -> Option<(f64, f64)> {
    if !(m1.is_finite() && m2.is_finite() && v1.is_finite() && v2.is_finite()) {
        return None;
    }
    let a = v1 / n1 as f64;
    let b = v2 / n2 as f64;
    let se2 = a + b;
    if se2 <= 0.0 {
        return None;
    }
    let t = (m2 - m1) / sqrt(se2);
    let df_den = a * a / (n1 as f64 - 1.0) + b * b / (n2 as f64 - 1.0);
    if df_den <= 0.0 {
        return None;
    }
    let df = se2 * se2 / df_den;
    if !(t.is_finite() && df.is_finite()) || df <= 0.0 {
        return None;
    }
    Some((t, df))
}

/// Two-sided p-value for Student's t with `df` degrees of freedom:
/// `I(df / (df + t^2); df/2, 1/2)` via the regularized incomplete beta.
pub fn t_two_sided_p(t: f64, df: f64) -> f64 {
    if !t.is_finite() || !df.is_finite() || df <= 0.0 {
        return 1.0;
    }
    let x = df / (df + t * t);
    betai(0.5 * df, 0.5, x).clamp(0.0, 1.0)
}

/// Regularized incomplete beta `I_x(a, b)`.
///
/// Numerical Recipes' formulation: the continued fraction converges quickly on
/// one side of `x = (a+1)/(a+b+2)` and is reflected on the other. This exists
/// here rather than as a dependency because it is 40 lines of textbook and the
/// alternative is a statistics crate in a `no_std` library that needs exactly
/// one function from it.
fn betai(a: f64, b: f64, x: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    let bt = exp(ln_gamma(a + b) - ln_gamma(a) - ln_gamma(b) + a * ln(x) + b * ln(1.0 - x));
    if x < (a + 1.0) / (a + b + 2.0) {
        bt * betacf(a, b, x) / a
    } else {
        1.0 - bt * betacf(b, a, 1.0 - x) / b
    }
}

/// Continued fraction for the incomplete beta, by modified Lentz.
fn betacf(a: f64, b: f64, x: f64) -> f64 {
    const MAX_ITER: u32 = 200;
    const EPS: f64 = 3.0e-16;
    const FPMIN: f64 = 1.0e-300;

    let qab = a + b;
    let qap = a + 1.0;
    let qam = a - 1.0;
    let mut c = 1.0;
    let mut d = 1.0 - qab * x / qap;
    if d.abs() < FPMIN {
        d = FPMIN;
    }
    d = 1.0 / d;
    let mut h = d;
    for m in 1..=MAX_ITER {
        let m_f = m as f64;
        let m2 = 2.0 * m_f;
        // Even step.
        let aa = m_f * (b - m_f) * x / ((qam + m2) * (a + m2));
        d = 1.0 + aa * d;
        if d.abs() < FPMIN {
            d = FPMIN;
        }
        c = 1.0 + aa / c;
        if c.abs() < FPMIN {
            c = FPMIN;
        }
        d = 1.0 / d;
        h *= d * c;
        // Odd step.
        let aa = -(a + m_f) * (qab + m_f) * x / ((a + m2) * (qap + m2));
        d = 1.0 + aa * d;
        if d.abs() < FPMIN {
            d = FPMIN;
        }
        c = 1.0 + aa / c;
        if c.abs() < FPMIN {
            c = FPMIN;
        }
        d = 1.0 / d;
        let del = d * c;
        h *= del;
        if (del - 1.0).abs() < EPS {
            break;
        }
    }
    h
}

/// `ln Γ(x)` by the Lanczos approximation, good to ~15 digits for `x > 0`.
fn ln_gamma(x: f64) -> f64 {
    const COF: [f64; 6] = [
        76.180_091_729_471_46,
        -86.505_320_329_416_77,
        24.014_098_240_830_91,
        -1.231_739_572_450_155,
        0.120_865_097_386_617_7e-2,
        -0.539_523_938_495_3e-5,
    ];
    let mut y = x;
    let tmp = x + 5.5;
    let tmp = tmp - (x + 0.5) * ln(tmp);
    let mut ser = 1.000_000_000_190_015;
    for c in COF {
        y += 1.0;
        ser += c / y;
    }
    -tmp + ln(2.506_628_274_631_000_5 * ser / x)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// Textbook critical values. If these drift, every p-value above is wrong
    /// and "significant" stops meaning anything.
    #[test]
    fn t_distribution_matches_published_critical_values() {
        // Two-sided 0.05 critical values: t(2.086, df=20), t(1.960, df=inf-ish),
        // t(2.228, df=10), t(12.706, df=1).
        for (t, df) in [(2.086, 20.0), (2.228, 10.0), (12.706, 1.0), (1.9600, 100_000.0)] {
            let p = t_two_sided_p(t, df);
            assert!(
                (p - 0.05).abs() < 0.001,
                "t={t} df={df} gave p={p}, expected ~0.05"
            );
        }
        // Two-sided 0.01: t(3.169, df=10), t(2.845, df=20).
        for (t, df) in [(3.169, 10.0), (2.845, 20.0)] {
            let p = t_two_sided_p(t, df);
            assert!((p - 0.01).abs() < 0.001, "t={t} df={df} gave p={p}");
        }
        // t = 0 is no evidence at all.
        assert!((t_two_sided_p(0.0, 10.0) - 1.0).abs() < 1e-12);
        // Sign does not matter for a two-sided test.
        assert!((t_two_sided_p(2.5, 15.0) - t_two_sided_p(-2.5, 15.0)).abs() < 1e-15);
        // Garbage in, no-evidence out.
        assert_eq!(t_two_sided_p(f64::NAN, 10.0), 1.0);
        assert_eq!(t_two_sided_p(2.0, 0.0), 1.0);
    }

    /// A series with `n` samples at `a`, then `n` at `b`, one second apart.
    fn step(a: f64, b: f64, n: usize, jitter: f64) -> Vec<(u64, f64)> {
        let mut out = Vec::new();
        for i in 0..(2 * n) {
            let base = if i < n { a } else { b };
            // Deterministic pseudo-jitter: a real series is never flat, and a
            // flat one makes Welch's denominator zero (which is refused).
            let wobble = ((i * 2_654_435_761) % 1000) as f64 / 1000.0 - 0.5;
            out.push((i as u64 * 1000, base + wobble * jitter));
        }
        out
    }

    #[test]
    fn a_clean_step_is_found_once_at_the_right_place() {
        let s = step(0.2, 12.0, 40, 0.2);
        let shifts = significant_shifts(&s, ShiftConfig::default());
        assert_eq!(shifts.len(), 1, "one step, one shift: {shifts:?}");
        let sh = shifts[0];
        assert!(
            sh.index.abs_diff(40) <= 2,
            "step is at index 40, reported {}",
            sh.index
        );
        assert!(sh.p_value < sh.alpha_corrected);
        assert!(sh.delta_mps() > 11.0, "delta {}", sh.delta_mps());
        assert!(sh.t_stat > 0.0, "a speed-up must have positive t");
        assert_eq!(sh.t_ms, s[sh.index].0);
    }

    #[test]
    fn a_slowdown_reports_negative_t() {
        let s = step(14.0, 0.3, 40, 0.3);
        let shifts = significant_shifts(&s, ShiftConfig::default());
        assert_eq!(shifts.len(), 1);
        assert!(shifts[0].t_stat < 0.0);
        assert!(shifts[0].delta_mps() < -13.0);
    }

    #[test]
    fn noise_alone_yields_nothing() {
        // 400 samples of the same distribution. With the Bonferroni correction
        // this must stay empty; without one, a run this long produces "findings"
        // on demand, which is the whole reason the correction is there.
        let mut s = Vec::new();
        for i in 0..400 {
            let wobble = ((i * 2_654_435_761_u64) % 10_000) as f64 / 10_000.0;
            s.push((i as u64 * 1000, 1.4 + wobble * 0.8));
        }
        let shifts = significant_shifts(&s, ShiftConfig::default());
        assert!(shifts.is_empty(), "noise produced {shifts:?}");
    }

    #[test]
    fn a_perfectly_still_series_is_not_a_detection() {
        // Zero variance on both sides: t is 0/0. Refusing that is the difference
        // between "no change" and an infinitely significant one.
        let s: Vec<(u64, f64)> = (0..80).map(|i| (i as u64 * 1000, 0.0)).collect();
        assert!(significant_shifts(&s, ShiftConfig::default()).is_empty());
    }

    #[test]
    fn a_noiseless_step_is_still_found_one_sample_off() {
        // Synthetic data with no jitter at all: at the exactly-aligned candidate
        // both windows have zero variance, so that test is refused (0/0). Every
        // straddling candidate has spread on one side and finds it, so the step
        // is reported a sample or two from where it is rather than missed. Worth
        // pinning, because the refusal above could otherwise look like a hole.
        let mut flat: Vec<(u64, f64)> = (0..40).map(|i| (i as u64 * 1000, 0.0)).collect();
        flat.extend((40..80).map(|i| (i as u64 * 1000, 10.0)));
        let shifts = significant_shifts(&flat, ShiftConfig::default());
        assert_eq!(shifts.len(), 1, "{shifts:?}");
        assert!(
            shifts[0].index.abs_diff(40) <= 2,
            "step at 40, reported {}",
            shifts[0].index
        );
        assert!(shifts[0].delta_mps() > 5.0);
    }

    #[test]
    fn statistical_significance_is_not_importance() {
        // 0.15 m/s of drift, dead steady, over a long series: detectable, and
        // meaningless. `min_delta_mps` is what keeps it out.
        let s = step(1.20, 1.35, 60, 0.02);
        assert!(significant_shifts(&s, ShiftConfig::default()).is_empty());
        // Lower the bar and the same data reports it, which is the knob doing
        // its job rather than the test being inconsistent.
        let loose = ShiftConfig { min_delta_mps: 0.05, ..ShiftConfig::default() };
        assert_eq!(significant_shifts(&s, loose).len(), 1);
    }

    #[test]
    fn short_series_and_degenerate_config_are_handled() {
        let s = step(0.0, 10.0, 4, 0.2);
        // 8 samples cannot support a 12-sample window each side.
        assert!(significant_shifts(&s, ShiftConfig::default()).is_empty());
        // A window of 0 or 1 is clamped to 2 rather than dividing by zero.
        let tiny = ShiftConfig { window: 0, min_separation: 0, ..ShiftConfig::default() };
        let shifts = significant_shifts(&s, tiny);
        assert!(shifts.len() <= s.len());
        assert!(significant_shifts(&[], ShiftConfig::default()).is_empty());
        assert!(significant_shifts(&[(0, 1.0)], ShiftConfig::default()).is_empty());
    }

    #[test]
    fn two_changes_report_twice_and_stay_separated() {
        // Stop, drive, stop: two boundaries, and neither should be reported as a
        // cluster of near-duplicates.
        let mut s = Vec::new();
        for i in 0..120 {
            let base = if i < 40 {
                0.2
            } else if i < 80 {
                13.0
            } else {
                0.3
            };
            let wobble = ((i * 2_654_435_761_u64) % 1000) as f64 / 1000.0 - 0.5;
            s.push((i as u64 * 1000, base + wobble * 0.4));
        }
        let shifts = significant_shifts(&s, ShiftConfig::default());
        assert_eq!(shifts.len(), 2, "expected two boundaries: {shifts:?}");
        assert!(shifts[0].index.abs_diff(40) <= 3, "first at {}", shifts[0].index);
        assert!(shifts[1].index.abs_diff(80) <= 3, "second at {}", shifts[1].index);
        assert!(shifts[0].t_stat > 0.0 && shifts[1].t_stat < 0.0);
        assert!(shifts[1].index - shifts[0].index >= 8);
    }

    #[test]
    fn nan_samples_cost_one_sample_not_the_window() {
        let mut s = step(0.2, 12.0, 40, 0.2);
        s[5].1 = f64::NAN;
        s[70].1 = f64::INFINITY;
        let shifts = significant_shifts(&s, ShiftConfig::default());
        assert_eq!(shifts.len(), 1, "{shifts:?}");
        assert!(shifts[0].index.abs_diff(40) <= 2);
    }

    #[test]
    fn output_is_sorted_and_deterministic() {
        let mut s = Vec::new();
        for i in 0..200 {
            let base = match i / 50 {
                0 => 0.2,
                1 => 12.0,
                2 => 1.3,
                _ => 9.0,
            };
            let wobble = ((i * 2_654_435_761_u64) % 1000) as f64 / 1000.0 - 0.5;
            s.push((i as u64 * 1000, base + wobble * 0.5));
        }
        let a = significant_shifts(&s, ShiftConfig::default());
        let b = significant_shifts(&s, ShiftConfig::default());
        assert_eq!(a, b, "same input, same output");
        assert!(a.windows(2).all(|w| w[0].index < w[1].index), "sorted: {a:?}");
        assert!(a.len() >= 3, "three boundaries expected, got {a:?}");
    }
}
