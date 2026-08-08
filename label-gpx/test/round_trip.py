#!/usr/bin/env python3
"""Does the page load, classify, and round-trip a labeled GPX?

`js/segments.js` is covered by `node --test` because it is deliberately DOM-free.
`js/gpx.js` cannot be: it is `DOMParser`/`XMLSerializer`, which node does not
provide, and swapping in a shim would test the shim. So it is checked here, in a
real browser, against the real fixtures -- which also exercises the parts that
only exist in a browser: the wasm module init, the file input, the Leaflet map
and app.js's wiring.

    python3 label-gpx/test/round_trip.py            # headless
    python3 label-gpx/test/round_trip.py --headed   # watch it

Needs `playwright` and a chromium install (`playwright install chromium`), the
same dependency web-demo/test/render_check.py already has. Exits non-zero on the
first failure with the assertion printed.
"""

import argparse
import http.server
import json
import socketserver
import sys
import threading
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
# A plain OSM trace: positions and timestamps, nothing else.
PLAIN = "test-fixtures/gpx/nc-sals-branch-1191748.gpx"

# A rook-flavour trace, inline rather than a fixture: it exists to carry the
# awkward cases (an ampersand in a name, a URL with a query string, every sensor
# field, a per-track context block) and keeping it next to the assertions makes
# it obvious what is being claimed. Mirrors SCHEMA.md.
ROOK_GPX = """<?xml version="1.0" encoding="UTF-8"?>
<gpx version="1.1" creator="Rook"
     xmlns="http://www.topografix.com/GPX/1/1"
     xmlns:rook="https://rookery.local/gpx/1">
<trk><name>stationary</name><trkseg>
<trkpt lat="35.9606" lon="-83.9207"><time>2026-08-08T09:00:00Z</time><extensions>
  <speed>0.1</speed><accuracy>8.0</accuracy>
  <accel_variance>0.02</accel_variance><accel_freq>0.3</accel_freq><accel_steps>0</accel_steps>
</extensions></trkpt>
<trkpt lat="35.9606" lon="-83.9207"><time>2026-08-08T09:00:07Z</time><extensions>
  <speed>0.2</speed><accuracy>9.0</accuracy>
  <accel_variance>0.03</accel_variance><accel_freq>0.4</accel_freq><accel_steps>0</accel_steps>
</extensions></trkpt>
<trkpt lat="35.9607" lon="-83.9207"><time>2026-08-08T09:00:14Z</time></trkpt>
<extensions><rook:context lat="35.9606" lon="-83.9207" resolved="2026-08-08T09:00:02Z">
  <rook:admin><country>US</country><state>Tennessee</state><county>Knox</county></rook:admin>
  <rook:road><osm_id>42</osm_id><name>Gay St</name><class>residential</class>
             <distance_m>2.4</distance_m></rook:road>
  <rook:businesses>
    <rook:business><name>Bob &amp; Sons</name>
                   <website>https://example.com/?a=1&amp;b=2</website></rook:business>
  </rook:businesses>
</rook:context></extensions>
</trkseg></trk>
</gpx>
"""

CASES = """
// Runs in the page. Returns a list of [name, ok, detail].
async (rookXml) => {
  const out = [];
  const ok = (name, cond, detail = "") => out.push([name, !!cond, String(detail)]);
  const gpx = await import("./js/gpx.js");
  const seg = await import("./js/segments.js");
  // Same URL app.js imported, so this is the already-initialised module
  // instance rather than a second copy with an uninstantiated wasm binary.
  const wasm = await import("./lib/client/ptiles_client.js");

  // --- plain OSM flavour
  const plainText = await (await fetch("../%(plain)s")).text();
  const plain = gpx.parseGpx(plainText);
  ok("plain: point count", plain.points.length === 721, plain.points.length);
  ok("plain: flavour", plain.flavour === "plain", plain.flavour);
  ok("plain: no reported speed", plain.points[10].speed === undefined, plain.points[10].speed);
  ok("plain: has time", Number.isFinite(plain.points[10].t_ms));

  // --- rook flavour
  const rook = gpx.parseGpx(rookXml);
  ok("rook: flavour", rook.flavour === "rook", rook.flavour);
  ok("rook: points", rook.points.length === 3, rook.points.length);
  ok("rook: speed read", rook.points[0].speed === 0.1, rook.points[0].speed);
  ok("rook: accuracy read", rook.points[0].accuracy === 8.0, rook.points[0].accuracy);
  ok("rook: accel read", rook.points[0].accel &&
     rook.points[0].accel.dominant_frequency === 0.3, JSON.stringify(rook.points[0].accel));
  ok("rook: absent sensors stay absent", rook.points[2].speed === undefined &&
     rook.points[2].accel === null, JSON.stringify(rook.points[2]));
  // The accel gap: the app sends three of five fields. The missing two must be
  // absent from the object, not zero -- 0 is AccelStats::EMPTY's value, i.e.
  // "there was no accelerometer". See ANDROID_INTEGRATION.md.
  ok("rook: omitted accel fields are absent, not zero",
     !("mean_magnitude" in rook.points[0].accel) &&
     !("window_duration_s" in rook.points[0].accel),
     JSON.stringify(rook.points[0].accel));
  ok("rook: context preserved", !!rook.tracks[0].context);

  // --- classify + label + write + re-read
  const results = seg.classifyTrace(wasm, plain.points);
  let segments = seg.coalesce(plain.points, results);
  ok("segments: produced", segments.length > 0, segments.length);
  segments = seg.relabel(segments, 0, "running");
  const xml = gpx.writeGpx(plain, segments, { snapshot: "2026-08-07", derived: "speed" });
  const again = gpx.parseGpx(xml);
  ok("round trip: points survive", again.points.length === plain.points.length,
     `${plain.points.length} -> ${again.points.length}`);
  ok("round trip: label survives", again.tracks[0].name === "running", again.tracks[0].name);
  ok("round trip: human marked", /source="human"/.test(xml));
  ok("round trip: derived flagged", /<speed derived="true">/.test(xml));
  ok("round trip: provenance", /rook:provenance/.test(xml) && /derived="speed"/.test(xml));
  ok("round trip: no zero-filled accuracy", !/<accuracy>0<\\/accuracy>/.test(xml));
  ok("round trip: time reserialized", again.points[0].t_ms === plain.points[0].t_ms,
     `${plain.points[0].t_ms} vs ${again.points[0].t_ms}`);

  // --- escaping, the reason writing goes through XMLSerializer
  const rookOut = gpx.writeGpx(
    rook,
    [{ start: 0, end: 2, type: "stationary", edited: false, sourceContext: rook.tracks[0].context }],
    {},
    wasm.intersection_type_name,
  );
  // The intersection vocabulary comes from wasm, not a JS copy of the mapping.
  ok("vocabulary: wasm names the intersection type",
     wasm.intersection_type_name(1) === "traffic_signals" &&
     wasm.intersection_type_name(0) === "junction" &&
     wasm.intersection_holds_traffic(1) === true &&
     wasm.intersection_holds_traffic(4) === false,
     wasm.intersection_type_name(1));
  const named = gpx.writeGpx(
    plain,
    [{ start: 0, end: 5, type: "walking", edited: true,
       context: { snapshot: "2026-08-07", resolved: Date.now(),
                  intersection: { lat: 35.88, lon: -78.75, distance_m: 9, intersection_type: 1 } } }],
    {},
    wasm.intersection_type_name,
  );
  ok("vocabulary: the written context uses the name, not the integer",
     /<type>traffic_signals<\/type>/.test(named) && !/<type>1<\/type>/.test(named), "");
  ok("rook: export does not invent accel fields",
     !/accel_mean/.test(rookOut) && !/accel_window_s/.test(rookOut) &&
     /accel_variance/.test(rookOut), "");
  ok("escaping: ampersand escaped in output", rookOut.includes("Bob &amp; Sons"), "");
  ok("escaping: no raw bare ampersand", !/&(?!amp;|lt;|gt;|quot;|apos;|#)/.test(rookOut));
  const rookBack = gpx.parseGpx(rookOut);
  const biz = [...rookBack.doc.getElementsByTagName("*")]
    .filter((n) => n.localName === "name").map((n) => n.textContent);
  ok("escaping: name round-trips", biz.includes("Bob & Sons"), biz.join("|"));
  const site = [...rookBack.doc.getElementsByTagName("*")]
    .find((n) => n.localName === "website");
  ok("escaping: url round-trips", site && site.textContent === "https://example.com/?a=1&b=2",
     site && site.textContent);
  ok("rook: field context passed through, not regenerated",
     /rook:context/.test(rookOut) && /Gay St/.test(rookOut));

  return out;
}
""" % {"plain": PLAIN}


class CoepHandler(http.server.SimpleHTTPRequestHandler):
    """Serve with the headers production serves.

    steele.red sends COEP: require-corp / COOP: same-origin. Without them here,
    a cross-origin tile <img> loads fine locally and is blocked on the live site
    -- which is exactly how the OSM basemap shipped broken while this suite was
    green. The harness has to be as strict as the host.
    """

    def end_headers(self):
        self.send_header("Cross-Origin-Embedder-Policy", "require-corp")
        self.send_header("Cross-Origin-Opener-Policy", "same-origin")
        super().end_headers()


def serve(directory, port):
    handler = lambda *a, **kw: CoepHandler(*a, directory=str(directory), **kw)
    # allow_reuse_address, or a re-run inside the TIME_WAIT window of the last
    # one dies on "Address already in use" rather than testing anything.
    socketserver.TCPServer.allow_reuse_address = True
    httpd = socketserver.TCPServer(("127.0.0.1", port), handler)
    threading.Thread(target=httpd.serve_forever, daemon=True).start()
    return httpd


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--headed", action="store_true")
    # 0 asks the OS for a free port; a fixed one collides with whatever else is
    # already listening on this machine. Pass --port to pin it.
    ap.add_argument("--port", type=int, default=0)
    args = ap.parse_args()

    try:
        from playwright.sync_api import sync_playwright
    except ImportError:
        print("playwright not installed: pip install playwright && playwright install chromium")
        return 2

    # Serve the repo root, not label-gpx/, so the page can fetch the committed
    # GPX fixtures. `file://` would not do: Cache API is undefined on an
    # insecure origin, and module imports are blocked.
    httpd = serve(ROOT, args.port)
    url = f"http://127.0.0.1:{httpd.server_address[1]}/label-gpx/index.html"

    failures = []
    with sync_playwright() as pw:
        browser = pw.chromium.launch(headless=not args.headed)
        page = browser.new_page()
        errors = []
        page.on("pageerror", lambda e: errors.append(str(e)))
        page.on("console", lambda m: errors.append(m.text) if m.type == "error" else None)
        page.goto(url, wait_until="networkidle")

        results = page.evaluate(CASES, ROOK_GPX)
        for name, good, detail in results:
            print(f"{'ok  ' if good else 'FAIL'} {name}{'' if good else f'  ({detail})'}")
            if not good:
                failures.append(name)

        # Drive the real UI once: the file input, the classifier, the map.
        page.set_input_files("#file", str(ROOT / PLAIN))
        page.wait_for_function("window.__labelGpx && window.__labelGpx.segments.length > 0",
                              timeout=30000)
        n = page.evaluate("window.__labelGpx.segments.length")
        pts = page.evaluate("window.__labelGpx.parsed.points.length")
        rows = page.locator("#segments tbody tr").count()
        print(f"ok   ui: {pts} points -> {n} segments, {rows} table rows")
        if rows != n:
            failures.append("ui: table rows do not match segments")
        if pts != 721:
            failures.append(f"ui: expected 721 points, got {pts}")

        # --- the ribbon: the trace's whole timeline, to scale
        bands = page.locator("#ribbon .seg").count()
        if bands != n:
            failures.append(f"ui: {bands} ribbon bands for {n} segments")
        # The bands must divide the track exactly. They did not at first: CSS
        # gives each item only `flex-grow x free-space` when the growth factors
        # sum to less than 1, so duration fractions left a gap at the end.
        fill = page.evaluate(
            "() => { const r = document.getElementById('ribbon');"
            " const w = [...r.querySelectorAll('.seg')].reduce((a, e) => a + e.offsetWidth, 0);"
            " return [w, r.clientWidth]; }"
        )
        if abs(fill[0] - fill[1]) > 2:
            failures.append(f"ui: ribbon bands cover {fill[0]}px of {fill[1]}px")
        print(f"ok   ui: ribbon {bands} bands covering {fill[0]}/{fill[1]}px")

        # Clicking a band selects that segment -- the ribbon is navigation, not
        # decoration.
        # The last band, whatever the trace happens to produce -- this fixture
        # yields two segments, so a hardcoded index would depend on the
        # classifier's segmentation staying put.
        last = bands - 1
        page.locator("#ribbon .seg").nth(last).click()
        sel = page.evaluate("window.__labelGpx.selected")
        if sel != last:
            failures.append(f"ui: ribbon click selected {sel}, expected {last}")

        # --- speed chart and significant shifts
        page.click("#chartToggle")
        page.wait_for_selector("#chart svg", timeout=20000)
        chart = page.evaluate(
            "() => { const s = window.__labelGpx.shifts || [];"
            " return [s.length, document.querySelectorAll('#chart .shift').length,"
            "   document.querySelectorAll('#chart .speed').length,"
            "   s.every(x => typeof x.t_ms === 'number' && Number.isFinite(x.t_ms)),"
            "   s.every(x => x.p_value <= x.alpha_corrected)]; }"
        )
        n, marks, lines, plain_numbers, significant = chart
        print(f"ok   ui: chart drew {lines} speed line(s) and {marks} shift marks for {n} shifts")
        if marks != n:
            failures.append(f"ui: {n} shifts but {marks} marks drawn")
        if lines != 1:
            failures.append(f"ui: expected one speed polyline, got {lines}")
        # A u64 timestamp serialises as BigInt by default, and the first thing any
        # consumer does with a timestamp is subtract it -- which throws "Cannot
        # mix BigInt and other types" and leaves the chart blank. The wasm
        # boundary converts; this is what keeps it converted.
        if not plain_numbers:
            failures.append("ui: shift t_ms is not a plain JS number (BigInt leak?)")
        if not significant:
            failures.append("ui: a reported shift does not clear its corrected level")
        # Speed-band zones, drawn from the library's thresholds rather than a copy.
        zones = page.evaluate(
            "() => [document.querySelectorAll('#chart .zone').length,"
            " document.querySelectorAll('#chart .floor').length,"
            " [...document.querySelectorAll('#chart .chart-ticks .tick')].map(e => e.textContent)]"
        )
        print(f"ok   ui: chart drew {zones[0]} speed-band zones, {zones[1]} floor line(s), "
              f"ticks {zones[2]}")
        # Four bands: stationary / walking / running / driving, the running split
        # being the documented labelling aid.
        if zones[0] != 4:
            failures.append(f"ui: expected 4 speed-band zones, got {zones[0]}")
        # The axis has to carry numbers at all -- there was no readable speed
        # anywhere on this page before.
        if len(zones[2]) < 2:
            failures.append(f"ui: speed axis has {len(zones[2])} labels")
        if not any("*" in t for t in zones[2]) and len(zones[2]) > 3:
            failures.append("ui: the labelling-aid tick is not marked")

        # A speed column in the table, with a real number in it.
        speeds = page.evaluate(
            "() => { const h = [...document.querySelectorAll('#segments thead th')]"
            "   .map(e => e.textContent.trim());"
            " const col = h.indexOf('m/s');"
            " const vals = [...document.querySelectorAll('#segments tbody tr')]"
            "   .map(r => r.children[col] && r.children[col].textContent.trim());"
            " return [col, vals.filter(v => v && v !== '\u2014').length]; }"
        )
        print(f"ok   ui: table has a speed column at index {speeds[0]} with {speeds[1]} values")
        if speeds[0] < 0:
            failures.append("ui: no m/s column in the segment table")
        if speeds[1] == 0:
            failures.append("ui: the speed column is empty for every segment")

        # Drag a rectangle: the range becomes its own segment, labelled by the
        # dominant band inside the box, and nothing outside it moves.
        before = page.evaluate("() => window.__labelGpx.segments.map(s => [s.start, s.end, s.type])")
        box = page.locator("#chart svg").bounding_box()
        x0, x1 = box["x"] + box["width"] * 0.3, box["x"] + box["width"] * 0.5
        ytop, ybot = box["y"] + box["height"] * 0.2, box["y"] + box["height"] * 0.8
        page.mouse.move(x0, ytop)
        page.mouse.down()
        page.mouse.move((x0 + x1) / 2, (ytop + ybot) / 2, steps=5)
        brushed = page.evaluate("() => !document.querySelector('#chart .brush').hidden")
        page.mouse.move(x1, ybot, steps=5)
        page.mouse.up()
        page.wait_for_timeout(800)
        after = page.evaluate(
            "() => { const s = window.__labelGpx.segments;"
            " return { n: s.length, human: s.filter(x => x.edited).length,"
            "   tiled: s.every((x, i) => i === 0 || x.start === s[i-1].end + 1),"
            "   times: s.every(x => Number.isFinite(x.t0) && Number.isFinite(x.t1) && x.t1 >= x.t0),"
            "   uniqueStarts: new Set(s.map(x => x.t0)).size === s.length }; }"
        )
        note = page.eval_on_selector("#warn", "e => e.textContent")
        print(f"ok   ui: drag sliced {len(before)} -> {after['n']} segments · {note.strip()[:80]}")
        if not brushed:
            failures.append("ui: no selection rectangle appeared during the drag")
        if after["human"] != 1:
            failures.append(f"ui: {after['human']} segments marked human, expected exactly 1")
        if not after["tiled"]:
            failures.append("ui: segments no longer tile the trace after a slice")
        # The split halves used to inherit the parent's timestamps, so three
        # segments reported the same start time and duration -- and those fields
        # are what a fixture exports as start_time/end_time.
        if not after["times"] or not after["uniqueStarts"]:
            failures.append("ui: segment timestamps do not match their spans after a slice")
        page.click("#undo")
        page.wait_for_timeout(300)
        restored = page.evaluate("() => window.__labelGpx.segments.length")
        if restored != len(before):
            failures.append(f"ui: undo left {restored} segments, expected {len(before)}")

        # --- synced zoom
        # The overview strip must exist with a window box, and wheeling over the
        # chart must narrow the shared window -- which every time view reads.
        ov = page.evaluate(
            "() => [document.querySelectorAll('#overview .ov').length,"
            " !!document.querySelector('#overview .window')]"
        )
        if ov[0] == 0 or not ov[1]:
            failures.append(f"ui: overview strip missing bands or window ({ov})")
        cbox = page.locator("#chart svg").bounding_box()
        page.mouse.move(cbox["x"] + cbox["width"] / 2, cbox["y"] + cbox["height"] / 2)
        page.mouse.wheel(0, -500)
        page.wait_for_timeout(500)
        zoom = page.evaluate(
            "() => { const s = window.__labelGpx; const v = s.view;"
            " const pts = s.parsed.points;"
            " const full = pts[pts.length - 1].t_ms - pts[0].t_ms;"
            " const bands = document.querySelectorAll('#ribbon .seg').length;"
            " const r = document.getElementById('ribbon');"
            " const w = [...r.querySelectorAll('.seg')].reduce((a, e) => a + e.offsetWidth, 0);"
            " return { zoomed: !!v, span: v ? v.t1 - v.t0 : full, full, bands,"
            "   fill: [Math.round(w), r.clientWidth],"
            "   lines: document.querySelectorAll('#chart .speed').length,"
            "   segs: s.segments.length }; }"
        )
        print(f"ok   ui: wheel zoomed to {zoom['span'] / 60000:.0f} of "
              f"{zoom['full'] / 60000:.0f} min, ribbon kept {zoom['bands']} bands "
              f"filling {zoom['fill'][0]}/{zoom['fill'][1]}px")
        if not zoom["zoomed"] or zoom["span"] >= zoom["full"]:
            failures.append("ui: wheel over the chart did not narrow the window")
        # The invariants that make the ribbon honest have to survive zooming: one
        # band per segment, and the bands still tiling the whole track.
        if zoom["bands"] != zoom["segs"]:
            failures.append(f"ui: {zoom['bands']} bands for {zoom['segs']} segments while zoomed")
        if abs(zoom["fill"][0] - zoom["fill"][1]) > 2:
            failures.append(f"ui: zoomed ribbon covers {zoom['fill'][0]} of {zoom['fill'][1]}px")
        if zoom["lines"] != 1:
            failures.append(f"ui: {zoom['lines']} speed polylines while zoomed, expected 1")

        # Double-clicking the overview resets to the whole trace. Do it on the
        # strip's own left edge rather than its centre, which is inside the window
        # box and starts a pan.
        obox = page.locator("#overview").bounding_box()
        page.mouse.dblclick(obox["x"] + 3, obox["y"] + obox["height"] / 2)
        page.wait_for_timeout(400)
        if page.evaluate("() => window.__labelGpx.view") is not None:
            failures.append("ui: double-click on the overview did not reset the zoom")
            # The handle test below counts boundaries inside the window, so a
            # failed reset would cascade into a confusing second failure.
            page.evaluate("() => { window.__labelGpx.view = null; }")
            page.evaluate("() => window.dispatchEvent(new Event('resize'))")

        # --- a handle sits on the edge of the band it moves
        # (Run before the zoom checks below, which leave the view reset.)
        # It did not before: `.seg` was a flex item with `min-width: 2px`, so a
        # band's laid-out left edge was not its time fraction, while the handles
        # were positioned from time. With 100 out-of-window bands clamped to 2 px
        # each, the drift was 200 px.
        edges = page.evaluate(
            "() => [...document.querySelectorAll('#ribbon .seg')].map(e =>"
            "  ({ i: +e.dataset.i, left: e.getBoundingClientRect().left }))"
        )
        handle_pos = page.evaluate(
            "() => [...document.querySelectorAll('#ribbonHandles .handle')].map(e =>"
            "  ({ i: +e.dataset.boundary, left: e.getBoundingClientRect().left"
            "     + e.getBoundingClientRect().width / 2 }))"
        )
        by_i = {e["i"]: e["left"] for e in edges}
        worst = 0.0
        for h in handle_pos:
            if h["i"] in by_i:
                worst = max(worst, abs(h["left"] - by_i[h["i"]]))
        print(f"ok   ui: {len(handle_pos)} handles sit within {worst:.1f}px of their band edge")
        if worst > 2:
            failures.append(f"ui: a boundary handle is {worst:.1f}px from its band's edge")

        # --- wheel over the ribbon zooms, not only over the chart
        page.evaluate("() => { window.__labelGpx.view = null; }")
        page.evaluate("() => window.dispatchEvent(new Event('resize'))")
        page.wait_for_timeout(200)
        rb = page.locator("#ribbon").bounding_box()
        page.mouse.move(rb["x"] + rb["width"] * 0.5, rb["y"] + rb["height"] / 2)
        page.mouse.wheel(0, -240)
        page.wait_for_timeout(400)
        rib_zoom = page.evaluate("() => window.__labelGpx.view")
        span_min = None if not rib_zoom else (rib_zoom["t1"] - rib_zoom["t0"]) / 60000
        print("ok   ui: wheel over the ribbon zoomed to "
              + ("nothing" if span_min is None else f"{span_min:.0f} min"))
        if not rib_zoom:
            failures.append("ui: wheeling over the ribbon did not zoom")
        obox = page.locator("#overview").bounding_box()
        page.mouse.dblclick(obox["x"] + 3, obox["y"] + obox["height"] / 2)
        page.wait_for_timeout(400)
        if page.evaluate("() => window.__labelGpx.view") is not None:
            failures.append("ui: could not reset the zoom after the ribbon wheel")

        # --- hovering the ribbon marks the point on the map
        rb = page.locator("#ribbon").bounding_box()
        page.mouse.move(rb["x"] + rb["width"] * 0.4, rb["y"] + rb["height"] / 2)
        page.wait_for_timeout(250)
        hover = page.evaluate(
            "() => { const s = window.__labelGpx;"
            " const txt = document.getElementById('hoverInfo').textContent.trim();"
            " const m = txt.match(/point (\\d+)/);"
            " const idx = m ? +m[1] - 1 : null;"
            " return { txt, idx, t: idx === null ? null : s.parsed.points[idx].t_ms,"
            "   markers: s.hoverMarkerLatLng ? s.hoverMarkerLatLng() : null }; }"
        )
        want = page.evaluate(
            "() => { const s = window.__labelGpx; const w = s.view ??"
            " { t0: s.parsed.points[0].t_ms, t1: s.parsed.points.at(-1).t_ms };"
            " return w.t0 + (w.t1 - w.t0) * 0.4; }"
        )
        drift = abs((hover["t"] or 0) - want) / 1000
        print(f"ok   ui: hovering the ribbon read '{hover['txt'][:40]}' "
              f"({drift:.0f}s from the hovered time)")
        if hover["idx"] is None:
            failures.append("ui: hovering the ribbon reported no point")
        elif drift > 30:
            failures.append(f"ui: the hovered point is {drift:.0f}s from the hovered x")
        if hover["markers"] is None:
            failures.append("ui: hovering the ribbon put no marker on the map")
        else:
            p = page.evaluate(
                f"() => {{ const p = window.__labelGpx.parsed.points[{hover['idx']}];"
                " return [p.lat, p.lon]; }"
            )
            if abs(p[0] - hover["markers"][0]) > 1e-6 or abs(p[1] - hover["markers"][1]) > 1e-6:
                failures.append("ui: the map marker is not on the hovered point")
        page.mouse.move(rb["x"] + rb["width"] / 2, rb["y"] - 40)
        page.wait_for_timeout(150)

        # --- boundary handles
        handles = page.locator("#ribbonHandles .handle")
        n_handles = handles.count()
        segs_before = page.evaluate("() => window.__labelGpx.segments.map(s => s.start)")
        if n_handles != len(segs_before) - 1:
            failures.append(f"ui: {n_handles} handles for {len(segs_before)} segments")
        # This fixture yields two segments, so there is exactly one interior
        # boundary; index the last handle rather than assuming several exist.
        if n_handles == 0:
            failures.append(
                f"ui: no boundary handles for {len(segs_before)} segments "
                f"(view={page.evaluate('() => window.__labelGpx.view')})"
            )
        hbox = handles.nth(max(0, n_handles - 1)).bounding_box() if n_handles else None
        rbox = page.locator("#ribbon").bounding_box()
        if hbox is None:
            rbox = None
        if hbox is not None:
          page.mouse.move(hbox["x"] + hbox["width"] / 2, hbox["y"] + hbox["height"] / 2)
        page.mouse.down()
        page.mouse.move(
            hbox["x"] + hbox["width"] / 2 - rbox["width"] * 0.08,
            hbox["y"] + hbox["height"] / 2,
            steps=5,
        )
        page.mouse.up()
        page.wait_for_timeout(400)
        moved = page.evaluate(
            "() => { const s = window.__labelGpx.segments;"
            " return { starts: s.map(x => x.start), human: s.filter(x => x.edited).length,"
            "   tiled: s.every((x, i) => i === 0 || x.start === s[i-1].end + 1),"
            "   times: s.every(x => Number.isFinite(x.t0) && x.t1 >= x.t0) }; }"
        )
        changed = [i for i, (a, b) in enumerate(zip(segs_before, moved["starts"])) if a != b]
        print(f"ok   ui: handle drag moved boundary indices {changed}, "
              f"{moved['human']} segments human")
        if not changed:
            failures.append("ui: dragging a handle moved no boundary")
        if not moved["tiled"] or not moved["times"]:
            failures.append("ui: a handle drag broke the tiling or the timestamps")
        page.click("#undo")
        page.wait_for_timeout(300)

        # --- a rectangle over empty speed range still slices
        # This did nothing before: the box's height names the label, so a box drawn
        # in the driving band over a stationary stretch is a driving slice.
        page.evaluate(
            "() => { const s = window.__labelGpx;"
            " const i = s.segments.findIndex(x => x.type === 'stationary' && x.points > 5);"
            " window.__target = i >= 0 ? i : 0; }"
        )
        target = page.evaluate("() => window.__target")
        before_type = page.evaluate("() => window.__labelGpx.segments[window.__target].type")
        span = page.evaluate(
            "() => { const s = window.__labelGpx.segments[window.__target];"
            " const p = window.__labelGpx.parsed.points;"
            " return [s.t0, s.t1, p[0].t_ms, p[p.length-1].t_ms]; }"
        )
        # Convert that time range to chart x, and drag high up in the driving zone.
        cbox = page.locator("#chart svg").bounding_box()
        frac = lambda t: (t - span[2]) / max(1, span[3] - span[2])
        x0 = cbox["x"] + cbox["width"] * frac(span[0]) + 2
        x1 = cbox["x"] + cbox["width"] * frac(span[1]) - 2
        if x1 - x0 > 6:
            ytop = cbox["y"] + cbox["height"] * 0.05
            ybot = cbox["y"] + cbox["height"] * 0.25
            page.mouse.move(x0, ytop)
            page.mouse.down()
            page.mouse.move((x0 + x1) / 2, (ytop + ybot) / 2, steps=4)
            page.mouse.move(x1, ybot, steps=4)
            page.mouse.up()
            page.wait_for_timeout(600)
            note = page.eval_on_selector("#warn", "e => e.textContent").strip()
            print(f"ok   ui: empty-band slice over a {before_type} stretch — {note[:90]}")
            if "box height" not in note:
                failures.append(f"ui: a box in an empty band did not slice ({note[:60]})")
            page.click("#undo")
            page.wait_for_timeout(200)
        else:
            print("skip ui: no stationary stretch wide enough to drag over")

        # The sensitivity knob has to actually change the answer.
        page.select_option("#chartWindow", "6")
        page.wait_for_timeout(700)
        fine = page.evaluate("() => (window.__labelGpx.shifts || []).length")
        page.select_option("#chartWindow", "24")
        page.wait_for_timeout(700)
        coarse = page.evaluate("() => (window.__labelGpx.shifts || []).length")
        print(f"ok   ui: sensitivity fine={fine} normal={n} coarse={coarse}")
        if fine == coarse and n == fine:
            failures.append("ui: the window control changed nothing")
        page.select_option("#chartWindow", "12")
        page.click("#chartToggle")

        # --- basemap switch
        # Painted, not merely present. Under COEP a blocked tile still leaves its
        # <img> in the DOM, so counting elements says the map works when it is
        # entirely blank -- which is what the live site was doing.
        page.wait_for_timeout(2500)
        tiles = page.evaluate(
            "() => { const i = [...document.querySelectorAll('.leaflet-tile-pane img')];"
            " return [i.length, i.filter(t => t.complete && t.naturalWidth > 0).length]; }"
        )
        print(f"ok   ui: OSM tiles {tiles[1]} painted of {tiles[0]} requested")
        if tiles[1] == 0:
            failures.append(f"ui: {tiles[0]} OSM tiles in the DOM, none painted (COEP?)")
        page.click('.basemap button[data-mode="ptiles"]')
        page.wait_for_timeout(600)
        on = page.eval_on_selector(".basemap button.on", "e => e.dataset.mode")
        if on != "ptiles":
            failures.append(f"ui: basemap switch stuck on {on}")
        # The vector draw needs the public tile host. Report what happened either
        # way rather than failing a UI test on somebody else's uptime.
        try:
            # Wait for a note that reports an *outcome*. Waiting for "not empty"
            # returned instantly on the note setTrace had already written ("3
            # cells cover this trace") and reported a draw that had not started.
            page.wait_for_function(
                "() => /features|unavailable|zoom|too large|basemap:/.test("
                "document.getElementById('basemapNote').textContent || '')",
                timeout=30000,
            )
            note = page.eval_on_selector("#basemapNote", "e => e.textContent")
            # Count what is inside the ptiles panes: with preferCanvas the
            # geometry is one canvas per pane, so querying for `path` finds
            # nothing even when thousands of features are drawn.
            drew = page.evaluate(
                "() => [...document.querySelectorAll('.leaflet-pane')]"
                ".filter(p => p.className.includes('ptiles'))"
                ".reduce((n, p) => n + p.children.length, 0)"
            )
            print(f"ok   ui: ptiles basemap — {note} ({drew} pane layers)")
            if "features" in note:
                drawn = int(note.split()[0])
                if drawn <= 0 or drew == 0:
                    failures.append(f"ui: ptiles basemap reported '{note}' but painted nothing")
            else:
                print("     (nothing drawn: see the note above)")
        except Exception:
            print("skip ui: ptiles basemap drew nothing — tile host unreachable?")
        # The space argument: the vector basemap must fetch the trace's own cells,
        # not the viewport's. This fixture is ~1 km of trail, so a handful.
        cells = page.evaluate(
            "() => (document.getElementById('basemapNote').textContent.match"
            "(/(\\d+) cells/) || [])[1]"
        )
        page.click('.basemap button[data-mode="osm"]')

        # --- place lookup, and attaching it into the trace
        # Click on the trace itself, so the lookup lands somewhere the layers
        # actually cover. The building/address/business layers are all remote, so
        # this is reported rather than asserted when the host is unreachable.
        page.evaluate(
            "() => { const p = window.__labelGpx.parsed.points;"
            " const m = window.__leafletMap; m.setView([p[0].lat, p[0].lon], 17);"
            " m.fire('click', { latlng: L.latLng(p[0].lat, p[0].lon) }); }"
        )
        try:
            page.wait_for_selector("#placeAttach", timeout=30000)
            title = page.eval_on_selector("#place .title", "e => e.textContent").strip()
            page.click("#placeAttach")
            attached = page.evaluate(
                "() => { const s = window.__labelGpx.segments.find(x => x.edited && x.context);"
                " return s ? [s.type, !!s.context.building,"
                " (s.context.addresses||[]).length, (s.context.businesses||[]).length] : null; }"
            )
            if not attached:
                failures.append("ui: attach did not write context onto a segment")
            else:
                print(f"ok   ui: looked up '{title}' and attached it "
                      f"(building={attached[1]}, {attached[2]} addresses, {attached[3]} businesses)")
                # And it must survive export: an annotation that does not reach
                # the file is not a fixture.
                xml = page.evaluate(
                    "async () => { const gpx = await import('./js/gpx.js');"
                    " const wasm = await import('./lib/client/ptiles_client.js');"
                    " return gpx.writeGpx(window.__labelGpx.parsed, window.__labelGpx.segments,"
                    " { snapshot: '2026-08-07' }, wasm.intersection_type_name); }"
                )
                wrote = [t for t in ("rook:building", "rook:addresses", "rook:businesses")
                         if t in xml]
                if 'source="human"' not in xml:
                    failures.append("ui: attached segment did not export as human")
                print(f"ok   ui: export carries {', '.join(wrote) or 'no place blocks'}"
                      + ("" if wrote else " (this trail has none nearby)"))
        except Exception as e:
            print(f"skip ui: place lookup unavailable ({str(e)[:60]})")

        # --- the trace-wide building scan
        # Remote layers again, so a host failure reports rather than fails. What
        # is asserted when it does run: the outlines are drawn in their own layer
        # (not the vector basemap's shared pane), each one really does contain a
        # trace point, and the inspector names them for the selected segment.
        try:
            page.click("#scanBuildings")
            page.wait_for_function(
                "() => document.getElementById('scanBuildings').textContent"
                "  === 'Buildings on trace'",
                timeout=60000,
            )
            page.wait_for_timeout(300)
            scan = page.evaluate(
                "() => { const s = window.__labelGpx; const hits = s.traceBuildings || [];"
                " const note = document.getElementById('warn').textContent;"
                " return { n: hits.length, note,"
                "   allInside: hits.every(b => b.points.length > 0),"
                "   firstSeg: hits.length ? hits[0].points[0] : null }; }"
            )
            print(f"ok   ui: building scan outlined {scan['n']} footprint(s) — {scan['note'][:60]}")
            if not scan["allInside"]:
                failures.append("ui: a scanned building contains no trace point")
            if not scan["n"]:
                # Every committed fixture is a trail through woodland, so zero is
                # the honest answer there. Exercise the containment path with a
                # synthetic one-point trace inside a footprint the place lookup
                # already confirmed we are standing in.
                synth = page.evaluate(
                    "async () => { const s = window.__labelGpx;"
                    " const p0 = s.parsed.points[0];"
                    # Any footprint near the trace will do; its own centroid is a
                    # point inside it, which is what the scan has to find.
                    " const b = await s.resolverForTests.buildingAt(p0.lat, p0.lon);"
                    " if (!b) return { skipped: 'no building within 50 m of the trace start' };"
                    " const hits = await s.scanForTests("
                    "   [{ lat: b.centroid_lat, lon: b.centroid_lon, t_ms: 0 }]);"
                    " return { n: hits.length, name: hits.length ? hits[0].name : b.name }; }"
                )
                if synth.get("skipped"):
                    print(f"skip ui: synthetic building scan — {synth['skipped']}")
                else:
                    print(f"ok   ui: scan over a point inside a footprint found "
                          f"{synth['n']} ({synth['name'] or 'unnamed'})")
                    if not synth["n"]:
                        failures.append(
                            "ui: the scan found no footprint at a point buildingAt calls inside"
                        )
            if scan["n"]:
                # Select the segment the first hit belongs to; the inspector must
                # name it under "inside".
                page.evaluate(
                    f"() => {{ const s = window.__labelGpx;"
                    f" const i = s.segments.findIndex(x => {scan['firstSeg']} >= x.start"
                    f"   && {scan['firstSeg']} <= x.end);"
                    " if (i >= 0) document.querySelector(`#segments tr[data-i=\"${i}\"]`)?.click(); }"
                )
                page.wait_for_timeout(300)
                dl = page.eval_on_selector("#detail", "e => e.textContent")
                if "inside" not in dl:
                    failures.append("ui: the inspector did not list the scanned building")
                else:
                    print("ok   ui: the inspector lists the footprint the segment is inside")
        except Exception as e:
            print(f"skip ui: building scan unavailable ({str(e)[:200]})")

        # The export half, deterministically: the live lookup above lands on a
        # rural trail with no buildings, addresses or businesses within range, so
        # asserting on its output would pass whether or not the writers work.
        # A synthetic context of the exact shape `attachPlace` produces does not
        # depend on what happens to be mapped in North Carolina.
        blocks = page.evaluate(
            "async () => { const gpx = await import('./js/gpx.js');"
            " const wasm = await import('./lib/client/ptiles_client.js');"
            " const st = window.__labelGpx;"
            " const seg = { ...st.segments[0], edited: true, context: {"
            "   lat: 35.96, lon: -83.92, snapshot: '2026-08-07', resolved: Date.now(),"
            "   building: { osm_id: 1314765907, name: 'Bob & Sons', building_type: 'retail',"
            "               category: 'shop', distance_m: 3.2, inside: true },"
            "   addresses: [{ housenumber: '36', street: 'Market Sq', distance_m: 8 }],"
            "   businesses: [{ osm_id: '9007199254740993', name: 'Taco Bell',"
            "                  category_idx: 7, operating_status: 'open', distance_m: 34 }] } };"
            " const xml = gpx.writeGpx(st.parsed, [seg], { snapshot: '2026-08-07' },"
            "   wasm.intersection_type_name);"
            " return xml; }"
        )
        for want in ("rook:building", "Bob &amp; Sons", "<inside>true</inside>",
                     "rook:addresses", "Market Sq", "rook:businesses", "Taco Bell",
                     "9007199254740993"):
            if want not in blocks:
                failures.append(f"ui: exported context is missing {want!r}")
        print("ok   ui: an attached place exports as rook:building / addresses / businesses")

        # A page that throws while still drawing looks fine in a screenshot.
        real = [e for e in errors if "favicon" not in e]
        if real:
            failures.append("page errors: " + "; ".join(real[:3]))
            print("FAIL page errors: " + "; ".join(real[:3]))
        browser.close()

    print(f"\n{len(results) + 1 - len(failures)} passed, {len(failures)} failed")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
