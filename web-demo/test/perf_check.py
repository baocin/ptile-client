#!/usr/bin/env python3
"""How long does each layer take, and where does the time go?

`render_check.py` answers "did it draw?" and never looks at a clock. GOAL.md
quotes per-layer seconds that no script in this repo produces, so every
optimization claim about this page has been unfalsifiable. This is the missing
half: same page, same live tiles, but timed and attributed.

Per layer, at a fixed viewport, cold and warm:

  open    ticking the box -> the layer's reader exists (header + dict + index)
  render  clicking PTILES Mode -> the feature count stops moving
  split   fetch / zstd / decode / other, from window.__ptiles.perf()
  wire    range requests and compressed bytes, counted at the fetch call

`other` is a residual (wall - fetch - zstd - decode), which is Leaflet plus the
page's own JS. It is computed rather than timed because the draw loops are
interleaved with the decode calls; a residual that dominates is the interesting
reading and is exactly what buildings reports.

Method notes, each one a mistake that produced a wrong number first:

- Cold means a fresh browser context: the reader's Cache API store and the HTTP
  cache both live there, and reusing a context measures a replay, not a load.
  Warm is a second page in the *same* context.
- Every other layer's checkbox is unticked first. Roads and water ship
  `checked`, so "enable buildings and time it" was timing three layers.
- The clock stops when the count last *changed*, not when the settle loop
  notices. Otherwise every reading carries a constant 3-sample tail and the
  ratio between a fast layer and a slow one is compressed toward 1.
- Setting `.checked` fires nothing, and several boxes ship checked, so the
  change event is dispatched unconditionally.
- PTILES Mode gates all rendering. With it off a layer opens its index and then
  never requests a block, which is why `open` and `render` can be separated at
  all -- and why forgetting the click reports a layer that costs nothing.
- Three runs, median reported with the min-max spread. A single number over a
  live CDN is noise.

Tiles come from the live host (which supports Range); only the page's own
assets are served locally, so no Range-capable handler is needed here --
`demo/test/coarse_check.py` has one for measurements against local files.

Usage:
    python3 web-demo/test/perf_check.py                  # every layer, 3 runs
    python3 web-demo/test/perf_check.py --layers bldgs --runs 1
    python3 web-demo/test/perf_check.py --json out.json  # for a before/after
"""
import argparse
import http.server
import json
import socketserver
import statistics
import sys
import threading
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
WEB_DEMO = HERE.parent

NASHVILLE = (36.1627, -86.7816, 14)
SIOUX_FALLS = (43.61707, -96.95255, 13)

# key -> checkbox id, location. Same fixed viewports render_check.py uses, so
# the two harnesses are talking about the same picture.
LAYERS = {
    "roads":  ("chkRoads", NASHVILLE),
    "water":  ("chkWater", NASHVILLE),
    "bldgs":  ("chkBldgs", NASHVILLE),
    "parks":  ("chkParks", NASHVILLE),
    "rail":   ("chkRail", NASHVILLE),
    "camera": ("chkCamera", SIOUX_FALLS),
    "signal": ("chkSignal", NASHVILLE),
}
ALL_BOXES = [c for c, _ in LAYERS.values()]

SET_BOX = """([id, on]) => {
  const e = document.getElementById(id);
  if (!e) return false;
  e.checked = on;
  e.dispatchEvent(new Event('change', {bubbles: true}));
  return true;
}"""


def serve(directory, port):
    handler = lambda *a, **kw: http.server.SimpleHTTPRequestHandler(
        *a, directory=str(directory), **kw)
    socketserver.TCPServer.allow_reuse_address = True
    httpd = socketserver.TCPServer(("127.0.0.1", port), handler)
    threading.Thread(target=httpd.serve_forever, daemon=True).start()
    return httpd


def settle(page, key, timeout_s, poll_s=0.25):
    """Wall seconds until the feature count stopped changing, and the count.

    Returns the moment of the *last change*, so the samples spent confirming
    stability are not billed to the layer.
    """
    t0 = time.monotonic()
    last, last_change, stable = -1, t0, 0
    deadline = t0 + timeout_s
    while time.monotonic() < deadline:
        page.wait_for_timeout(int(poll_s * 1000))
        n = page.evaluate("() => window.__ptiles.featureCounts()")[key]["features"]
        if n == last:
            stable += 1
            if stable >= 3 and n > 0:
                return last_change - t0, n
        else:
            stable = 0
            last_change = time.monotonic()
        last = n
    return last_change - t0, last


def measure(page, base, key, checkbox, where, timeout_s):
    lat, lon, zoom = where
    page.goto(f"{base}#lat={lat}&lon={lon}&zoom={zoom}", wait_until="load", timeout=90_000)
    page.wait_for_function("() => !!window.__ptiles", timeout=30_000)
    # The wasm module and the map both settle here; timing through it would
    # bill the layer for the page's own start-up.
    page.wait_for_timeout(2500)

    for box in ALL_BOXES:
        if box != checkbox:
            page.evaluate(SET_BOX, [box, False])
    page.wait_for_timeout(300)
    page.evaluate("() => window.__ptiles.perfReset()")

    t0 = time.monotonic()
    page.evaluate(SET_BOX, [checkbox, True])
    ok = page.wait_for_function(
        f"() => window.__ptiles.featureCounts()['{key}'].hasReader",
        timeout=timeout_s * 1000)
    open_s = time.monotonic() - t0

    page.click("#btnPtiles")
    render_s, features = settle(page, key, timeout_s)

    perf = page.evaluate("(ms) => window.__ptiles.perf(ms)", (open_s + render_s) * 1000)
    perf.update(openMs=round(open_s * 1000), renderMs=round(render_s * 1000),
                features=features)
    return perf


def med(rows, field):
    vals = [r[field] for r in rows if r.get(field) is not None]
    if not vals:
        return None, None, None
    return statistics.median(vals), min(vals), max(vals)


def fmt(rows, field, unit=""):
    m, lo, hi = med(rows, field)
    if m is None:
        return "-"
    if lo == hi:
        return f"{m:.0f}{unit}"
    return f"{m:.0f}{unit} [{lo:.0f}-{hi:.0f}]"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=8901)
    ap.add_argument("--runs", type=int, default=3)
    ap.add_argument("--layers", nargs="*", default=list(LAYERS))
    ap.add_argument("--timeout", type=int, default=90, help="seconds per phase")
    ap.add_argument("--json", help="write the raw samples here")
    ap.add_argument("--label", default="", help="tag for the json, e.g. 'before'")
    args = ap.parse_args()

    try:
        from playwright.sync_api import sync_playwright
    except ImportError:
        sys.exit("needs playwright: pip install playwright && playwright install chromium")

    unknown = [k for k in args.layers if k not in LAYERS]
    if unknown:
        sys.exit(f"unknown layers: {unknown}")

    httpd = serve(WEB_DEMO, args.port)
    base = f"http://127.0.0.1:{args.port}/index.html"
    print(f"serving {WEB_DEMO} at {base}")
    print(f"{args.runs} run(s) per layer, live tiles from maps.mydatatimeline.com\n")

    samples = {k: {"cold": [], "warm": []} for k in args.layers}

    with sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        for key in args.layers:
            checkbox, where = LAYERS[key]
            for run in range(args.runs):
                # A fresh context is what makes "cold" cold: both the HTTP cache
                # and the Cache API store the reader uses live in it.
                ctx = browser.new_context(viewport={"width": 1400, "height": 900})
                try:
                    page = ctx.new_page()
                    errs = []
                    page.on("pageerror", lambda e: errs.append(str(e)))
                    cold = measure(page, base, key, checkbox, where, args.timeout)
                    page.close()

                    page = ctx.new_page()
                    page.on("pageerror", lambda e: errs.append(str(e)))
                    warm = measure(page, base, key, checkbox, where, args.timeout)
                    page.close()
                finally:
                    ctx.close()

                samples[key]["cold"].append(cold)
                samples[key]["warm"].append(warm)
                print(f"  {key:7s} run {run + 1}  "
                      f"cold {cold['openMs'] + cold['renderMs']:6d} ms "
                      f"({cold['features']} feats, {cold['requests']} req, "
                      f"{cold['bytes'] / 1024:.0f} KiB)   "
                      f"warm {warm['openMs'] + warm['renderMs']:6d} ms "
                      f"({warm['requests']} req)"
                      + (f"   [{len(errs)} page errors]" if errs else ""))
                for e in errs[:2]:
                    print(f"          {e}")

        browser.close()
    httpd.shutdown()

    hdr = (f"\n{'layer':8s} {'cache':5s} {'open':>13s} {'render':>15s} "
           f"{'total':>15s} {'net':>13s} {'netsum':>13s} {'zstd':>8s} "
           f"{'decode':>8s} {'rest':>13s} {'req':>5s} {'KiB':>8s} {'feats':>7s}")
    print(hdr)
    print("-" * len(hdr))
    for key in args.layers:
        for cache in ("cold", "warm"):
            rows = samples[key][cache]
            kib = [{"kib": r["bytes"] / 1024} for r in rows]
            total = [{"t": r["openMs"] + r["renderMs"]} for r in rows]
            print(f"{key:8s} {cache:5s} "
                  f"{fmt(rows, 'openMs'):>13s} {fmt(rows, 'renderMs'):>15s} "
                  f"{fmt(total, 't'):>15s} "
                  f"{fmt(rows, 'netWallMs'):>13s} {fmt(rows, 'netSumMs'):>13s} "
                  f"{fmt(rows, 'zstdMs'):>8s} {fmt(rows, 'decodeMs'):>8s} "
                  f"{fmt(rows, 'restMs'):>13s} "
                  f"{fmt(rows, 'requests'):>5s} {fmt(kib, 'kib'):>8s} "
                  f"{fmt(rows, 'features'):>7s}")
    print(f"\nms, median of {args.runs} run(s) with [min-max]. "
          "net = wall time with a request in flight, netsum = summed request "
          "time\n(netsum/net is the concurrency achieved). "
          "rest = total - net - zstd - decode.")

    if args.json:
        Path(args.json).write_text(json.dumps(
            {"label": args.label, "runs": args.runs, "samples": samples}, indent=2))
        print(f"wrote {args.json}")

    empty = [k for k in args.layers
             if not samples[k]["cold"] or samples[k]["cold"][0]["features"] == 0]
    if empty:
        print(f"\nFAILURES:\n  - drew nothing, so its timing means nothing: {empty}")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
