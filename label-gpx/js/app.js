// Page wiring: file in, map and table, labels out. All the interesting logic
// lives in gpx.js (XML), segments.js (labeling, DOM-free and tested) and
// context.js (layers); this file is the parts that only make sense in a browser.

import init_wasm, * as wasm from "../lib/client/ptiles_client.js";
import { createPtiles } from "./ptiles.js";
import { parseGpx, writeGpx, LABELS } from "./gpx.js";
import {
  classifyTrace, coalesce, splitSegment, mergeWithPrevious, relabel,
  sampleIndices, createHistory, timePerLabel,
} from "./segments.js";
import { createResolver, SNAPSHOT, stateAt, stateUrl } from "./context.js";
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
        state.segments = splitSegment(state.segments, state.selected, i);
        render();
      })
      .bindTooltip(`split here (point ${i})`)
      .addTo(vertexLayer);
  }
}

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
  return wrap;
};
basemapControl.addTo(map);

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
  const t0 = state.segments[0].t0;
  const t1 = state.segments.at(-1).t1;
  const span = Math.max(1, t1 - t0);
  wrap.innerHTML = state.segments
    .map((s, i) => {
      // Growth factors, not a percentage basis, so the bands always divide the
      // track exactly. Scaled by 100 on purpose: when flex-grow factors sum to
      // less than 1, CSS hands each item only `grow x free-space` and leaves the
      // remainder empty -- which is why the fractions alone left a gap at the
      // end of the ribbon.
      const grow = Math.max(0.05, ((s.t1 - s.t0) / span) * 100);
      const mins = ((s.t1 - s.t0) / 60000).toFixed(1);
      return `<span class="seg${s.edited ? " human" : ""}${i === state.selected ? " sel" : ""}"
        data-i="${i}" style="flex: ${grow} 1 0; background: ${COLORS[s.type] ?? COLORS.unknown};
        color: ${COLORS[s.type] ?? COLORS.unknown}"
        title="${i + 1}. ${s.type} · ${mins} min · ${s.points} pts${s.edited ? " · human" : ""}"></span>`;
    })
    .join("");
  wrap.querySelectorAll(".seg").forEach((band) => {
    const i = Number(band.dataset.i);
    band.addEventListener("click", () => select(i));
    band.addEventListener("mouseover", () => emphasize(i, true));
    band.addEventListener("mouseout", () => emphasize(i, false));
  });
  const hhmm = (t) => new Date(t).toISOString().slice(11, 16);
  el("tStart").textContent = hhmm(t0);
  el("tEnd").textContent = hhmm(t1);
  el("tSpan").textContent = `${(span / 60000).toFixed(0)} min · ${state.segments.length} segments`;
}

// ---------------------------------------------------------------- table

function renderTable() {
  const rows = state.segments
    .map((s, i) => {
      const mins = ((s.t1 - s.t0) / 60000).toFixed(1);
      const opts = LABELS.map(
        (l) => `<option value="${l}"${l === s.type ? " selected" : ""}>${l}</option>`,
      ).join("");
      return `<tr data-i="${i}" class="${i === state.selected ? "sel" : ""}">
        <td class="num">${i + 1}</td>
        <td class="label-cell"><span class="swatch" style="background:${COLORS[s.type] ?? COLORS.unknown}"></span>
            <select data-relabel="${i}" aria-label="Label for segment ${i + 1}">${opts}</select></td>
        <td class="num">${new Date(s.t0).toISOString().slice(11, 19)}</td>
        <td class="num">${mins}m</td>
        <td class="num">${s.points}</td>
        <td class="num">${s.edited ? '<span class="tag">human</span>' : (s.confidence ?? 0).toFixed(2)}</td>
        <td class="num">${i > 0 ? `<button class="ghost" data-merge="${i}" title="Merge into the previous segment" aria-label="Merge segment ${i + 1} into the previous one">&uarr;</button>` : ""}</td>
      </tr>`;
    })
    .join("");
  el("segments").innerHTML = `<table>
    <colgroup><col class="c-n"><col class="c-label"><col class="c-start"><col class="c-dur">
      <col class="c-pts"><col class="c-conf"><col></colgroup>
    <thead><tr><th>#</th><th>label</th><th>start</th><th>dur</th><th>pts</th><th>conf</th><th></th></tr></thead>
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
      state.segments = mergeWithPrevious(state.segments, Number(b.dataset.merge));
      state.selected = -1;
      render();
    });
  });
}

function emphasize(i, on) {
  const line = polylines[i];
  if (line) line.setStyle({ weight: on ? 8 : i === state.selected ? 7 : 4, color: on ? "#fff" : COLORS[state.segments[i].type] });
}

function highlightRow(i, on) {
  const tr = el("segments").querySelector(`tr[data-i="${i}"]`);
  if (tr) tr.style.background = on ? "#2a3240" : "";
}

function select(i) {
  state.selected = i;
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
      <dt>per-fix vote</dt><dd>${s.vote}${s.atControl ? " · at a mapped traffic control" : ""}</dd>
      <dt>road</dt><dd>${road}</dd>
      <dt>intersection</dt><dd>${ix}</dd>
      <dt>admin</dt><dd>${adm}</dd>
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
  renderRibbon();
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
  el("status").textContent = state.resolved ? "Context resolved" : "Context not resolved";
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
  state.segments = coalesce(parsed.points, state.results);
  // A rook file's own context is kept as-is rather than recomputed: it was
  // captured in the field, and this snapshot is years newer.
  parsed.tracks.forEach((t) => {
    if (!t.context) return;
    const s = state.segments.find((x) => x.start >= t.firstPoint);
    if (s) s.sourceContext = t.context;
  });
  el("resolve").disabled = false;
  el("download").disabled = false;
  if (parsed.dropped) warn(`${parsed.dropped} point(s) dropped for having no usable time`);
  map.fitBounds(parsed.points.map((p) => [p.lat, p.lon]), { padding: [20, 20] });
  render();
});

el("resolve").addEventListener("click", async () => {
  el("resolve").disabled = true;
  const btn = el("resolve");
  btn.textContent = "Resolving…";
  try {
    const cells = await resolver.prefetch(state.parsed.points);
    const cellCount = [...cells.values()].reduce((n, s) => n + s.size, 0);
    for (const s of state.segments) {
      if (s.sourceContext) continue; // keep field-captured context
      s.context = await resolver.forSegment(state.parsed.points, sampleIndices(s, 5));
      status();
    }
    state.resolved = true;
    const failed = [...resolver.failures.entries()];
    warn(
      failed.length
        ? `no roads layer for ${failed.map(([st]) => st).join(", ")} in snapshot ${SNAPSHOT} — those segments have no road context`
        : `resolved ${cellCount} cells across ${cells.size} state(s)`,
    );
    el("reclassify").disabled = false;
  } catch (err) {
    warn(`context resolution failed: ${err.message}`);
  } finally {
    btn.textContent = "Resolve map context";
    btn.disabled = false;
    render();
  }
});

// The payoff pass: with road and intersection context per segment, the priors
// can see a walk that speed alone cannot, and a stop at a signal stops reading
// as an arrival. Segments a human edited are left alone.
el("reclassify").addEventListener("click", () => {
  state.history.snapshot(state.segments);
  const ctxAt = (i) => {
    const s = state.segments.find((x) => i >= x.start && i <= x.end);
    return s && s.context ? { road: s.context.road, intersection: s.context.intersection } : {};
  };
  const edited = state.segments.filter((s) => s.edited);
  state.results = classifyTrace(wasm, state.parsed.points, { contextFor: ctxAt });
  const fresh = coalesce(state.parsed.points, state.results);
  // Re-apply human labels: a re-run must never overwrite a human decision.
  for (const h of edited) {
    for (const s of fresh) {
      if (s.start >= h.start && s.end <= h.end) {
        s.type = h.type;
        s.edited = true;
      }
    }
  }
  state.segments = fresh;
  // Context is per-segment and the boundaries just moved, so re-attach by span.
  state.selected = -1;
  render();
  warn(`re-classified with road context: ${state.segments.length} segments`);
});

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
// polyline's colour), same idea as web-demo/test/render_check.py's hooks.
window.__labelGpx = state;
