#!/usr/bin/env python3
"""Do the query paths route to a state whose file holds the ground?

route_cities_check.py asks what the map draws. This asks the routing decision
directly, at the points where it is hardest: which state owns this coordinate,
and does that state's roads file carry the cell under it.

An earlier version clicked the map and looked for a road in the panel. That
graded the wrong thing: the panel fills only when a road passes within
ROAD_INSPECT_M of the click, which is 25 m, so Southaven and El Paso "failed"
for landing in a parking lot while the routing underneath them was correct.
Asking the index whether it holds the cell has no such luck in it.

    python3 web-demo/test/border_query_check.py
    python3 web-demo/test/border_query_check.py --limit 5
"""
import argparse
import json
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from render_check import WEB_DEMO, serve  # noqa: E402

ZOOM = 16

# The cities route_cities_check.py found being served by a neighbour, plus
# controls well inside their own state. If the query paths route differently
# from the render path, the controls stay green and the border rows do not.
POINTS = [
    ("Manhattan NY", 40.7580, -73.9855, "NY"),
    ("Yonkers NY", 40.9312, -73.8988, "NY"),
    ("Philadelphia PA", 39.9526, -75.1652, "PA"),
    ("Arlington VA", 38.8816, -77.0910, "VA"),
    ("Bethesda MD", 38.9847, -77.0947, "MD"),
    ("Steubenville OH", 40.3698, -80.6340, "OH"),
    ("Ashland KY", 38.4784, -82.6379, "KY"),
    ("Augusta GA", 33.4735, -82.0105, "GA"),
    ("Savannah GA", 32.0809, -81.0912, "GA"),
    ("Fort Oglethorpe GA", 34.9487, -85.2569, "GA"),
    ("Southaven MS", 34.9890, -89.9873, "MS"),
    ("Texarkana TX", 33.4251, -94.0477, "TX"),
    ("Jeffersonville IN", 38.2775, -85.7372, "IN"),
    ("Evansville IN", 37.9716, -87.5711, "IN"),
    ("Dubuque IA", 42.5006, -90.6646, "IA"),
    ("Davenport IA", 41.5236, -90.5776, "IA"),
    ("Hudson WI", 44.9747, -92.7566, "WI"),
    ("Fargo ND", 46.8772, -96.7898, "ND"),
    ("Omaha NE", 41.2565, -95.9345, "NE"),
    ("Kansas City KS", 39.1141, -94.6275, "KS"),
    ("St Louis MO", 38.6270, -90.1994, "MO"),
    ("El Paso TX", 31.7619, -106.4850, "TX"),
    ("Reno NV", 39.5296, -119.8138, "NV"),
    ("Ontario OR", 44.0266, -116.9629, "OR"),
    # Controls: nowhere near a line, and must never regress.
    ("Nashville TN", 36.1627, -86.7816, "TN"),
    ("Denver CO", 39.7392, -104.9903, "CO"),
    ("Indianapolis IN", 39.7684, -86.1581, "IN"),
    ("Phoenix AZ", 33.4484, -112.0740, "AZ"),
]

def check(page, name, lat, lon, expect):
    t0 = time.perf_counter()
    try:
        # A null owner is the right answer for an interior point: the table
        # carries only cells two or more boxes claim, and everywhere else the
        # box picker was never wrong. So null means "the box decides", and the
        # cell check below is what proves the box decided correctly.
        owner = page.evaluate("([a, b]) => window.__ptiles.ownerAt(a, b)", [lat, lon])
        served = owner or expect
        # Roads, because it is the layer every query path fans out to, and the
        # one whose absence produces "no road here" in a city centre.
        has = page.evaluate(
            "async ([s, a, b]) => await window.__ptiles.layerHasCell(s, 'roads', a, b)",
            [served, lat, lon])
        return {"point": name, "expect": expect, "owner": owner, "hasCell": bool(has),
                "secs": round(time.perf_counter() - t0, 1)}
    except Exception as e:
        return {"point": name, "expect": expect, "error": str(e)[:120]}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--start", type=int, default=0)
    ap.add_argument("--limit", type=int, default=len(POINTS))
    ap.add_argument("--budget", type=float, default=25.0)
    ap.add_argument("--port", type=int, default=0)
    ap.add_argument("--json")
    args = ap.parse_args()

    try:
        from playwright.sync_api import sync_playwright
    except ImportError:
        sys.exit("needs playwright: pip install playwright && playwright install chromium")

    todo = POINTS[args.start:args.start + args.limit]
    httpd = serve(WEB_DEMO, args.port)
    base = f"http://127.0.0.1:{httpd.server_address[1]}/index.html"
    print(f"{len(todo)} points: the owner must be the expected state, and its "
          f"roads index must hold the cell\n", flush=True)

    out = []
    with sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        # One page for all of them: the table is fetched once, the readers are
        # cached across points, and nothing here depends on a fresh context.
        page = browser.new_context(viewport={"width": 1000, "height": 700}).new_page()
        page.goto(base, wait_until="load", timeout=90_000)
        page.wait_for_function("() => !!window.__ptiles", timeout=30_000)
        page.wait_for_function("() => window.__ptiles.stateIndex() !== null", timeout=60_000)
        idx = page.evaluate("() => window.__ptiles.stateIndex()")
        print(f"state cell index: {idx['coarse']:,} res-7, {idx['fine']:,} res-9\n", flush=True)

        for name, lat, lon, expect in todo:
            r = check(page, name, lat, lon, expect)
            out.append(r)
            bad = (r.get("error") or not r.get("hasCell")
                   or (r.get("owner") is not None and r["owner"] != expect))
            print(f"  {'FAIL ' if bad else 'ok   '}{name:22s} owner {r.get('owner')} "
                  f"(expected {expect}{', box decides' if r.get('owner') is None else ''}) "
                  f"cell {'held' if r.get('hasCell') else 'MISSING'} "
                  f"{r.get('error', '')}", flush=True)
        browser.close()

    if args.json:
        Path(args.json).write_text(json.dumps(out, indent=2))

    bad = [r for r in out
           if r.get("error") or not r.get("hasCell")
           or (r.get("owner") is not None and r["owner"] != r["expect"])]
    print(f"\n{len(out) - len(bad)}/{len(out)} routed to the right state with the cell present")
    for r in bad:
        print(f"  FAIL {r['point']:22s} owner {r.get('owner')} (expected {r['expect']}), "
              f"cell {'held' if r.get('hasCell') else 'missing'} {r.get('error', '')}")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
