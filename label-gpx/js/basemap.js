// Two basemaps, one switch: OSM raster tiles, or the ptiles layers drawn
// directly from the same files the classifier reads.
//
// The vector one is not a novelty. When a segment's label hinges on "was I on the
// footway or in the traffic lane", the honest backdrop is the geometry that
// produced the road context -- not a raster tile rendered from a different
// snapshot of OSM at a different time. Switching between them is also the fastest
// way to see that the tiles and the layer disagree, which is a real thing that
// happens and which a raster-only map hides.
//
// Style rule for both modes: the basemap is chrome, so it stays achromatic and
// desaturated. Saturated colour on this page means one thing only -- a movement
// label. A basemap that competes with the data is a basemap that has to be turned
// off to work.

const OSM_URL = "https://tile.openstreetmap.org/{z}/{x}/{y}.png";

/** Below this zoom the viewport needs more than the 512-cell cap. */
const MIN_VECTOR_ZOOM = 11;
/** Buildings are only legible (and only affordable) close in. */
const BUILDING_ZOOM = 15;
/**
 * How far off a trace still counts as "the trace is in that cell".
 *
 * A res-7 cell is ~1.2 km across, so a trace running along a boundary belongs to
 * both cells it hugs -- and drawing only the cells its *points* land in leaves
 * the road it is walking on unpainted on one side. Probing four offsets per
 * point at this distance picks up exactly the clipped neighbours, where taking
 * every point's whole ring-1 would triple the download to catch the same few.
 */
const CLIP_BUFFER_M = 180;

// Desaturated so the trace and its labels stay the only saturated things.
const WATER = "#243b4a";
const WATER_LINE = "#2d4a5c";
const PARK = "#25352a";
const PARK_EDGE = "#31462f";
const BUILDING = "#20242a";
const BUILDING_EDGE = "#2b3037";
const RAIL = "#3d3a44";
const TRAIL = "#3f4a3c";

/**
 * Road weight/colour by OSM class. Motorways read heaviest, service roads
 * thinnest -- the same hierarchy a paper map uses, so the eye can find an
 * arterial without reading labels.
 */
const ROAD_STYLE = {
  motorway: { color: "#4a505a", weight: 3.2 },
  trunk: { color: "#474d56", weight: 2.8 },
  primary: { color: "#434952", weight: 2.4 },
  secondary: { color: "#3f444d", weight: 2.0 },
  tertiary: { color: "#3b4048", weight: 1.7 },
  residential: { color: "#363b43", weight: 1.4 },
  unclassified: { color: "#363b43", weight: 1.3 },
  service: { color: "#31353c", weight: 1.0 },
  living_street: { color: "#31353c", weight: 1.1 },
  pedestrian: { color: "#3a4a44", weight: 1.2 },
  footway: { color: "#3a4a44", weight: 0.9 },
  path: { color: "#3a4a44", weight: 0.9 },
  steps: { color: "#42413a", weight: 0.9 },
  cycleway: { color: "#3a4448", weight: 1.0 },
  track: { color: "#34322c", weight: 0.9 },
};
const ROAD_FALLBACK = { color: "#33373e", weight: 1.2 };

/**
 * Build the basemap controller.
 *
 * `ctx` supplies `stateAt` and `stateUrl` from context.js, so the filename and
 * state-resolution rules live in exactly one place.
 */
export function createBasemap(map, P, wasm, ctx, { onStatus = () => {} } = {}) {
  // `corpSafe` fetches each tile with fetch() and hands Leaflet a blob URL.
  // Required wherever the page is served with COEP: require-corp (steele.red is),
  // because a plain cross-origin tile <img> is blocked with
  // ERR_BLOCKED_BY_RESPONSE.NotSameOriginAfterDefaultedToSameOriginByCoep --
  // which looks exactly like "the map is broken": the img elements are all there
  // and every one of them paints nothing. Falls back to the plain layer if the
  // shim did not load, which is fine on an origin without COEP.
  const tileLayer = (L.tileLayer.corpSafe ?? L.tileLayer).bind(L.tileLayer);
  const raster = tileLayer(OSM_URL, {
    maxZoom: 19,
    attribution:
      '&copy; <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a>',
  });
  // Panes so the basemap can never paint over the trace, whatever order things
  // get added in.
  map.createPane("ptilesFill").style.zIndex = 210;
  map.createPane("ptilesLine").style.zIndex = 220;
  const fill = L.layerGroup([], { pane: "ptilesFill" });
  const line = L.layerGroup([], { pane: "ptilesLine" });

  const layers = new Map(); // "ST/layer" -> Promise<Layer|null>
  const drawn = new Set(); // cellHex/layer already painted
  let mode = "osm";
  let pending = null;
  let generation = 0;
  /**
   * The cells the loaded trace occupies, or null for "whatever is on screen".
   *
   * This is the whole space argument. A 90 km drive crosses ~50 cells; the
   * viewport at a working zoom asks for a few hundred, nearly all of them
   * nowhere near the trace. Restricting to the trace's own cells (plus the ones
   * it clips) is a 5-10x cut in blocks fetched and decompressed, for a backdrop
   * that covers everything you can actually label against.
   */
  let traceCells = null;

  function layerFor(state, name) {
    const key = `${state}/${name}`;
    if (!layers.has(key)) {
      layers.set(
        key,
        P.open(P.httpSource(ctx.stateUrl(state, name))).catch(() => null),
      );
    }
    return layers.get(key);
  }

  raster.addTo(map);

  /**
   * Restrict drawing to the cells a trace occupies. Pure wasm, no I/O: it is
   * `cell_for_coord` per point plus four offset probes, all local.
   *
   * Pass `null` to go back to drawing whatever the viewport covers (which is
   * what happens before a file is open).
   */
  function setTrace(points) {
    traceCells = null;
    drawn.clear();
    fill.clearLayers();
    line.clearLayers();
    if (!points || !points.length) return 0;
    const cells = new Set();
    // Metres per degree at this latitude; good enough for a boundary probe.
    const midLat = points[Math.floor(points.length / 2)].lat;
    const dLat = CLIP_BUFFER_M / 111_320;
    const dLon = CLIP_BUFFER_M / (111_320 * Math.cos((midLat * Math.PI) / 180) || 1);
    for (const p of points) {
      cells.add(wasm.cell_for_coord(p.lat, p.lon));
      cells.add(wasm.cell_for_coord(p.lat + dLat, p.lon));
      cells.add(wasm.cell_for_coord(p.lat - dLat, p.lon));
      cells.add(wasm.cell_for_coord(p.lat, p.lon + dLon));
      cells.add(wasm.cell_for_coord(p.lat, p.lon - dLon));
    }
    traceCells = cells;
    if (mode === "ptiles") refresh();
    return cells.size;
  }

  /** OSM raster or ptiles vector. Returns the mode actually in effect. */
  function setMode(next) {
    if (next === mode) return mode;
    mode = next;
    if (mode === "osm") {
      raster.addTo(map);
      map.removeLayer(fill);
      map.removeLayer(line);
      onStatus("");
    } else {
      map.removeLayer(raster);
      fill.addTo(map);
      line.addTo(map);
      refresh();
    }
    return mode;
  }

  function currentMode() {
    return mode;
  }

  /**
   * Draw (or top up) the vector basemap for the current viewport.
   *
   * Debounced by the caller on `moveend`. Cells already painted are skipped, so
   * panning costs only the new cells; `P.stats` is the honest counter for what
   * that actually spent.
   */
  function refresh() {
    if (mode !== "ptiles") return;
    let cells;
    if (traceCells) {
      // Only what the trace touches, and only what is near the current view out
      // of that -- panning away from the trace should fetch nothing, because
      // there is nothing there to label.
      //
      // Tested against the cell *centre* but with the bounds grown by a cell
      // radius: a res-7 cell is ~1.2 km across, so a cell can cover most of the
      // screen while its centre sits outside it. Filtering on plain containment
      // drew nothing at all for a 1 km trace, which is the common case here.
      const CELL_RADIUS_DEG = 0.01;
      const v = map.getBounds().pad(0.15);
      const b = L.latLngBounds(
        [v.getSouth() - CELL_RADIUS_DEG, v.getWest() - CELL_RADIUS_DEG],
        [v.getNorth() + CELL_RADIUS_DEG, v.getEast() + CELL_RADIUS_DEG],
      );
      cells = [...traceCells].filter((hex) => {
        const [lat, lon] = wasm.cell_center(hex);
        return b.contains([lat, lon]);
      });
      if (!cells.length) {
        onStatus(`drawing the trace's ${traceCells.size} cells — pan back to the trace`);
        return;
      }
    } else {
      if (map.getZoom() < MIN_VECTOR_ZOOM) {
        onStatus(`zoom to ${MIN_VECTOR_ZOOM}+ to draw ptiles layers, or open a trace`);
        return;
      }
      const b = map.getBounds();
      try {
        cells = wasm.cells_for_bounds(b.getSouth(), b.getWest(), b.getNorth(), b.getEast());
      } catch {
        // The 512-cell cap: "you are looking at more than a metro area", which
        // zooming fixes. Not worth an error banner.
        onStatus("viewport too large for the cell cap — zoom in");
        return;
      }
    }
    const gen = ++generation;
    pending = draw(cells, gen).catch((e) => onStatus(`basemap: ${e.message}`));
    return pending;
  }

  async function draw(cells, gen) {
    const zoom = map.getZoom();
    // Everything the client can decode for a backdrop, drawn bottom-up. Water and
    // parks are fills, rail and roads are lines, buildings are the close-in
    // detail -- ptile-client ships decoders for all of them, so there is no
    // reason for the basemap to know about only some.
    const wanted = [
      ["water", drawWater],
      ["parks", drawParks],
      ["rail", drawRail],
      ["trails", drawTrails],
      ["roads", drawRoads],
      ...(zoom >= BUILDING_ZOOM ? [["buildings", drawBuildings]] : []),
    ];

    // Group cells by state once: a viewport straddling a border needs two files
    // per layer, and one file open costs three requests.
    const byState = new Map();
    for (const hex of cells) {
      const [lat, lon] = wasm.cell_center(hex);
      const st = ctx.stateAt(lat, lon);
      if (!st) continue;
      if (!byState.has(st)) byState.set(st, []);
      byState.get(st).push(hex);
    }

    let painted = 0;
    for (const [state, hexes] of byState) {
      for (const [name, painter] of wanted) {
        const layer = await layerFor(state, name);
        if (gen !== generation) return; // the view moved on; drop this pass
        if (!layer) {
          onStatus(`${state}.${name} unavailable in this snapshot`);
          continue;
        }
        const todo = hexes.filter((h) => !drawn.has(`${h}/${name}`));
        if (!todo.length) continue;
        const ids = todo.map((h) => BigInt("0x" + h));
        await layer.prefetch(ids);
        if (gen !== generation) return;
        for (let i = 0; i < todo.length; i++) {
          const bytes = await layer.cellRecords(ids[i]).catch(() => null);
          if (gen !== generation) return;
          drawn.add(`${todo[i]}/${name}`);
          if (!bytes) continue;
          painted += painter(bytes, ids[i]);
        }
      }
    }
    const s = P.stats;
    onStatus(
      `${painted} features · ${s.requests} requests · ${(s.bytes / 1e6).toFixed(1)} MB`,
    );
  }

  // `coords` are [lon, lat] everywhere in the format; Leaflet wants [lat, lon].
  const toLatLngs = (coords) => coords.map((c) => [c[1], c[0]]);

  function drawWater(bytes) {
    let n = 0;
    for (const f of wasm.decode_water(bytes)) {
      if (!f.coords || f.coords.length < 2) continue;
      const latlngs = toLatLngs(f.coords);
      // geom_type 0 = polygon, 1 = linestring, 2 = a reference to geometry that
      // lives in another block: nothing to draw for that one.
      if (f.geom_type === 0 && f.coords.length >= 3) {
        L.polygon(latlngs, {
          pane: "ptilesFill", color: WATER_LINE, weight: 0.6,
          fillColor: WATER, fillOpacity: 0.9, interactive: false,
        }).addTo(fill);
      } else if (f.geom_type === 1) {
        L.polyline(latlngs, {
          pane: "ptilesFill", color: WATER_LINE,
          weight: Math.min(3, 1 + (f.width ?? 0) / 12), interactive: false,
        }).addTo(fill);
      } else {
        continue;
      }
      n++;
    }
    return n;
  }

  function drawParks(bytes) {
    let n = 0;
    for (const f of wasm.decode_parks(bytes)) {
      if (!f.coords || f.coords.length < 3) continue;
      L.polygon(toLatLngs(f.coords), {
        pane: "ptilesFill", color: PARK_EDGE, weight: 0.6,
        fillColor: PARK, fillOpacity: 0.85, interactive: false,
      }).addTo(fill);
      n++;
    }
    return n;
  }

  function drawRoads(bytes) {
    let n = 0;
    for (const r of wasm.decode_roads(bytes)) {
      if (!r.coords || r.coords.length < 2) continue;
      const style = ROAD_STYLE[r.road_class] ?? ROAD_FALLBACK;
      L.polyline(toLatLngs(r.coords), {
        pane: "ptilesLine", ...style, opacity: 0.95, interactive: false,
      }).addTo(line);
      n++;
    }
    return n;
  }

  function drawRail(bytes) {
    let n = 0;
    for (const f of wasm.decode_rail(bytes)) {
      if (!f.coords || f.coords.length < 2) continue;
      L.polyline(toLatLngs(f.coords), {
        pane: "ptilesLine", color: RAIL, weight: 1.4, dashArray: "5 4",
        opacity: 0.85, interactive: false,
      }).addTo(line);
      n++;
    }
    return n;
  }

  function drawTrails(bytes) {
    let n = 0;
    for (const f of wasm.decode_trails(bytes)) {
      if (!f.coords || f.coords.length < 2) continue;
      // Trails matter more than anything else here: five of the six committed
      // fixtures are foot routes, and a walk that follows a trail is the label
      // you most often need to confirm by eye.
      L.polyline(toLatLngs(f.coords), {
        pane: "ptilesLine", color: TRAIL, weight: 1.1, dashArray: "3 3",
        opacity: 0.9, interactive: false,
      }).addTo(line);
      n++;
    }
    return n;
  }

  function drawBuildings(bytes, cellId) {
    // v9 building coordinates are deltas from the cell centre, so the decoder
    // needs the centre -- getting this wrong yields plausible geometry in the
    // wrong place rather than an error.
    const [clat, clon] = wasm.cell_center(cellId.toString(16));
    let n = 0;
    for (const b of wasm.decode_buildings(bytes, clat, clon)) {
      const coords = b.coords || b.coordinates;
      if (!coords || coords.length < 3) continue;
      L.polygon(toLatLngs(coords), {
        pane: "ptilesFill", color: BUILDING_EDGE, weight: 0.5,
        fillColor: BUILDING, fillOpacity: 0.9, interactive: false,
      }).addTo(fill);
      n++;
    }
    return n;
  }

  /** Forget what has been painted, e.g. after a snapshot change. */
  function clear() {
    fill.clearLayers();
    line.clearLayers();
    drawn.clear();
    generation++;
  }

  return { setMode, mode: currentMode, refresh, clear, setTrace, MIN_VECTOR_ZOOM };
}
