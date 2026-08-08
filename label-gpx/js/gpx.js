// GPX in, labeled GPX out. The only module in this page that touches XML.
//
// Reading is `DOMParser`, writing is `XMLSerializer`, and both choices are the
// point of the module rather than an implementation detail:
//
// - The interesting input format (SCHEMA.md, the rook flavor) comes from an
//   Android app that is still changing, so the reader has to be lenient.
//   `DOMParser` gives that by construction: every lookup returns null when the
//   element is missing, unknown elements are simply never asked for, CDATA and
//   entities are already decoded, and namespaces are resolved. A hand-rolled
//   parser would have to earn each of those.
// - Writing through the DOM means the browser owns escaping. `&` in a business
//   name, `?a=1&b=2` in a URL, quotes inside an attribute: all correct, in both
//   attribute and text position, which a `.replace()` chain gets wrong at
//   exactly the edges that appear in real OSM data.
//
// Export works by cloning the input document and restructuring its tracks, so
// anything this page does not understand -- <wpt>, <ele>, <desc>, unknown
// extensions, the <?xml-stylesheet?> instruction -- survives untouched. It was
// never parsed into a lossy intermediate, so it cannot be lost.

export const GPX_NS = "http://www.topografix.com/GPX/1/1";
export const ROOK_NS = "https://rookery.local/gpx/1";

/** The five MovementType names, from motion/src/movement.rs. */
export const LABELS = ["unknown", "stationary", "walking", "running", "driving"];

/**
 * First descendant of `el` whose *local* name matches, ignoring namespace.
 *
 * SCHEMA.md reading rule 3: `<speed>`, `<rook:speed>` and `<gpxtpx:speed>` are
 * the same field. `querySelector("speed")` would miss the prefixed ones (CSS
 * selectors treat `rook:speed` as a namespace selector), so match by localName.
 */
function child(el, name) {
  if (!el) return null;
  for (const n of el.getElementsByTagName("*")) {
    if (n.localName === name) return n;
  }
  return null;
}

/** All descendants with a given local name. */
function children(el, name) {
  if (!el) return [];
  return [...el.getElementsByTagName("*")].filter((n) => n.localName === name);
}

/** Trimmed text of a named child, or undefined. Empty text reads as absent. */
function text(el, name) {
  const n = child(el, name);
  const v = n && n.textContent.trim();
  return v ? v : undefined;
}

/**
 * Numeric text of a named child, or undefined.
 *
 * SCHEMA.md rule 5: absent is never zero. A missing, empty or unparseable
 * field returns undefined, which is what `MovementTracker.push` wants for "the
 * platform did not report this" -- passing 0 instead would claim a perfect fix
 * or a perfectly still phone.
 */
function num(el, name) {
  const v = text(el, name);
  if (v === undefined) return undefined;
  const f = parseFloat(v);
  return Number.isFinite(f) ? f : undefined;
}

/**
 * Parse a GPX document.
 *
 * Returns `{doc, points, tracks, flavour, dropped}` where `points` is one flat
 * array across every `<trk>`/`<trkseg>` in file order -- segments are decided
 * by the classifier here, not by how the recorder happened to split the file --
 * and `tracks` records the original structure and any per-track rook context so
 * export can preserve what it did not regenerate.
 *
 * Throws on malformed XML. Points without a `<time>` are dropped and counted:
 * motion classification is temporal, so a point that cannot be placed in the
 * sequence is unusable.
 */
export function parseGpx(xmlText) {
  const doc = new DOMParser().parseFromString(xmlText, "application/xml");
  const err = doc.querySelector("parsererror");
  if (err) throw new Error(err.textContent.trim().split("\n")[0]);
  if (!doc.documentElement || doc.documentElement.localName !== "gpx") {
    throw new Error("not a GPX document (root element is not <gpx>)");
  }

  const points = [];
  const tracks = [];
  let dropped = 0;
  let sawSensors = false;

  for (const trk of children(doc.documentElement, "trk")) {
    const track = {
      name: text(trk, "name"),
      // Kept so export can carry forward context this page did not resolve.
      context: trackBlock(trk, "context"),
      segment: trackBlock(trk, "segment"),
      firstPoint: points.length,
      pointCount: 0,
    };
    for (const seg of children(trk, "trkseg")) {
      for (const pt of children(seg, "trkpt")) {
        const lat = parseFloat(pt.getAttribute("lat"));
        const lon = parseFloat(pt.getAttribute("lon"));
        const t = text(pt, "time");
        const t_ms = t ? Date.parse(t) : NaN;
        if (!Number.isFinite(lat) || !Number.isFinite(lon) || !Number.isFinite(t_ms)) {
          dropped++;
          continue;
        }
        const ext = child(pt, "extensions");
        const speed = num(ext, "speed");
        const accuracy = num(ext, "accuracy");
        const accel = readAccel(ext);
        if (speed !== undefined || accuracy !== undefined || accel) sawSensors = true;
        points.push({
          lat,
          lon,
          t_ms,
          ele: num(pt, "ele"),
          speed,
          accuracy,
          accel,
          // The source element, so export can copy anything unrecognised.
          el: pt,
        });
        track.pointCount++;
      }
    }
    tracks.push(track);
  }

  return {
    doc,
    points,
    tracks,
    dropped,
    // Which input flavour this is, per SCHEMA.md. Only affects whether the page
    // offers to re-resolve context: a rook file already has field-captured
    // truth and must not be silently overwritten with a newer map snapshot.
    flavour: sawSensors || tracks.some((t) => t.context) ? "rook" : "plain",
  };
}

/**
 * A track-level `<rook:*>` block, wherever the producer put it.
 *
 * The Android app writes `<rook:context>` inside the `<trkseg>`'s
 * `<extensions>`, while SCHEMA.md's example shows it in the `<trk>`'s. Both are
 * accepted (reading rule 1: the format is still moving) -- the only thing that
 * actually disqualifies a match is sitting inside a `<trkpt>`, where it would be
 * that point's data rather than the track's.
 */
function trackBlock(trk, localName) {
  for (const n of trk.getElementsByTagName("*")) {
    if (n.localName !== localName) continue;
    let p = n.parentElement;
    let inPoint = false;
    while (p && p !== trk) {
      if (p.localName === "trkpt") {
        inPoint = true;
        break;
      }
      p = p.parentElement;
    }
    if (!inPoint) return n;
  }
  return null;
}

/**
 * The accel fields, as the `AccelStats` shape `MovementTracker.push` wants, or
 * null when the point carries no accelerometer data at all.
 *
 * `mean_magnitude` and `window_duration_s` are the two fields the rook format
 * does not carry (SCHEMA.md "The accel gap"). They are sent as 0 because the
 * wasm boundary needs all five numbers -- which is *also* `AccelStats::EMPTY`,
 * so a partial reading is indistinguishable from no reading in those two
 * fields. Nothing in the current accel table reads either, so this is latent,
 * not active; it becomes real the day a rule uses mean magnitude.
 */
function readAccel(ext) {
  if (!ext) return null;
  const variance = num(ext, "accel_variance");
  const freq = num(ext, "accel_freq");
  const steps = num(ext, "accel_steps");
  if (variance === undefined && freq === undefined && steps === undefined) return null;
  return {
    variance: variance ?? 0,
    dominant_frequency: freq ?? 0,
    step_count: steps ?? 0,
    mean_magnitude: num(ext, "accel_mean") ?? 0,
    window_duration_s: num(ext, "accel_window_s") ?? 0,
  };
}

/**
 * Serialize labeled segments back to GPX.
 *
 * `parsed` is a `parseGpx` result, `segments` is the labeled list from
 * segments.js, `provenance` is `{snapshot, derived, synthetic, samples}`.
 *
 * One `<trk>` per segment, `<name>` = the label, so a plain GPX viewer shows
 * the labels without knowing anything about this format.
 */
export function writeGpx(parsed, segments, provenance = {}) {
  const doc = parsed.doc.cloneNode(true);
  const root = doc.documentElement;
  root.setAttribute("xmlns:rook", ROOK_NS);
  root.setAttribute("creator", "label-gpx");

  const el = (name) => doc.createElementNS(GPX_NS, name);
  const rook = (name) => doc.createElementNS(ROOK_NS, "rook:" + name);

  // Provenance, once, in <metadata>. Says which fields are computed and which
  // are invented, so a consumer can refuse to validate a classifier against
  // numbers that were generated from the answer.
  let meta = [...root.children].find((n) => n.localName === "metadata");
  if (!meta) {
    meta = el("metadata");
    root.insertBefore(meta, root.firstChild);
  }
  for (const old of children(meta, "provenance")) old.remove();
  const prov = rook("provenance");
  prov.setAttribute("tool", "label-gpx");
  prov.setAttribute("version", "1");
  if (provenance.snapshot) prov.setAttribute("snapshot", provenance.snapshot);
  prov.setAttribute("derived", provenance.derived ?? "");
  prov.setAttribute("synthetic", provenance.synthetic ?? "");
  if (provenance.samples !== undefined) {
    prov.setAttribute("context_samples_per_segment", String(provenance.samples));
  }
  meta.appendChild(prov);

  // Replace the track structure wholesale: the labels are the new structure.
  for (const trk of children(root, "trk")) trk.remove();

  for (const s of segments) {
    const pts = parsed.points.slice(s.start, s.end + 1);
    if (!pts.length) continue;
    const trk = el("trk");
    const name = el("name");
    name.textContent = s.type;
    trk.appendChild(name);

    const ext = el("extensions");
    const seg = rook("segment");
    seg.setAttribute("source", s.edited ? "human" : "auto");
    seg.setAttribute("edited", s.edited ? "true" : "false");
    if (s.confidence !== undefined) {
      seg.setAttribute("confidence", s.confidence.toFixed(2));
    }
    seg.setAttribute("start_time", new Date(pts[0].t_ms).toISOString());
    seg.setAttribute("end_time", new Date(pts[pts.length - 1].t_ms).toISOString());
    ext.appendChild(seg);
    const ctx = contextNode(doc, rook, s);
    if (ctx) ext.appendChild(ctx);
    trk.appendChild(ext);

    const trkseg = el("trkseg");
    for (const p of pts) trkseg.appendChild(pointNode(doc, el, p, s));
    trk.appendChild(trkseg);
    root.appendChild(trk);
  }

  const xml = new XMLSerializer().serializeToString(doc);
  return xml.startsWith("<?xml") ? xml : '<?xml version="1.0" encoding="UTF-8"?>\n' + xml;
}

/**
 * One `<trkpt>`, preserving what the input had and marking what we computed.
 *
 * A value read from the file is written back verbatim with no `derived`
 * attribute; a speed this page derived from positions carries
 * `derived="true"`. Unknown values are omitted entirely rather than written as
 * 0 (SCHEMA.md "Absent vs zero").
 */
function pointNode(doc, el, p, seg) {
  const pt = el("trkpt");
  pt.setAttribute("lat", String(p.lat));
  pt.setAttribute("lon", String(p.lon));
  if (p.ele !== undefined) {
    const e = el("ele");
    e.textContent = String(p.ele);
    pt.appendChild(e);
  }
  const t = el("time");
  t.textContent = new Date(p.t_ms).toISOString();
  pt.appendChild(t);

  const ext = el("extensions");
  let any = false;
  const add = (name, value, derived) => {
    if (value === undefined || value === null || !Number.isFinite(value)) return;
    const n = el(name);
    n.textContent = trimNum(value);
    if (derived) n.setAttribute("derived", "true");
    ext.appendChild(n);
    any = true;
  };
  // Reported speed wins; otherwise the speed this page derived, flagged.
  if (p.speed !== undefined) add("speed", p.speed, false);
  else add("speed", p.derivedSpeed, true);
  add("accuracy", p.accuracy, false);
  if (p.accel) {
    add("accel_variance", p.accel.variance, false);
    add("accel_freq", p.accel.dominant_frequency, false);
    add("accel_steps", p.accel.step_count, false);
    if (p.accel.mean_magnitude) add("accel_mean", p.accel.mean_magnitude, false);
    if (p.accel.window_duration_s) add("accel_window_s", p.accel.window_duration_s, false);
  }
  if (seg && seg.vote && p.index === seg.start) {
    // The classifier's own read of this point, kept once per segment so a
    // fixture records what it believed at label time.
    const n = el("classified");
    n.textContent = seg.vote;
    ext.appendChild(n);
    any = true;
  }
  if (any) pt.appendChild(ext);
  return pt;
}

/** Up to 6 decimals, without trailing zeros. */
function trimNum(v) {
  return String(Math.round(v * 1e6) / 1e6);
}

/**
 * The `<rook:context>` block for a segment.
 *
 * A context read from the input file is passed through unchanged (imported into
 * the new document) -- SCHEMA.md: a 2012 trace's field-captured context must
 * not be silently replaced by what a 2026 map says. Only a context this page
 * resolved is generated here.
 */
function contextNode(doc, rook, s) {
  if (s.sourceContext) return doc.importNode(s.sourceContext, true);
  const c = s.context;
  if (!c) return null;
  const ctx = rook("context");
  if (c.lat !== undefined) ctx.setAttribute("lat", String(c.lat));
  if (c.lon !== undefined) ctx.setAttribute("lon", String(c.lon));
  ctx.setAttribute("resolved", new Date(c.resolved ?? Date.now()).toISOString());
  if (c.snapshot) ctx.setAttribute("snapshot", c.snapshot);

  const leaf = (parent, name, value) => {
    if (value === undefined || value === null || value === "") return;
    const n = doc.createElementNS(GPX_NS, name);
    n.textContent = String(value);
    parent.appendChild(n);
  };

  if (c.admin) {
    const a = rook("admin");
    leaf(a, "country", c.admin.country);
    leaf(a, "state", c.admin.state);
    leaf(a, "county", c.admin.county);
    leaf(a, "zip", c.admin.zip);
    leaf(a, "timezone", c.admin.timezone);
    ctx.appendChild(a);
  }
  if (c.road) {
    const r = rook("road");
    // osm_id can exceed 2^53 on some layers, so it is carried as a string.
    leaf(r, "osm_id", c.road.osm_id);
    leaf(r, "name", c.road.name);
    leaf(r, "class", c.road.road_class);
    leaf(r, "distance_m", round1(c.road.distance_m));
    ctx.appendChild(r);
  }
  if (c.intersection) {
    const i = rook("intersection");
    leaf(i, "lat", c.intersection.lat);
    leaf(i, "lon", c.intersection.lon);
    leaf(i, "distance_m", round1(c.intersection.distance_m));
    leaf(i, "type", INTERSECTION_TYPES[c.intersection.intersection_type] ?? "junction");
    ctx.appendChild(i);
  }
  return ctx.children.length ? ctx : null;
}

/** Numeric `intersection_type` -> the name SCHEMA.md specifies. */
export const INTERSECTION_TYPES = {
  1: "signals",
  2: "stop",
  3: "give_way",
  4: "roundabout",
  0: "junction",
};

function round1(v) {
  return v === undefined || v === null ? undefined : Math.round(v * 10) / 10;
}
