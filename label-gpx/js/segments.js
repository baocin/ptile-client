// Auto-split, then correct: run the trace through the classifier, coalesce the
// result into labeled segments, and let a human fix them.
//
// Deliberately free of DOM and fetch. Everything here is a function over plain
// arrays, so `node --test` can drive the whole labeling pipeline against the
// real wasm build without a browser -- which is why the only tests that exist
// for this page cover this file.

import { LABELS } from "./gpx.js";

/**
 * Run every point through `MovementTracker` and return one result per point.
 *
 * `contextFor(index)` optionally supplies `{road, intersection}` for a point;
 * the first pass calls this with nothing resolved (road-blind), and a second
 * pass can feed back per-segment context. Speed is passed as `undefined` when
 * the file did not report one, which makes the tracker derive it from position
 * deltas through core's own smoother -- the alternative would be a second,
 * disagreeing speed implementation in JavaScript.
 */
export function classifyTrace(wasm, points, { config = null, contextFor = null } = {}) {
  const tracker = new wasm.MovementTracker(config);
  const out = [];
  for (let i = 0; i < points.length; i++) {
    const p = points[i];
    const ctx = contextFor ? contextFor(i) || {} : {};
    const r = tracker.push(
      p.t_ms,
      p.lat,
      p.lon,
      p.speed,
      p.accuracy,
      p.accel ?? null,
      ctx.road ?? null,
      ctx.intersection ?? null,
    );
    // Record what the tracker derived, so export can mark it derived="true"
    // rather than presenting a computed speed as a measured one.
    if (p.speed === undefined && r.smoothed_speed_mps != null) {
      p.derivedSpeed = r.smoothed_speed_mps;
    }
    p.index = i;
    out.push({
      movement: r.movement,
      vote: r.vote.movement,
      confidence: r.vote.confidence,
      speed: r.smoothed_speed_mps ?? p.speed,
      atControl: r.at_traffic_control,
    });
  }
  return out;
}

/**
 * Coalesce per-point results into contiguous segments.
 *
 * Runs shorter than `minPoints` are absorbed into the previous segment: the
 * debouncer already suppresses brief states, so a 1-2 point run here is an
 * artefact of the *first* committed state at the start of a trace rather than a
 * real stretch, and a table full of them is unlabelable.
 */
export function coalesce(points, results, { minPoints = 3 } = {}) {
  const segs = [];
  for (let i = 0; i < results.length; i++) {
    const type = results[i].movement;
    const last = segs[segs.length - 1];
    if (last && last.type === type) {
      last.end = i;
      continue;
    }
    segs.push({ start: i, end: i, type, edited: false });
  }
  // Absorb runt runs, then merge any neighbours that became identical.
  const kept = [];
  for (const s of segs) {
    const len = s.end - s.start + 1;
    if (len < minPoints && kept.length) {
      kept[kept.length - 1].end = s.end;
    } else {
      kept.push(s);
    }
  }
  const merged = autoMerge(kept);
  for (const s of merged) annotate(s, points, results);
  return merged;
}

/** Mean confidence, dominant vote and time span for a segment. */
function annotate(s, points, results) {
  let sum = 0;
  const votes = new Map();
  for (let i = s.start; i <= s.end; i++) {
    sum += results[i].confidence ?? 0;
    votes.set(results[i].vote, (votes.get(results[i].vote) ?? 0) + 1);
  }
  const n = s.end - s.start + 1;
  s.confidence = sum / n;
  s.vote = [...votes.entries()].sort((a, b) => b[1] - a[1])[0][0];
  s.points = n;
  s.t0 = points[s.start].t_ms;
  s.t1 = points[s.end].t_ms;
  s.atControl = false;
  for (let i = s.start; i <= s.end; i++) {
    if (results[i].atControl) {
      s.atControl = true;
      break;
    }
  }
  return s;
}

/**
 * Merge adjacent segments that carry the same label *and the same provenance*.
 *
 * The provenance half is not fussiness. Slicing 25 minutes out of a 67-minute
 * driving stretch and labelling it `driving` used to merge straight back into its
 * neighbours, and because the merged span inherited `edited`, the whole 67
 * minutes then exported as `source="human"` -- a human decision about a quarter
 * of it, laundered into a claim about all of it. Fixtures are only worth having
 * if that attribute means what it says, so a human-marked span and an
 * auto-labelled one stay separate even when they agree on the label.
 */
export function autoMerge(segments) {
  const out = [];
  for (const s of segments) {
    const last = out[out.length - 1];
    if (last && last.type === s.type && !!last.edited === !!s.edited) {
      last.end = s.end;
      last.points = last.end - last.start + 1;
      last.t1 = s.t1;
      last.t0 = Math.min(last.t0 ?? s.t0, s.t0 ?? last.t0);
    } else {
      out.push({ ...s });
    }
  }
  return out;
}

/**
 * Split segment `i` so that `at` starts a new segment.
 *
 * Each half keeps whatever provenance it had. Splitting is a statement about a
 * *boundary*, not about either label: a person who cuts a stretch in two has not
 * thereby vouched for what lies on both sides, and marking both halves human
 * meant a split-then-merge exported a whole hour as `source="human"` off the back
 * of one click. `relabel` and `sliceRange` are what claim a label.
 */
export function splitSegment(segments, i, at, points = null) {
  const s = segments[i];
  if (!s || at <= s.start || at > s.end) return segments;
  const head = retime({ ...s, end: at - 1, points: at - s.start }, points);
  const tail = retime({ ...s, start: at, points: s.end - at + 1 }, points);
  const out = segments.slice();
  out.splice(i, 1, head, tail);
  return out;
}

/**
 * Recompute a segment's `t0`/`t1` from the points it now covers.
 *
 * Every structural change has to do this. Splitting used to copy the parent's
 * timestamps into both halves, so three consecutive segments all reported the
 * same start time and the same 67-minute duration -- in the table, in the ribbon
 * widths, and in the `start_time`/`end_time` a fixture exports. Wrong times in a
 * fixture are worse than missing ones.
 */
function retime(seg, points) {
  if (!points || !points[seg.start] || !points[seg.end]) return seg;
  return { ...seg, t0: points[seg.start].t_ms, t1: points[seg.end].t_ms };
}

/** Merge segment `i` into its predecessor. */
export function mergeWithPrevious(segments, i, points = null) {
  if (i <= 0 || i >= segments.length) return segments;
  const out = segments.slice();
  let prev = { ...out[i - 1] };
  prev.end = out[i].end;
  prev.t1 = out[i].t1;
  prev.points = prev.end - prev.start + 1;
  prev.edited = true;
  prev = retime(prev, points);
  out.splice(i - 1, 2, prev);
  return out;
}

/**
 * Relabel segment `i`. `edited` becomes true even when the label is unchanged:
 * a human confirming the classifier is a stronger claim than the classifier
 * asserting itself, and SCHEMA.md distinguishes the two.
 */
export function relabel(segments, i, type) {
  if (!LABELS.includes(type)) throw new Error(`not a MovementType: ${type}`);
  const out = segments.slice();
  out[i] = { ...out[i], type, edited: true };
  return autoMerge(out);
}

/**
 * Carve `[startIdx, endIdx]` out of the segment list and label it `type`.
 *
 * This is what a dragged rectangle on the speed chart turns into: the range
 * becomes its own segment, whatever it used to straddle. Splitting at both ends
 * first means the operation never disturbs a point outside the range -- a slice
 * that quietly relabelled the rest of the segment it landed in would be worse
 * than useless for building fixtures.
 *
 * The result is marked edited, because a person drew it.
 */
export function sliceRange(segments, startIdx, endIdx, type, points = null) {
  if (!LABELS.includes(type)) throw new Error(`not a MovementType: ${type}`);
  const lo = Math.min(startIdx, endIdx);
  const hi = Math.max(startIdx, endIdx);
  let out = segments.slice();

  // Split at the start, then at one past the end. Both are no-ops when the
  // boundary already exists, which is what makes repeated slices idempotent.
  const first = out.findIndex((s) => lo > s.start && lo <= s.end);
  if (first >= 0) out = splitSegment(out, first, lo, points);
  const last = out.findIndex((s) => hi + 1 > s.start && hi + 1 <= s.end);
  if (last >= 0) out = splitSegment(out, last, hi + 1, points);

  out = out.map((s) =>
    s.start >= lo && s.end <= hi ? { ...s, type, edited: true } : s,
  );
  return autoMerge(out);
}

/**
 * The dominant band inside a rectangle drawn on the speed chart.
 *
 * `bandOf` is the library's own bucketing (`wasm.speed_band`), so the answer here
 * is the classifier's vocabulary and thresholds rather than a second opinion
 * invented in JavaScript. Samples outside the rectangle's speed range are
 * excluded, which is the reason to drag a rectangle rather than a time range: it
 * lets a person leave a GPS spike out of the vote.
 *
 * Returns `{type, share, counted}` -- the share matters, because "62% walking"
 * and "98% walking" are different claims about the same slice.
 */
export function dominantBand(points, results, rect, bandOf) {
  const counts = new Map();
  let counted = 0;
  for (let i = 0; i < points.length; i++) {
    const p = points[i];
    if (p.t_ms < rect.t0 || p.t_ms > rect.t1) continue;
    const v = results[i] && results[i].speed;
    if (!Number.isFinite(v)) continue;
    if (v < rect.vMin || v > rect.vMax) continue;
    const band = bandOf(v);
    counts.set(band, (counts.get(band) ?? 0) + 1);
    counted++;
  }
  if (!counted) return { type: null, share: 0, counted: 0 };
  const [type, n] = [...counts.entries()].sort((a, b) => b[1] - a[1])[0];
  return { type, share: n / counted, counted };
}

/**
 * Up to `n` point indices spread across a segment: both ends, the middle, and
 * the quartiles. This is the sampling that keeps context resolution affordable
 * -- see README.md's request budget and the ponytail note in context.js.
 */
export function sampleIndices(seg, n = 5) {
  const len = seg.end - seg.start + 1;
  if (len <= n) {
    return Array.from({ length: len }, (_, k) => seg.start + k);
  }
  const out = new Set();
  for (let k = 0; k < n; k++) {
    out.add(seg.start + Math.round(((len - 1) * k) / (n - 1)));
  }
  return [...out];
}

/**
 * Single-level-per-step undo, capped.
 *
 * Twenty minutes of labeling lost to a misclick is data loss, not a corner
 * worth cutting, so this exists even though it is the only state machinery on
 * the page. `structuredClone` because segments are plain data and a shallow
 * copy would share the objects the mutators replace.
 */
export function createHistory(limit = 20) {
  const stack = [];
  return {
    snapshot(segments) {
      stack.push(structuredClone(segments));
      if (stack.length > limit) stack.shift();
    },
    undo() {
      return stack.pop() ?? null;
    },
    get depth() {
      return stack.length;
    },
  };
}

/** Total wall-clock ms per label, for the summary line. */
export function timePerLabel(segments) {
  const out = new Map();
  for (const s of segments) {
    out.set(s.type, (out.get(s.type) ?? 0) + (s.t1 - s.t0));
  }
  return out;
}
