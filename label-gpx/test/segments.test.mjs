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
  sampleIndices, createHistory, timePerLabel,
} from "../js/segments.js";
import { LABELS } from "../js/gpx.js";

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
  // The round trip is a human action, so it stays marked as one.
  assert.equal(back[target].edited, true);
});

test("a split outside the segment is refused rather than corrupting it", { skip: !have }, () => {
  const pts = points(DRIVE);
  const segs = coalesce(pts, classifyTrace(wasm, pts));
  assert.equal(splitSegment(segs, 0, segs[0].start), segs, "splitting at the start is a no-op");
  assert.equal(splitSegment(segs, 0, segs[0].end + 1), segs, "past the end is a no-op");
  assert.equal(mergeWithPrevious(segs, 0), segs, "nothing to merge the first into");
});

test("relabelling marks human intent and re-merges neighbours", () => {
  const segs = [
    { start: 0, end: 4, type: "walking", edited: false, points: 5, t0: 0, t1: 4000 },
    { start: 5, end: 9, type: "stationary", edited: false, points: 5, t0: 5000, t1: 9000 },
    { start: 10, end: 14, type: "walking", edited: false, points: 5, t0: 10000, t1: 14000 },
  ];
  const out = relabel(segs, 1, "walking");
  assert.equal(out.length, 1, "the three became one run of walking");
  assert.equal(out[0].start, 0);
  assert.equal(out[0].end, 14);
  assert.equal(out[0].edited, true);
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

test("autoMerge keeps spans contiguous", () => {
  const merged = autoMerge([
    { start: 0, end: 3, type: "walking", edited: false, t0: 0, t1: 3 },
    { start: 4, end: 7, type: "walking", edited: true, t0: 4, t1: 7 },
    { start: 8, end: 9, type: "driving", edited: false, t0: 8, t1: 9 },
  ]);
  assert.equal(merged.length, 2);
  assert.deepEqual([merged[0].start, merged[0].end], [0, 7]);
  assert.equal(merged[0].edited, true, "an edited half makes the merge edited");
  assert.equal(merged[0].points, 8);
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
