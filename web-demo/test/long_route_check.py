#!/usr/bin/env python3
"""Can it route a trip that leaves the state it started in?

Everything else here tests a point. A drive is the case where the routing
decision is made repeatedly, on ground the map has never drawn, and where being
one state off is not a cosmetic problem: the graph simply has no edges, and the
page says "no route" for a highway that exists.

Both directions of every pair are run. A route that works one way and not the
other means the routing follows wherever it started rather than the ground it
covers, which is the failure the state-cell table exists to fix.

The distance test is loose on purpose -- roads are not great circles, and a
river crossing can add a lot -- so it only catches a route that wandered into
another state and back.

    python3 web-demo/test/long_route_check.py
    python3 web-demo/test/long_route_check.py --limit 3
"""
import argparse
import json
import math
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from render_check import WEB_DEMO, serve  # noqa: E402

# A road route is longer than the straight line, but not unboundedly: past this
# the path went somewhere it had no business going.
MAX_DETOUR = 2.0

TRIPS = [
    # name, from, to -- each crosses at least one state line
    ("Nashville -> Bowling Green", (36.1627, -86.7816), (36.9685, -86.4808)),
    ("Memphis -> West Memphis", (35.1495, -90.0490), (35.1465, -90.1845)),
    ("Kansas City MO -> KS", (39.0997, -94.5786), (39.1141, -94.6275)),
    ("Cincinnati -> Covington", (39.1031, -84.5120), (39.0837, -84.5086)),
    ("Portland -> Vancouver WA", (45.5152, -122.6784), (45.6387, -122.6615)),
    ("Philadelphia -> Camden", (39.9526, -75.1652), (39.9259, -75.1196)),
    ("Omaha -> Council Bluffs", (41.2565, -95.9345), (41.2619, -95.8608)),
    ("El Paso -> Las Cruces NM", (31.7619, -106.4850), (32.3199, -106.7637)),
    # Longer hauls: several state lines, or a long run inside one state that
    # the corridor has to hold together.
    ("Nashville -> Louisville", (36.1627, -86.7816), (38.2527, -85.7585)),
    ("Chattanooga -> Atlanta", (35.0456, -85.3097), (33.7490, -84.3880)),
    ("Reno -> Sacramento", (39.5296, -119.8138), (38.5816, -121.4944)),
    ("Fargo -> Sioux Falls", (46.8772, -96.7898), (43.5460, -96.7313)),
]


def haversine_m(a, b):
    r = 6371000.0
    p1, p2 = math.radians(a[0]), math.radians(b[0])
    dp = p2 - p1
    dl = math.radians(b[1] - a[1])
    h = math.sin(dp / 2) ** 2 + math.cos(p1) * math.cos(p2) * math.sin(dl / 2) ** 2
    return 2 * r * math.asin(math.sqrt(h))


def one_route(page, a, b, timeout_ms):
    # A long route takes minutes of tile fetching, and page.evaluate has no
    # timeout of its own -- so it is kicked off and polled, with the default
    # 30 s action timeout applying only to the poll.
    page.evaluate("""([a, b]) => {
      window.__routeDone = null;
      window.__ptiles.routeAt(a, b)
        .then((r) => { window.__routeDone = r; })
        .catch((e) => { window.__routeDone = { found: false, error: String(e).slice(0, 100) }; });
    }""", [list(a), list(b)])
    page.wait_for_function("() => window.__routeDone !== null", timeout=timeout_ms)
    return page.evaluate("() => window.__routeDone")


def run_trip(browser, base, name, a, b, timeout_ms):
    """Both directions, in one page: the second leg starts with whatever state
    the first left behind, which is exactly the condition that used to strand
    a route."""
    ctx = browser.new_context(viewport={"width": 1200, "height": 800})
    page = ctx.new_page()
    t0 = time.perf_counter()
    try:
        page.goto(f"{base}#lat={a[0]}&lon={a[1]}&zoom=12", wait_until="load", timeout=90_000)
        page.wait_for_function("() => !!window.__ptiles", timeout=30_000)
        page.wait_for_timeout(2000)
        straight = haversine_m(a, b)
        legs = []
        for label, p, q in (("out", a, b), ("back", b, a)):
            try:
                r = one_route(page, p, q, timeout_ms)
            except Exception as e:
                r = {"found": False, "error": str(e)[:100]}
            legs.append({
                "leg": label,
                "found": bool(r.get("found")),
                "distanceM": r.get("distanceM", 0),
                "detour": round(r.get("distanceM", 0) / straight, 2) if straight else 0,
                "state": page.evaluate("() => window.__ptiles.state()"),
                "failure": r.get("failure"),
                "error": r.get("error"),
            })
        return {"trip": name, "straightM": round(straight), "legs": legs,
                "secs": round(time.perf_counter() - t0, 1)}
    finally:
        ctx.close()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--start", type=int, default=0)
    ap.add_argument("--limit", type=int, default=len(TRIPS))
    ap.add_argument("--timeout", type=float, default=180.0, help="seconds per leg")
    ap.add_argument("--port", type=int, default=0)
    ap.add_argument("--json")
    args = ap.parse_args()

    try:
        from playwright.sync_api import sync_playwright
    except ImportError:
        sys.exit("needs playwright: pip install playwright && playwright install chromium")

    todo = TRIPS[args.start:args.start + args.limit]
    httpd = serve(WEB_DEMO, args.port)
    base = f"http://127.0.0.1:{httpd.server_address[1]}/index.html"
    print(f"{len(todo)} trips, both directions each\n", flush=True)

    out = []
    with sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        for name, a, b in todo:
            r = run_trip(browser, base, name, a, b, args.timeout * 1000)
            out.append(r)
            for leg in r["legs"]:
                bad = not leg["found"] or leg["detour"] > MAX_DETOUR
                print(f"  {'FAIL ' if bad else 'ok   '}{name:30s} {leg['leg']:4s} "
                      f"{leg['distanceM'] / 1000:7.1f} km  x{leg['detour']:<5} "
                      f"in {leg['state']} {leg.get('failure') or leg.get('error') or ''}",
                      flush=True)
        browser.close()

    if args.json:
        Path(args.json).write_text(json.dumps(out, indent=2))

    bad = [(r, leg) for r in out for leg in r["legs"]
           if not leg["found"] or leg["detour"] > MAX_DETOUR]
    legs = sum(len(r["legs"]) for r in out)
    print(f"\n{legs - len(bad)}/{legs} legs routed within x{MAX_DETOUR} of the straight line")
    for r, leg in bad:
        print(f"  FAIL {r['trip']:30s} {leg['leg']:4s} "
              f"{'no route' if not leg['found'] else str(leg['detour']) + 'x'} "
              f"in {leg['state']} {leg.get('failure') or leg.get('error') or ''}")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
