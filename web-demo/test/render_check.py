#!/usr/bin/env python3
"""Does the wasm-only page actually draw each layer?

The byte-level suites prove the reader agrees with the generator; a page that
renders nothing is perfectly compatible with that. This drives the real page in
a real browser against the live tile host and counts what each layer put on the
map.

It is the parity gate for the port: web-demo decodes every layer through
ptiles-core, so if the counts here match what demo/test/render_check.py reports
for the legacy page, the two agree on what is in the files.

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
