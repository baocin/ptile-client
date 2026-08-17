#!/usr/bin/env python3
"""Do the *query* paths route to a state that has the ground, not just the map?

route_cities_check.py asks what the map draws. This asks what a click answers,
which goes through a different set of `stateAt` calls (nearestRoadDetail,
the business lookup, the address search) with no rendered layer involved. A
wrong state there is worse than an empty map: the panel says "no road here" in
the middle of a city, and nothing distinguishes that from a real gap.

The invariant is the same shape as the render one and just as blunt: a click on
a city centre is within a few hundred metres of a named road. Anything else
means the file being asked does not hold that ground.

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
# A road within this distance of a city centre. Generous on purpose: the point
# is to catch "served by the wrong state", which shows up as no road at all or
# one kilometres away, not to grade the nearest-road search.
MAX_ROAD_M = 400

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

ROAD = """() => {
  const sec = document.getElementById("roadSection");
  if (!sec || sec.style.display === "none") return null;
  const d = document.getElementById("roadDist").textContent;
  return {
    name: document.getElementById("roadName").textContent,
    klass: document.getElementById("roadClass").textContent,
    dist: parseInt(d, 10),
  };
}"""


def check(browser, base, name, lat, lon, expect, budget_s):
    ctx = browser.new_context(viewport={"width": 1000, "height": 700})
    page = ctx.new_page()
    t0 = time.perf_counter()
    try:
        page.goto(f"{base}#lat={lat}&lon={lon}&zoom={ZOOM}", wait_until="load", timeout=90_000)
        page.wait_for_function("() => !!window.__ptiles", timeout=30_000)
        page.wait_for_timeout(1500)
        # A click, not a synthetic call: this is the handler a user reaches, and
        # it is the one that fans out to the state-routed queries.
        page.evaluate("([a, b]) => window.__ptiles.clickAt(a, b)", [lat, lon])
        road = None
        while time.perf_counter() - t0 < budget_s:
            road = page.evaluate(ROAD)
            if road:
                break
            page.wait_for_timeout(250)
        return {"point": name, "expect": expect,
                "state": page.evaluate("() => window.__ptiles.state()"),
                "road": road, "secs": round(time.perf_counter() - t0, 1)}
    except Exception as e:
        return {"point": name, "expect": expect, "error": str(e)[:120]}
    finally:
        ctx.close()


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
    print(f"{len(todo)} points, a road must be within {MAX_ROAD_M} m\n", flush=True)

    out = []
    with sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        for name, lat, lon, expect in todo:
            r = check(browser, base, name, lat, lon, expect, args.budget)
            out.append(r)
            road = r.get("road")
            bad = r.get("error") or not road or road["dist"] > MAX_ROAD_M
            print(f"  {'FAIL ' if bad else 'ok   '}{name:22s} served {r.get('state')} "
                  + (f"road {road['name']} {road['dist']}m ({road['klass']})" if road
                     else r.get("error", "no road")), flush=True)
        browser.close()

    if args.json:
        Path(args.json).write_text(json.dumps(out, indent=2))

    bad = [r for r in out
           if r.get("error") or not r.get("road") or r["road"]["dist"] > MAX_ROAD_M]
    print(f"\n{len(out) - len(bad)}/{len(out)} answered with a road within {MAX_ROAD_M} m")
    for r in bad:
        print(f"  FAIL {r['point']:22s} served {r.get('state')} "
              f"(expected {r['expect']}): {r.get('road') or r.get('error') or 'no road'}")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
