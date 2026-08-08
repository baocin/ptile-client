// PTILES wasm demo: all format work (header/index parsing, cell math,
// decompression, record decoding, nearest-road, business search) goes
// through ./pkg/ptiles_wasm.js (built from this repo's wasm/ crate). This
// file only does: Leaflet map/UI wiring, HTTP Range requests (via
// ./ptiles-remote.js), and turning already-decoded plain JS
// objects/arrays into Leaflet layers. No PTILES record format is
// hand-decoded here.

import init, * as wasm from "../pkg/ptiles_wasm.js";
import { PtilesRemoteFile, stateLayerUrl } from "./ptiles-remote.js";

const STATE_CENTERS = {
  AL: { center: [32.8, -86.8], zoom: 7 },
  AK: { center: [64.2, -153.4], zoom: 4 },
  AZ: { center: [34.3, -111.1], zoom: 7 },
  AR: { center: [34.9, -92.4], zoom: 7 },
  CA: { center: [36.8, -119.5], zoom: 6 },
  CO: { center: [38.9, -105.5], zoom: 7 },
  CT: { center: [41.6, -72.7], zoom: 9 },
  DC: { center: [38.9, -77.0], zoom: 12 },
  DE: { center: [39.0, -75.5], zoom: 9 },
  FL: { center: [27.7, -81.5], zoom: 7 },
  GA: { center: [32.7, -83.5], zoom: 7 },
  HI: { center: [20.8, -156.3], zoom: 7 },
  ID: { center: [44.4, -114.5], zoom: 7 },
  IL: { center: [40.0, -89.2], zoom: 7 },
  IN: { center: [39.9, -86.3], zoom: 7 },
  IA: { center: [41.9, -93.1], zoom: 7 },
  KS: { center: [38.5, -98.4], zoom: 7 },
  KY: { center: [37.7, -85.3], zoom: 7 },
  LA: { center: [31.2, -91.8], zoom: 7 },
  ME: { center: [45.3, -69.2], zoom: 7 },
  MD: { center: [39.1, -76.8], zoom: 8 },
  MA: { center: [42.3, -71.8], zoom: 8 },
  MI: { center: [44.3, -85.4], zoom: 7 },
  MN: { center: [46.3, -94.2], zoom: 7 },
  MS: { center: [32.6, -89.7], zoom: 7 },
  MO: { center: [38.4, -92.5], zoom: 7 },
  MT: { center: [46.9, -110.5], zoom: 6 },
  NE: { center: [41.5, -99.9], zoom: 7 },
  NV: { center: [39.3, -116.6], zoom: 7 },
  NH: { center: [43.7, -71.6], zoom: 8 },
  NJ: { center: [40.2, -74.7], zoom: 8 },
  NM: { center: [34.5, -106.0], zoom: 7 },
  NY: { center: [42.9, -75.5], zoom: 7 },
  NC: { center: [35.6, -79.4], zoom: 7 },
  ND: { center: [47.5, -100.4], zoom: 7 },
  OH: { center: [40.4, -82.8], zoom: 7 },
  OK: { center: [35.6, -96.9], zoom: 7 },
  OR: { center: [43.9, -120.6], zoom: 7 },
  PA: { center: [40.9, -77.8], zoom: 7 },
  RI: { center: [41.7, -71.5], zoom: 10 },
  SC: { center: [33.9, -80.9], zoom: 8 },
  SD: { center: [44.4, -100.3], zoom: 7 },
  TN: { center: [35.96, -86.52], zoom: 8 },
  TX: { center: [31.2, -99.3], zoom: 6 },
  UT: { center: [39.3, -111.1], zoom: 7 },
  VT: { center: [44.0, -72.7], zoom: 8 },
  VA: { center: [37.5, -78.8], zoom: 7 },
  WA: { center: [47.4, -120.5], zoom: 7 },
  WV: { center: [38.7, -80.7], zoom: 7 },
  WI: { center: [44.2, -89.8], zoom: 7 },
  WY: { center: [43.0, -107.6], zoom: 7 },
};

const ROAD_STYLES = [
  { color: "#f0883e", weight: 4 },
  { color: "#f0883e", weight: 3 },
  { color: "#e3b341", weight: 3 },
  { color: "#e3b341", weight: 2 },
  { color: "#d29922", weight: 2 },
  { color: "#d29922", weight: 2 },
  { color: "#58a6ff", weight: 2 },
  { color: "#388bfd", weight: 1.5 },
  { color: "#8b949e", weight: 1 },
];
function roadStyle(roadClass) {
  const idx = [
    "motorway",
    "trunk",
    "primary",
    "secondary",
    "tertiary",
    "unclassified",
    "residential",
    "service",
    "path",
  ].indexOf(roadClass);
  const s = ROAD_STYLES[idx] || { color: "#8b949e", weight: 0.7 };
  return { color: s.color, weight: s.weight, opacity: 0.85 };
}

function setStatus(msg) {
  document.getElementById("status").textContent = msg;
}
function setLayerStatus(msg) {
  document.getElementById("layerStatus").textContent = msg;
}

let wasmMod;

class Layer {
  constructor(name, decode, style) {
    this.name = name;
    this.decode = decode; // (wasmMod, blockBytes, cellHex) -> features array
    this.style = style;
    this.group = L.layerGroup();
    this.file = null;
    this.loading = false;
    this.rendered = new Set();
    this.enabled = false;
  }
  reset() {
    this.file = null;
    this.rendered.clear();
    this.group.clearLayers();
  }
  async ensureOpen(state) {
    if (this.file || this.loading) return;
    this.loading = true;
    try {
      this.file = new PtilesRemoteFile(
        wasmMod,
        stateLayerUrl(state, this.name),
      );
      await this.file.open();
    } catch (e) {
      setLayerStatus(`${this.name}: ${e.message}`);
      this.file = null;
    }
    this.loading = false;
  }
  async renderCell(cellHex) {
    if (!this.file || this.rendered.has(cellHex)) return 0;
    this.rendered.add(cellHex);
    let raw;
    try {
      raw = await this.file.blockForCell(cellHex);
    } catch (e) {
      return 0;
    }
    if (!raw) return 0;
    const features = this.decode(raw, cellHex);
    for (const layer of features) this.group.addLayer(layer);
    return features.length;
  }
}

const layers = {
  roads: new Layer("roads", (raw) => {
    const segs = wasmMod.decode_roads(raw);
    return segs
      .filter((s) => s.coords.length >= 2)
      .map((s) =>
        L.polyline(
          s.coords.map((c) => [c[1], c[0]]),
          roadStyle(s.road_class),
        ),
      );
  }),
  water: new Layer("water", (raw) => {
    const feats = wasmMod.decode_water(raw);
    return feats
      .filter((f) => f.coords.length >= 2)
      .map((f) =>
        f.geom_type === 0
          ? L.polygon(
              f.coords.map((c) => [c[1], c[0]]),
              {
                color: "#1f6feb",
                weight: 1,
                fillColor: "#1f6feb",
                fillOpacity: 0.25,
              },
            )
          : L.polyline(
              f.coords.map((c) => [c[1], c[0]]),
              { color: "#1f6feb", weight: 1.5, opacity: 0.7 },
            ),
      );
  }),
  parks: new Layer("parks", (raw) => {
    const feats = wasmMod.decode_parks(raw);
    return feats
      .filter((f) => f.coords.length >= 3)
      .map((f) =>
        L.polygon(
          f.coords.map((c) => [c[1], c[0]]),
          {
            color: "#238636",
            weight: 1,
            fillColor: "#238636",
            fillOpacity: 0.2,
          },
        ),
      );
  }),
  rail: new Layer("rail", (raw) => {
    const feats = wasmMod.decode_rail(raw);
    return feats.map((f) =>
      f.geom_type === 0 && f.coords.length >= 2
        ? L.polyline(
            f.coords.map((c) => [c[1], c[0]]),
            { color: "#484f58", weight: 2, opacity: 0.9 },
          )
        : L.circleMarker([f.coords[0][1], f.coords[0][0]], {
            radius: 4,
            color: "#484f58",
            fillColor: "#484f58",
            fillOpacity: 0.8,
            weight: 1,
          }),
    );
  }),
  buildings: new Layer("buildings_v9", (raw, cellHex) => {
    const [lat, lon] = wasmMod.cell_center(cellHex);
    const bldgs = wasmMod.decode_buildings(raw, lat, lon);
    const colors = [
      "#6e40c9",
      "#388bfd",
      "#3fb950",
      "#f0883e",
      "#da3633",
      "#d29922",
    ];
    return bldgs
      .filter((b) => b.coords.length >= 3)
      .map((b) => {
        const c =
          colors[
            Number(
              BigInt(b.osm_id) < 0n ? -BigInt(b.osm_id) : BigInt(b.osm_id),
            ) % colors.length
          ];
        return L.polygon(
          b.coords.map((c2) => [c2[1], c2[0]]),
          { color: c, weight: 1, fillColor: c, fillOpacity: 0.1 },
        );
      });
  }),
};

let currentState = "TN";
let viewportTimer = null;

async function renderViewport() {
  const anyEnabled = Object.values(layers).some((l) => l.enabled);
  if (!anyEnabled) return;
  const b = map.getBounds();
  const sw = b.getSouthWest(),
    ne = b.getNorthEast();
  let cells;
  try {
    cells = wasmMod.cells_for_bounds(sw.lat, sw.lng, ne.lat, ne.lng);
  } catch (e) {
    setLayerStatus("zoom in to render (" + e + ")");
    return;
  }
  setLayerStatus(`rendering ${cells.length} cells...`);
  let count = 0;
  for (const [key, layer] of Object.entries(layers)) {
    if (!layer.enabled) continue;
    await layer.ensureOpen(currentState);
    for (const cellHex of cells) count += await layer.renderCell(cellHex);
  }
  setLayerStatus(count > 0 ? `+${count} features` : "no features in view");
}

function scheduleRender() {
  clearTimeout(viewportTimer);
  viewportTimer = setTimeout(renderViewport, 600);
}

// --- nearest-road click handling ---
async function findNearestRoad(lat, lon) {
  await layers.roads.ensureOpen(currentState);
  if (!layers.roads.file) return null;
  const cellHex = wasmMod.cell_for_coord(lat, lon);
  const candidates = [cellHex, ...wasmMod.neighbor_cells(cellHex)];
  let best = null;
  for (const c of candidates) {
    let raw;
    try {
      raw = await layers.roads.file.blockForCell(c);
    } catch (e) {
      continue;
    }
    if (!raw) continue;
    const found = wasmMod.nearest_road(raw, lat, lon, undefined);
    if (found && (!best || found.distance_m < best.distance_m)) best = found;
  }
  return best;
}

let clickMarker = null;
let clickHighlight = null;

async function onMapClick(e) {
  const { lat, lng } = e.latlng;
  setStatus("looking up nearest road...");
  if (clickMarker) map.removeLayer(clickMarker);
  clickMarker = L.circleMarker([lat, lng], {
    radius: 5,
    color: "#f0883e",
    fillColor: "#f0883e",
    fillOpacity: 0.9,
  }).addTo(map);
  try {
    const road = await findNearestRoad(lat, lng);
    if (clickHighlight) {
      map.removeLayer(clickHighlight);
      clickHighlight = null;
    }
    const panel = document.getElementById("infoPanel");
    if (!road) {
      panel.classList.add("show");
      document.getElementById("infoBody").textContent =
        "No road found within threshold.";
      setStatus("no road found");
      return;
    }
    clickHighlight = L.polyline(road.geometry, {
      color: "#3fb950",
      weight: 4,
      opacity: 0.9,
    }).addTo(map);
    panel.classList.add("show");
    document.getElementById("infoBody").innerHTML = `
      <div class="name">${road.name || "(unnamed road)"}</div>
      <div class="row"><span>OSM ID</span><span>${road.osm_id}</span></div>
      <div class="row"><span>Class</span><span>${road.road_class}</span></div>
      <div class="row"><span>Distance</span><span>${road.distance_m.toFixed(1)} m</span></div>
    `;
    setStatus(
      `nearest road: ${road.name || road.road_class} (${road.distance_m.toFixed(1)} m)`,
    );
  } catch (err) {
    setStatus("error: " + err.message);
  }
}

// --- business search ---
async function searchBusinesses(state, query, limit) {
  const resultsEl = document.getElementById("searchResults");
  resultsEl.innerHTML = "";
  setStatus(`searching "${query}" in ${state}...`);
  // Preferred path: the {STATE}.business_name_index.ptiles sidecar (fast,
  // one-bucket-block fetch). Falls back to brute-force scanning
  // {STATE}.business.ptiles's blocks if the sidecar isn't present.
  try {
    const idx = new PtilesRemoteFile(
      wasmMod,
      stateLayerUrl(state, "business_name_index"),
    );
    await idx.open();
    const key = wasmMod.key_for_business_name_query(query);
    const bucketHex = key.toString(16);
    const raw = await idx.blockForCell(bucketHex);
    const hits = raw
      ? wasmMod.match_business_name_block(raw, query, limit)
      : [];
    renderSearchResults(hits);
    setStatus(`${hits.length} result(s) (indexed)`);
    return;
  } catch (e) {
    setLayerStatus(
      `name index unavailable (${e.message}), falling back to brute force`,
    );
  }

  // Brute-force fallback: open business.ptiles, scan every block's
  // records (via decode_business_versioned) for a substring match. Slow over the
  // network -- see docs/INTEGRATION.md's pitfalls section.
  try {
    const biz = new PtilesRemoteFile(wasmMod, stateLayerUrl(state, "business"));
    await biz.open();
    const entries = wasmMod.parse_index_entries(biz.indexBytes);
    const queryLower = query.toLowerCase();
    const hits = [];
    for (const entry of entries) {
      if (hits.length >= limit) break;
      const cellHex = entry.h3_cell.toString(16);
      let raw;
      try {
        raw = await biz.blockForCell(cellHex);
      } catch (e) {
        continue;
      }
      if (!raw) continue;
      // Versioned, with the cell: the published business layer is v4, whose
      // records have no length prefix and whose coordinates are i16 offsets from
      // the cell centre. The sniffing decoder read it as v3 and produced garbage.
      const records = wasmMod.decode_business_versioned(
        raw, biz.header.version, cellHex,
      );
      for (const r of records) {
        if (r.name.toLowerCase().includes(queryLower)) hits.push(r);
      }
    }
    renderSearchResults(hits.slice(0, limit));
    setStatus(`${hits.length} result(s) (brute force)`);
  } catch (e) {
    setStatus("search failed: " + e.message);
  }
}

function renderSearchResults(hits) {
  const resultsEl = document.getElementById("searchResults");
  resultsEl.innerHTML = "";
  for (const hit of hits) {
    const div = document.createElement("div");
    div.className = "search-result";
    div.textContent = `${hit.name} (${hit.lat.toFixed(4)}, ${hit.lon.toFixed(4)})`;
    div.addEventListener("click", () => {
      map.setView([hit.lat, hit.lon], 16);
      if (clickMarker) map.removeLayer(clickMarker);
      clickMarker = L.circleMarker([hit.lat, hit.lon], {
        radius: 6,
        color: "#d29922",
        fillColor: "#d29922",
        fillOpacity: 0.9,
      }).addTo(map);
    });
    resultsEl.appendChild(div);
  }
}

// --- map setup ---
let map;

function resetLayersForStateChange() {
  Object.values(layers).forEach((l) => l.reset());
}

async function main() {
  setStatus("loading wasm...");
  wasmMod = await init().then(() => wasm);
  setStatus("ready");

  map = L.map("map").setView(
    STATE_CENTERS[currentState].center,
    STATE_CENTERS[currentState].zoom,
  );
  L.tileLayer("https://tile.openstreetmap.org/{z}/{x}/{y}.png", {
    attribution: '&copy; <a href="https://openstreetmap.org/copyright">OSM</a>',
    maxZoom: 19,
  }).addTo(map);

  Object.values(layers).forEach((l) => l.group.addTo(map));

  map.on("moveend zoomend", scheduleRender);
  map.on("click", onMapClick);

  const stateSelect = document.getElementById("stateSelect");
  Object.keys(STATE_CENTERS)
    .sort()
    .forEach((s) => {
      const opt = document.createElement("option");
      opt.value = s;
      opt.textContent = s;
      if (s === currentState) opt.selected = true;
      stateSelect.appendChild(opt);
    });
  stateSelect.addEventListener("change", () => {
    currentState = stateSelect.value;
    resetLayersForStateChange();
    const sv = STATE_CENTERS[currentState];
    map.setView(sv.center, sv.zoom);
    scheduleRender();
  });

  document.querySelectorAll(".layer-toggle").forEach((chk) => {
    chk.addEventListener("change", () => {
      layers[chk.dataset.layer].enabled = chk.checked;
      scheduleRender();
    });
  });

  document.getElementById("btnSearch").addEventListener("click", () => {
    const q = document.getElementById("searchInput").value.trim();
    if (q) searchBusinesses(currentState, q, 25);
  });
  document.getElementById("searchInput").addEventListener("keydown", (e) => {
    if (e.key === "Enter") document.getElementById("btnSearch").click();
  });
  document.getElementById("btnCloseInfo").addEventListener("click", () => {
    document.getElementById("infoPanel").classList.remove("show");
  });

  scheduleRender();
}

main().catch((e) => setStatus("fatal: " + e.message));
