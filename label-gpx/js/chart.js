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
export function renderChart(host, { series, shifts, segments, colors, onSeek }) {
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
    <polyline class="speed" points="${line}"></polyline>
    ${bands}
    ${marks}
  </svg>
  <div class="chart-axis">
    <span>0 m/s — ${vmax.toFixed(1)} m/s peak · biggest jump ${biggest.toFixed(1)} m/s</span>
    <span>${(shifts ?? []).length} significant shift${(shifts ?? []).length === 1 ? "" : "s"}
      ${shifts && shifts[0]
        ? `· Welch t-test, p ≤ ${shifts[0].alpha_corrected.toExponential(1)} after correction`
        : ""}</span>
  </div>`;

  const svg = host.querySelector("svg");
  if (svg && onSeek) {
    svg.addEventListener("click", (e) => {
      const box = svg.getBoundingClientRect();
      onSeek(t0 + ((e.clientX - box.left) / box.width) * span);
    });
  }
}
