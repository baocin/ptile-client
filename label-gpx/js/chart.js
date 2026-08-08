// The speed profile, with statistically significant shifts marked.
//
// The ribbon shows what the classifier *decided*; this shows what the data *did*,
// and where a Welch t-test says the behaviour genuinely changed
// (`motion/src/shifts.rs`, reached through `wasm.significant_shifts`). The two
// disagreeing is the interesting case, not an error: a shift with no segment
// boundary near it is usually a real change the thresholds missed, and a boundary
// with no shift near it is usually the debouncer reacting to noise.
//
// Inline SVG rather than canvas: a few hundred points and a dozen markers, and
// SVG gets crisp lines, hover targets and text without a redraw loop or a
// device-pixel-ratio dance.

const H = 116; // drawing height, matching the CSS box
const PAD_T = 8;
const PAD_B = 16;
/**
 * Left gutter for the speed labels, in user-space units.
 *
 * The viewBox is non-uniform (`preserveAspectRatio="none"`), so a "pixel" is
 * wider than it is tall and text would stretch with it. The labels therefore live
 * in an HTML layer beside the SVG rather than inside it -- see `ticksHtml`.
 */
const GUTTER_PX = 44;

/**
 * The label bands on a speed axis, low to high.
 *
 * Three of the four boundaries are the classifier's own (`stationary_max_mps`,
 * `driving_min_mps`). The walking/running split is `running_hint_mps`, which the
 * classifier never uses -- `Running` comes from accelerometer cadence -- so it is
 * marked `aid: true` and drawn and labelled differently. Showing it unmarked would
 * claim the classifier splits walking from running by speed, which it does not.
 */
export function speedBands(t) {
  if (!Number.isFinite(t?.stationary_max_mps) || !Number.isFinite(t?.driving_min_mps)) return [];
  const run = Number.isFinite(t.running_hint_mps)
    ? Math.min(Math.max(t.running_hint_mps, t.stationary_max_mps), t.driving_min_mps)
    : null;
  const out = [{ label: "stationary", lo: 0, hi: t.stationary_max_mps }];
  if (run && run > t.stationary_max_mps && run < t.driving_min_mps) {
    out.push({ label: "walking", lo: t.stationary_max_mps, hi: run });
    out.push({ label: "running", lo: run, hi: t.driving_min_mps, aid: true });
  } else {
    out.push({ label: "walking", lo: t.stationary_max_mps, hi: t.driving_min_mps });
  }
  out.push({ label: "driving", lo: t.driving_min_mps, hi: Infinity });
  return out;
}

/**
 * Speed labels for the gutter, as HTML rather than SVG `<text>`.
 *
 * The SVG uses `preserveAspectRatio="none"` so that one user-space unit of width
 * scales with the panel; text inside it stretches horizontally with the same
 * factor and looks wrong at every width but one. HTML absolutely positioned
 * against the same percentage heights sidesteps that entirely.
 */
function ticksHtml(bands, vmax) {
  const pct = (v) => {
    const frac = 1 - Math.min(v, vmax) / vmax;
    return (PAD_T + frac * (H - PAD_T - PAD_B)) / H;
  };
  // Exactly one boundary is a labelling aid: the running band's *lower* edge.
  // Its upper edge is `driving_min_mps`, a real classifier threshold, and starring
  // that would tell the reader the opposite of the truth.
  const running = bands.find((b) => b.aid);
  const aidAt = running ? running.lo : null;
  const isAid = (v) => aidAt !== null && Math.abs(v - aidAt) < 1e-9;
  const marks = [{ v: 0, aid: false }];
  for (const b of bands) {
    if (b.hi < vmax && Number.isFinite(b.hi)) marks.push({ v: b.hi, aid: isAid(b.hi) });
    if (b.aid && b.lo < vmax) marks.push({ v: b.lo, aid: true });
  }
  marks.push({ v: vmax, aid: false });
  marks.sort((a, b) => a.v - b.v);

  // Drop labels that would collide. The axis rescales to the zoom window, so on a
  // fast trace the 0.5 and 5.0 thresholds sit within a couple of pixels of each
  // other and of zero -- three numbers printed on top of one another is worse than
  // two of them missing. Zero and the peak always survive; the thresholds between
  // them are kept only where there is room, largest first, because the higher
  // boundary is the one you are usually reading against.
  const MIN_GAP = 0.085; // fraction of the box height, ~10px at 116px tall
  const seen = new Set();
  const unique = marks.filter((m) => {
    const k = m.v.toFixed(2);
    if (seen.has(k)) return false;
    seen.add(k);
    return true;
  });
  const kept = [unique[0], unique[unique.length - 1]].filter(Boolean);
  for (const m of unique.slice(1, -1).reverse()) {
    if (kept.every((k) => Math.abs(pct(k.v) - pct(m.v)) >= MIN_GAP)) kept.push(m);
  }
  kept.sort((a, b) => a.v - b.v);
  return `<div class="chart-ticks">${kept
    .map(
      (m) => `<span class="tick${m.aid ? " aid" : ""}" style="top:${(pct(m.v) * 100).toFixed(2)}%">${
        m.v.toFixed(m.v >= 10 ? 0 : 1)}${m.aid ? "*" : ""}</span>`,
    )
    .join("")}</div>`;
}

/**
 * Render the chart into `host`.
 *
 * `series` is `[{t_ms, speed}]`, `shifts` is what `significant_shifts` returned,
 * `segments` supplies the boundaries to compare against, and `colors` maps a
 * movement label to its hue so the chart agrees with the rest of the page.
 * `onSeek(t_ms)` is called when the user clicks, so the chart selects a segment
 * like the ribbon does.
 */
export function renderChart(host, {
  series, shifts, segments, colors, thresholds, view, onSeek, onSlice, onZoom,
}) {
  if (!series || series.length < 2) {
    host.innerHTML = `<div class="chart-empty">No speed series yet — a trace needs at least
      two timed points.</div>`;
    return;
  }
  const fullT0 = series[0].t_ms;
  const fullT1 = series[series.length - 1].t_ms;
  // The window, or the whole trace. Everything below is written against t0/t1, so
  // zooming is a change of these two numbers and nothing else.
  const t0 = view ? Math.max(fullT0, view.t0) : fullT0;
  const t1 = view ? Math.min(fullT1, view.t1) : fullT1;
  const span = Math.max(1, t1 - t0);

  // Samples inside the window, plus one either side so the line reaches both
  // edges instead of stopping short of them.
  const inWindow = [];
  for (let i = 0; i < series.length; i++) {
    const s = series[i];
    if (s.t_ms >= t0 && s.t_ms <= t1) {
      if (!inWindow.length && i > 0) inWindow.push(series[i - 1]);
      inWindow.push(s);
    } else if (inWindow.length && s.t_ms > t1) {
      inWindow.push(s);
      break;
    }
  }
  const shown = inWindow.length >= 2 ? inWindow : series;
  // Rescaled to the window: zooming into a slow stretch of a fast trace should
  // show its detail, at the cost of the axis moving as you pan. That is the
  // documented choice, and the axis labels are what keep it honest.
  const vmax = Math.max(1, ...shown.map((s) => s.speed));
  // A 0-1000 user-space width with a non-uniform viewBox: the SVG scales to
  // whatever the panel is wide, and no resize handler is needed.
  const W = 1000;
  const x = (t) => ((t - t0) / span) * W;
  const y = (v) => PAD_T + (1 - v / vmax) * (H - PAD_T - PAD_B);

  const line = shown.map((s) => `${x(s.t_ms).toFixed(2)},${y(s.speed).toFixed(2)}`).join(" ");

  // Speed bands, filled across the whole width at the height their threshold
  // sits at. The numbers come from the library (`wasm.motion_thresholds`), never
  // from a copy here: a chart whose bands disagree with the classifier's is worse
  // than a chart with no bands.
  //
  // There is no running band, and that is not an omission. `Running` comes from
  // accelerometer cadence, never from speed alone, so a speed axis has nothing to
  // draw for it -- and inventing one would tell the reader something false.
  const t = thresholds ?? {};
  const zones = speedBands(t)
    .map((b) => {
      const top = y(Math.min(b.hi, vmax));
      const h = Math.max(0.5, y(Math.min(b.lo, vmax)) - top);
      if (b.lo >= vmax) return "";
      return `<rect class="zone${b.aid ? " aid" : ""}" x="0" y="${top.toFixed(2)}"
        width="${W}" height="${h.toFixed(2)}"
        fill="${colors[b.label] ?? colors.unknown}"></rect>`;
    })
    .join("");
  // The stateless tree's own floor, which sits above the smoothed walking band:
  // below this line, speed alone cannot see a walk at all, which is the single
  // most useful thing to know while labelling a slow trace.
  const floorLine = Number.isFinite(t.walking_ceiling_mps) && t.walking_ceiling_mps < vmax
    ? `<line class="floor" x1="0" y1="${y(t.walking_ceiling_mps).toFixed(2)}"
         x2="${W}" y2="${y(t.walking_ceiling_mps).toFixed(2)}"></line>`
    : "";

  // Segment bands along the bottom, so the classifier's opinion sits under the
  // measurement rather than on top of it.
  const bands = (segments ?? [])
    .filter((seg) => seg.t1 >= t0 && seg.t0 <= t1)
    .map((seg) => {
      // Clamped, or a segment straddling the window draws a rect far outside the
      // viewBox.
      const left = x(Math.max(seg.t0, t0));
      const w = Math.max(0.6, x(Math.min(seg.t1, t1)) - left);
      const fill = colors[seg.type] ?? colors.unknown;
      return `<rect x="${left.toFixed(2)}" y="${H - PAD_B + 3}" width="${w.toFixed(2)}"
        height="4" fill="${fill}" opacity="${seg.edited ? 1 : 0.45}"></rect>`;
    })
    .join("");

  // A shift is a vertical rule with a tick whose direction is the sign of the
  // change: up for a speed-up, down for a slowdown.
  //
  // Prominence scales with |delta| rather than with the p-value. A real drive
  // produces dozens of significant changes and they are not equally interesting:
  // a 15 m/s jump onto a highway and a 1 m/s dip for a corner are both far past
  // any threshold, so p-value ordering would make them look alike. Nothing is
  // hidden -- the small ones are drawn faintly, so the eye finds the structure
  // first and the detail is still there to hover.
  const visible = (shifts ?? []).filter((s) => s.t_ms >= t0 && s.t_ms <= t1);
  const biggest = Math.max(1, ...visible.map((s) => Math.abs(s.delta_mps ?? 0)));
  const marks = visible
    .map((s) => {
      const px = x(s.t_ms);
      const delta = s.delta_mps ?? s.after_mps - s.before_mps;
      const up = delta > 0;
      const weight = Math.min(1, 0.28 + (Math.abs(delta) / biggest) * 0.72);
      const tick = 4 + weight * 5;
      const label =
        `${up ? "+" : ""}${delta.toFixed(1)} m/s · ` +
        `${s.before_mps.toFixed(1)} → ${s.after_mps.toFixed(1)} m/s · ` +
        `p=${s.p_value < 1e-6 ? "<1e-6" : s.p_value.toExponential(1)}`;
      return `<g class="shift" style="opacity:${weight.toFixed(2)}">
        <line x1="${px.toFixed(2)}" y1="${PAD_T}" x2="${px.toFixed(2)}" y2="${H - PAD_B}"></line>
        <polygon points="${up
          ? `${px - 3},${PAD_T + tick} ${px + 3},${PAD_T + tick} ${px},${PAD_T}`
          : `${px - 3},${PAD_T} ${px + 3},${PAD_T} ${px},${PAD_T + tick}`}"></polygon>
        <title>${label}</title>
      </g>`;
    })
    .join("");

  host.innerHTML = `<div class="chart-body">
    ${ticksHtml(speedBands(t), vmax)}
    <svg viewBox="0 0 ${W} ${H}" preserveAspectRatio="none" role="img"
      aria-label="Speed over time with significant shifts marked">
    ${zones}
    ${floorLine}
    <polyline class="speed" points="${line}"></polyline>
    ${bands}
    ${marks}
    <rect class="brush" x="0" y="0" width="0" height="0" hidden></rect>
    </svg>
  </div>
  <div class="chart-axis">
    <span id="chartCursor">m/s axis${
      speedBands(t).some((b) => b.aid) ? " · * = labelling aid, not a classifier threshold" : ""
    } · peak ${vmax.toFixed(1)}${view ? " in view" : ""}</span>
    <span>${visible.length} significant shift${visible.length === 1 ? "" : "s"}${
      visible.length !== (shifts ?? []).length ? ` of ${(shifts ?? []).length}` : ""}
      ${visible[0]
        ? `· Welch t-test, p ≤ ${visible[0].alpha_corrected.toExponential(1)} after correction`
        : ""}</span>
  </div>`;

  const svg = host.querySelector("svg");
  if (!svg) return;

  // Screen pixels -> data. The viewBox is non-uniform, so both axes convert
  // through the rendered box rather than through the user-space width.
  const toData = (e) => {
    const box = svg.getBoundingClientRect();
    const fx = Math.min(1, Math.max(0, (e.clientX - box.left) / box.width));
    const fy = Math.min(1, Math.max(0, (e.clientY - box.top) / box.height));
    const yUser = fy * H;
    const speed = ((H - PAD_B - yUser) / (H - PAD_T - PAD_B)) * vmax;
    return { t_ms: t0 + fx * span, speed: Math.max(0, Math.min(vmax, speed)), px: fx * W, py: yUser };
  };

  // Drag a rectangle to cut a slice; click to select. A drag narrower than a few
  // pixels is a click that wobbled, so it stays a click.
  const brush = svg.querySelector(".brush");
  let from = null;
  const MIN_DRAG_PX = 4;

  svg.addEventListener("pointerdown", (e) => {
    from = toData(e);
    svg.setPointerCapture(e.pointerId);
  });

  svg.addEventListener("pointermove", (e) => {
    if (!from) return;
    const now = toData(e);
    if (Math.abs(now.px - from.px) < MIN_DRAG_PX) return;
    brush.hidden = false;
    brush.setAttribute("x", Math.min(from.px, now.px).toFixed(2));
    brush.setAttribute("width", Math.abs(now.px - from.px).toFixed(2));
    brush.setAttribute("y", Math.min(from.py, now.py).toFixed(2));
    brush.setAttribute("height", Math.max(1, Math.abs(now.py - from.py)).toFixed(2));
  });

  svg.addEventListener("pointerup", (e) => {
    if (!from) return;
    const now = toData(e);
    const start = from;
    from = null;
    brush.hidden = true;
    if (Math.abs(now.px - start.px) < MIN_DRAG_PX) {
      if (onSeek) onSeek(now.t_ms);
      return;
    }
    if (onSlice) {
      onSlice({
        t0: Math.min(start.t_ms, now.t_ms),
        t1: Math.max(start.t_ms, now.t_ms),
        vMin: Math.min(start.speed, now.speed),
        vMax: Math.max(start.speed, now.speed),
      });
    }
  });

  svg.addEventListener("pointercancel", () => {
    from = null;
    brush.hidden = true;
  });

  // Speed under the pointer, in the axis caption. The chart had no readable
  // number on it at all before this: the y axis was unlabelled and the only
  // figure anywhere was the peak.
  const caption = host.querySelector("#chartCursor");
  const captionText = caption ? caption.textContent : "";
  svg.addEventListener("pointermove", (e) => {
    if (!caption) return;
    const at = toData(e);
    const clock = new Date(at.t_ms).toISOString().slice(11, 19);
    caption.textContent = `${clock} · ${at.speed.toFixed(1)} m/s`;
  });
  svg.addEventListener("pointerleave", () => {
    if (caption) caption.textContent = captionText;
  });

  // Wheel zooms about the pointer, so the sample under the cursor stays put --
  // the whole point of zooming is to look closer at *that* moment.
  if (onZoom) {
    svg.addEventListener(
      "wheel",
      (e) => {
        e.preventDefault();
        const at = toData(e);
        const factor = e.deltaY > 0 ? 1.35 : 1 / 1.35;
        const width = Math.min(fullT1 - fullT0, Math.max(5000, span * factor));
        const frac = (at.t_ms - t0) / span;
        let lo = at.t_ms - width * frac;
        let hi = lo + width;
        if (lo < fullT0) { lo = fullT0; hi = lo + width; }
        if (hi > fullT1) { hi = fullT1; lo = hi - width; }
        onZoom(hi - lo >= fullT1 - fullT0 - 1 ? null : { t0: lo, t1: hi });
      },
      { passive: false },
    );
  }
}
