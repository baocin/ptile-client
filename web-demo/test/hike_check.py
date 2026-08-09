#!/usr/bin/env python3
"""Does trail mode record a hike, point at the car, and survive a refresh?

Trail mode's whole claim is that it still has your track after the phone
reloads the tab in your pocket, and that the needle points at the car. Both
are testable without a mountain: feed fixes the way `watchPosition` would,
then reload the page and ask what came back.

Asserted here, because a plausible stub passes none of them:

  * fixes 8 m apart are recorded; a fix that has not moved is not
  * walked distance tracks the synthetic trail's real length
  * the needle turns as the walker rounds the loop
  * the GPX has one waypoint for the car and one trkpt per recorded point
  * a page reload restores the track, the car and the elapsed clock
  * ending the hike clears the stored session

Usage: python3 web-demo/test/hike_check.py
"""

import argparse
import http.server
import json
import math
import socketserver
import sys
import threading
from pathlib import Path

WEB_DEMO = Path(__file__).resolve().parent.parent

# A trailhead in Glacier: Logan Pass, walking the Hidden Lake boardwalk south.
CAR = (48.6962, -113.7180)


def haversine_m(lat1, lon1, lat2, lon2):
    r = 6371000.0
    p1, p2 = math.radians(lat1), math.radians(lat2)
    dp = math.radians(lat2 - lat1)
    dl = math.radians(lon2 - lon1)
    a = math.sin(dp / 2) ** 2 + math.cos(p1) * math.cos(p2) * math.sin(dl / 2) ** 2
    return 2 * r * math.asin(math.sqrt(a))


def serve(directory, port):
    handler = lambda *a, **kw: http.server.SimpleHTTPRequestHandler(
        *a, directory=str(directory), **kw)
    socketserver.TCPServer.allow_reuse_address = True
    httpd = socketserver.TCPServer(("127.0.0.1", port), handler)
    threading.Thread(target=httpd.serve_forever, daemon=True).start()
    return httpd


def trail(car, n=40):
    """A quarter-circle of walking, ~20 m a step, starting at the car."""
    out = []
    for i in range(n):
        b = math.radians(180 + i * 2.0)
        d = 20.0 * i
        lat = car[0] + (d * math.cos(b)) / 111320.0
        lon = car[1] + (d * math.sin(b)) / (111320.0 * math.cos(math.radians(car[0])))
        out.append((lat, lon))
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=8127)
    ap.add_argument("--headed", action="store_true")
    args = ap.parse_args()

    from playwright.sync_api import sync_playwright

    httpd = serve(WEB_DEMO, args.port)
    base = f"http://127.0.0.1:{args.port}/index.html"
    print(f"serving {WEB_DEMO} at {base}\n")
    failures = []

    with sync_playwright() as p:
        browser = p.chromium.launch(headless=not args.headed)
        page = browser.new_page(viewport={"width": 420, "height": 900})  # a phone
        errors = []
        page.on("pageerror", lambda e: errors.append(str(e)))
        page.on("console",
                lambda m: errors.append(f"console.error: {m.text}") if m.type == "error" else None)

        page.goto(f"{base}#lat={CAR[0]}&lon={CAR[1]}&zoom=14", wait_until="load", timeout=90_000)
        page.wait_for_function("() => !!window.__ptiles && !!window.__ptiles.hikeStart",
                               timeout=40_000)

        # --- the walk ------------------------------------------------------
        page.evaluate("async (c) => await window.__ptiles.hikeStart(c[0], c[1])", list(CAR))
        state = page.evaluate("() => window.__ptiles.hikeState()")
        if not state["visible"]:
            failures.append("trail mode started but the compass screen is not showing")

        pts = trail(CAR)
        walked = sum(haversine_m(*pts[i], *pts[i + 1]) for i in range(len(pts) - 1))
        needles = []
        t0 = state["startedAt"]
        for i, (lat, lon) in enumerate(pts):
            state = page.evaluate(
                "([lat, lon, t]) => window.__ptiles.hikeFix(lat, lon, 6, t)",
                [lat, lon, t0 + i * 4000])
            needles.append(state["needle"])
        print(f"  walked         {state['walked_m']} m over {state['fixes']} fixes, "
              f"{state['points']} recorded")
        print(f"  compass        back={state['back']!r} {state['heading']!r}")

        if abs(state["walked_m"] - walked) > 30:
            failures.append(f"walked {state['walked_m']} m; the trail is {walked:.0f} m")
        if state["points"] < 30:
            failures.append(f"only {state['points']} of {len(pts)} fixes were recorded")
        if len(set(needles)) < 5:
            failures.append("the needle never turned while walking a curve")

        # A fix that has not moved is not a new point.
        before = state["points"]
        state = page.evaluate(
            "([lat, lon, t]) => window.__ptiles.hikeFix(lat, lon, 6, t)",
            [pts[-1][0], pts[-1][1], t0 + len(pts) * 4000])
        if state["points"] != before:
            failures.append("standing still recorded another track point")

        # --- the file ------------------------------------------------------
        gpx = page.evaluate("() => window.__ptiles.hikeGpx()")
        trkpts = gpx.count("<trkpt")
        print(f"  gpx            {len(gpx)} bytes, {trkpts} trkpt, "
              f"{gpx.count('<wpt')} wpt")
        if gpx.count("<wpt") != 1:
            failures.append("the GPX has no waypoint for the car")
        if trkpts != state["points"]:
            failures.append(f"GPX has {trkpts} trkpt for {state['points']} recorded points")
        if "<?xml" not in gpx or "</gpx>" not in gpx:
            failures.append("the GPX is not a well-formed document")

        # --- the refresh ---------------------------------------------------
        # The one thing a hiker cannot redo: the walk they already took.
        page.reload(wait_until="load", timeout=90_000)
        page.wait_for_function("() => !!window.__ptiles && !!window.__ptiles.hikeState",
                               timeout=40_000)
        page.wait_for_timeout(1500)
        after = page.evaluate("() => window.__ptiles.hikeState()")
        print(f"  after reload   on={after['on']} points={after['points']} "
              f"walked={after['walked_m']} m note={after['note']!r}")
        if not after["on"]:
            failures.append("the hike did not resume after a refresh")
        if after["points"] != state["points"]:
            failures.append(
                f"{after['points']} points came back from {state['points']} recorded")
        if not after["car"] or abs(after["car"]["lat"] - CAR[0]) > 1e-6:
            failures.append("the parking spot was lost across the refresh")
        if after["startedAt"] != state["startedAt"]:
            failures.append("the elapsed clock restarted on resume")

        # --- ending --------------------------------------------------------
        ended = page.evaluate("() => window.__ptiles.hikeStop()")
        print(f"  ended          on={ended['on']} stored={ended['stored']}")
        if ended["on"] or ended["stored"] or ended["visible"]:
            failures.append("ending the hike left the session behind")

        page.reload(wait_until="load", timeout=90_000)
        page.wait_for_function("() => !!window.__ptiles && !!window.__ptiles.hikeState",
                               timeout=40_000)
        page.wait_for_timeout(1000)
        if page.evaluate("() => window.__ptiles.hikeState().on"):
            failures.append("an ended hike came back after a refresh")

        browser.close()

    httpd.shutdown()
    real = [e for e in errors if "favicon" not in e]
    if real:
        print("\n  page errors:")
        for e in real[:10]:
            print(f"    {e}")
        failures.extend(real[:10])

    if failures:
        print("\n  FAILURES:")
        for f in failures:
            print(f"    - {f}")
        sys.exit(1)
    print("\n  trail mode ok")


if __name__ == "__main__":
    main()
