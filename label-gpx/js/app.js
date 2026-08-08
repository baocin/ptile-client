// Page wiring: file in, map and table, labels out. All the interesting logic
// lives in gpx.js (XML), segments.js (labeling, DOM-free and tested) and
// context.js (layers); this file is the parts that only make sense in a browser.

import init_wasm, * as wasm from "../lib/client/ptiles_client.js";
import { createPtiles } from "./ptiles.js";
import { parseGpx, writeGpx, LABELS } from "./gpx.js";
import {
  classifyTrace, coalesce, splitSegment, mergeWithPrevious, relabel,
  sampleIndices, createHistory, timePerLabel, sliceRange, dominantBand,
  bandByHeight, moveBoundary,
} from "./segments.js";
import { createResolver, SNAPSHOT, stateAt, stateUrl } from "./context.js";
import { renderChart, speedBands } from "./chart.js";
import { createBasemap } from "./basemap.js";

const COLORS = {
  unknown: "#6b7280", stationary: "#a78bfa", walking: "#34d399",
  running: "#fbbf24", driving: "#f87171",
};

const el = (id) => document.getElementById(id);
const state = {
  file: null,
  parsed: null,
  results: null,
  segments: [],
  selected: -1,
  resolved: false,
  history: createHistory(20),
  chartOn: false,
  chartWindow: 12,
  /**
   * The zoom window, `{t0, t1}`, or null for the whole trace.
   *
   * One window, shared by the overview strip, the ribbon and the chart -- they all
   * derive their time axis from it, so zooming anywhere zooms everything. View
   * state, not data: it never reaches the exported file.
   */
  view: null,
  // Invalidated whenever the classification changes: the shifts describe a
  // particular speed series, and a re-classify produces a different one.
  shifts: null,
};

let P = null;
let resolver = null;
const ready = init_wasm().then(() => {
  P = createPtiles(wasm);
  resolver = createResolver(P, wasm);
  basemap = createBasemap(map, P, wasm, { stateAt, stateUrl }, { onStatus: basemapNote });
});

// ---------------------------------------------------------------- map

// preferCanvas: a 2,000-vertex polyline plus per-segment overlays as SVG is
// visibly sluggish to pan; as canvas it is not.
const map = L.map("map", { preferCanvas: true, zoomControl: true }).setView([36.16, -86.78], 12);
let basemap = null;
const segLayer = L.layerGroup().addTo(map);
const vertexLayer = L.layerGroup().addTo(map);
// The hovered trace point. Its own layer, and one reused marker, so `render()`
// never touches it: `drawSegments`/`drawVertices` clear their layers wholesale,
// which would delete the marker mid-hover.
const hoverLayer = L.layerGroup().addTo(map);
const hoverMarker = L.circleMarker([0, 0], {
  radius: 6, color: "#fff", weight: 2, fillColor: "#fff", fillOpacity: 0.9,
});
let polylines = [];

function drawSegments() {
  segLayer.clearLayers();
  polylines = [];
  state.segments.forEach((s, i) => {
    const latlngs = state.parsed.points
      .slice(s.start, s.end + 1)
      .map((p) => [p.lat, p.lon]);
    const line = L.polyline(latlngs, {
      color: COLORS[s.type] ?? COLORS.unknown,
      weight: i === state.selected ? 7 : 4,
      opacity: 0.9,
    });
    line.on("click", () => select(i));
    line.on("mouseover", () => highlightRow(i, true));
    line.on("mouseout", () => highlightRow(i, false));
    line.addTo(segLayer);
    polylines.push(line);
  });
  drawVertices();
}

// Vertex markers only for the selected segment: 2,000 always-on markers is what
// makes a page like this feel broken, and they are only needed to split.
function drawVertices() {
  vertexLayer.clearLayers();
  const s = state.segments[state.selected];
  if (!s) return;
  const step = Math.max(1, Math.ceil((s.end - s.start + 1) / 200));
  for (let i = s.start; i <= s.end; i += step) {
    const p = state.parsed.points[i];
    L.circleMarker([p.lat, p.lon], {
      radius: 3, color: "#fff", weight: 1, fillColor: COLORS[s.type], fillOpacity: 0.9,
    })
      .on("click", () => {
        // Clicking a vertex of the selected segment splits there.
        state.history.snapshot(state.segments);
        state.segments = splitSegment(state.segments, state.selected, i, state.parsed.points);
        render();
      })
      .bindTooltip(`split here (point ${i})`)
      .addTo(vertexLayer);
  }
}

// ---------------------------------------------------------------- progress

/**
 * A named-phase progress sheet.
 *
 * Resolving context is a dozen-odd range reads against a public host: quick, but
 * not instant. It used to sit behind its own button which then greyed itself out
 * with no explanation, so the two-step order was discoverable only by trying it.
 * One action, one sheet, each phase named as it happens.
 */
function sheet(steps) {
  const list = el("modalSteps");
  list.innerHTML = steps
    .map((s, i) => `<li data-step="${i}"><span class="mark">·</span><span>${s}</span>
      <span class="detail"></span></li>`)
    .join("");
  el("modalNote").textContent = "";
  el("modalClose").hidden = true;
  el("modal").hidden = false;
  const at = (i) => list.querySelector(`li[data-step="${i}"]`);
  return {
    doing(i, detail = "") {
      const li = at(i);
      if (!li) return;
      li.className = "doing";
      li.querySelector(".mark").textContent = "›";
      li.querySelector(".detail").textContent = detail;
    },
    done(i, detail = "") {
      const li = at(i);
      if (!li) return;
      li.className = "done";
      li.querySelector(".mark").textContent = "✓";
      if (detail) li.querySelector(".detail").textContent = detail;
    },
    failed(i, detail = "") {
      const li = at(i);
      if (!li) return;
      li.className = "failed";
      li.querySelector(".mark").textContent = "!";
      li.querySelector(".detail").textContent = detail;
    },
    note(msg) {
      el("modalNote").textContent = msg;
    },
    close() {
      el("modal").hidden = true;
    },
    hold(msg) {
      el("modalNote").textContent = msg;
      el("modalClose").hidden = false;
    },
  };
}

el("modalClose").addEventListener("click", () => el("modal").hidden = true);
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && !el("modal").hidden) el("modal").hidden = true;
});

// ---------------------------------------------------------------- basemap

// Two backdrops, one switch. The raster tiles are always right about the world;
// the ptiles layers are right about *what the classifier read*. When a label
// hinges on footway-versus-traffic-lane, the second one is the honest backdrop --
// and flipping between them is the fastest way to spot the tiles and the layer
// disagreeing, which happens and which a raster-only map hides.
const basemapControl = L.control({ position: "bottomleft" });
basemapControl.onAdd = () => {
  const wrap = L.DomUtil.create("div");
  wrap.innerHTML = `
    <div class="basemap">
      <button data-mode="osm" class="on">OSM tiles</button>
      <button data-mode="ptiles">PTiles layers</button>
      <button id="scanBuildings" title="Outline every building footprint the trace goes inside">Buildings on trace</button>
    </div>
    <div class="basemap-note" id="basemapNote"></div>`;
  L.DomEvent.disableClickPropagation(wrap);
  wrap.querySelectorAll("button[data-mode]").forEach((b) => {
    b.addEventListener("click", () => {
      if (!basemap) return;
      const mode = basemap.setMode(b.dataset.mode);
      wrap.querySelectorAll("button[data-mode]").forEach((x) => {
        x.classList.toggle("on", x.dataset.mode === mode);
      });
      status();
    });
  });
  wrap.querySelector("#scanBuildings").addEventListener("click", scanBuildings);
  return wrap;
};
basemapControl.addTo(map);

/**
 * Outline every building footprint the trace passes inside.
 *
 * Its own layer, deliberately not the vector basemap's `ptilesFill` pane: those
 * polygons are `interactive: false`, carry no id, share one canvas, and only
 * exist in ptiles mode above zoom 15. This works in either basemap mode at any
 * zoom, and each outline is clickable to attach that building to a segment.
 *
 * On demand, not automatic: it decodes every buildings block the trace touches,
 * which is the most expensive thing this page can ask for.
 */
const buildingLayer = L.layerGroup().addTo(map);
let scanning = false;
async function scanBuildings() {
  if (!state.parsed || scanning) return;
  const btn = el("scanBuildings");
  scanning = true;
  if (btn) btn.textContent = "Scanning...";
  buildingLayer.clearLayers();
  try {
    const hits = await resolver.buildingsOnTrace(state.parsed.points);
    for (const b of hits) {
      // Rings are [lon, lat]; Leaflet wants [lat, lon].
      const ring = b.coords.map(([lon, lat]) => [lat, lon]);
      const name = b.name || b.category || b.type || "building";
      L.polygon(ring, { color: "#fff", weight: 2, fillColor: "#fff", fillOpacity: 0.15 })
        .bindTooltip(`${name} · ${b.points.length} pts inside · click to attach`)
        .on("click", () => attachBuilding(b))
        .addTo(buildingLayer);
    }
    state.traceBuildings = hits;
    warn(
      hits.length
        ? `${hits.length} building${hits.length === 1 ? "" : "s"} contain trace points`
        : "no building footprint on this trace contains a trace point",
    );
    render();
  } catch (e) {
    warn(`building scan: ${e?.message ?? e}`);
  } finally {
    scanning = false;
    if (btn) btn.textContent = "Buildings on trace";
  }
}

/**
 * Attach a scanned building to the segment its points belong to -- the same
 * `context.building` shape the click-a-place path writes, so the export and the
 * schema stay one thing.
 */
function attachBuilding(b) {
  const mid = b.points[Math.floor(b.points.length / 2)];
  const i = state.segments.findIndex((s) => mid >= s.start && mid <= s.end);
  if (i < 0) return;
  state.history.snapshot(state.segments);
  const seg = state.segments[i];
  const ctx = { ...(seg.context ?? {}) };
  ctx.snapshot = SNAPSHOT;
  ctx.building = {
    osm_id: b.osm_id,
    name: b.name,
    type: b.type,
    category: b.category,
    distance_m: 0,
    inside: true,
  };
  state.segments[i] = { ...seg, context: ctx, edited: true };
  delete state.segments[i].sourceContext;
  select(i);
  warn(`attached ${b.name || "building"} to segment ${i + 1}`);
}

function basemapNote(msg) {
  const n = el("basemapNote");
  if (n) n.textContent = msg ?? "";
  status();
}

// Debounced: panning fires moveend continuously, and each pass may open layers.
let moveTimer = null;
map.on("moveend zoomend", () => {
  clearTimeout(moveTimer);
  moveTimer = setTimeout(() => basemap && basemap.refresh(), 250);
});

// ---------------------------------------------------------------- place lookup

// Where the last lookup happened, and what it found.
let place = null;
const placeMarker = L.layerGroup().addTo(map);

/**
 * Ask the map what is at a point, and offer to write the answer into the trace.
 *
 * This is the question labelling actually runs into: a 12-minute stop is
 * "stationary" either way, but *whether it happened at the hardware store* is
 * what makes the fixture worth keeping. The lookup goes to the same layers the
 * classifier reads, and attaching it writes a rook:building / rook:addresses /
 * rook:businesses block into the exported segment (SCHEMA.md).
 */
map.on("click", async (e) => {
  if (!P) return;
  const { lat, lng: lon } = e.latlng;
  place = { lat, lon, loading: true };
  renderPlace();
  placeMarker.clearLayers();
  L.circleMarker([lat, lon], {
    radius: 6, color: "var(--accent)", weight: 2, fillColor: "#3ec8d4", fillOpacity: 0.25,
  }).addTo(placeMarker);
  // Three lookups in parallel (different layers, so serialising them would only
  // add round trips), and settled rather than all-or-nothing: one layer that
  // fails to decode must not take the answers from the other two with it. Each
  // failure is reported on the card instead of vanishing into a catch.
  const [b, a, biz] = await Promise.allSettled([
    resolver.buildingAt(lat, lon),
    resolver.addressesAt(lat, lon),
    resolver.businessesNear(lat, lon),
  ]);
  place = {
    lat,
    lon,
    building: b.status === "fulfilled" ? b.value : null,
    addresses: a.status === "fulfilled" ? a.value : [],
    businesses: biz.status === "fulfilled" ? biz.value : [],
    // A decoder can reject with a bare string from wasm rather than an Error, so
    // reading `.message` blindly rendered the word "undefined" as the problem.
    errors: [b, a, biz]
      .filter((r) => r.status === "rejected")
      .map((r) => String(r.reason?.message ?? r.reason)),
  };
  renderPlace();
  status();
});

/** Which segment an attach would land on: the selection, else the nearest fix. */
function nearestSegment(lat, lon) {
  if (state.selected >= 0) return state.selected;
  if (!state.parsed) return -1;
  let best = -1;
  let bestD = Infinity;
  state.segments.forEach((s, i) => {
    for (let k = s.start; k <= s.end; k++) {
      const p = state.parsed.points[k];
      const d = wasm.distance_m(lat, lon, p.lat, p.lon);
      if (d < bestD) {
        bestD = d;
        best = i;
      }
    }
  });
  return best;
}

function renderPlace() {
  const host = el("place");
  if (!place) {
    host.innerHTML = "";
    return;
  }
  const coords = `${place.lat.toFixed(5)}, ${place.lon.toFixed(5)}`;
  if (place.loading) {
    host.innerHTML = `<div class="row"><span class="sub data">${coords}</span>
      <span class="sub">reading the layers…</span></div>`;
    return;
  }

  const b = place.building;
  // `building=yes` is OSM's "this is a building and nothing more is claimed",
  // which is the most common tag there is. Printing it as a title reads as a
  // bug; printing "Building" reads as the truth.
  const kind = b && b.building_type && b.building_type !== "yes" ? b.building_type : null;
  const title = b ? escapeHtml(b.name || kind || "Building") : "No building here";
  const sub = b
    ? [
        kind && b.name ? escapeHtml(kind) : null,
        b.category ? escapeHtml(b.category) : null,
        b.inside ? "you are inside it" : `${b.distance_m.toFixed(0)} m away`,
      ]
        .filter(Boolean)
        .join(" · ")
    : "nothing within 50 m";
  const problems = (place.errors ?? [])
    .map((e) => `<div class="sub">${escapeHtml(e)}</div>`)
    .join("");
  const addrs = (place.addresses ?? [])
    .map((a) => `<li>${escapeHtml(a.housenumber)} ${escapeHtml(a.street)}
      <span class="sub data">${a.distance_m.toFixed(0)} m</span></li>`)
    .join("");
  const biz = (place.businesses ?? [])
    .map((x) => `<li>${escapeHtml(x.name)}${x.category ? ` <span class="sub">${escapeHtml(x.category)}</span>` : ""}
      <span class="sub data">${x.distance_m.toFixed(0)} m</span></li>`)
    .join("");
  const target = nearestSegment(place.lat, place.lon);
  const opts = state.segments
    .map((s, i) => `<option value="${i}"${i === target ? " selected" : ""}>${i + 1}. ${s.type}</option>`)
    .join("");
  host.innerHTML = `
    <div class="row"><span class="title">${title}</span><span class="sub data">${coords}</span></div>
    <div class="sub">${sub}</div>
    ${problems}
    ${addrs ? `<div class="sub">Addresses</div><ul>${addrs}</ul>` : ""}
    ${biz ? `<div class="sub">Businesses nearby</div><ul>${biz}</ul>` : ""}
    ${state.segments.length ? `<div class="acts">
      <select id="placeSeg" aria-label="Segment to attach this place to">${opts}</select>
      <button id="placeAttach">Attach to segment</button>
      <button id="placeClear" class="ghost">Dismiss</button>
    </div>` : ""}`;

  const attach = el("placeAttach");
  if (attach) attach.addEventListener("click", attachPlace);
  const clear = el("placeClear");
  if (clear) {
    clear.addEventListener("click", () => {
      place = null;
      placeMarker.clearLayers();
      renderPlace();
    });
  }
}

/**
 * Write the looked-up place onto a segment's context, and mark the segment
 * human-edited: a person decided this place belongs to this stretch, and that is
 * exactly what `source="human"` means in the exported file.
 */
function attachPlace() {
  const i = Number(el("placeSeg").value);
  const seg = state.segments[i];
  if (!seg || !place) return;
  state.history.snapshot(state.segments);
  const ctx = { ...(seg.context ?? {}) };
  ctx.lat = place.lat;
  ctx.lon = place.lon;
  ctx.snapshot = SNAPSHOT;
  ctx.resolved = Date.now();
  if (place.building) ctx.building = place.building;
  if (place.addresses && place.addresses.length) ctx.addresses = place.addresses;
  if (place.businesses && place.businesses.length) ctx.businesses = place.businesses;
  state.segments[i] = { ...seg, context: ctx, edited: true };
  // An attached place is the user's own annotation, so a rook file's captured
  // context no longer speaks for this segment.
  delete state.segments[i].sourceContext;
  state.selected = i;
  render();
  warn(`attached ${place.building?.name ?? "this place"} to segment ${i + 1}`);
}

// ---------------------------------------------------------------- chart

/**
 * The speed series the chart and the shift detector both run on.
 *
 * Built from what the tracker derived per point, not from raw position deltas: it
 * is the same smoothed series the classifier saw, so a shift the chart marks is a
 * shift in the evidence the classifier had.
 */
function speedSeries() {
  if (!state.parsed || !state.results) return [];
  const out = [];
  state.parsed.points.forEach((p, i) => {
    const v = state.results[i] && state.results[i].speed;
    if (Number.isFinite(v)) out.push({ t_ms: p.t_ms, speed: v });
  });
  return out;
}

/** Significant shifts, computed once per classification and cached. */
function shifts() {
  if (state.shifts) return state.shifts;
  const series = speedSeries();
  if (series.length < 30) return (state.shifts = []);
  const t = new Float64Array(series.map((s) => s.t_ms));
  const v = new Float64Array(series.map((s) => s.speed));
  try {
    // The window is the one knob worth exposing: it sets what counts as an
    // "event". Six samples finds a pause at a junction, twenty-four finds the
    // change from town driving to highway, and neither is more correct.
    state.shifts = wasm.significant_shifts(t, v, { window: state.chartWindow });
  } catch (err) {
    warn(`shift detection failed: ${err}`);
    state.shifts = [];
  }
  return state.shifts;
}

/** The classifier's speed thresholds, fetched once. */
let thresholds = null;

function renderChartIfShown() {
  if (!state.chartOn) return;
  if (!thresholds) {
    try {
      thresholds = wasm.motion_thresholds();
    } catch {
      thresholds = {};
    }
  }
  renderChart(el("chart"), {
    series: speedSeries(),
    shifts: shifts(),
    segments: state.segments,
    colors: COLORS,
    thresholds,
    view: state.view,
    onZoomAbout: zoomAbout,
    onHover: showHover,
    onHoverEnd: hideHover,
    onSeek: (t_ms) => {
      const i = state.segments.findIndex((s) => t_ms >= s.t0 && t_ms <= s.t1);
      if (i >= 0) select(i);
    },
    onSlice: sliceFromRect,
  });
}

/**
 * Turn a dragged rectangle into a new labelled slice.
 *
 * The label is the dominant speed band among the samples *inside the rectangle*,
 * bucketed by the library's own `speed_band` -- so the slice gets the
 * classifier's vocabulary and thresholds rather than a JavaScript opinion. The
 * vertical extent is what makes this worth dragging as a rectangle: pull the top
 * edge below a GPS spike and the spike stops voting.
 */
function sliceFromRect(rect) {
  if (!state.parsed || !state.results) return;
  const pts = state.parsed.points;
  const inRange = [];
  for (let i = 0; i < pts.length; i++) {
    if (pts[i].t_ms >= rect.t0 && pts[i].t_ms <= rect.t1) inRange.push(i);
  }
  if (inRange.length < 2) {
    warn("that slice covers fewer than two points — drag a wider box");
    return;
  }
  // The label comes from the box's *height*: whichever band covers most of the
  // vertical span you dragged. That works with no samples in the box at all,
  // which is the case that used to do nothing -- drag a box in the driving band
  // over a stretch the classifier called stationary and you get driving.
  const { type, share } = bandByHeight(rect, speedBands(thresholds ?? {}));
  if (!type) {
    warn("that box does not cover a speed band — drag inside the chart");
    return;
  }
  // What the samples say, reported alongside rather than instead: agreement is
  // reassuring and disagreement is the interesting case.
  const sampled = dominantBand(pts, state.results, { ...rect, vMin: 0, vMax: Infinity }, (v) =>
    wasm.speed_band(v),
  );
  state.history.snapshot(state.segments);
  const lo = inRange[0];
  const hi = inRange[inRange.length - 1];
  state.segments = sliceRange(state.segments, lo, hi, type, pts);
  state.selected = state.segments.findIndex((s) => s.start <= lo && s.end >= lo);
  render();
  const mins = ((rect.t1 - rect.t0) / 60000).toFixed(1);
  const agree =
    sampled.type === type
      ? `samples agree (${Math.round(sampled.share * 100)}%)`
      : sampled.type
        ? `samples said ${sampled.type} (${Math.round(sampled.share * 100)}%)`
        : "no samples to compare";
  warn(
    `sliced ${mins} min as ${type} — ${Math.round(share * 100)}% of the box height; ${agree}`,
  );
}

el("chartWindow").addEventListener("change", (e) => {
  state.chartWindow = Number(e.target.value);
  state.shifts = null;
  renderChartIfShown();
});

el("chartToggle").addEventListener("click", () => {
  state.chartOn = !state.chartOn;
  el("chart").hidden = !state.chartOn;
  el("chartToggle").setAttribute("aria-pressed", String(state.chartOn));
  renderChartIfShown();
  if (state.chartOn) {
    const n = shifts().length;
    warn(n ? "" : "no shift clears the corrected significance level on this trace");
  }
});

// ---------------------------------------------------------------- zoom

/** Full extent of the loaded trace, or null. */
function fullSpan() {
  const pts = state.parsed && state.parsed.points;
  if (!pts || pts.length < 2) return null;
  return { t0: pts[0].t_ms, t1: pts[pts.length - 1].t_ms };
}

/** The effective window: the zoom if set, else the whole trace. */
function viewWindow() {
  return state.view ?? fullSpan();
}

/**
 * The one px<->time conversion on the page, both directions, against the ribbon
 * track. The bands, the boundary handles, the hover marker and the wheel zoom all
 * go through this pair; when each computed its own the handles drifted.
 */
function timeAtX(clientX) {
  const w = viewWindow();
  if (!w) return null;
  const r = el("ribbon").getBoundingClientRect();
  if (!r.width) return null;
  const f = Math.min(1, Math.max(0, (clientX - r.left) / r.width));
  return w.t0 + f * (w.t1 - w.t0);
}

/** Fraction across the current window, 0..1, for a timestamp. */
function xForTime(t_ms) {
  const w = viewWindow();
  if (!w) return 0;
  const span = Math.max(1, w.t1 - w.t0);
  return Math.min(1, Math.max(0, (t_ms - w.t0) / span));
}

/**
 * Zoom by `factor` about a fixed timestamp, keeping that instant under the
 * pointer. `factor < 1` zooms in.
 */
function zoomAbout(t_ms, factor) {
  const w = viewWindow();
  if (!w || !Number.isFinite(t_ms)) return;
  const width = Math.max(5000, (w.t1 - w.t0) * factor);
  const at = Math.min(1, Math.max(0, (t_ms - w.t0) / Math.max(1, w.t1 - w.t0)));
  setView({ t0: t_ms - at * width, t1: t_ms + (1 - at) * width });
}

/**
 * Set the zoom window, clamped to the trace and to a floor of five seconds.
 *
 * `null` means the whole trace, and a window that covers everything collapses to
 * `null` so "zoomed out" has exactly one representation rather than two.
 */
function setView(next) {
  const full = fullSpan();
  if (!full || !next) {
    state.view = null;
    render();
    return;
  }
  const width = Math.max(5000, Math.min(full.t1 - full.t0, next.t1 - next.t0));
  let t0 = Math.max(full.t0, Math.min(next.t0, full.t1 - width));
  const t1 = Math.min(full.t1, t0 + width);
  t0 = t1 - width;
  state.view = t1 - t0 >= full.t1 - full.t0 - 1 ? null : { t0, t1 };
  render();
}

/**
 * The overview strip: the whole trace, and where the window sits inside it.
 *
 * Deliberately not a chart -- it is the segment bands plus a box. Its job is the
 * question a zoomed view cannot answer, "where am I in the trace?", and it is the
 * coarse control for moving there.
 */
function renderOverview() {
  const host = el("overview");
  const full = fullSpan();
  if (!full || !state.segments.length) {
    host.innerHTML = "";
    return;
  }
  const span = Math.max(1, full.t1 - full.t0);
  const pct = (t) => ((t - full.t0) / span) * 100;
  const bands = state.segments
    .map((seg) => {
      const left = pct(seg.t0);
      const w = Math.max(0.15, pct(seg.t1) - left);
      return `<span class="ov" style="left:${left.toFixed(3)}%; width:${w.toFixed(3)}%;
        background:${COLORS[seg.type] ?? COLORS.unknown}; opacity:${seg.edited ? 1 : 0.5}"></span>`;
    })
    .join("");
  const w = viewWindow();
  const left = pct(w.t0);
  const width = Math.max(0.6, pct(w.t1) - left);
  host.innerHTML = `${bands}
    <span class="window" style="left:${left.toFixed(3)}%; width:${width.toFixed(3)}%">
      <span class="grip left" data-grip="left"></span>
      <span class="grip right" data-grip="right"></span>
    </span>`;

}

/**
 * Wire the overview strip once.
 *
 * `renderOverview` replaces the strip's innerHTML on every render, but the strip
 * element itself persists -- so binding these there added a fresh listener per
 * frame, and after a few dozen renders one double-click fired dozens of resets.
 * Handlers live here, bound once, and read the current window lazily.
 */
function wireOverview() {
  const host = el("overview");
  let drag = null;
  const timeAt = (e) => {
    const full = fullSpan();
    if (!full) return null;
    const r = host.getBoundingClientRect();
    const f = Math.min(1, Math.max(0, (e.clientX - r.left) / r.width));
    return full.t0 + f * (full.t1 - full.t0);
  };

  host.addEventListener("pointerdown", (e) => {
    const at = timeAt(e);
    if (at === null) return;
    const cur = viewWindow();
    const grip = e.target.dataset && e.target.dataset.grip;
    const box = host.querySelector(".window");
    if (grip) {
      drag = { kind: grip, cur };
    } else if (box && box.contains(e.target)) {
      drag = { kind: "pan", cur, from: at };
    } else {
      // A click on empty track centres the window there -- the fastest way across
      // a two-hour trace.
      const width = cur.t1 - cur.t0;
      setView({ t0: at - width / 2, t1: at + width / 2 });
      return;
    }
    host.setPointerCapture(e.pointerId);
  });

  host.addEventListener("pointermove", (e) => {
    if (!drag) return;
    const at = timeAt(e);
    if (at === null) return;
    if (drag.kind === "pan") {
      const shift = at - drag.from;
      setView({ t0: drag.cur.t0 + shift, t1: drag.cur.t1 + shift });
    } else if (drag.kind === "left") {
      setView({ t0: Math.min(at, drag.cur.t1 - 5000), t1: drag.cur.t1 });
    } else {
      setView({ t0: drag.cur.t0, t1: Math.max(at, drag.cur.t0 + 5000) });
    }
  });

  const stop = () => {
    drag = null;
  };
  host.addEventListener("pointerup", stop);
  host.addEventListener("pointercancel", stop);
  host.addEventListener("dblclick", () => setView(null));
}

wireOverview();

/**
 * The ribbon's own listeners, bound **once** on the persistent hosts rather than
 * inside `renderRibbon`. Binding per render is the leak `wireOverview` was
 * extracted to fix; a hover handler would have added one listener per render.
 *
 * `wheel` is bound on `#ribbonWrap`, which covers the overview, the ribbon and
 * the chart, so scrolling anywhere in the timeline strip zooms about the pointer.
 * `#map` is deliberately left alone: Leaflet's own `scrollWheelZoom` owns it.
 */
function wireRibbon() {
  const wrap = el("ribbonWrap");
  const ribbon = el("ribbon");
  if (!wrap || !ribbon) return;
  wrap.addEventListener(
    "wheel",
    (e) => {
      if (!state.parsed) return;
      // The chart's own svg handler already zoomed; do not zoom twice.
      if (e.target.closest && e.target.closest("#chart svg")) return;
      e.preventDefault();
      const at = timeAtX(e.clientX);
      if (at === null) return;
      zoomAbout(at, e.deltaY > 0 ? 1.35 : 1 / 1.35);
    },
    { passive: false },
  );
  // Delegated, so it survives every re-render of the bands.
  ribbon.addEventListener("mousemove", (e) => {
    if (!state.parsed) return;
    const at = timeAtX(e.clientX);
    if (at !== null) showHover(at);
  });
  ribbon.addEventListener("mouseleave", hideHover);
}

wireRibbon();

// ---------------------------------------------------------------- ribbon

/**
 * The trace as a time-proportional strip: the one view that shows how long each
 * label actually lasted. Doubles as navigation -- click a band to select it.
 */
function renderRibbon() {
  const wrap = el("ribbon");
  if (!state.parsed || !state.segments.length) {
    wrap.innerHTML = '<span class="empty">The trace timeline appears here, to scale, once a file is open</span>';
    el("tStart").textContent = el("tSpan").textContent = el("tEnd").textContent = "";
    return;
  }
  // The ribbon follows the zoom, but keeps *every* band: a band outside the
  // window gets zero width rather than being dropped. That keeps the element
  // count equal to the segment count and the in-window bands tiling the track,
  // which is what makes the ribbon an honest picture of durations.
  //
  // Positioned absolutely from time, not laid out by flex: `xForTime` is the same
  // function the handles and the hover marker use, so a band's left edge and its
  // boundary handle cannot disagree. Under flex they did -- see the CSS.
  const w = viewWindow();
  const t0 = w.t0;
  const t1 = w.t1;
  const span = Math.max(1, t1 - t0);
  wrap.innerHTML = state.segments
    .map((s, i) => {
      const from = Math.max(s.t0, t0);
      const to = Math.min(s.t1, t1);
      const left = ((from - t0) / span) * 100;
      // Sub-pixel bands are unclickable on the ribbon. They already were, and the
      // table row and the boundary handles still reach them.
      const width = Math.max(0, ((to - from) / span) * 100);
      const mins = ((s.t1 - s.t0) / 60000).toFixed(1);
      const mean = meanSpeed(s);
      return `<span class="seg${s.edited ? " human" : ""}${i === state.selected ? " sel" : ""}"
        data-i="${i}" style="left: ${left.toFixed(4)}%; width: ${width.toFixed(4)}%;
        background: ${COLORS[s.type] ?? COLORS.unknown};
        color: ${COLORS[s.type] ?? COLORS.unknown}"
        title="${i + 1}. ${s.type} · ${mins} min · ${s.points} pts${
          mean === null ? "" : ` · ${mean.toFixed(1)} m/s mean`}${s.edited ? " · human" : ""}"></span>`;
    })
    .join("");
  // The hover cursor is part of the ribbon's markup, so it has to be re-added
  // after the bands are rewritten. Parked off-screen until a hover moves it.
  wrap.insertAdjacentHTML("beforeend", '<span class="cursor" style="left:-10px"></span>');
  renderHandles(t0, span);
  wrap.querySelectorAll(".seg").forEach((band) => {
    const i = Number(band.dataset.i);
    band.addEventListener("click", () => select(i));
    band.addEventListener("mouseover", () => emphasize(i, true));
    band.addEventListener("mouseout", () => emphasize(i, false));
  });
  const hhmm = (t) => new Date(t).toISOString().slice(11, 16);
  el("tStart").textContent = hhmm(t0);
  el("tEnd").textContent = hhmm(t1);
  const full = fullSpan();
  const zoomed = state.view && full;
  el("tSpan").textContent =
    `${(span / 60000).toFixed(0)} min · ${state.segments.length} segments` +
    (zoomed ? ` · zoomed from ${((full.t1 - full.t0) / 60000).toFixed(0)} min` : "");
}

/**
 * Boundary handles, as an overlay.
 *
 * One per interior boundary, positioned by percentage across the visible window.
 * They live outside the flex track on purpose (see the CSS): the bands must stay
 * exactly one element per segment with widths that sum to the track.
 */
function renderHandles(t0, span) {
  const host = el("ribbonHandles");
  if (!host) return;
  const inWindow = state.segments
    .map((s, i) => ({ i, at: s.t0 }))
    .filter(({ i, at }) => i > 0 && at >= t0 && at <= t0 + span);
  host.innerHTML = inWindow
    .map(({ i, at }) => {
      const left = ((at - t0) / span) * 100;
      return `<span class="handle" data-boundary="${i}" style="left:${left.toFixed(3)}%"
        title="Drag to move the boundary between segments ${i} and ${i + 1}"></span>`;
    })
    .join("");

  host.querySelectorAll(".handle").forEach((h) => {
    h.addEventListener("pointerdown", (e) => {
      e.preventDefault();
      e.stopPropagation();
      const i = Number(h.dataset.boundary);
      const track = el("ribbon").getBoundingClientRect();
      h.classList.add("dragging");
      h.setPointerCapture(e.pointerId);
      const move = (ev) => {
        const f = Math.min(1, Math.max(0, (ev.clientX - track.left) / track.width));
        h.style.left = `${(f * 100).toFixed(3)}%`;
      };
      const drop = (ev) => {
        h.removeEventListener("pointermove", move);
        h.removeEventListener("pointerup", drop);
        h.classList.remove("dragging");
        const f = Math.min(1, Math.max(0, (ev.clientX - track.left) / track.width));
        const at = t0 + f * span;
        const idx = nearestPointIndex(at);
        if (idx === null) return;
        state.history.snapshot(state.segments);
        state.segments = moveBoundary(state.segments, i, idx, state.parsed.points);
        render();
        warn(`moved the boundary to ${new Date(at).toISOString().slice(11, 19)}`);
      };
      h.addEventListener("pointermove", move);
      h.addEventListener("pointerup", drop);
    });
  });
}

/** Index of the trace point nearest a timestamp. */
function nearestPointIndex(t_ms) {
  const pts = state.parsed && state.parsed.points;
  if (!pts || !pts.length) return null;
  // Binary search, not a scan: this runs on every mousemove over the ribbon and
  // the chart, and a trace is time-ordered by construction (`js/gpx.js` sorts).
  let lo = 0;
  let hi = pts.length - 1;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (pts[mid].t_ms < t_ms) lo = mid + 1;
    else hi = mid;
  }
  // `lo` is the first point at or after `t_ms`; its predecessor can be nearer.
  if (lo > 0 && Math.abs(pts[lo - 1].t_ms - t_ms) <= Math.abs(pts[lo].t_ms - t_ms)) return lo - 1;
  return lo;
}

/** Mean smoothed speed over a segment, or null when nothing was derived. */
function meanSpeed(seg) {
  if (!state.results) return null;
  let sum = 0;
  let n = 0;
  for (let i = seg.start; i <= seg.end; i++) {
    const v = state.results[i] && state.results[i].speed;
    if (Number.isFinite(v)) {
      sum += v;
      n++;
    }
  }
  return n ? sum / n : null;
}

/** Peak smoothed speed over a segment, or null. */
function peakSpeed(seg) {
  if (!state.results) return null;
  let best = null;
  for (let i = seg.start; i <= seg.end; i++) {
    const v = state.results[i] && state.results[i].speed;
    if (Number.isFinite(v) && (best === null || v > best)) best = v;
  }
  return best;
}

// ---------------------------------------------------------------- table

function renderTable() {
  const rows = state.segments
    .map((s, i) => {
      const mins = ((s.t1 - s.t0) / 60000).toFixed(1);
      const opts = LABELS.map(
        (l) => `<option value="${l}"${l === s.type ? " selected" : ""}>${l}</option>`,
      ).join("");
      const mean = meanSpeed(s);
      const w = viewWindow();
      const inView = w && s.t1 >= w.t0 && s.t0 <= w.t1;
      return `<tr data-i="${i}" class="${i === state.selected ? "sel" : ""}${
        state.view && inView ? " in-view" : ""}">
        <td class="num">${i + 1}</td>
        <td class="label-cell"><span class="swatch" style="background:${COLORS[s.type] ?? COLORS.unknown}"></span>
            <select data-relabel="${i}" aria-label="Label for segment ${i + 1}">${opts}</select></td>
        <td class="num">${new Date(s.t0).toISOString().slice(11, 19)}</td>
        <td class="num">${mins}m</td>
        <td class="num">${mean === null ? "—" : mean.toFixed(1)}</td>
        <td class="num">${s.points}</td>
        <td class="num">${s.edited ? '<span class="tag">human</span>' : (s.confidence ?? 0).toFixed(2)}</td>
        <td class="num">${i > 0 ? `<button class="ghost" data-merge="${i}" title="Merge into the previous segment" aria-label="Merge segment ${i + 1} into the previous one">&uarr;</button>` : ""}</td>
      </tr>`;
    })
    .join("");
  el("segments").innerHTML = `<table>
    <colgroup><col class="c-n"><col class="c-label"><col class="c-start"><col class="c-dur">
      <col class="c-speed"><col class="c-pts"><col class="c-conf"><col></colgroup>
    <thead><tr><th>#</th><th>label</th><th>start</th><th>dur</th><th>m/s</th><th>pts</th>
      <th>conf</th><th></th></tr></thead>
    <tbody>${rows}</tbody></table>`;
  el("segCount").textContent = state.segments.length
    ? `${state.segments.length} · ${state.segments.filter((s) => s.edited).length} human`
    : "";

  el("segments").querySelectorAll("tr[data-i]").forEach((tr) => {
    const i = Number(tr.dataset.i);
    tr.addEventListener("click", (e) => {
      if (e.target.tagName === "SELECT" || e.target.tagName === "BUTTON") return;
      select(i);
    });
    tr.addEventListener("mouseover", () => emphasize(i, true));
    tr.addEventListener("mouseout", () => emphasize(i, false));
  });
  el("segments").querySelectorAll("select[data-relabel]").forEach((sel) => {
    sel.addEventListener("change", () => {
      state.history.snapshot(state.segments);
      state.segments = relabel(state.segments, Number(sel.dataset.relabel), sel.value);
      state.selected = Math.min(state.selected, state.segments.length - 1);
      render();
    });
  });
  el("segments").querySelectorAll("button[data-merge]").forEach((b) => {
    b.addEventListener("click", () => {
      state.history.snapshot(state.segments);
      state.segments = mergeWithPrevious(
        state.segments,
        Number(b.dataset.merge),
        state.parsed.points,
      );
      state.selected = -1;
      render();
    });
  });
}

function emphasize(i, on) {
  const line = polylines[i];
  if (line) line.setStyle({ weight: on ? 8 : i === state.selected ? 7 : 4, color: on ? "#fff" : COLORS[state.segments[i].type] });
}

/**
 * Mark the trace point at `t_ms` on the map, and read it out under the ribbon.
 *
 * The ribbon shows *when*; the map shows *where*. Hovering one and seeing the
 * other is the only way to tell whether a band covers the stretch of road you
 * think it does. Driven by both the ribbon and the chart, through the same
 * function, so the two views cannot answer differently.
 */
function showHover(t_ms) {
  const idx = nearestPointIndex(t_ms);
  if (idx === null) return;
  const p = state.parsed.points[idx];
  hoverMarker.setLatLng([p.lat, p.lon]);
  if (!hoverLayer.hasLayer(hoverMarker)) hoverMarker.addTo(hoverLayer);
  const info = el("hoverInfo");
  if (info) {
    const clock = new Date(p.t_ms).toISOString().slice(11, 19);
    const seg = state.segments.findIndex((s) => idx >= s.start && idx <= s.end);
    const speed = p.speed_mps ?? speedAt(idx);
    info.textContent =
      `${clock} · point ${idx + 1}/${state.parsed.points.length}` +
      (speed === null ? "" : ` · ${speed.toFixed(1)} m/s`) +
      (seg >= 0 ? ` · segment ${seg + 1} ${state.segments[seg].type}` : "");
  }
  const marker = el("ribbon").querySelector(".cursor");
  if (marker) marker.style.left = `${(xForTime(t_ms) * 100).toFixed(3)}%`;
}

function hideHover() {
  hoverLayer.removeLayer(hoverMarker);
  const info = el("hoverInfo");
  // A non-breaking space, not "": an empty span collapses the row, the chart
  // below it moves, and a drag in flight over the chart lands somewhere else.
  if (info) info.textContent = "\u00a0";
  const marker = el("ribbon").querySelector(".cursor");
  if (marker) marker.style.left = "-10px";
}

/**
 * Speed at a point index, derived from its neighbours when the trace carries no
 * `speed` extension -- which most GPX files do not.
 */
function speedAt(idx) {
  const pts = state.parsed && state.parsed.points;
  if (!pts || pts.length < 2) return null;
  const a = pts[Math.max(0, idx - 1)];
  const b = pts[Math.min(pts.length - 1, idx + 1)];
  const dt = (b.t_ms - a.t_ms) / 1000;
  if (!(dt > 0)) return null;
  return wasm.distance_m(a.lat, a.lon, b.lat, b.lon) / dt;
}

function highlightRow(i, on) {
  const tr = el("segments").querySelector(`tr[data-i="${i}"]`);
  if (tr) tr.style.background = on ? "#2a3240" : "";
}

function select(i) {
  state.selected = i;
  const sel = state.segments[i];
  if (sel && state.view && (sel.t1 < state.view.t0 || sel.t0 > state.view.t1)) {
    // Selecting from the table while zoomed elsewhere would leave every time view
    // showing something unrelated to the inspector, so the window follows.
    const pad = Math.max(30_000, (sel.t1 - sel.t0) * 0.25);
    setView({ t0: sel.t0 - pad, t1: sel.t1 + pad });
    return;
  }
  render();
  const s = state.segments[i];
  if (s) {
    map.fitBounds(
      state.parsed.points.slice(s.start, s.end + 1).map((p) => [p.lat, p.lon]),
      { padding: [30, 30] },
    );
  }
}

function renderDetail() {
  const s = state.segments[state.selected];
  if (!s) {
    el("detail").innerHTML = state.parsed
      ? `<div class="hint">Select a segment — in the table, on the ribbon, or on the map — to
           inspect it. With one selected, clicking a vertex splits it there.</div>`
      : `<div class="hint">Open a GPX file to start. Parsing and classification run in this tab;
           map context is read from the tile host by byte range, and only when you ask for it.</div>`;
    return;
  }
  const c = s.context;
  // Scanned footprints whose contained points fall in this segment. From the
  // on-demand trace scan, so it is empty until the user runs it.
  const insideHere = (state.traceBuildings ?? []).filter((b) =>
    b.points.some((i) => i >= s.start && i <= s.end),
  );
  const road = c && c.road
    ? `<b>${escapeHtml(c.road.name ?? "(unnamed)")}</b> ${escapeHtml(c.road.road_class)}, ${c.road.distance_m.toFixed(1)} m`
    : state.resolved ? "nothing within 30 m" : "not resolved";
  const ix = c && c.intersection
    ? `${wasm.intersection_type_name(c.intersection.intersection_type)} at ${c.intersection.distance_m.toFixed(1)} m`
    : state.resolved ? "none nearby" : "not resolved";
  const adm = c && c.admin
    ? `${escapeHtml(c.admin.county ?? "?")} · ${escapeHtml(c.admin.zip ?? "?")} · ${escapeHtml(c.admin.timezone ?? "?")}`
    : "not resolved";
  const mins = ((s.t1 - s.t0) / 60000).toFixed(1);
  el("detail").innerHTML = `
    <div class="hdr">
      <span class="swatch" style="background:${COLORS[s.type] ?? COLORS.unknown}"></span>
      <span class="label">${s.type}</span>
      <span class="meta">segment ${state.selected + 1} of ${state.segments.length} ·
        ${mins} min · ${s.points} points ·
        ${s.edited ? "labeled by you" : `proposed, confidence ${(s.confidence ?? 0).toFixed(2)}`}</span>
    </div>
    <dl>
      <dt>speed</dt><dd>${
        meanSpeed(s) === null
          ? "not derived"
          : `<b>${meanSpeed(s).toFixed(1)} m/s</b> mean · ${peakSpeed(s).toFixed(1)} peak · ${
              (meanSpeed(s) * 2.23694).toFixed(1)} mph`}</dd>
      <dt>per-fix vote</dt><dd>${s.vote}${s.atControl ? " · at a mapped traffic control" : ""}</dd>
      <dt>road</dt><dd>${road}</dd>
      <dt>intersection</dt><dd>${ix}</dd>
      <dt>admin</dt><dd>${adm}</dd>
      ${s.context && s.context.building ? `<dt>building</dt><dd><b>${escapeHtml(
        s.context.building.name || s.context.building.building_type || "unnamed",
      )}</b>${s.context.building.inside ? " · inside" : ` · ${s.context.building.distance_m.toFixed(0)} m`}</dd>` : ""}
      ${s.context && s.context.businesses && s.context.businesses.length ? `<dt>businesses</dt><dd>${
        s.context.businesses.map((b) => escapeHtml(b.category ? `${b.name} (${b.category})` : b.name)).join(", ")}</dd>` : ""}
      ${insideHere.length ? `<dt>inside</dt><dd>${insideHere
        .map((b) => `${escapeHtml(b.name || b.type || "building")} <span class="sub">${b.points.length} pts</span>`)
        .join(", ")}</dd>` : ""}
      ${s.context && s.context.addresses && s.context.addresses.length ? `<dt>address</dt><dd>${
        escapeHtml(s.context.addresses[0].housenumber)} ${escapeHtml(s.context.addresses[0].street)}</dd>` : ""}
      <dt>vintage</dt><dd>trace recorded ${new Date(s.t0).getFullYear()}, map snapshot
        ${SNAPSHOT} — context is what the map says <em>now</em></dd>
    </dl>`;
}

function renderLegend() {
  const per = timePerLabel(state.segments);
  const mins = (ms) => (ms >= 60000 ? `${Math.round(ms / 60000)} min` : `${Math.round(ms / 1000)} s`);
  el("legend").innerHTML = LABELS.filter((l) => (per.get(l) ?? 0) > 0)
    .map((l) => `<span><span class="swatch" style="background:${COLORS[l]}"></span>${l}
      ${mins(per.get(l))}</span>`)
    .join("");
}

function render() {
  drawSegments();
  renderOverview();
  renderRibbon();
  renderChartIfShown();
  renderPlace();
  renderTable();
  renderDetail();
  renderLegend();
  el("undo").disabled = state.history.depth === 0;
  status();
}

/**
 * Footer counters. The request/byte numbers come from js/ptiles.js's own stats,
 * so the page's claims about how little it fetches stay falsifiable rather than
 * decorative.
 */
function status() {
  const s = P ? P.stats : { requests: 0, bytes: 0 };
  el("fReq").textContent = s.requests;
  el("fBytes").textContent = `${(s.bytes / 1e6).toFixed(1)} MB`;
  el("fSnapshot").textContent = SNAPSHOT;
  if (!state.parsed) return;
  const mins = (
    (state.parsed.points.at(-1).t_ms - state.parsed.points[0].t_ms) / 60000
  ).toFixed(0);
  el("fPoints").textContent = state.parsed.points.length;
  el("fDur").textContent = `${mins} min`;
  el("fSegs").textContent = state.segments.length;
  el("fFlavour").textContent = state.parsed.flavour === "rook" ? "rook (with sensors)" : "plain GPX";
  // A disabled control has to say what would enable it, next to where the eye
  // already is. "Context not resolved" described a state; this names the action.
  // Short and shaped the same in every state: a status line that grows is a
  // status line that rewraps the toolbar. The long hint lives on the button's
  // title and in the footer instead.
  el("status").textContent = state.parsed
    ? `context: ${state.resolved ? "applied" : "none"}${state.view ? " · zoomed" : ""}`
    : "no trace";
}

function escapeHtml(v) {
  return String(v).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));
}

function warn(msg) {
  el("warn").textContent = msg ?? "";
}

// ---------------------------------------------------------------- actions

el("file").addEventListener("change", async (e) => {
  const file = e.target.files[0];
  if (!file) return;
  await ready;
  warn("");
  let parsed;
  try {
    parsed = parseGpx(await file.text());
  } catch (err) {
    warn(`could not parse ${file.name}: ${err.message}`);
    return;
  }
  if (!parsed.points.length) {
    warn(`${file.name} has no track points with timestamps`);
    return;
  }
  state.file = file.name;
  el("fileName").textContent = file.name;
  state.parsed = parsed;
  state.resolved = false;
  state.selected = -1;
  state.history = createHistory(20);
  state.results = classifyTrace(wasm, parsed.points);
  state.shifts = null;
  state.view = null;
  // Scanned footprints belong to the previous trace.
  state.traceBuildings = null;
  buildingLayer.clearLayers();
  hideHover();
  state.segments = coalesce(parsed.points, state.results);
  // A rook file's own context is kept as-is rather than recomputed: it was
  // captured in the field, and this snapshot is years newer.
  parsed.tracks.forEach((t) => {
    if (!t.context) return;
    const s = state.segments.find((x) => x.start >= t.firstPoint);
    if (s) s.sourceContext = t.context;
  });
  el("reclassify").disabled = false;
  el("download").disabled = false;
  el("chartToggle").disabled = false;
  el("chartWindow").disabled = false;
  // Only the cells the trace occupies are worth drawing or fetching -- see
  // basemap.setTrace. A 90 km drive is ~50 cells; the viewport at a working zoom
  // asks for hundreds, nearly all of them nowhere near the trace.
  const cells = basemap ? basemap.setTrace(parsed.points) : 0;
  if (cells) basemapNote(`${cells} cells cover this trace`);
  if (parsed.dropped) warn(`${parsed.dropped} point(s) dropped for having no usable time`);
  map.fitBounds(parsed.points.map((p) => [p.lat, p.lon]), { padding: [20, 20] });
  render();
});

/**
 * Resolve map context for every segment, then classify again with it.
 *
 * One action, because the two halves are never useful apart: the priors exist to
 * change the classification, and a resolve with no re-run just leaves numbers in
 * a panel. Segments a human has labelled are preserved across the re-run -- a
 * classifier pass must never overwrite a human decision.
 */
async function classifyWithContext() {
  const steps = sheet([
    "Read the map layers for the cells this trace touches",
    "Resolve a road and intersection per segment",
    "Classify again, with the priors",
  ]);
  el("reclassify").disabled = true;
  try {
    steps.doing(0);
    const cells = await resolver.prefetch(state.parsed.points);
    const cellCount = [...cells.values()].reduce((n, set) => n + set.size, 0);
    const failed = [...resolver.failures.keys()];
    if (failed.length) {
      steps.failed(0, `${failed.join(", ")} unavailable`);
    } else {
      steps.done(0, `${cellCount} cells across ${cells.size} state(s)`);
    }

    steps.doing(1, `0 / ${state.segments.length}`);
    let done = 0;
    for (const seg of state.segments) {
      // A rook file's own context was captured in the field; this snapshot is
      // years newer, so it is kept rather than overwritten.
      if (!seg.sourceContext) {
        seg.context = await resolver.forSegment(state.parsed.points, sampleIndices(seg, 5));
      }
      steps.doing(1, `${++done} / ${state.segments.length}`);
      status();
    }
    state.resolved = true;
    const withRoad = state.segments.filter((s) => s.context && s.context.road).length;
    steps.done(1, `${withRoad} of ${state.segments.length} got a road`);

    steps.doing(2);
    state.history.snapshot(state.segments);
    const edited = state.segments.filter((s) => s.edited);
    const ctxAt = (i) => {
      const s = state.segments.find((x) => i >= x.start && i <= x.end);
      return s && s.context ? { road: s.context.road, intersection: s.context.intersection } : {};
    };
    state.results = classifyTrace(wasm, state.parsed.points, { contextFor: ctxAt });
    state.shifts = null;
    const fresh = coalesce(state.parsed.points, state.results);
    for (const h of edited) {
      for (const seg of fresh) {
        if (seg.start >= h.start && seg.end <= h.end) {
          seg.type = h.type;
          seg.edited = true;
          seg.context = h.context;
          seg.sourceContext = h.sourceContext;
        }
      }
    }
    state.segments = fresh;
    state.selected = -1;
    steps.done(2, `${fresh.length} segments`);
    render();
    if (failed.length) {
      steps.hold(`Some layers were unavailable: ${failed.join(", ")}. Those segments have no road context.`);
      warn(`no layer for ${failed.join(", ")} in snapshot ${SNAPSHOT}`);
    } else {
      steps.close();
    }
  } catch (err) {
    steps.failed(0, err.message);
    steps.hold(`Could not finish: ${err.message}`);
    warn(`context resolution failed: ${err.message}`);
  } finally {
    el("reclassify").disabled = false;
    render();
  }
}

el("reclassify").addEventListener("click", classifyWithContext);

el("undo").addEventListener("click", () => {
  const prev = state.history.undo();
  if (prev) {
    state.segments = prev;
    state.selected = -1;
    render();
  }
});

document.addEventListener("keydown", (e) => {
  if ((e.ctrlKey || e.metaKey) && e.key === "z") {
    e.preventDefault();
    el("undo").click();
  }
});

el("download").addEventListener("click", () => {
  const xml = writeGpx(state.parsed, state.segments, {
    snapshot: state.resolved ? SNAPSHOT : undefined,
    // Speed is derived from positions when the file did not report one; nothing
    // here is invented, so `synthetic` stays empty (SCHEMA.md).
    derived: state.parsed.points.some((p) => p.derivedSpeed !== undefined) ? "speed" : "",
    synthetic: "",
    samples: state.resolved ? 5 : undefined,
  }, wasm.intersection_type_name);
  const name = (state.file || "trace.gpx").replace(/\.gpx$/i, "") + ".labeled.gpx";
  const url = URL.createObjectURL(new Blob([xml], { type: "application/gpx+xml" }));
  const a = document.createElement("a");
  a.href = url;
  a.download = name;
  a.click();
  URL.revokeObjectURL(url);
});

// Exposed for the browser check (a headless run cannot read "18 segments" off a
// polyline's colour), same idea as web-demo/test/render_check.py's hooks. The map
// too, so a test can fire a click at a real coordinate rather than guess a pixel.
window.__labelGpx = state;
window.__leafletMap = map;
// For the browser tests: where the hover marker is, or null when hidden. The
// marker is a Leaflet internal otherwise, and asserting on the DOM canvas cannot
// tell you *which* point it is on.
// For the browser tests: run the trace-wide building scan over an arbitrary point
// list, so a synthetic one-point "trace" inside a known footprint can exercise the
// containment path even when every committed fixture is a trail through woodland.
state.scanForTests = (pts) => resolver.buildingsOnTrace(pts);
// A getter, not a snapshot: `resolver` is null until wasm finishes initialising.
Object.defineProperty(state, "resolverForTests", { get: () => resolver });
state.hoverMarkerLatLng = () =>
  hoverLayer.hasLayer(hoverMarker)
    ? [hoverMarker.getLatLng().lat, hoverMarker.getLatLng().lng]
    : null;
