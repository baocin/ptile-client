#!/usr/bin/env python3
"""Does each layer actually draw on the map?

The byte-level suites (cargo test, node --test) prove both readers agree with
the generator. That is still compatible with the page rendering nothing, so
this drives the real page in a real browser and counts what each layer put on
the map.

Design notes, each one a mistake made first:

- The page's script is `type="module"`, so nothing is on `window`. It exposes
  `window.__ptiles` for exactly this; counting DOM nodes cannot attribute a
  shape to a layer.
- Enabling a layer adds to its group but disabling does NOT clear it, so one
  page load cannot measure several layers. Every layer gets a fresh load.
- The map is navigated by URL hash (`#lat=..&lon=..&zoom=..`), which the page
  already parses. Clicking zoom controls or scrolling the wheel put earlier
  runs over empty countryside where every layer correctly drew nothing.
- Roads and water are controls. If they report zero, the harness is broken,
  not the code -- do not read anything else from that run.

Usage:
    python3 -m http.server 8899 --bind 127.0.0.1   # from demo/
    python3 demo/test/render_check.py [--url URL] [--keep-open]
"""
import argparse
import json
import sys
import urllib.request

from playwright.sync_api import sync_playwright

# Downtown Nashville: dense enough for most layers.
NASHVILLE = (36.1627, -86.7816, 14)
# Cameras are sparse, and the published US.camera.ptiles is additionally
# gutted to one point per cell, so downtown Nashville legitimately has none in
# view. Aim that one at a cell the served file actually contains, otherwise a
# zero says nothing about whether the layer works.
SIOUX_FALLS = (43.61707, -96.95255, 13)

# key in ptilesLayers -> checkbox id, note, (lat, lon, zoom)
LAYERS = [
    ("roads", "chkRoads", "control", NASHVILLE),
    ("water", "chkWater", "control", NASHVILLE),
    ("bldgs", "chkBldgs", "", NASHVILLE),
    ("parks", "chkParks", "38-byte index", NASHVILLE),
    ("rail", "chkRail", "38-byte index", NASHVILLE),
    ("camera", "chkCamera", "38-byte index", SIOUX_FALLS),
    ("signal", "chkSignal", "38-byte index", NASHVILLE),
]

ENABLE = """(id) => {
  const e = document.getElementById(id);
  if (!e) return false;
  if (!e.checked) { e.checked = true; e.dispatchEvent(new Event('change', {bubbles: true})); }
  return true;
}"""


def run(base_url, keep_open):
    results, errors = {}, {}

    with sync_playwright() as p:
        browser = p.chromium.launch()
        for key, chk, note, (lat, lon, zoom) in LAYERS:
            url = f"{base_url}#lat={lat}&lon={lon}&zoom={zoom}"
            page = browser.new_page(viewport={"width": 1400, "height": 900})
            errs = []
            page.on("pageerror", lambda e: errs.append(str(e)))
            page.on("console",
                    lambda m: errs.append("console: " + m.text) if m.type == "error" else None)

            page.goto(url, wait_until="load", timeout=90_000)
            page.wait_for_function("() => !!window.__ptiles", timeout=30_000)
            page.wait_for_timeout(3000)

            # PTILES Mode gates all rendering: renderViewport() and
            # scheduleViewportRender() both return early on !ptilesModeActive.
            # With it off, layers fetch their header/dict/index and then never
            # request a single block -- reader present, group on the map, zero
            # features, no error. That is by design, and it is what made three
            # earlier harness runs report zero for every layer including roads.
            page.click("#btnPtiles")
            page.wait_for_timeout(2500)

            view = page.evaluate("() => window.__ptiles.view()")
            if not page.evaluate(ENABLE, chk):
                results[key] = {"error": f"no checkbox #{chk}"}
                page.close()
                continue

            # Wait for the count to stop moving, not for it to become non-zero.
            # Breaking on the first non-zero reading samples mid-render and
            # makes the numbers swing run to run (roads read 925 once and 4356
            # the next); worse, a layer still fetching reads 0 and looks broken.
            # US.signals is 29 MB and a dense viewport needs many block
            # fetches, so give it room.
            counts, stable = {}, 0
            for _ in range(75):
                page.wait_for_timeout(1000)
                prev = counts.get("features")
                counts = page.evaluate("() => window.__ptiles.featureCounts()")[key]
                if counts["loading"]:
                    stable = 0
                    continue
                stable = stable + 1 if counts["features"] == prev else 0
                if stable >= 4 and counts["features"] > 0:
                    break

            info = page.evaluate("(k) => window.__ptiles.layerInfo(k)", key)
            results[key] = {**counts, "note": note, "info": info,
                            "at": f"{lat:.3f},{lon:.3f} z{view['zoom']}"}
            if errs:
                errors[key] = list(dict.fromkeys(errs))[:5]

            if key == "roads":
                page.screenshot(path="/tmp/claude-1000/-home-aoi-kino/"
                                     "ec84e3e2-0a93-4c56-b16d-bdd348ef5e8d/scratchpad/"
                                     "render_roads.png")
            if key == "signal":
                page.screenshot(path="/tmp/claude-1000/-home-aoi-kino/"
                                     "ec84e3e2-0a93-4c56-b16d-bdd348ef5e8d/scratchpad/"
                                     "render_signal.png")
            if keep_open:
                page.wait_for_timeout(5000)
            page.close()
        browser.close()

    print(f"served from {base_url}\n")
    print(f"  {'layer':8s} {'features':>9s}  index           location              note")
    print("  " + "-" * 76)
    controls_ok, failures = True, []
    for key, _, note, _loc in LAYERS:
        r = results.get(key, {})
        if "error" in r:
            print(f"  {key:8s} {r['error']}")
            failures.append(key)
            continue
        n = r["features"]
        info = r.get("info") or {}
        idx = f"{info.get('entrySize', '?')}B {info.get('offsetBase', '?')}" if info else "-"
        print(f"  {key:8s} {n:>9d}  {idx:15s} {r['at']:21s} {note}")
        if n == 0:
            failures.append(key)
            if note == "control":
                controls_ok = False

    for key, errs in errors.items():
        print(f"\n  page errors during {key}:")
        for e in errs:
            print("    " + e[:150])

    print()
    if not controls_ok:
        print("CONTROL FAILED: roads and/or water drew nothing. The harness is "
              "broken, not the code -- ignore every other row above.")
        return 2
    if failures:
        print(f"drew nothing: {', '.join(failures)}")
        return 1
    print("every layer drew features")
    return 0


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", default="http://127.0.0.1:8899/index.html")
    ap.add_argument("--keep-open", action="store_true")
    a = ap.parse_args()
    try:
        with urllib.request.urlopen(a.url.split("#")[0], timeout=10) as r:
            if r.status != 200:
                sys.exit(f"server returned {r.status}; serve demo/ first")
    except Exception as e:
        sys.exit(f"cannot reach {a.url}: {e}\n"
                 f"run: python3 -m http.server 8899 --bind 127.0.0.1   (from demo/)")
    sys.exit(run(a.url, a.keep_open))
