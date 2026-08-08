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
    ap.add_argument("--port", type=int, default=8731)
    args = ap.parse_args()

    try:
        from playwright.sync_api import sync_playwright
    except ImportError:
        print("playwright not installed: pip install playwright && playwright install chromium")
        return 2

    # Serve the repo root, not label-gpx/, so the page can fetch the committed
    # GPX fixtures. `file://` would not do: Cache API is undefined on an
    # insecure origin, and module imports are blocked.
    serve(ROOT, args.port)
    url = f"http://127.0.0.1:{args.port}/label-gpx/index.html"

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
