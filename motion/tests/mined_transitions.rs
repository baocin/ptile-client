//! Replay real device recordings of moments the movement state actually moved.
//!
//! `tests/data/scenarios.json.gz` was mined on-device by the Rook Android client:
//! 462 windows of raw accelerometer and GPS captured around a debounced
//! transition, or around a section ambiguous enough that one nearly happened.
//! Each carries the state it was primed from and the state that was reached.
//!
//! The GPX suite next door measures the opposite property. It bounds how OFTEN
//! the state commits, because a classifier that flips every sample is useless
//! however responsive it is. This one measures whether a transition that really
//! happened is detected at all. A tuning can only be judged against both: the
//! shipped defaults hold the GPX rate at 0.96 per 10 minutes and reproduce 1 of
//! these 116 transitions, and [`DebounceConfig::responsive`] reproduces 64 of
//! them and reaches 5.35 per 10 minutes on one trail hike.
//!
//! Skips when the corpus is absent so a checkout without it still passes.
use std::collections::BTreeMap;

use ptiles_motion::movement::{
    classify_with_history, AccelStats, DebounceConfig, MovementType, Vote, VoteDebouncer,
};

const RATE_HZ: u32 = 50;
/// Trailing accelerometer samples per fix, mirroring the recorder's window.
const CAP: usize = 200;

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccelStream {
    t_ms: Vec<i64>,
    x: Vec<f32>,
    y: Vec<f32>,
    z: Vec<f32>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScFix {
    t_ms: i64,
    #[serde(default)]
    speed: Option<f64>,
    #[serde(default)]
    accuracy: Option<f64>,
}

#[derive(serde::Deserialize)]
struct Scenario {
    name: String,
    label: String,
    prime: String,
    expect: String,
    accel: AccelStream,
    fixes: Vec<ScFix>,
}

#[derive(serde::Deserialize)]
struct Corpus {
    scenarios: Vec<Scenario>,
}

fn movement(name: &str) -> MovementType {
    match name {
        "Stationary" => MovementType::Stationary,
        "Walking" => MovementType::Walking,
        "Running" => MovementType::Running,
        "Driving" => MovementType::Driving,
        _ => MovementType::Unknown,
    }
}

/// `None` only when the corpus is genuinely absent.
///
/// A parse failure PANICS rather than returning None. Folding both into "not
/// present" is how a corpus that silently stopped deserializing would read as a
/// clean skip forever -- which is exactly what happened when these structs were
/// first written against snake_case keys the Kotlin recorder does not emit.
fn load() -> Option<Corpus> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/scenarios.json.gz");
    let file = std::fs::File::open(path).ok()?;
    let reader = flate2::read::GzDecoder::new(file);
    Some(serde_json::from_reader(reader).expect("corpus present but unreadable"))
}

/// Prime into the recorded precondition, then replay the raw sensors.
fn replay(s: &Scenario, cfg: DebounceConfig) -> MovementType {
    let mut deb = VoteDebouncer::new(cfg);
    let prime = movement(&s.prime);
    let mut t: u64 = 0;
    for _ in 0..8 {
        t += 3_000;
        deb.tick(&Vote { movement: prime, confidence: 1.0 }, t);
    }
    let mut prev = prime;
    let mut state = prime;
    for fix in &s.fixes {
        let end = match s.accel.t_ms.iter().rposition(|&ts| ts <= fix.t_ms) {
            Some(i) => i + 1,
            None => continue,
        };
        let start = end.saturating_sub(CAP);
        if end - start < 8 {
            continue;
        }
        let stats = AccelStats::calculate(
            &s.accel.x[start..end],
            &s.accel.y[start..end],
            &s.accel.z[start..end],
            RATE_HZ,
        );
        let vote = classify_with_history(fix.speed, fix.accuracy, None, Some(&stats), None, prev);
        prev = vote.movement;
        // The recorder's clock runs after the priming window, so time stays monotonic.
        state = deb.tick(&vote, (fix.t_ms as u64) + 60_000);
    }
    state
}

fn score(corpus: &Corpus, cfg: DebounceConfig) -> BTreeMap<String, (usize, usize)> {
    let mut by_label: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for s in &corpus.scenarios {
        let reached = replay(s, cfg);
        let entry = by_label.entry(s.label.clone()).or_insert((0, 0));
        entry.1 += 1;
        if reached == movement(&s.expect) {
            entry.0 += 1;
        }
    }
    by_label
}

#[test]
fn the_corpus_is_present_and_covers_transitions_and_ambiguity() {
    let Some(corpus) = load() else {
        eprintln!("skipping: tests/data/scenarios.json.gz not present");
        return;
    };
    assert!(corpus.scenarios.len() >= 400, "expected a mined corpus, got {}", corpus.scenarios.len());
    let labels: std::collections::BTreeSet<_> = corpus.scenarios.iter().map(|s| s.label.as_str()).collect();
    assert!(labels.contains("transition"), "expected transitions, got {labels:?}");
    // The ambiguous buckets are the point: a corpus of clean transitions would
    // reward a classifier that only ever fires when the answer is obvious.
    assert!(labels.len() > 1, "expected ambiguous sections too, got {labels:?}");
}

#[test]
fn responsive_detects_transitions_the_defaults_miss() {
    let Some(corpus) = load() else {
        eprintln!("skipping: tests/data/scenarios.json.gz not present");
        return;
    };
    let default_scored = score(&corpus, DebounceConfig::default());
    let responsive_scored = score(&corpus, DebounceConfig::responsive());
    let d = default_scored.get("transition").copied().unwrap_or((0, 0));
    let r = responsive_scored.get("transition").copied().unwrap_or((0, 0));
    eprintln!("transitions: default {}/{}, responsive {}/{}", d.0, d.1, r.0, r.1);
    for (label, (ok, total)) in &responsive_scored {
        eprintln!("  responsive {label}: {ok}/{total}");
    }
    // The claim `responsive()` exists to make. Deliberately a floor rather than
    // an exact count: the point is the direction and its size, and pinning an
    // exact number would break on any classifier improvement.
    assert!(
        r.0 > d.0 * 4 && r.0 >= 40,
        "responsive should detect far more real transitions (default {}/{}, responsive {}/{})",
        d.0, d.1, r.0, r.1
    );
}
