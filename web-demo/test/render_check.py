#!/usr/bin/env python3
"""Does the wasm-only page actually draw each layer?

The byte-level suites prove the reader agrees with the generator; a page that
renders nothing is perfectly compatible with that. This drives the real page in
a real browser against the live tile host and counts what each layer put on the
map.

It is the parity gate for the port: web-demo decodes every layer through
ptiles-core, so if the counts here match what demo/test/render_check.py reports
for the legacy page, the two agree on what is in the files.

Camera is the one layer where the counts legitimately differ now: this page
draws a marker per camera plus a viewpoint wedge for each one that records a
facing, where the legacy page draws a single dot. Compare camera by marker
count, not by group size.

Inherited from the legacy harness, each point a mistake made there first:

- The page's script is `type="module"`, so nothing is on `window`. It exposes
  `window.__ptiles` for exactly this; counting DOM nodes cannot attribute a
  shape to a layer.
- Enabling a layer adds to its group but disabling does NOT clear it, so one
  page load cannot measure several layers. Every layer gets a fresh load.
- The map is navigated by URL hash, which the page already parses. Earlier runs
  that clicked zoom controls ended up over empty countryside, where every layer
  correctly drew nothing.
- Roads and water are controls. If they report zero, the harness is broken, not
  the code.

Usage:
    python3 web-demo/test/render_check.py            # serves web-demo/ itself
    python3 web-demo/test/render_check.py --keep-open
"""
import argparse
import http.server
import json
import os
import re
import socketserver
import sys
import threading
from pathlib import Path

HERE = Path(__file__).resolve().parent
WEB_DEMO = HERE.parent

NASHVILLE = (36.1627, -86.7816, 14)
# Camera coverage is thin; Sioux Falls is where the legacy harness found it.
SIOUX_FALLS = (43.61707, -96.95255, 13)

# key in ptilesLayers -> checkbox id, is-a-control, location
LAYERS = [
    ("roads", "chkRoads", True, NASHVILLE),
    ("water", "chkWater", True, NASHVILLE),
    ("bldgs", "chkBldgs", False, NASHVILLE),
    ("parks", "chkParks", False, NASHVILLE),
    ("rail", "chkRail", False, NASHVILLE),
    ("camera", "chkCamera", False, SIOUX_FALLS),
    ("signal", "chkSignal", False, NASHVILLE),
]

# The layer checkboxes live in a panel that can be collapsed, so Playwright's
# `check()` times out waiting for visibility. Set the property and fire the
# event the page listens for instead -- same code path, no UI dependency.
# Several boxes ship `checked`, so "set it and fire change" is not enough --
# with the box already checked nothing dispatches and the layer never loads.
# Force the event either way.
ENABLE = """(id) => {
  const e = document.getElementById(id);
  if (!e) return false;
  e.checked = true;
  e.dispatchEvent(new Event('change', {bubbles: true}));
  return true;
}"""


def serve(directory, port):
    """A plain static server. Only the page's own assets come from here -- the
    .ptiles files are fetched from the live host, which does support Range."""
    handler = lambda *a, **kw: http.server.SimpleHTTPRequestHandler(
        *a, directory=str(directory), **kw)
    socketserver.TCPServer.allow_reuse_address = True
    httpd = socketserver.TCPServer(("127.0.0.1", port), handler)
    threading.Thread(target=httpd.serve_forever, daemon=True).start()
    return httpd


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=8899)
    ap.add_argument("--timeout", type=int, default=45000)
    ap.add_argument("--keep-open", action="store_true")
    args = ap.parse_args()

    try:
        from playwright.sync_api import sync_playwright
    except ImportError:
        sys.exit("needs playwright: pip install playwright && playwright install chromium")

    httpd = serve(WEB_DEMO, args.port)
    base = f"http://127.0.0.1:{args.port}/index.html"
    print(f"serving {WEB_DEMO} at {base}\n")

    results = {}
    failures = []

    with sync_playwright() as pw:
        browser = pw.chromium.launch(headless=not args.keep_open)
        for key, checkbox, is_control, (lat, lon, zoom) in LAYERS:
            url = f"{base}#lat={lat}&lon={lon}&zoom={zoom}"
            page = browser.new_page(viewport={"width": 1400, "height": 900})
            errors = []
            page.on("pageerror", lambda e: errors.append(str(e)))
            page.on("console", lambda m: errors.append(f"console.{m.type}: {m.text}")
                    if m.type == "error" else None)

            page.goto(url, wait_until="load", timeout=90_000)
            page.wait_for_function("() => !!window.__ptiles", timeout=30_000)
            page.wait_for_timeout(3000)

            # Tick the box BEFORE enabling PTILES mode. That is the order a
            # user works in, and it is the order that was broken:
            # activatePtilesMode used to load a fixed list (roads, water,
            # camera, signal) rather than whatever was ticked, so parks, rail
            # and buildings were added to the map with no reader behind them
            # and drew nothing, silently. Testing the other order hid it.
            if not page.evaluate(ENABLE, checkbox):
                failures.append(f"{key}: no checkbox #{checkbox}")
                page.close()
                continue
            page.wait_for_timeout(500)

            # PTILES Mode gates all rendering: renderViewport() and
            # scheduleViewportRender() both return early on !ptilesModeActive.
            # With it off, a layer fetches its header/dict/index and then never
            # requests a block -- reader present, group on the map, zero
            # features, no error.
            page.click("#btnPtiles")
            page.wait_for_timeout(2500)

            # Wait for the count to stop moving, not for it to become non-zero.
            # Breaking on the first non-zero reading samples mid-render; a layer
            # still fetching reads 0 and looks broken.
            n, stable, last = 0, 0, -1
            for _ in range(60):
                page.wait_for_timeout(1000)
                n = page.evaluate("() => window.__ptiles.featureCounts()")[key]["features"]
                if n == last and n > 0:
                    stable += 1
                    if stable >= 3:
                        break
                else:
                    stable = 0
                last = n
            results[key] = n

            real_errors = [e for e in errors if e]
            status = "ok" if n > 0 else "EMPTY"
            print(f"  {key:8s} {n:6d} features   {status}"
                  + (f"   [{len(real_errors)} page errors]" if real_errors else ""))
            for e in real_errors[:3]:
                print(f"           {e}")
                failures.append(f"{key}: {e}")

            if n == 0:
                failures.append(
                    f"{key}: drew nothing"
                    + (" -- this is a control layer, so the harness is suspect"
                       if is_control else ""))
            page.close()

        # Untick a layer, move the map, tick it back. The layer must redraw.
        #
        # renderPtilesForCells only records a cell as rendered when the box was
        # ticked at the time, and the checkbox handler used to schedule a render
        # only when it had to open a reader. So a box unticked across a pan came
        # back to an empty group with nothing queued to refill it, and stayed
        # blank until the user happened to pan again. Every layer had it; roads
        # is where it showed, because roads is ticked by default.
        page = browser.new_page(viewport={"width": 1400, "height": 900})
        lat, lon, zoom = NASHVILLE
        page.goto(f"{base}#lat={lat}&lon={lon}&zoom={zoom}", wait_until="load", timeout=90_000)
        page.wait_for_function("() => !!window.__ptiles", timeout=30_000)
        page.wait_for_timeout(2000)
        page.click("#btnPtiles")

        def roads():
            return page.evaluate("() => window.__ptiles.featureCounts().roads.features")

        def settle(want_positive):
            last, stable = -1, 0
            for _ in range(40):
                page.wait_for_timeout(1000)
                n = roads()
                if n == last and (n > 0 or not want_positive):
                    stable += 1
                    if stable >= 3:
                        return n
                else:
                    stable = 0
                last = n
            return roads()

        before = settle(True)
        page.evaluate(ENABLE.replace("e.checked = true", "e.checked = false"), "chkRoads")
        page.wait_for_timeout(1000)
        # Pan far enough that every cell in view is one roads never drew.
        page.evaluate("() => window.__ptiles.setView(36.0526, -86.7146, 14)")
        page.wait_for_timeout(4000)
        page.evaluate(ENABLE, "chkRoads")
        after = settle(True)
        # `after > 0` is not the assertion. Unticking removes the group from the
        # map but leaves its polylines in it, so the features drawn before the
        # pan are still counted and a broken build reads a healthy non-zero.
        # What the bug destroys is the *new* cells, so the count must grow.
        grew = after > before
        print(f"\n  re-tick after pan: roads {before} -> {after} features "
              + ("ok" if grew else "NO NEW CELLS"))
        if not grew:
            failures.append(
                f"roads: re-ticking the box after a pan drew no new cells "
                f"({before} -> {after})")
        page.close()

        # High zoom, and a state that is not Tennessee.
        #
        # Two bugs met here. polygonToCells selects by cell *centre*, and a
        # res-7 hex is ~5 km across, so from zoom 18 up the viewport sits
        # inside one hex, contains no centre, and the page rendered 0 cells.
        # Separately, 49 of 51 states ship a buildings index written in build
        # order rather than cell order, which the reader rejected outright --
        # Tennessee and North Carolina are the only sorted ones, so testing in
        # Nashville could never have caught it.
        page = browser.new_page(viewport={"width": 1400, "height": 900})
        errors = []
        page.on("pageerror", lambda e: errors.append(str(e)))
        page.goto(f"{base}#state=GA&lat=33.76853&lon=-84.38587&zoom=17",
                  wait_until="load", timeout=90_000)
        page.wait_for_function("() => !!window.__ptiles", timeout=30_000)
        page.wait_for_timeout(2000)
        page.evaluate(ENABLE, "chkBldgs")
        page.click("#btnPtiles")
        page.wait_for_timeout(20000)

        bldgs = page.evaluate("() => window.__ptiles.featureCounts().bldgs.features")
        print(f"\n  GA buildings at z17: {bldgs} " + ("ok" if bldgs > 0 else "EMPTY"))
        if bldgs == 0:
            failures.append("bldgs: Georgia drew nothing (unsorted index rejected?)")

        page.evaluate("() => window.__ptiles.setView(33.76853, -84.38587, 19)")
        page.wait_for_timeout(6000)
        status = page.text_content("#layerStatus")
        print(f"  status at z19: {status!r} "
              + ("ok" if "rendering 0 cells" not in (status or "") else "ZERO CELLS"))
        if "rendering 0 cells" in (status or ""):
            failures.append("viewport: zoom 19 selected 0 cells")
        if errors:
            for e in errors[:3]:
                print(f"           {e}")
                failures.append(f"high-zoom page error: {e}")
        page.close()

        # 3D extrusion.
        #
        # Buildings memoize per cell in `bldgs.rendered`, so the failure mode to
        # guard is the toggle doing nothing at all for cells already drawn --
        # silent, no error, looks like "this area has no heights". `d3 > 0`
        # would not catch it; the count has to *grow*.
        #
        # The upper bound guards the other direction: each extruded building is
        # deliberately 2 paths (one multi-ring polygon for every visible wall,
        # one for the roof). A regression to a path per wall would be ~5x and
        # would lock the tab on a dense cell, so assert the shape rather than
        # trying to time it.
        page = browser.new_page(viewport={"width": 1400, "height": 900})
        errors = []
        page.on("pageerror", lambda e: errors.append(str(e)))
        lat, lon, _ = NASHVILLE
        page.goto(f"{base}#state=TN&lat={lat}&lon={lon}&zoom=17",
                  wait_until="load", timeout=90_000)
        page.wait_for_function("() => !!window.__ptiles", timeout=30_000)
        page.wait_for_timeout(2000)
        page.evaluate(ENABLE, "chkBldgs")
        page.click("#btnPtiles")

        def bldgs():
            return page.evaluate("() => window.__ptiles.featureCounts().bldgs.features")

        def settle_bldgs():
            last, stable = -1, 0
            for _ in range(60):
                page.wait_for_timeout(1000)
                n = bldgs()
                if n == last and n > 0:
                    stable += 1
                    if stable >= 3:
                        return n
                else:
                    stable = 0
                last = n
            return bldgs()

        flat = settle_bldgs()
        page.evaluate(ENABLE, "chk3D")
        d3 = settle_bldgs()
        cov = page.evaluate("() => window.__ptiles.bldgHeightCoverage()")
        print(f"\n  3D: {flat} -> {d3} paths ({d3/max(flat,1):.2f}x), "
              f"{cov['withHeight']}/{cov['total']} have heights")

        if d3 <= flat:
            failures.append(
                f"3D: toggling drew nothing new ({flat} -> {d3}) -- cells already "
                f"in `rendered` are skipped unless the mode guard clears them")
        if d3 > 2 * flat:
            failures.append(
                f"3D: {d3/max(flat,1):.1f}x flat -- expected <=2x (walls are one "
                f"multi-ring polygon, not one path per wall)")
        if cov["withHeight"] == 0:
            failures.append(
                "3D: no building in view carries a height -- core is dropping "
                "flags2 & 0x10, or downtown Nashville lost its coverage")

        # Walls sit over neighbouring footprints. If they are interactive they
        # eat the map click and `doLookup` answers for the wrong building or
        # not at all -- invisible to any feature count.
        page.evaluate("""() => {
          const m = document.getElementById('map');
          const r = m.getBoundingClientRect();
          m.dispatchEvent(new MouseEvent('click', {
            clientX: r.left + r.width / 2, clientY: r.top + r.height / 2, bubbles: true}));
        }""")
        page.wait_for_timeout(9000)
        opened = page.evaluate("() => document.getElementById('infoPanel').classList.contains('show')")
        print(f"  3D: map click still opens the info panel: {opened}")
        if not opened:
            failures.append("3D: clicking the map stopped working (walls swallowing clicks?)")

        for e in errors[:3]:
            print(f"           {e}")
            failures.append(f"3D page error: {e}")
        page.close()

        # Line of sight.
        #
        # The geometry has its own unit tests in core; this only checks the
        # page wires up to it. The assertion that carries weight is that
        # raising the eye height reveals more -- it is the one thing that
        # cannot pass if heights are being ignored, which is the whole point of
        # the mode. A count alone would pass on a 2D shadow test.
        page = browser.new_page(viewport={"width": 1400, "height": 900})
        errors = []
        page.on("pageerror", lambda e: errors.append(str(e)))
        page.goto(f"{base}#state=TN&lat=36.1627&lon=-86.7816&zoom=16",
                  wait_until="load", timeout=90_000)
        page.wait_for_function("() => !!window.__ptiles", timeout=30_000)
        page.wait_for_timeout(2500)

        ground = page.evaluate("() => window.__ptiles.losAt(36.1627, -86.7816)")
        page.evaluate("""() => { const s = document.getElementById('losEye');
          s.value = '90'; s.dispatchEvent(new Event('change', {bubbles:true})); }""")
        page.wait_for_timeout(8000)
        high = page.evaluate("""() => ({
          visible: parseInt(document.getElementById('losVisible').textContent,10)||0,
          hidden: parseInt(document.getElementById('losHidden').textContent,10)||0 })""")
        print(f"\n  line of sight: {ground['visible']} visible at 1.7 m, "
              f"{high['visible']} at 90 m (of {ground['visible']+ground['hidden']})")

        if ground["visible"] + ground["hidden"] == 0:
            failures.append("line of sight: no buildings tested at all")
        elif ground["visible"] == 0:
            failures.append("line of sight: nothing visible even from the observer's own spot")
        if high["visible"] <= ground["visible"]:
            failures.append(
                f"line of sight: raising the eye to 90 m revealed nothing extra "
                f"({ground['visible']} -> {high['visible']}) -- heights are not "
                f"reaching the occlusion test")
        # Cameras ride the same viewshed call. The geometry has a core test
        # (`a_camera_sized_marker_is_a_target_in_its_own_right`); what can only
        # break here is the plumbing -- cameras not fetched, or the results
        # read back at the wrong offset. Nashville's answer is legitimately
        # "0 watching", so assert the row was *computed*, not that it is
        # non-zero, and assert the totals are self-consistent.
        cams = ground.get("cameras", "")
        print(f"  cameras on you: {cams}")
        m = re.match(r"^(\d+) of (\d+) \((\d+) facing away\)$", cams)
        if cams == "none nearby":
            failures.append(
                "line of sight: no cameras within 800 m of downtown Nashville -- "
                "US.camera.ptiles is not being read")
        elif not m:
            failures.append(f"line of sight: camera row never filled in ('{cams}')")
        else:
            watching, total, away = (int(g) for g in m.groups())
            if watching + away > total:
                failures.append(
                    f"line of sight: camera counts do not add up ({watching} "
                    f"watching + {away} facing away > {total} nearby) -- the "
                    f"visibility results are being read at the wrong offset")

        for e in errors[:3]:
            print(f"           {e}")
            failures.append(f"line-of-sight page error: {e}")
        page.close()

        # View finder (the reverse viewshed).
        #
        # The load-bearing assertions are the two that cannot pass on a stub:
        # shrinking the radius must find fewer buildings, and raising the target
        # off the ground must find more. A bare count would pass on a function
        # that returned every building in view.
        page = browser.new_page(viewport={"width": 1400, "height": 900})
        errors = []
        page.on("pageerror", lambda e: errors.append(str(e)))
        # Centred on the Cumberland River through downtown Nashville.
        page.goto(f"{base}#state=TN&lat=36.1650&lon=-86.7750&zoom=16",
                  wait_until="load", timeout=90_000)
        page.wait_for_function("() => !!window.__ptiles", timeout=30_000)
        page.wait_for_timeout(2500)

        def view(tag, radius, eye, name=None):
            arg = "null" if name is None else json.dumps(name)
            return page.evaluate(
                f"() => window.__ptiles.viewFinderAt(36.1650, -86.7750, 16, "
                f"{json.dumps(tag)}, {radius}, {eye}, {arg})")

        wide = view("water:river", 800, 1.7)
        tight = view("water:river", 400, 1.7)
        high = view("water:river", 800, 20)
        biz = view("business:", 800, 3, "starbucks")
        none = view("water:canal", 800, 1.7)
        print(f"\n  view finder: river@800m {wide['found']}, @400m {tight['found']}, "
              f"target at 20 m {high['found']}; starbucks {biz['found']}")
        print(f"  view finder: {wide['samples']}")

        if wide["found"] == 0:
            failures.append(
                "view finder: nothing can see the Cumberland from downtown "
                "Nashville -- the water layer or the reverse viewshed is not wired up")
        if tight["found"] >= wide["found"]:
            failures.append(
                f"view finder: halving the radius did not narrow the answer "
                f"({wide['found']} -> {tight['found']}) -- the radius is being ignored")
        if high["found"] <= wide["found"]:
            failures.append(
                f"view finder: raising the target to 20 m revealed nothing extra "
                f"({wide['found']} -> {high['found']}) -- heights are not reaching "
                f"the occlusion test, same failure the eye-height check guards")
        if biz["found"] == 0 or "starbucks" not in biz["samples"]:
            failures.append(
                f"view finder: name target found nothing ('{biz['samples']}')")
        if none["found"] != 0 or "Nothing matching" not in none["summary"]:
            failures.append(
                f"view finder: a tag with no features should report that, got "
                f"'{none['summary']}' / {none['found']}")

        for e in errors[:3]:
            print(f"           {e}")
            failures.append(f"view finder page error: {e}")
        page.close()

        # Inspector rows.
        #
        # The road fields (name/class/lanes/surface/bridge-tunnel) are decoded
        # on every segment and were read by nothing in this repo, so the whole
        # failure mode is silence: the section stays hidden and the panel looks
        # exactly as it did before. Assert the section is visible AND that the
        # street name is not the placeholder, or a wired-up-but-always-empty
        # regression passes.
        #
        # Jurisdiction is deliberately opt-in (28 MB grid), so the check drives
        # the load link the way a user would rather than asserting it appears
        # on its own.
        page = browser.new_page(viewport={"width": 1400, "height": 900})
        errors = []
        page.on("pageerror", lambda e: errors.append(str(e)))
        lat, lon, _ = NASHVILLE
        page.goto(f"{base}#state=TN&lat={lat}&lon={lon}&zoom=17",
                  wait_until="load", timeout=90_000)
        page.wait_for_function("() => !!window.__ptiles", timeout=30_000)
        page.wait_for_timeout(2000)

        # Driven through the coordinate box rather than a synthesized click at
        # the map centre: a pixel click lands wherever the centre happens to be
        # (measured: seven of eight downtown points sit nearer a sidewalk than
        # a street), which tests the snap policy instead of the panel wiring.
        page.fill("#coordInput", "36.1635,-86.7810")
        page.click("#btnQuery")
        page.wait_for_timeout(12000)

        road = page.evaluate("""() => ({
          shown: document.getElementById('roadSection').style.display !== 'none',
          name: document.getElementById('roadName').textContent,
          cls: document.getElementById('roadClass').textContent,
          optional: ['roadRefRow','roadOnewayRow','roadSpeedRow','roadLanesRow',
                     'roadSurfaceRow','roadStructRow']
            .filter(id => document.getElementById(id).style.display !== 'none').length
        })""")
        print(f"\n  inspector: road section {road['shown']}, "
              f"'{road['name']}' ({road['cls']}), {road['optional']}/6 optional rows")
        if not road["shown"]:
            failures.append("inspector: no road found for a downtown Nashville lookup")
        elif not road["name"] or not road["cls"]:
            failures.append(
                f"inspector: road row shown but blank ('{road['name']}' / "
                f"'{road['cls']}') -- fields are not reaching the panel")

        page.evaluate("() => document.getElementById('adminLoad').click()")
        page.wait_for_timeout(45000)
        admin = page.evaluate("""() => ({
          value: document.getElementById('adminValue').textContent.trim(),
          tz: document.getElementById('adminTz').textContent.trim(),
          tzShown: document.getElementById('adminTzRow').style.display !== 'none'
        })""")
        print(f"  inspector: jurisdiction '{admin['value']}' tz '{admin['tz']}'")
        if "failed" in admin["value"] or "loading" in admin["value"]:
            failures.append(f"inspector: US.admin never resolved ({admin['value']})")
        elif "Davidson" not in admin["value"] or "Tennessee" not in admin["value"]:
            failures.append(
                f"inspector: downtown Nashville resolved to '{admin['value']}' "
                f"-- expected Davidson county, Tennessee")
        if not admin["tzShown"] or "Chicago" not in admin["tz"]:
            failures.append(f"inspector: timezone row wrong or missing ('{admin['tz']}')")

        for e in errors[:3]:
            print(f"           {e}")
            failures.append(f"inspector page error: {e}")
        page.close()

        # Which business a named building resolves to.
        #
        # The Target in Smyrna (OSM way/636880358, name=Target) has 64 business
        # records within 200 m and a pharmacy counter 30 m inside it. Ranking
        # every kind of name match into one bucket and breaking the tie on
        # distance answered "CVS Pharmacy at Target" -- a concession inside the
        # store, closer to the centroid than the store's own record.
        #
        # The assertion is not "found something" and not "found something with
        # Target in the name": seven of the neighbours contain the word. It is
        # that the exact match wins, and it fails on the ranking this replaced.
        page = browser.new_page(viewport={"width": 1400, "height": 900})
        errors = []
        page.on("pageerror", lambda e: errors.append(str(e)))
        page.goto(f"{base}#state=TN&lat=35.979629&lon=-86.571064&zoom=18",
                  wait_until="load", timeout=90_000)
        page.wait_for_function("() => !!window.__ptiles", timeout=30_000)
        page.wait_for_timeout(2000)
        page.fill("#coordInput", "35.979629,-86.571064")
        page.click("#btnQuery")
        page.wait_for_timeout(15000)

        biz = page.evaluate("""() => ({
          bldg: document.getElementById('bldgName').textContent.trim(),
          shown: document.getElementById('bizRow').style.display !== 'none',
          name: document.getElementById('bizName').textContent.trim()
        })""")
        # The row carries a "(+63 more)" tail and a confidence mark, so compare
        # the name itself rather than the cell's text.
        picked = re.sub(r"^[^A-Za-z0-9]*", "", biz["name"]).split(" (+")[0].strip()
        print(f"\n  building '{biz['bldg']}' -> business '{picked}'")
        if not biz["shown"]:
            failures.append("business match: no business found for the Smyrna Target")
        elif picked.lower() != "target":
            failures.append(
                f"business match: the building named Target resolved to "
                f"'{picked}' -- an exact name match must outrank a record that "
                f"merely contains the name, even one inside the footprint")

        for e in errors[:3]:
            print(f"           {e}")
            failures.append(f"business match page error: {e}")
        page.close()

        if args.keep_open:
            input("\npress enter to close the browser...")
        browser.close()

    httpd.shutdown()

    print("\n" + json.dumps(results, indent=2))
    if failures:
        print("\nFAILURES:")
        for f in failures:
            print(f"  - {f}")
        return 1
    print("\nevery layer drew something")
    return 0


if __name__ == "__main__":
    sys.exit(main())
