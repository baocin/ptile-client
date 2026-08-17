#!/usr/bin/env python3
"""Does every city centre get served by a state file that holds it?

State routing is bounding-box based, and bounding boxes overlap along every
border. Manhattan sits inside NJ's box, Kansas City straddles two, Texarkana is
one town in two states -- and when the picker chooses the neighbour, the page
fetches nothing, draws nothing and says nothing. One such case was found by
hand. This asks the same question 100 times, at the places where the boxes
overlap most.

The assertion is deliberately about data, not about geography: a city centre at
zoom 16 has buildings, so whatever state answers must produce some. The
expected state is printed when it differs, but a mismatch alone is not a
failure -- a town on the line can legitimately be served by either file. Zero
buildings is the failure, because that is what the user sees.

Each point gets a fresh browser context (so state starts at the page default,
as it does for a first visitor), then a small nudge after the layers load, to
put the choice through the moveend path where it is re-evaluated.

    python3 web-demo/test/route_cities_check.py
    python3 web-demo/test/route_cities_check.py --start 40 --limit 10
    python3 web-demo/test/route_cities_check.py --json /tmp/routing.json
"""
import argparse
import json
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from render_check import ENABLE, WEB_DEMO, serve  # noqa: E402
from bench_render import SETTLE  # noqa: E402

ZOOM = 16

# name, lat, lon, the state whose file ought to answer. Weighted towards
# border pairs -- two towns facing each other across a line are where a bbox
# picker goes wrong, and an interior city can only catch a gross failure.
CITIES = [
    ("Manhattan NY", 40.7580, -73.9855, "NY"),
    ("Brooklyn NY", 40.6782, -73.9442, "NY"),
    ("Staten Island NY", 40.5795, -74.1502, "NY"),
    ("Yonkers NY", 40.9312, -73.8988, "NY"),
    ("Jersey City NJ", 40.7178, -74.0431, "NJ"),
    ("Newark NJ", 40.7357, -74.1724, "NJ"),
    ("Trenton NJ", 40.2206, -74.7597, "NJ"),
    ("Camden NJ", 39.9259, -75.1196, "NJ"),
    ("Philadelphia PA", 39.9526, -75.1652, "PA"),
    ("Wilmington DE", 39.7391, -75.5398, "DE"),
    ("Baltimore MD", 39.2904, -76.6122, "MD"),
    ("Washington DC", 38.9072, -77.0369, "DC"),
    ("Arlington VA", 38.8816, -77.0910, "VA"),
    ("Alexandria VA", 38.8048, -77.0469, "VA"),
    ("Bethesda MD", 38.9847, -77.0947, "MD"),
    ("Stamford CT", 41.0534, -73.5387, "CT"),
    ("Greenwich CT", 41.0262, -73.6282, "CT"),
    ("Hartford CT", 41.7658, -72.6734, "CT"),
    ("Providence RI", 41.8240, -71.4128, "RI"),
    ("Boston MA", 42.3601, -71.0589, "MA"),
    ("Nashua NH", 42.7654, -71.4676, "NH"),
    ("Portsmouth NH", 43.0718, -70.7626, "NH"),
    ("Portland ME", 43.6591, -70.2568, "ME"),
    ("Burlington VT", 44.4759, -73.2121, "VT"),
    ("Albany NY", 42.6526, -73.7562, "NY"),
    ("Pittsburgh PA", 40.4406, -79.9959, "PA"),
    ("Wheeling WV", 40.0640, -80.7209, "WV"),
    ("Steubenville OH", 40.3698, -80.6340, "OH"),
    ("Charleston WV", 38.3498, -81.6326, "WV"),
    ("Huntington WV", 38.4192, -82.4452, "WV"),
    ("Ashland KY", 38.4784, -82.6379, "KY"),
    ("Ironton OH", 38.5365, -82.6829, "OH"),
    ("Richmond VA", 37.5407, -77.4360, "VA"),
    ("Norfolk VA", 36.8508, -76.2859, "VA"),
    ("Raleigh NC", 35.7796, -78.6382, "NC"),
    ("Charlotte NC", 35.2271, -80.8431, "NC"),
    ("Rock Hill SC", 34.9249, -81.0251, "SC"),
    ("Columbia SC", 34.0007, -81.0348, "SC"),
    ("Augusta GA", 33.4735, -82.0105, "GA"),
    ("North Augusta SC", 33.5018, -81.9651, "SC"),
    ("Savannah GA", 32.0809, -81.0912, "GA"),
    ("Jacksonville FL", 30.3322, -81.6557, "FL"),
    ("Tallahassee FL", 30.4383, -84.2807, "FL"),
    ("Miami FL", 25.7617, -80.1918, "FL"),
    ("Atlanta GA", 33.7490, -84.3880, "GA"),
    ("Chattanooga TN", 35.0456, -85.3097, "TN"),
    ("Fort Oglethorpe GA", 34.9487, -85.2569, "GA"),
    ("Knoxville TN", 35.9606, -83.9207, "TN"),
    ("Bristol TN", 36.5951, -82.1887, "TN"),
    ("Nashville TN", 36.1627, -86.7816, "TN"),
    ("Clarksville TN", 36.5298, -87.3595, "TN"),
    ("Hopkinsville KY", 36.8656, -87.4886, "KY"),
    ("Memphis TN", 35.1495, -90.0490, "TN"),
    ("West Memphis AR", 35.1465, -90.1845, "AR"),
    ("Southaven MS", 34.9890, -89.9873, "MS"),
    ("Jackson MS", 32.2988, -90.1848, "MS"),
    ("Birmingham AL", 33.5186, -86.8104, "AL"),
    ("Mobile AL", 30.6954, -88.0399, "AL"),
    ("New Orleans LA", 29.9511, -90.0715, "LA"),
    ("Shreveport LA", 32.5252, -93.7502, "LA"),
    ("Texarkana TX", 33.4251, -94.0477, "TX"),
    ("Little Rock AR", 34.7465, -92.2896, "AR"),
    ("Fort Smith AR", 35.3859, -94.3985, "AR"),
    ("Louisville KY", 38.2527, -85.7585, "KY"),
    ("Jeffersonville IN", 38.2775, -85.7372, "IN"),
    ("Cincinnati OH", 39.1031, -84.5120, "OH"),
    ("Covington KY", 39.0837, -84.5086, "KY"),
    ("Evansville IN", 37.9716, -87.5711, "IN"),
    ("Henderson KY", 37.8362, -87.5900, "KY"),
    ("Indianapolis IN", 39.7684, -86.1581, "IN"),
    ("Chicago IL", 41.8781, -87.6298, "IL"),
    ("Hammond IN", 41.5834, -87.5000, "IN"),
    ("Gary IN", 41.5934, -87.3464, "IN"),
    ("Milwaukee WI", 43.0389, -87.9065, "WI"),
    ("Rockford IL", 42.2711, -89.0940, "IL"),
    ("Dubuque IA", 42.5006, -90.6646, "IA"),
    ("Davenport IA", 41.5236, -90.5776, "IA"),
    ("Rock Island IL", 41.5095, -90.5787, "IL"),
    ("Detroit MI", 42.3314, -83.0458, "MI"),
    ("Toledo OH", 41.6528, -83.5379, "OH"),
    ("Cleveland OH", 41.4993, -81.6944, "OH"),
    ("Columbus OH", 39.9612, -82.9988, "OH"),
    ("Minneapolis MN", 44.9778, -93.2650, "MN"),
    ("Hudson WI", 44.9747, -92.7566, "WI"),
    ("Duluth MN", 46.7867, -92.1005, "MN"),
    ("Superior WI", 46.7208, -92.1041, "WI"),
    ("Fargo ND", 46.8772, -96.7898, "ND"),
    ("Moorhead MN", 46.8738, -96.7678, "MN"),
    ("Sioux Falls SD", 43.5460, -96.7313, "SD"),
    ("Sioux City IA", 42.4999, -96.4003, "IA"),
    ("South Sioux City NE", 42.4667, -96.4133, "NE"),
    ("Omaha NE", 41.2565, -95.9345, "NE"),
    ("Council Bluffs IA", 41.2619, -95.8608, "IA"),
    ("Kansas City MO", 39.0997, -94.5786, "MO"),
    ("Kansas City KS", 39.1141, -94.6275, "KS"),
    ("St Louis MO", 38.6270, -90.1994, "MO"),
    ("East St Louis IL", 38.6245, -90.1509, "IL"),
    ("Denver CO", 39.7392, -104.9903, "CO"),
    ("Cheyenne WY", 41.1400, -104.8202, "WY"),
    ("Salt Lake City UT", 40.7608, -111.8910, "UT"),
    ("Las Vegas NV", 36.1699, -115.1398, "NV"),
    ("Bullhead City AZ", 35.1478, -114.5683, "AZ"),
    ("Phoenix AZ", 33.4484, -112.0740, "AZ"),
    ("Albuquerque NM", 35.0844, -106.6504, "NM"),
    ("El Paso TX", 31.7619, -106.4850, "TX"),
    ("Dallas TX", 32.7767, -96.7970, "TX"),
    ("Houston TX", 29.7604, -95.3698, "TX"),
    ("Oklahoma City OK", 35.4676, -97.5164, "OK"),
    ("Tulsa OK", 36.1540, -95.9928, "OK"),
    ("Wichita KS", 37.6872, -97.3301, "KS"),
    ("Portland OR", 45.5152, -122.6784, "OR"),
    ("Vancouver WA", 45.6387, -122.6615, "WA"),
    ("Seattle WA", 47.6062, -122.3321, "WA"),
    ("Spokane WA", 47.6588, -117.4260, "WA"),
    ("Coeur d'Alene ID", 47.6777, -116.7805, "ID"),
    ("Boise ID", 43.6150, -116.2023, "ID"),
    ("Ontario OR", 44.0266, -116.9629, "OR"),
    ("Reno NV", 39.5296, -119.8138, "NV"),
    ("Sacramento CA", 38.5816, -121.4944, "CA"),
    ("San Francisco CA", 37.7749, -122.4194, "CA"),
    ("Los Angeles CA", 34.0522, -118.2437, "CA"),
    ("San Diego CA", 32.7157, -117.1611, "CA"),
    ("Yuma AZ", 32.6927, -114.6277, "AZ"),
    ("Missoula MT", 46.8721, -113.9940, "MT"),
    ("Rapid City SD", 44.0805, -103.2310, "SD"),
]


def check_city(browser, base, name, lat, lon, expect, budget_s):
    ctx = browser.new_context(viewport={"width": 1000, "height": 700})
    page = ctx.new_page()
    t0 = time.perf_counter()
    try:
        page.goto(f"{base}#lat={lat}&lon={lon}&zoom={ZOOM}", wait_until="load", timeout=90_000)
        page.wait_for_function("() => !!window.__ptiles", timeout=30_000)
        page.wait_for_timeout(1500)
        if not page.evaluate(ENABLE, "chkBldgs"):
            return {"city": name, "error": "no buildings checkbox"}
        page.click("#btnPtiles")
        page.wait_for_timeout(2500)
        # Nudge, so the choice goes through moveend once with the layers open.
        # That is the path where a wrong state can be detected and released;
        # the load-time choice is made before any index exists to consult.
        page.evaluate("([a, b, c]) => window.__ptiles.setView(a, b, c)",
                      [lat + 0.0008, lon, ZOOM])

        count, state, reader = 0, None, False
        while time.perf_counter() - t0 < budget_s:
            state = page.evaluate("() => window.__ptiles.state()")
            c = page.evaluate("() => window.__ptiles.featureCounts().bldgs") or {}
            reader = bool(c.get("hasReader"))
            count = page.evaluate(SETTLE, "bldgs")
            if count > 0:
                break
            page.wait_for_timeout(250)
        return {"city": name, "expect": expect, "state": state, "features": count,
                "hasReader": reader, "secs": round(time.perf_counter() - t0, 1)}
    except Exception as e:  # a hung page must not lose the other 99 results
        return {"city": name, "expect": expect, "error": str(e)[:120],
                "secs": round(time.perf_counter() - t0, 1)}
    finally:
        ctx.close()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--start", type=int, default=0)
    ap.add_argument("--limit", type=int, default=len(CITIES))
    ap.add_argument("--budget", type=float, default=45.0, help="seconds per city")
    ap.add_argument("--port", type=int, default=0)
    ap.add_argument("--json")
    args = ap.parse_args()

    try:
        from playwright.sync_api import sync_playwright
    except ImportError:
        sys.exit("needs playwright: pip install playwright && playwright install chromium")

    todo = CITIES[args.start:args.start + args.limit]
    httpd = serve(WEB_DEMO, args.port)
    base = f"http://127.0.0.1:{httpd.server_address[1]}/index.html"
    print(f"{len(todo)} cities, zoom {ZOOM}\n", flush=True)

    out = []
    with sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        for i, (name, lat, lon, expect) in enumerate(todo):
            r = check_city(browser, base, name, lat, lon, expect, args.budget)
            out.append(r)
            if r.get("error"):
                mark = "ERR "
            elif not r["features"]:
                mark = "EMPTY"
            elif r["state"] != expect:
                mark = "other"
            else:
                mark = "ok"
            print(f"  {args.start + i:3d} {mark:5s} {name:22s} "
                  f"served {r.get('state')} feats {r.get('features')} "
                  f"{r.get('error', '')}", flush=True)
        browser.close()

    if args.json:
        Path(args.json).write_text(json.dumps(out, indent=2))

    empty = [r for r in out if not r.get("error") and not r["features"]]
    errs = [r for r in out if r.get("error")]
    other = [r for r in out if not r.get("error") and r["features"] and r["state"] != r["expect"]]
    print(f"\n{len(out) - len(empty) - len(errs)}/{len(out)} drew buildings; "
          f"{len(empty)} empty, {len(errs)} errored, {len(other)} served by a neighbour")
    for r in empty:
        print(f"  EMPTY {r['city']:22s} served {r['state']} (expected {r['expect']}), "
              f"reader {r['hasReader']}")
    for r in other:
        print(f"  other {r['city']:22s} served {r['state']} (expected {r['expect']}), "
              f"{r['features']} features")
    return 1 if empty or errs else 0


if __name__ == "__main__":
    sys.exit(main())
