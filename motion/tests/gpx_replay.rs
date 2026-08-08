//! Replay real GPS traces through the classifier.
//!
//! Every other test in this crate is synthetic: hand-built `AccelStats`, votes
//! fed one per second from a loop. This one feeds the six ODbL OpenStreetMap
//! traces in `test-fixtures/gpx/` -- irregular sampling, real noise, real
//! multi-minute holes, one trace whose points are not in time order -- and
//! pins what the implementation actually does with them.
//!
//! What these traces can and cannot exercise: they carry `<trkpt lat lon>` and
//! `<time>`, and nothing else. No reported speed (so speed comes from position
//! deltas through `MotionClassifier`), no accuracy (so the 30 m gate never
//! fires), no accelerometer (so `AccelStats::EMPTY`). That leaves the
//! stateless `classify` blind to a stroll -- pinned below in
//! `speed_alone_cannot_see_a_stroll_but_can_see_a_jog`, which is the concrete
//! argument for the road priors that `road_context.rs` tests.
//!
//! Skips silently if the fixture directory is absent, matching `block()` in
//! `core/tests/prefix_sweep.rs`.

use ptiles_core::Fix;

use ptiles_motion::{
    classify, AccelStats, DebounceConfig, MotionClassifier, MotionConfig, MovementType, TimedFix,
    VoteDebouncer,
};

// ---------------------------------------------------------------- fixtures

/// One trace point. Accuracy and speed are absent by construction -- see the
/// module docs.
#[derive(Clone, Copy, Debug)]
struct Pt {
    lat: f64,
    lon: f64,
    t_ms: u64,
}

/// How fast a trace actually moves, which is not what its filename suggests.
/// The README calls five of these "foot routes", but only one is a stroll: the
/// NC traces average 2.9-3.4 m/s, i.e. a jog, and one TN "trails" trace has a
/// p95 of 8.4 m/s. That distinction is load-bearing, because the stateless
/// tree's walking floor is 2.2 m/s -- a stroll is invisible to speed alone
/// while a jog is not.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Pace {
    /// Moving mean below the 2.2 m/s walking floor.
    Stroll,
    /// Moving mean between the walking floor and the driving floor.
    Jog,
    Vehicle,
}

/// Measured expectations per fixture, all from an actual replay (see
/// `derived_speed_agrees_with_the_traces_own_statistics` for the independent
/// cross-check that these numbers are not merely self-consistent).
///
/// Pinning measured values rather than loose ranges is deliberate: the input is
/// a fixed file and the classifier is deterministic, so anything that moves
/// these numbers is a behavior change to look at, not to tolerate.
struct Expect {
    stem: &'static str,
    points: usize,
    pace: Pace,
    /// Mean of the smoothed speeds above the 0.5 m/s stationary ceiling.
    moving_mean_mps: f64,
    /// Ceiling on the share of samples the speed bands put in `Driving`.
    max_drive_share: f64,
    /// Most common stateless vote. Note this is not implied by `pace`:
    /// `moving_mean_mps` only averages the samples that were moving, so a
    /// trace that spends most of its wall clock paused votes `Stationary` even
    /// with a brisk moving mean.
    dominant_vote: MovementType,
}

const TRACES: &[Expect] = &[
    Expect { stem: "nc-sals-branch-1191748", points: 721, pace: Pace::Jog,
             moving_mean_mps: 3.43, max_drive_share: 0.06, dominant_vote: MovementType::Walking },
    Expect { stem: "nc-mine-creek-1184364", points: 838, pace: Pace::Jog,
             moving_mean_mps: 2.88, max_drive_share: 0.04, dominant_vote: MovementType::Walking },
    Expect { stem: "nc-umstead-trails-1184467", points: 1957, pace: Pace::Jog,
             moving_mean_mps: 3.33, max_drive_share: 0.07, dominant_vote: MovementType::Walking },
    // A true stroll: 1.2 m/s for 94 minutes, entirely below the tree's floor.
    Expect { stem: "tn-maryville-hike-1063250", points: 1124, pace: Pace::Stroll,
             moving_mean_mps: 1.21, max_drive_share: 0.01, dominant_vote: MovementType::Stationary },
    // Mixed: 442 points over 109 minutes, so mostly paused, with stretches
    // fast enough (p95 8.4 m/s) to band as driving.
    Expect { stem: "tn-maryville-trails-1283272", points: 442, pace: Pace::Jog,
             moving_mean_mps: 2.77, max_drive_share: 0.20, dominant_vote: MovementType::Stationary },
    Expect { stem: "tn-middle-tennessee-3605997", points: 1187, pace: Pace::Vehicle,
             moving_mean_mps: 16.53, max_drive_share: 1.0, dominant_vote: MovementType::Driving },
];

fn gpx(stem: &str) -> Option<String> {
    let p = format!(
        "{}/../test-fixtures/gpx/{stem}.gpx",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(p).ok()
}

/// All fixtures as `(expectations, points)`, or `None` if the directory is
/// missing. Every caller starts with this so one absent-fixture check covers
/// the whole file.
fn load_all() -> Option<Vec<(&'static Expect, Vec<Pt>)>> {
    let mut out = Vec::new();
    for e in TRACES {
        let text = gpx(e.stem)?;
        out.push((e, parse_trkpts(&text)));
    }
    Some(out)
}

/// Pull `<trkpt lat lon>` + its `<time>` out of GPX.
///
/// ponytail: string scanning, not an XML parser -- same call as
/// `test-fixtures/parse_gpx.py`, which does this with one regex. The workspace
/// has no XML crate and adding one to read two attributes and a timestamp
/// would be the whole dependency for the whole job. Points whose element has
/// no `<time>` are dropped (they cannot be placed in the sequence); `<wpt>`
/// and `<metadata>` timestamps are never seen because the scan only looks
/// between `<trkpt` and `</trkpt>`.
fn parse_trkpts(xml: &str) -> Vec<Pt> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find("<trkpt") {
        rest = &rest[i + 6..];
        // The element body ends at </trkpt>, or at the next <trkpt for a
        // self-closing point.
        let end = rest.find("</trkpt>").unwrap_or(rest.len());
        let (body, after) = rest.split_at(end);
        let pt = (|| {
            Some(Pt {
                lat: attr(body, "lat")?,
                lon: attr(body, "lon")?,
                t_ms: epoch_ms(tag(body, "time")?)?,
            })
        })();
        if let Some(p) = pt {
            out.push(p);
        }
        rest = after;
    }
    out
}

/// `name="<f64>"` out of an element's text, first occurrence.
fn attr(s: &str, name: &str) -> Option<f64> {
    let needle = format!("{name}=\"");
    let i = s.find(&needle)? + needle.len();
    let j = s[i..].find('"')? + i;
    s[i..j].trim().parse().ok()
}

/// Text content of the first `<tag>` in `s`.
fn tag<'a>(s: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let i = s.find(&open)? + open.len();
    let j = s[i..].find(&close)? + i;
    Some(s[i..j].trim())
}

/// `2012-03-09T18:24:48Z` -> milliseconds since the Unix epoch.
///
/// Fixed-offset field slicing plus days-from-civil, rather than a date crate:
/// GPX timestamps are always this shape, always UTC, and this is a dozen
/// lines. Fractional seconds (`...:48.500Z`) are truncated, which is fine at
/// the second-scale cadence these traces sample at.
fn epoch_ms(iso: &str) -> Option<u64> {
    let b = iso.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| iso.get(r)?.parse::<i64>().ok();
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hh, mm, ss) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let days = days_from_civil(y, m, d);
    let secs = days * 86_400 + hh * 3600 + mm * 60 + ss;
    u64::try_from(secs.checked_mul(1000)?).ok()
}

/// Howard Hinnant's days_from_civil: days since 1970-01-01 for a proleptic
/// Gregorian date. Handles the leap rules the traces span (2011-2020) without
/// a lookup table.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

// ---------------------------------------------------------------- replay

/// What one replay produced.
#[derive(Debug, Default)]
struct Replay {
    /// Sample count per band from the stateful speed classifier.
    band: Vec<(MovementType, usize)>,
    /// Sample count per vote from the stateless decision tree.
    vote: Vec<(MovementType, usize)>,
    /// Debounced transitions, in order.
    transitions: Vec<MovementType>,
    /// Wall-clock milliseconds spent in each debounced state. Sample counts
    /// overweight densely-sampled stretches; a trip is measured in time.
    stable_ms: Vec<(MovementType, u64)>,
    /// Smoothed speeds, one per accepted fix that produced one.
    speeds: Vec<f64>,
    /// Pushes where the smoothed speed was `None` (first fix, or a gap reset).
    speed_gaps: usize,
}

fn count(list: &mut Vec<(MovementType, usize)>, t: MovementType) {
    match list.iter_mut().find(|(k, _)| *k == t) {
        Some((_, n)) => *n += 1,
        None => list.push((t, 1)),
    }
}

fn top(list: &[(MovementType, usize)]) -> MovementType {
    list.iter()
        .max_by_key(|(_, n)| *n)
        .map(|(t, _)| *t)
        .unwrap_or(MovementType::Unknown)
}

fn share(list: &[(MovementType, usize)], t: MovementType) -> f64 {
    let total: usize = list.iter().map(|(_, n)| n).sum();
    let hit = list.iter().find(|(k, _)| *k == t).map_or(0, |(_, n)| *n);
    if total == 0 {
        0.0
    } else {
        hit as f64 / total as f64
    }
}

/// Fraction of *time* spent in `t`.
fn time_share(list: &[(MovementType, u64)], t: MovementType) -> f64 {
    let total: u64 = list.iter().map(|(_, n)| n).sum();
    let hit = list.iter().find(|(k, _)| *k == t).map_or(0, |(_, n)| *n);
    if total == 0 {
        0.0
    } else {
        hit as f64 / total as f64
    }
}

/// Feed a trace through the whole pipeline: `MotionClassifier` for a smoothed
/// speed, `classify` for a per-fix vote, `VoteDebouncer` for the transitions.
/// No road context and no accelerometer -- that is all these files carry.
fn replay(points: &[Pt]) -> Replay {
    let mut speed = MotionClassifier::new(MotionConfig::default());
    let mut debouncer = VoteDebouncer::new(DebounceConfig::default());
    let mut r = Replay::default();
    let mut last_stable = MovementType::Unknown;
    let mut last_t: Option<u64> = None;

    for p in points {
        let fix = Fix {
            lat: p.lat,
            lon: p.lon,
            // The traces report no accuracy. 0.0 is "unknown but usable" for
            // the stateful gate; `classify` gets `None`, which is the honest
            // "the platform did not say".
            horizontal_accuracy_m: 0.0,
            speed_mps: None,
        };
        let band = speed.push(TimedFix::new(fix, p.t_ms));
        count(&mut r.band, band);
        match speed.smoothed_speed_mps() {
            Some(v) => r.speeds.push(v),
            None => r.speed_gaps += 1,
        }

        let vote = classify(speed.smoothed_speed_mps(), None, None, &AccelStats::EMPTY);
        count(&mut r.vote, vote.movement);

        let stable = debouncer.tick(&vote, p.t_ms);
        if let Some(prev_t) = last_t {
            let dt = p.t_ms.saturating_sub(prev_t);
            match r.stable_ms.iter_mut().find(|(k, _)| *k == last_stable) {
                Some((_, ms)) => *ms += dt,
                None => r.stable_ms.push((last_stable, dt)),
            }
        }
        last_t = Some(p.t_ms);
        if stable != last_stable {
            r.transitions.push(stable);
            last_stable = stable;
        }
    }
    r
}

// ---------------------------------------------------------------- tests

#[test]
fn fixtures_parse_to_the_expected_point_counts() {
    let Some(traces) = load_all() else {
        eprintln!("skipping: test-fixtures/gpx/ not present");
        return;
    };
    for (e, points) in &traces {
        let (stem, expected) = (e.stem, e.points);
        assert_eq!(points.len(), expected, "{stem} point count");
        // Every point must have parsed into a plausible coordinate and a
        // timestamp inside the traces' 2011-2021 window (1.26e12..1.61e12 ms).
        for p in points {
            assert!((-90.0..=90.0).contains(&p.lat), "{stem} lat {}", p.lat);
            assert!((-180.0..=180.0).contains(&p.lon), "{stem} lon {}", p.lon);
            assert!(
                (1_260_000_000_000..1_650_000_000_000).contains(&p.t_ms),
                "{stem} timestamp {} out of the fixtures' era",
                p.t_ms
            );
        }
    }
}

#[test]
fn real_traces_are_not_evenly_sampled_and_one_is_out_of_order() {
    // The synthetic tests all feed 1 Hz monotonic samples. Establish that the
    // fixtures do not, so the assertions that follow are known to be running
    // against irregular input rather than accidentally-clean input.
    let Some(traces) = load_all() else { return };
    let mut saw_backwards = false;
    let mut saw_big_gap = false;
    for (e, points) in &traces {
        let stem = e.stem;
        let mut gaps = Vec::new();
        for w in points.windows(2) {
            if w[1].t_ms < w[0].t_ms {
                saw_backwards = true;
            }
            gaps.push(w[1].t_ms as i64 - w[0].t_ms as i64);
        }
        let max_gap = gaps.iter().copied().max().unwrap_or(0);
        if max_gap > MotionConfig::default().max_gap_ms as i64 {
            saw_big_gap = true;
        }
        let distinct: std::collections::BTreeSet<i64> = gaps.iter().copied().collect();
        assert!(
            distinct.len() > 1,
            "{stem} would be a perfectly regular trace, which no real GPS is"
        );
    }
    assert!(saw_backwards, "expected at least one trace with a backwards timestamp");
    assert!(
        saw_big_gap,
        "expected at least one hole longer than max_gap_ms, which is what makes the gap path real here"
    );
}

#[test]
fn speed_bands_match_the_measured_pace_of_every_trace() {
    // `MotionClassifier`'s bands are the layer that can see motion from
    // position deltas alone (Stationary <= 0.5, Driving >= 5.0, Walking
    // between). Both foot paces land in the walking band; only the drive
    // dominates the driving one.
    let Some(traces) = load_all() else { return };
    for (e, points) in &traces {
        let (stem, pace) = (e.stem, e.pace);
        let r = replay(points);
        let (expected_mean, max_drive_share) = (e.moving_mean_mps, e.max_drive_share);

        let moving: Vec<f64> = r.speeds.iter().copied().filter(|v| *v > 0.5).collect();
        let mean = moving.iter().sum::<f64>() / moving.len() as f64;
        assert!(
            (mean - expected_mean).abs() < 0.1,
            "{stem}: moving mean {mean:.2} m/s, table says {expected_mean:.2}"
        );

        match pace {
            Pace::Vehicle => assert_eq!(
                top(&r.band),
                MovementType::Driving,
                "{stem}: bands {:?}",
                r.band
            ),
            Pace::Stroll | Pace::Jog => {
                assert_eq!(
                    top(&r.band),
                    MovementType::Walking,
                    "{stem}: on-foot trace should band as walking, got {:?}",
                    r.band
                );
                // GPS noise throws occasional fast samples; the smoothing
                // window is what stops them dominating. The bound per trace is
                // measured, not guessed -- `tn-maryville-trails` genuinely
                // spends a fifth of its samples above 5 m/s.
                let drive = share(&r.band, MovementType::Driving);
                assert!(
                    drive <= max_drive_share,
                    "{stem}: {:.1}% of samples banded as driving, table allows {:.1}%",
                    drive * 100.0,
                    max_drive_share * 100.0
                );
            }
        }
    }
}

#[test]
fn speed_alone_cannot_see_a_stroll_but_can_see_a_jog() {
    // The stateless tree's walking floor is 2.2 m/s (~5 mph), which is above a
    // stroll and below a jog. So with no accelerometer and no road context:
    // `tn-maryville-hike` (1.21 m/s) votes Stationary for all 1,124 of its
    // points -- an hour and a half of walking, invisible -- while the 2.9-3.4
    // m/s traces vote Walking.
    //
    // That asymmetry is the concrete argument for the road priors: a footway
    // hit at 1.2 m/s votes Walking at 0.90 confidence where speed alone sees
    // nothing (see road_context.rs). If a future change lets speed alone see a
    // stroll, this test should fail and be rewritten deliberately.
    let Some(traces) = load_all() else { return };
    for (e, points) in &traces {
        let r = replay(points);
        assert_eq!(
            top(&r.vote),
            e.dominant_vote,
            "{}: votes {:?}",
            e.stem,
            r.vote
        );
    }
}

#[test]
fn the_drive_is_classified_as_driving_end_to_end() {
    let Some(traces) = load_all() else { return };
    let (e, points) = traces
        .iter()
        .find(|(e, _)| e.pace == Pace::Vehicle)
        .expect("the vehicular fixture must be present");
    let stem = e.stem;
    let r = replay(points);
    assert_eq!(top(&r.vote), MovementType::Driving, "{stem} votes: {:?}", r.vote);
    assert!(
        r.transitions.contains(&MovementType::Driving),
        "{stem}: debouncer never committed to driving, transitions {:?}",
        r.transitions
    );
}

#[test]
fn derived_speed_agrees_with_the_traces_own_statistics() {
    // Three traces were recorded by My Tracks, which writes its own summary
    // into a waypoint description: "Average moving speed: 11.19 km/h". That is
    // an independent number -- not computed by this repo -- so it is the only
    // check here that can catch the smoother being systematically wrong rather
    // than merely self-consistent.
    let Some(traces) = load_all() else { return };
    let mut checked = 0;
    for (e, points) in &traces {
        let stem = e.stem;
        let text = gpx(stem).unwrap();
        let Some(reported_kmh) = reported_moving_speed_kmh(&text) else {
            continue;
        };
        let r = replay(points);
        // "Moving" excludes the stopped stretches the average also excludes;
        // 0.5 m/s is the classifier's own stationary ceiling.
        let moving: Vec<f64> = r.speeds.iter().copied().filter(|v| *v > 0.5).collect();
        assert!(!moving.is_empty(), "{stem}: no moving samples");
        let mean_kmh = moving.iter().sum::<f64>() / moving.len() as f64 * 3.6;
        let ratio = mean_kmh / reported_kmh;
        assert!(
            (0.6..1.6).contains(&ratio),
            "{stem}: derived {mean_kmh:.2} km/h vs trace-reported {reported_kmh:.2} km/h (ratio {ratio:.2})"
        );
        checked += 1;
    }
    assert!(checked >= 2, "expected at least two traces to carry their own statistics");
}

/// `Average moving speed: 11.19 km/h (7.0 mi/h)` out of a My Tracks summary
/// waypoint, in km/h.
fn reported_moving_speed_kmh(xml: &str) -> Option<f64> {
    let i = xml.find("Average moving speed:")? + "Average moving speed:".len();
    let rest = &xml[i..];
    let j = rest.find("km/h")?;
    rest[..j].trim().parse().ok()
}

#[test]
fn a_real_hole_in_a_trace_resets_the_smoothing_window() {
    // Find an actual gap longer than max_gap_ms in a real trace, replay up to
    // it, and confirm the classifier reports no speed across it rather than
    // dividing a multi-minute displacement by the elapsed time.
    let Some(traces) = load_all() else { return };
    let cfg = MotionConfig::default();
    let mut tested = 0;
    for (e, points) in &traces {
        let stem = e.stem;
        let Some(idx) = points
            .windows(2)
            .position(|w| w[1].t_ms.saturating_sub(w[0].t_ms) > cfg.max_gap_ms)
        else {
            continue;
        };
        let mut c = MotionClassifier::new(cfg);
        for p in &points[..=idx] {
            c.push(TimedFix::new(fix_at(p), p.t_ms));
        }
        assert!(
            c.smoothed_speed_mps().is_some() || idx == 0,
            "{stem}: expected a speed before the gap"
        );
        let after = points[idx + 1];
        c.push(TimedFix::new(fix_at(&after), after.t_ms));
        assert_eq!(
            c.smoothed_speed_mps(),
            None,
            "{stem}: window should be empty right after a {} ms hole",
            after.t_ms - points[idx].t_ms
        );
        tested += 1;
    }
    assert!(tested > 0, "no trace had a hole longer than max_gap_ms");
}

fn fix_at(p: &Pt) -> Fix {
    Fix {
        lat: p.lat,
        lon: p.lon,
        horizontal_accuracy_m: 0.0,
        speed_mps: None,
    }
}

#[test]
fn transitions_stay_bounded_on_real_noise() {
    // The debouncer's whole job. Bound the *rate*, not the count: an
    // hour-and-a-half trail hike with rest stops legitimately commits more
    // transitions than a 30-minute run, and a flat count would just be a
    // length limit wearing a stability costume. Measured worst case across the
    // six traces is 0.96 per 10 min; 1.5 leaves room without letting a twitchy
    // classifier through.
    let Some(traces) = load_all() else { return };
    for (e, points) in &traces {
        let stem = e.stem;
        let r = replay(points);
        let minutes = (points.last().unwrap().t_ms - points[0].t_ms) as f64 / 60_000.0;
        let per_10min = r.transitions.len() as f64 / (minutes / 10.0);
        assert!(
            per_10min <= 1.5,
            "{stem}: {:.2} transitions per 10 min over {minutes:.0} min: {:?}",
            per_10min,
            r.transitions
        );
        assert!(
            !r.transitions.is_empty(),
            "{stem}: never committed to any state in {minutes:.0} minutes"
        );
    }
}

#[test]
fn time_is_spent_in_the_state_the_trace_actually_is() {
    // Sample counts overweight densely-sampled stretches, so measure the
    // debounced state by wall clock. A trip is what you spent your time doing.
    let Some(traces) = load_all() else { return };
    for (e, points) in &traces {
        let (stem, pace) = (e.stem, e.pace);
        let r = replay(points);
        let driving = time_share(&r.stable_ms, MovementType::Driving);
        let on_foot = time_share(&r.stable_ms, MovementType::Walking)
            + time_share(&r.stable_ms, MovementType::Stationary);
        match pace {
            Pace::Vehicle => assert!(
                driving > 0.5,
                "{stem}: only {:.0}% of the drive read as driving: {:?}",
                driving * 100.0,
                r.stable_ms
            ),
            Pace::Stroll | Pace::Jog => assert!(
                on_foot > 0.75 && driving < 0.25,
                "{stem}: {:.0}% on foot / {:.0}% driving: {:?}",
                on_foot * 100.0,
                driving * 100.0,
                r.stable_ms
            ),
        }
    }
}

#[test]
fn replay_is_deterministic() {
    let Some(traces) = load_all() else { return };
    for (e, points) in &traces {
        let stem = e.stem;
        let a = replay(points);
        let b = replay(points);
        assert_eq!(a.transitions, b.transitions, "{stem} transitions differ between runs");
        assert_eq!(a.speeds.len(), b.speeds.len(), "{stem} speed counts differ");
        assert_eq!(a.speed_gaps, b.speed_gaps, "{stem} gap counts differ");
    }
}

#[test]
fn classify_never_returns_unknown() {
    // `Unknown` is the debouncer's initial state, never a vote: the stateless
    // tree always ends at some accel row. Nothing else asserts the split, and
    // a caller that surfaces a vote directly would show the user "unknown"
    // forever if this ever stopped holding.
    let inputs = [
        None,
        Some(0.0),
        Some(-1.0),
        Some(1.0),
        Some(2.2),
        Some(8.9),
        Some(1e6),
        Some(f64::NAN),
        Some(f64::INFINITY),
    ];
    let accels = [
        AccelStats::EMPTY,
        AccelStats { variance: f64::NAN, ..AccelStats::EMPTY },
        AccelStats { dominant_frequency: f64::INFINITY, variance: 1e9, ..AccelStats::EMPTY },
        AccelStats { dominant_frequency: 2.0, step_count: u32::MAX, variance: 0.5, ..AccelStats::EMPTY },
    ];
    for speed in inputs {
        for acc in inputs {
            for a in &accels {
                let v = classify(speed, acc, None, a);
                assert_ne!(v.movement, MovementType::Unknown, "speed {speed:?} acc {acc:?}");
                assert!(v.confidence.is_finite() && v.confidence > 0.0);
            }
        }
    }
}

#[test]
fn adversarial_fixes_never_panic() {
    // Coordinate poles, the antimeridian, NaN, and a backwards clock, in one
    // sequence. The haversine and the local tangent projection both have
    // wrap-around edges; a panic here would be an unhandled real-world fix.
    let mut c = MotionClassifier::new(MotionConfig::default());
    let coords = [
        (90.0, 180.0),
        (-90.0, -180.0),
        (0.0, 179.999_999),
        (0.0, -179.999_999),
        (f64::NAN, 0.0),
        (0.0, f64::NAN),
        (f64::INFINITY, f64::NEG_INFINITY),
        (36.16, -86.79),
    ];
    let times = [0u64, 1000, 500, 0, u64::MAX, u64::MAX - 1, 2000, 3000];
    for (i, (lat, lon)) in coords.iter().enumerate() {
        let fix = Fix {
            lat: *lat,
            lon: *lon,
            horizontal_accuracy_m: if i % 3 == 0 { 0.0 } else { f64::NAN },
            speed_mps: if i % 2 == 0 { None } else { Some(f64::NAN) },
        };
        // The contract is only "does not panic and stays a valid state".
        let s = c.push(TimedFix::new(fix, times[i]));
        assert!(matches!(
            s,
            MovementType::Unknown
                | MovementType::Stationary
                | MovementType::Walking
                | MovementType::Running
                | MovementType::Driving
        ));
    }
    // An empty trace is a valid trace.
    let empty = replay(&[]);
    assert!(empty.transitions.is_empty());
    // So is a one-point trace: nothing to derive a speed from.
    let one = replay(&[Pt { lat: 36.16, lon: -86.79, t_ms: 1_300_000_000_000 }]);
    assert!(one.speeds.is_empty());
}
