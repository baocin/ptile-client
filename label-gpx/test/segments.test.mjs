// The labeling pipeline, against the real wasm build and the real traces.
//
// `js/segments.js` is written DOM-free precisely so this can run under
// `node --test` with no browser and no dependencies: it drives the same
// `MovementTracker` the page drives, over the same six ODbL traces the Rust
// tests replay (`motion/tests/gpx_replay.rs`), which makes this a cross-language
// agreement check as well as a unit test.
//
// `js/gpx.js` is NOT covered here -- it is `DOMParser`/`XMLSerializer`, which
// node does not provide. Its round trip is checked in the browser, by
// test/round_trip.py.
//
//   PATH="$HOME/.cargo/bin:$PATH" wasm-pack build wasm --target nodejs --out-dir ../wasm-pkg --release
//   node --test label-gpx/test/segments.test.mjs

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { createRequire } from "node:module";

import {
  classifyTrace, coalesce, autoMerge, splitSegment, mergeWithPrevious, relabel,
  sampleIndices, createHistory, timePerLabel, sliceRange, dominantBand,
  bandByHeight, moveBoundary,
} from "../js/segments.js";
import { speedBands } from "../js/chart.js";
import { LABELS } from "../js/gpx.js";
import {
  PTILES_BASE, SNAPSHOT, categoryLabel, pointInPolygon, stateAt, stateUrl,
} from "../js/context.js";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = join(HERE, "..", "..");
const WASM_PKG = join(ROOT, "wasm-pkg", "ptiles_wasm.js");
const GPX = join(ROOT, "test-fixtures", "gpx");

if (!existsSync(WASM_PKG)) {
  throw new Error(
    `wasm-pkg/ not built -- this test cannot run.\n` +
      `  PATH="$HOME/.cargo/bin:$PATH" wasm-pack build wasm --target nodejs --out-dir ../wasm-pkg --release`,
  );
}
const wasm = createRequire(import.meta.url)(WASM_PKG);
const GOLDEN = join(ROOT, "test-fixtures", "golden");

/** A golden fixture as bytes, or `null` when it is not checked out. */
function fixture(name) {
  const path = join(GOLDEN, name);
  return existsSync(path) ? new Uint8Array(readFileSync(path)) : null;
}

/**
 * Minimal trkpt scan, deliberately not `js/gpx.js`.
 *
 * The point of these tests is the labeling pipeline, and node has no DOM, so
 * the fixtures are read with the same string scan `motion/tests/gpx_replay.rs`
 * uses. That also means a bug in gpx.js cannot make these tests pass or fail
 * for the wrong reason.
 */
function points(stem) {
  const xml = readFileSync(join(GPX, `${stem}.gpx`), "utf8");
  const out = [];
  const re = /<trkpt[^>]*lat="([-\d.]+)"[^>]*lon="([-\d.]+)"[^>]*>([\s\S]*?)<\/trkpt>/g;
  for (const m of xml.matchAll(re)) {
    const t = /<time>([^<]+)<\/time>/.exec(m[3]);
    if (!t) continue;
    const t_ms = Date.parse(t[1]);
    if (!Number.isFinite(t_ms)) continue;
    out.push({ lat: parseFloat(m[1]), lon: parseFloat(m[2]), t_ms });
  }
  return out;
}

/** The vehicular trace and a foot trace, the two ends of the behaviour. */
const DRIVE = "tn-middle-tennessee-3605997";
const WALK = "tn-maryville-hike-1063250";

const have = existsSync(join(GPX, `${DRIVE}.gpx`));

test("the label vocabulary matches the Rust MovementType", () => {
  // Cross-language drift check: `LABELS` is what the dropdown offers and what
  // lands in a fixture's <name>, and it has to be the enum in
  // motion/src/movement.rs. A variant added there and not here silently becomes
  // unlabelable.
  const rust = readFileSync(join(ROOT, "motion", "src", "movement.rs"), "utf8");
  const body = /pub enum MovementType \{([\s\S]*?)\n\}/.exec(rust);
  assert.ok(body, "could not find MovementType in movement.rs");
  const variants = [...body[1].matchAll(/^\s{4}(\w+),/gm)].map((m) => m[1].toLowerCase());
  assert.deepEqual([...LABELS].sort(), variants.sort());
});

test("a real drive classifies as driving, a real walk does not", { skip: !have }, () => {
  const drive = points(DRIVE);
  assert.equal(drive.length, 1187);
  const results = classifyTrace(wasm, drive);
  assert.equal(results.length, drive.length);
  const segs = coalesce(drive, results);
  assert.ok(segs.length > 0);
  const driving = segs.filter((s) => s.type === "driving");
  assert.ok(driving.length > 0, `no driving segments: ${segs.map((s) => s.type).join(",")}`);
  // Time, not segment count: a trip is what you spent it doing.
  const per = timePerLabel(segs);
  const total = [...per.values()].reduce((a, b) => a + b, 0);
  assert.ok(per.get("driving") / total > 0.5, `driving share ${per.get("driving") / total}`);

  const walk = points(WALK);
  const walkSegs = coalesce(walk, classifyTrace(wasm, walk));
  const walkPer = timePerLabel(walkSegs);
  assert.ok(
    !walkPer.get("driving"),
    `a 1.2 m/s walk must not produce driving segments: ${walkSegs.map((s) => s.type).join(",")}`,
  );
});

test("the tracker derives speed when the file reports none", { skip: !have }, () => {
  const pts = points(DRIVE);
  classifyTrace(wasm, pts);
  const derived = pts.filter((p) => p.derivedSpeed !== undefined);
  assert.ok(derived.length > pts.length * 0.9, `${derived.length}/${pts.length} derived`);
  // Sanity: a highway drive averages well above a walking pace.
  const mean = derived.reduce((a, p) => a + p.derivedSpeed, 0) / derived.length;
  assert.ok(mean > 5, `mean derived speed ${mean} m/s`);
  // And every point knows its index, which the exporter relies on.
  assert.equal(pts[42].index, 42);
});

test("segments tile the trace exactly, with no gaps or overlaps", { skip: !have }, () => {
  const pts = points(DRIVE);
  const segs = coalesce(pts, classifyTrace(wasm, pts));
  assert.equal(segs[0].start, 0);
  assert.equal(segs.at(-1).end, pts.length - 1);
  for (let i = 1; i < segs.length; i++) {
    assert.equal(segs[i].start, segs[i - 1].end + 1, `gap before segment ${i}`);
  }
  for (const s of segs) {
    assert.equal(s.points, s.end - s.start + 1);
    assert.ok(s.t1 >= s.t0);
    assert.ok(LABELS.includes(s.type));
    assert.equal(s.edited, false, "a fresh coalesce is all auto");
  }
});

test("adjacent runs of the same label are never left split", { skip: !have }, () => {
  const pts = points(DRIVE);
  const segs = coalesce(pts, classifyTrace(wasm, pts));
  for (let i = 1; i < segs.length; i++) {
    assert.notEqual(segs[i].type, segs[i - 1].type, `segments ${i - 1}/${i} share a label`);
  }
});

test("split then merge is the identity", { skip: !have }, () => {
  const pts = points(DRIVE);
  const segs = coalesce(pts, classifyTrace(wasm, pts));
  const target = segs.findIndex((s) => s.points > 10);
  const at = segs[target].start + 5;
  const split = splitSegment(segs, target, at);
  assert.equal(split.length, segs.length + 1);
  assert.equal(split[target].end, at - 1);
  assert.equal(split[target + 1].start, at);
  const back = mergeWithPrevious(split, target + 1);
  assert.equal(back.length, segs.length);
  assert.equal(back[target].start, segs[target].start);
  assert.equal(back[target].end, segs[target].end);
  // Merging *is* a claim -- "this whole span is one thing" -- so the result is
  // marked human even though the split that preceded it was not.
  assert.equal(back[target].edited, true);
});

test("a split outside the segment is refused rather than corrupting it", { skip: !have }, () => {
  const pts = points(DRIVE);
  const segs = coalesce(pts, classifyTrace(wasm, pts));
  assert.equal(splitSegment(segs, 0, segs[0].start), segs, "splitting at the start is a no-op");
  assert.equal(splitSegment(segs, 0, segs[0].end + 1), segs, "past the end is a no-op");
  assert.equal(mergeWithPrevious(segs, 0), segs, "nothing to merge the first into");
});

test("relabelling marks human intent, and does not extend it to its neighbours", () => {
  const segs = [
    { start: 0, end: 4, type: "walking", edited: false, points: 5, t0: 0, t1: 4000 },
    { start: 5, end: 9, type: "stationary", edited: false, points: 5, t0: 5000, t1: 9000 },
    { start: 10, end: 14, type: "walking", edited: false, points: 5, t0: 10000, t1: 14000 },
  ];
  const out = relabel(segs, 1, "walking");
  // Three spans of walking, and only the middle one is a human's claim. Merging
  // them would export the outer two as `source="human"` as well, which is a
  // statement about ground the person never looked at.
  assert.equal(out.length, 3, JSON.stringify(out.map((s) => [s.start, s.end, s.edited])));
  assert.deepEqual(out.map((s) => s.type), ["walking", "walking", "walking"]);
  assert.deepEqual(out.map((s) => s.edited), [false, true, false]);
  assert.deepEqual([out[1].start, out[1].end], [5, 9]);
  // Confirming the classifier's own label still counts as human input --
  // SCHEMA.md distinguishes source="human" from source="auto".
  const same = relabel(segs, 0, "walking");
  assert.equal(same[0].edited, true);
  assert.throws(() => relabel(segs, 0, "cycling"), /not a MovementType/);
});

test("runt runs are absorbed instead of cluttering the table", () => {
  // Synthetic results: one 2-point blip inside a long run. minPoints=3 folds it
  // into the previous segment rather than leaving a segment nobody can label.
  const results = [
    ...Array(10).fill({ movement: "driving", vote: "driving", confidence: 0.9 }),
    ...Array(2).fill({ movement: "walking", vote: "walking", confidence: 0.5 }),
    ...Array(10).fill({ movement: "driving", vote: "driving", confidence: 0.9 }),
  ];
  const pts = results.map((_, i) => ({ lat: 36, lon: -86, t_ms: i * 1000 }));
  const segs = coalesce(pts, results);
  assert.equal(segs.length, 1);
  assert.equal(segs[0].type, "driving");
  assert.equal(segs[0].points, 22);
});

test("autoMerge joins same label and same provenance, and not across them", () => {
  // Same label, both auto: one segment.
  const both = autoMerge([
    { start: 0, end: 3, type: "walking", edited: false, t0: 0, t1: 3 },
    { start: 4, end: 7, type: "walking", edited: false, t0: 4, t1: 7 },
  ]);
  assert.equal(both.length, 1);
  assert.deepEqual([both[0].start, both[0].end, both[0].points], [0, 7, 8]);

  // Same label, one human: kept apart. Merging them would extend a human's claim
  // over ground they never looked at, and `source="human"` is the whole basis for
  // treating a segment as evidence.
  const mixed = autoMerge([
    { start: 0, end: 3, type: "walking", edited: false, t0: 0, t1: 3 },
    { start: 4, end: 7, type: "walking", edited: true, t0: 4, t1: 7 },
    { start: 8, end: 9, type: "driving", edited: false, t0: 8, t1: 9 },
  ]);
  assert.equal(mixed.length, 3);
  assert.deepEqual(mixed.map((s) => s.edited), [false, true, false]);
});

test("a slice keeps its human span exactly, without absorbing its neighbours", () => {
  // The case that motivated the provenance rule: cutting a range out of a
  // like-labelled stretch. The slice must not swallow the rest of it.
  const segs = [{ start: 0, end: 99, type: "driving", edited: false, points: 100, t0: 0, t1: 99000 }];
  const out = sliceRange(segs, 40, 59, "driving");
  assert.equal(out.length, 3, JSON.stringify(out.map((s) => [s.start, s.end, s.edited])));
  assert.deepEqual(out.map((s) => [s.start, s.end]), [[0, 39], [40, 59], [60, 99]]);
  // Only the drawn range is the human's claim; the pieces either side keep the
  // provenance they had, which is what stops one drag from vouching for an hour.
  assert.deepEqual(out.map((s) => s.edited), [false, true, false]);
});

test("sampleIndices spans the segment and never leaves it", () => {
  const seg = { start: 100, end: 199 };
  const s = sampleIndices(seg, 5);
  assert.deepEqual(s, [100, 125, 150, 174, 199]);
  for (const i of s) assert.ok(i >= seg.start && i <= seg.end);
  // A short segment yields every point, without duplicates.
  assert.deepEqual(sampleIndices({ start: 7, end: 9 }, 5), [7, 8, 9]);
  assert.deepEqual(sampleIndices({ start: 7, end: 7 }, 5), [7]);
});

test("undo restores a snapshot and is bounded", () => {
  const h = createHistory(2);
  const a = [{ start: 0, end: 1, type: "walking", edited: false }];
  h.snapshot(a);
  a[0].type = "driving";
  const back = h.undo();
  assert.equal(back[0].type, "walking", "snapshot must be a deep copy");
  assert.equal(h.undo(), null, "empty history returns null");
  h.snapshot(a);
  h.snapshot(a);
  h.snapshot(a);
  assert.equal(h.depth, 2, "history is capped");
});

test("road context changes the answer on a real trace", { skip: !have }, () => {
  // The whole reason the page resolves map context. Feeding a footway prior at
  // every point turns a walk the speed bands cannot see (1.2 m/s is below the
  // 2.2 m/s floor) into walking. This uses a synthetic context rather than the
  // tile host so the test stays hermetic; road_context.rs does the same thing
  // against real decoded road records.
  const pts = points(WALK);
  const blind = coalesce(pts, classifyTrace(wasm, pts));
  const withRoad = coalesce(
    pts,
    classifyTrace(wasm, pts, {
      contextFor: () => ({ road: { road_class: "footway", distance_m: 2.0 } }),
    }),
  );
  const share = (segs, type) => {
    const per = timePerLabel(segs);
    const total = [...per.values()].reduce((a, b) => a + b, 0);
    return (per.get(type) ?? 0) / total;
  };
  assert.ok(
    share(withRoad, "walking") > share(blind, "walking"),
    `walking share ${share(blind, "walking")} -> ${share(withRoad, "walking")} with a footway prior`,
  );
});

test("an intersection keeps a stop from ending the drive", { skip: !have }, () => {
  // Same trace, twice: once with a signals node reported at every point, once
  // without. The stops inside the drive must survive longer with the map's help,
  // so the trip is not chopped into arrivals at every red light.
  const pts = points(DRIVE);
  const plain = coalesce(pts, classifyTrace(wasm, pts));
  const sticky = coalesce(
    pts,
    classifyTrace(wasm, pts, {
      contextFor: () => ({ intersection: { distance_m: 8, intersection_type: 1 } }),
    }),
  );
  const stationary = (segs) => segs.filter((s) => s.type === "stationary").length;
  assert.ok(
    stationary(sticky) <= stationary(plain),
    `stationary segments ${stationary(plain)} -> ${stationary(sticky)} with signals reported`,
  );
  assert.ok(sticky.some((s) => s.atControl), "at_traffic_control should be reported back");
});

test("a sliced range becomes its own segment and disturbs nothing outside it", () => {
  const segs = [
    { start: 0, end: 29, type: "driving", edited: false, points: 30, t0: 0, t1: 29000 },
    { start: 30, end: 59, type: "walking", edited: false, points: 30, t0: 30000, t1: 59000 },
  ];
  const out = sliceRange(segs, 10, 39, "stationary");
  // The slice exists, exactly where it was drawn.
  const slice = out.find((s) => s.type === "stationary");
  assert.ok(slice, `no slice in ${JSON.stringify(out.map((s) => s.type))}`);
  assert.equal(slice.start, 10);
  assert.equal(slice.end, 39);
  assert.equal(slice.edited, true, "a person drew it");
  // And the untouched remainder keeps its old labels and its old boundaries.
  assert.deepEqual(
    out.map((s) => [s.start, s.end, s.type]),
    [[0, 9, "driving"], [10, 39, "stationary"], [40, 59, "walking"]],
  );
  // Still a complete tiling: this is the invariant a botched split breaks.
  assert.equal(out[0].start, 0);
  assert.equal(out.at(-1).end, 59);
  for (let i = 1; i < out.length; i++) assert.equal(out[i].start, out[i - 1].end + 1);
});

test("slicing is idempotent and order-insensitive", () => {
  const segs = [{ start: 0, end: 99, type: "driving", edited: false, points: 100, t0: 0, t1: 99000 }];
  const once = sliceRange(segs, 20, 40, "walking");
  const twice = sliceRange(once, 20, 40, "walking");
  assert.deepEqual(
    twice.map((s) => [s.start, s.end, s.type]),
    once.map((s) => [s.start, s.end, s.type]),
  );
  // Dragging right-to-left is the same slice.
  const backwards = sliceRange(segs, 40, 20, "walking");
  assert.deepEqual(
    backwards.map((s) => [s.start, s.end, s.type]),
    once.map((s) => [s.start, s.end, s.type]),
  );
  assert.throws(() => sliceRange(segs, 1, 2, "cycling"), /not a MovementType/);
});

test("a slice at a segment edge does not create empty segments", () => {
  const segs = [
    { start: 0, end: 9, type: "driving", edited: false, points: 10, t0: 0, t1: 9000 },
    { start: 10, end: 19, type: "walking", edited: false, points: 10, t0: 10000, t1: 19000 },
  ];
  for (const [lo, hi] of [[0, 9], [10, 19], [0, 19]]) {
    const out = sliceRange(segs, lo, hi, "stationary");
    assert.ok(out.every((s) => s.end >= s.start), `empty segment from ${lo}-${hi}`);
    assert.equal(out[0].start, 0);
    assert.equal(out.at(-1).end, 19);
  }
});

test("the dominant band ignores samples outside the rectangle", () => {
  // Eight points: four slow, four fast, and one absurd spike among the slow ones.
  const points = Array.from({ length: 8 }, (_, i) => ({ lat: 36, lon: -86, t_ms: i * 1000 }));
  const results = [1.0, 1.1, 40.0, 1.2, 12.0, 12.5, 13.0, 12.2].map((speed) => ({ speed }));
  const band = (v) => (v <= 0.5 ? "stationary" : v >= 5 ? "driving" : "walking");

  // Whole series, whole speed range: driving wins 5-3 because the spike counts.
  const all = dominantBand(points, results, { t0: 0, t1: 7000, vMin: 0, vMax: 100 }, band);
  assert.equal(all.type, "driving");
  assert.equal(all.counted, 8);

  // Same time range, but the rectangle's top excludes the spike: walking wins.
  const capped = dominantBand(points, results, { t0: 0, t1: 3000, vMin: 0, vMax: 5 }, band);
  assert.equal(capped.type, "walking");
  assert.equal(capped.counted, 3, "the 40 m/s spike is outside the box");
  assert.equal(capped.share, 1);

  // An empty box reports nothing rather than guessing a label.
  const none = dominantBand(points, results, { t0: 0, t1: 3000, vMin: 20, vMax: 30 }, band);
  assert.equal(none.type, null);
  assert.equal(none.counted, 0);
});

test("the dominant band reports its share, not just a winner", { skip: !have }, () => {
  const pts = points(DRIVE);
  const results = classifyTrace(wasm, pts);
  const band = (v) => wasm.speed_band(v);
  const t0 = pts[0].t_ms;
  const t1 = pts[Math.floor(pts.length / 4)].t_ms;
  const got = dominantBand(pts, results, { t0, t1, vMin: 0, vMax: 1e9 }, band);
  assert.ok(got.counted > 50, `only ${got.counted} samples counted`);
  assert.ok(got.share > 0 && got.share <= 1, `share ${got.share}`);
  // The label has to be one the library recognises -- it came from wasm.
  assert.ok(LABELS.includes(got.type), got.type);
});

test("the library owns the thresholds the chart draws", { skip: !have }, () => {
  const t = wasm.motion_thresholds();
  assert.ok(t.stationary_max_mps > 0 && t.driving_min_mps > t.stationary_max_mps, JSON.stringify(t));
  // The stateless tree's floors sit above the smoothed bands on purpose: that
  // path has no smoothing behind it, so one fast fix must not read as driving.
  assert.ok(t.walking_ceiling_mps > t.stationary_max_mps, JSON.stringify(t));
  assert.ok(t.driving_floor_mps > t.driving_min_mps, JSON.stringify(t));
  // And speed_band agrees with those numbers rather than having its own.
  assert.equal(wasm.speed_band(t.stationary_max_mps), "stationary");
  assert.equal(wasm.speed_band(t.stationary_max_mps + 0.01), "walking");
  assert.equal(wasm.speed_band(t.driving_min_mps), "driving");
});

test("every structural change keeps the timestamps matching the spans", { skip: !have }, () => {
  // The bug this pins was visible in the table: three consecutive segments all
  // showing 18:03:50 and 67.6 min, because splitting copied the parent's
  // timestamps into both halves. Those same fields export as start_time and
  // end_time, so wrong times reach the fixture.
  const pts = points(DRIVE);
  const base = coalesce(pts, classifyTrace(wasm, pts));
  const check = (segs, what) => {
    for (const s of segs) {
      assert.equal(s.t0, pts[s.start].t_ms, `${what}: t0 of ${s.start}-${s.end}`);
      assert.equal(s.t1, pts[s.end].t_ms, `${what}: t1 of ${s.start}-${s.end}`);
    }
    const starts = segs.map((s) => s.t0);
    assert.equal(new Set(starts).size, starts.length, `${what}: duplicate start times`);
  };
  check(base, "coalesce");

  const big = base.findIndex((s) => s.points > 40);
  const at = base[big].start + 20;
  const split = splitSegment(base, big, at, pts);
  check(split, "split");

  const merged = mergeWithPrevious(split, big + 1, pts);
  check(merged, "merge");

  const sliced = sliceRange(base, at, at + 19, "stationary", pts);
  check(sliced, "slice");
  // And the sliced span really is the one that was asked for.
  const mine = sliced.find((s) => s.edited);
  assert.equal(mine.start, at);
  assert.equal(mine.end, at + 19);
  assert.equal(mine.t0, pts[at].t_ms);
});

test("speedBands come from the library's numbers, with the aid marked", { skip: !have }, () => {
  const t = wasm.motion_thresholds();
  const bands = speedBands(t);
  assert.deepEqual(bands.map((b) => b.label), ["stationary", "walking", "running", "driving"]);
  // Contiguous, ascending, and pinned to the library's thresholds at both real
  // boundaries.
  assert.equal(bands[0].lo, 0);
  assert.equal(bands[0].hi, t.stationary_max_mps);
  assert.equal(bands[1].hi, t.running_hint_mps);
  assert.equal(bands[2].hi, t.driving_min_mps);
  assert.equal(bands[3].hi, Infinity);
  for (let i = 1; i < bands.length; i++) assert.equal(bands[i].lo, bands[i - 1].hi);
  // Exactly one band is flagged as a labelling aid, and it is the running one --
  // the classifier never infers Running from speed.
  assert.deepEqual(bands.filter((b) => b.aid).map((b) => b.label), ["running"]);
  // Degenerate thresholds yield nothing rather than a bogus axis.
  assert.deepEqual(speedBands({}), []);
  assert.deepEqual(speedBands(null), []);
});

test("a dragged box takes its label from its own height", { skip: !have }, () => {
  const bands = speedBands(wasm.motion_thresholds());
  // A box wholly inside one band is that band, regardless of samples.
  assert.equal(bandByHeight({ vMin: 8, vMax: 20 }, bands).type, "driving");
  assert.equal(bandByHeight({ vMin: 0.1, vMax: 0.4 }, bands).type, "stationary");
  assert.equal(bandByHeight({ vMin: 3.0, vMax: 4.5 }, bands).type, "running");
  assert.equal(bandByHeight({ vMin: 0.8, vMax: 2.0 }, bands).type, "walking");

  // Straddling: the band covering most of the height wins, and the share says by
  // how much rather than implying certainty.
  const straddle = bandByHeight({ vMin: 4.0, vMax: 20.0 }, bands);
  assert.equal(straddle.type, "driving");
  assert.ok(straddle.share > 0.9, `share ${straddle.share}`);
  const mostlyRunning = bandByHeight({ vMin: 2.7, vMax: 5.3 }, bands);
  assert.equal(mostlyRunning.type, "running");
  assert.ok(mostlyRunning.share > 0.5 && mostlyRunning.share < 1);

  // Drag direction does not matter, and a flat drag still points at one band.
  assert.equal(bandByHeight({ vMin: 20, vMax: 8 }, bands).type, "driving");
  assert.equal(bandByHeight({ vMin: 12, vMax: 12 }, bands).type, "driving");
  assert.equal(bandByHeight({ vMin: 12, vMax: 12 }, bands).share, 1);
  // No bands to compare against: no claim.
  assert.equal(bandByHeight({ vMin: 1, vMax: 2 }, []).type, null);
});

test("moving a boundary shifts exactly one, and marks both sides human", () => {
  const points = Array.from({ length: 30 }, (_, i) => ({ lat: 36, lon: -86, t_ms: i * 1000 }));
  const segs = [
    { start: 0, end: 9, type: "driving", edited: false, points: 10, t0: 0, t1: 9000 },
    { start: 10, end: 19, type: "walking", edited: false, points: 10, t0: 10000, t1: 19000 },
    { start: 20, end: 29, type: "stationary", edited: false, points: 10, t0: 20000, t1: 29000 },
  ];
  const out = moveBoundary(segs, 1, 5, points);
  assert.deepEqual(out.map((s) => [s.start, s.end]), [[0, 4], [5, 19], [20, 29]]);
  // Both sides of the boundary asserted something, so both are human. The third
  // segment was not touched and must not claim to be.
  assert.deepEqual(out.map((s) => s.edited), [true, true, false]);
  // Times track the new spans -- the same class of bug as the split fix.
  assert.equal(out[0].t1, points[4].t_ms);
  assert.equal(out[1].t0, points[5].t_ms);
  assert.equal(out[1].points, 15);

  // Clamped so neither side can be emptied.
  const toStart = moveBoundary(segs, 1, 0, points);
  assert.equal(toStart[0].start, 0);
  assert.ok(toStart[0].end >= toStart[0].start, "left side survived");
  assert.equal(toStart[0].points, 1);
  const toEnd = moveBoundary(segs, 1, 99, points);
  assert.ok(toEnd[1].end >= toEnd[1].start, "right side survived");
  assert.equal(toEnd[1].points, 1);

  // A no-op move changes nothing, and the ends have no boundary to move.
  assert.equal(moveBoundary(segs, 1, 10, points), segs);
  assert.equal(moveBoundary(segs, 0, 5, points), segs);
  assert.equal(moveBoundary(segs, 3, 5, points), segs);
});

test("a boundary move keeps the trace tiled", () => {
  const points = Array.from({ length: 40 }, (_, i) => ({ lat: 36, lon: -86, t_ms: i * 1000 }));
  let segs = [
    { start: 0, end: 19, type: "driving", edited: false, points: 20, t0: 0, t1: 19000 },
    { start: 20, end: 39, type: "walking", edited: false, points: 20, t0: 20000, t1: 39000 },
  ];
  for (const at of [10, 30, 5, 35, 20]) {
    segs = moveBoundary(segs, 1, at, points);
    assert.equal(segs[0].start, 0);
    assert.equal(segs.at(-1).end, 39);
    for (let i = 1; i < segs.length; i++) {
      assert.equal(segs[i].start, segs[i - 1].end + 1, `gap after moving to ${at}`);
    }
    for (const s of segs) assert.ok(s.end >= s.start, `empty segment after moving to ${at}`);
  }
});

// --- the categories sidecar, and the v4 decoder it labels ------------------

test("categoryLabel reads the sidecar 1-based and flattens both name shapes", () => {
  // Shape and indexing taken from the published TN sidecar: index 83 is
  // "public_and_government_association", which is `categories[82]`.
  const names = ["Community and Government > Spiritual Center > Church", "church_cathedral", "Office"];
  assert.equal(categoryLabel(names, 1), "Church");
  assert.equal(categoryLabel(names, 2), "church cathedral");
  assert.equal(categoryLabel(names, 3), "Office");
  // 0 is "no category", not the first entry -- reading it 0-based would label
  // every uncategorised POI as a church and nothing would error.
  assert.equal(categoryLabel(names, 0), null);
  // Past the end means the sidecar is a different vintage than the layer.
  assert.equal(categoryLabel(names, 4), null);
  assert.equal(categoryLabel(names, 255), null);
  // A missing or malformed sidecar degrades to no label, never to a throw.
  assert.equal(categoryLabel([], 3), null);
  assert.equal(categoryLabel(null, 3), null);
  assert.equal(categoryLabel([""], 1), null);
});

test("pointInPolygon answers containment on [lon, lat] rings", () => {
  // A unit square from (0,0) to (2,2) in [lon, lat] order.
  const square = [[0, 0], [2, 0], [2, 2], [0, 2]];
  assert.equal(pointInPolygon(1, 1, square), true);
  assert.equal(pointInPolygon(3, 1, square), false);
  assert.equal(pointInPolygon(1, 3, square), false);
});

test("stateAt resolves representative states and rejects points outside coverage", () => {
  assert.equal(stateAt(36.1627, -86.7816), "TN", "Nashville");
  assert.equal(stateAt(35.7796, -78.6382), "NC", "Raleigh");
  assert.equal(stateAt(38.9072, -77.0369), "DC", "Washington");
  assert.equal(stateAt(20.7984, -156.3319), "HI", "Maui");
  assert.equal(stateAt(64.2008, -149.4937), "AK", "Alaska");
  assert.equal(stateAt(0, 0), null);
  assert.equal(stateAt(Number.NaN, -86), null);
  assert.equal(stateAt(36, Number.POSITIVE_INFINITY), null);
});

test("stateAt chooses the nearest state centre when rough bboxes overlap", () => {
  // Rough state boxes intentionally overlap. Each capital-ish point below is
  // inside at least one neighbouring box too, so nearest-centre selection is
  // what keeps the filename stable rather than object iteration order.
  assert.equal(stateAt(39.0458, -76.6413), "MD", "Annapolis");
  assert.equal(stateAt(39.1582, -75.5244), "DE", "Dover");
  assert.equal(stateAt(41.7658, -72.6734), "CT", "Hartford");
});

test("stateUrl uses the dated snapshot and every versioned layer stem", () => {
  assert.match(SNAPSHOT, /^\d{4}-\d{2}-\d{2}$/);
  assert.ok(PTILES_BASE.endsWith(`/${SNAPSHOT}/`));
  const stems = {
    roads: "roads_v2",
    water: "water_v1",
    parks: "parks_v1",
    trails: "trails_v1",
    rail: "rail_v1",
    buildings: "buildings_v9",
    business: "business_v4",
    address: "address_v2",
    admin: "admin",
  };
  for (const [layer, stem] of Object.entries(stems)) {
    assert.equal(stateUrl("TN", layer), `${PTILES_BASE}TN.${stem}.ptiles`, layer);
  }
  // Unknown layer names remain usable for forward-compatible experiments.
  assert.equal(stateUrl("US", "camera"), `${PTILES_BASE}US.camera.ptiles`);
});

test("the real v4 business block decodes flush and in place", () => {
  const block = fixture("business_v4.block.bin");
  const meta = fixture("business_v4.meta.json");
  if (!block || !meta) return; // fixture absent: nothing to assert
  const m = JSON.parse(new TextDecoder().decode(meta));
  const recs = wasm.decode_business_for_cell(block, m.cell_id_hex);
  // Exactly the index's count. This is the assertion the trailer bug broke: the
  // old sniffing decoder produced garbage records and then threw
  // "unexpected end of input at offset 42".
  assert.equal(recs.length, m.feature_count_in_index);
  for (const b of recs) {
    // Not JSON.stringify: osm_id is a BigInt, which it refuses to serialise.
    assert.ok(b.name.length > 0, `unnamed record at ${b.lat},${b.lon}`);
    assert.ok([1, 2].includes(b.source_type), `source_type ${b.source_type} on ${b.name}`);
    // Inside its own cell, not off Null Island -- v4 coordinates are offsets
    // from the cell centre, so a wrong origin is silent.
    assert.ok(Math.abs(b.lat - m.cell_center_lat) < 0.05, `${b.name} lat ${b.lat}`);
    assert.ok(Math.abs(b.lon - m.cell_center_lon) < 0.05, `${b.name} lon ${b.lon}`);
  }
});
