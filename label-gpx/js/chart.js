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
 * Render the chart into `host`.
 *
 * `series` is `[{t_ms, speed}]`, `shifts` is what `significant_shifts` returned,
 * `segments` supplies the boundaries to compare against, and `colors` maps a
 * movement label to its hue so the chart agrees with the rest of the page.
 * `onSeek(t_ms)` is called when the user clicks, so the chart selects a segment
 * like the ribbon does.
 */
export function renderChart(host, {
  series, shifts, segments, colors, thresholds, onSeek, onSlice,
}) {
  if (!series || series.length < 2) {
    host.innerHTML = `<div class="chart-empty">No speed series yet — a trace needs at least
      two timed points.</div>`;
    return;
  }
  const t0 = series[0].t_ms;
  const t1 = series[series.length - 1].t_ms;
  const span = Math.max(1, t1 - t0);
  const vmax = Math.max(1, ...series.map((s) => s.speed));
  // A 0-1000 user-space width with a non-uniform viewBox: the SVG scales to
  // whatever the panel is wide, and no resize handler is needed.
  const W = 1000;
  const x = (t) => ((t - t0) / span) * W;
  const y = (v) => PAD_T + (1 - v / vmax) * (H - PAD_T - PAD_B);

  const line = series.map((s) => `${x(s.t_ms).toFixed(2)},${y(s.speed).toFixed(2)}`).join(" ");

  // Speed bands, filled across the whole width at the height their threshold
  // sits at. The numbers come from the library (`wasm.motion_thresholds`), never
  // from a copy here: a chart whose bands disagree with the classifier's is worse
  // than a chart with no bands.
  //
  // There is no running band, and that is not an omission. `Running` comes from
  // accelerometer cadence, never from speed alone, so a speed axis has nothing to
  // draw for it -- and inventing one would tell the reader something false.
  const t = thresholds ?? {};
  const bandsOf = [];
  if (Number.isFinite(t.stationary_max_mps) && Number.isFinite(t.driving_min_mps)) {
    bandsOf.push(
      { label: "stationary", lo: 0, hi: Math.min(t.stationary_max_mps, vmax) },
      { label: "walking", lo: t.stationary_max_mps, hi: Math.min(t.driving_min_mps, vmax) },
      { label: "driving", lo: t.driving_min_mps, hi: vmax },
    );
  }
  const zones = bandsOf
    .filter((b) => b.hi > b.lo)
    .map((b) => {
      const top = y(b.hi);
      const h = Math.max(0.5, y(b.lo) - top);
      return `<rect class="zone" x="0" y="${top.toFixed(2)}" width="${W}" height="${h.toFixed(2)}"
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
    .map((seg) => {
      const left = x(seg.t0);
      const w = Math.max(0.6, x(seg.t1) - left);
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
  const biggest = Math.max(1, ...(shifts ?? []).map((s) => Math.abs(s.delta_mps ?? 0)));
  const marks = (shifts ?? [])
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

  host.innerHTML = `<svg viewBox="0 0 ${W} ${H}" preserveAspectRatio="none" role="img"
      aria-label="Speed over time with significant shifts marked">
    ${zones}
    ${floorLine}
    <polyline class="speed" points="${line}"></polyline>
    ${bands}
    ${marks}
    <rect class="brush" x="0" y="0" width="0" height="0" hidden></rect>
  </svg>
  <div class="chart-axis">
    <span>0 m/s — ${vmax.toFixed(1)} m/s peak · biggest jump ${biggest.toFixed(1)} m/s</span>
    <span>${(shifts ?? []).length} significant shift${(shifts ?? []).length === 1 ? "" : "s"}
      ${shifts && shifts[0]
        ? `· Welch t-test, p ≤ ${shifts[0].alpha_corrected.toExponential(1)} after correction`
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
}
