#!/usr/bin/env python3
"""Do the three routing modes actually route?

render_check.py proves the layers draw. This drives the router in the real
page, against the live tile host, and checks the three answers that are easy
to fake and hard to get right:

  driving      a road route exists and is roughly as long as the crow flies
  trails only  a walk exists, and is slower than the drive over the same pair
               of points -- a "walk" at driving speed means the foot profile
               never took effect and the router used posted limits
  ev           a range too small for the leg produces charging stops, and the
               route through them is longer than the direct one

The third is the one a plausible stub passes by accident: a page that ignores
the range box entirely still draws a line, still reports a distance, and only
the stop count and the added distance say whether it planned anything.

Usage: python3 web-demo/test/route_check.py
"""

import argparse
import http.server
import math
import socketserver
import sys
import threading
from pathlib import Path

WEB_DEMO = Path(__file__).resolve().parent.parent

# Downtown Nashville to Percy Warner Park: far enough that a walk and a drive
# take visibly different routes, close enough that both exist.
A = (36.1627, -86.7816)
B = (36.0836, -86.8925)

# Nashville to Manchester down I-24: ~103 km, and 40 miles of range covers
# only 51 km of that once the 20% reserve is taken off, so it needs a stop.
EV_A = (36.1627, -86.7816)
EV_B = (35.4817, -86.0886)
EV_RANGE_MI = 40

# The old sparse corridor failed here: one isolated H3 disk every 12 km did
# not make a connected graph. This is deliberately a plain drive, independent
# of EV planning, so a regression identifies the corridor rather than stops.
LONG_A = EV_A
LONG_B = (35.0456, -85.3097)  # downtown Chattanooga, ~182 km crow distance


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


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=0)
    ap.add_argument("--keep-open", action="store_true")
    args = ap.parse_args()

    try:
        from playwright.sync_api import sync_playwright
    except ImportError:
        sys.exit("needs playwright: pip install playwright && playwright install chromium")

    httpd = serve(WEB_DEMO, args.port)
    base = f"http://127.0.0.1:{httpd.server_address[1]}/index.html"
    print(f"serving {WEB_DEMO} at {base}\n")

    failures = []
    with sync_playwright() as pw:
        browser = pw.chromium.launch(headless=not args.keep_open)
        page = browser.new_page(viewport={"width": 1400, "height": 900})
        errors = []
        page.on("pageerror", lambda e: errors.append(str(e)))
        page.on("console",
                lambda m: errors.append(f"console.error: {m.text}") if m.type == "error" else None)

        page.goto(f"{base}#lat={A[0]}&lon={A[1]}&zoom=12", wait_until="load", timeout=90_000)
        page.wait_for_function("() => !!window.__ptiles && !!window.__ptiles.routeAt",
                               timeout=30_000)
        page.wait_for_timeout(2000)

        def route(a, b, **opts):
            return page.evaluate(
                "async ([a, b, opts]) => await window.__ptiles.routeAt(a, b, opts)",
                [list(a), list(b), opts])

        corridor = page.evaluate(
            "([a, b]) => window.__ptiles.corridorPlan(a, b, 2)",
            [list(LONG_A), list(LONG_B)])
        print(f"  corridor    {corridor['spine']:4d} spine / {corridor['cells']:4d} cells  "
              f"width {corridor['appliedWidth']}/{corridor['requestedWidth']}")
        if not corridor["adjacent"]:
            failures.append("corridor: consecutive spine cells are not adjacent")
        if not corridor["spineKept"]:
            failures.append("corridor: the cell budget removed mandatory spine cells")
        if corridor["cells"] > 400:
            failures.append(f"corridor: {corridor['cells']} cells exceeds the 400-cell budget")

        retry_corridor = page.evaluate(
            "([a, b]) => window.__ptiles.corridorPlan(a, b, 4, 900)",
            [list(LONG_A), list(LONG_B)])
        print(f"  retry band {retry_corridor['spine']:4d} spine / "
              f"{retry_corridor['cells']:4d} cells  width "
              f"{retry_corridor['appliedWidth']}/{retry_corridor['requestedWidth']}")
        if not retry_corridor["adjacent"] or not retry_corridor["spineKept"]:
            failures.append("retry corridor: widening broke the mandatory spine")
        if retry_corridor["cells"] > 900:
            failures.append(
                f"retry corridor: {retry_corridor['cells']} cells exceeds the 900-cell budget")

        crow = haversine_m(*A, *B)

        drive = route(A, B)
        print(f"  driving      {drive['distanceM']/1000:7.1f} km  "
              f"{drive['durationS']/60:6.1f} min  {drive['points']:5d} pts")
        if not drive["found"]:
            failures.append(f"driving: no route ({drive['status']})")
        elif drive["distanceM"] < crow * 0.9:
            failures.append(f"driving: {drive['distanceM']:.0f} m is shorter than the "
                            f"straight line ({crow:.0f} m)")

        walk = route(A, B, trailsOnly=True)
        print(f"  trails only  {walk['distanceM']/1000:7.1f} km  "
              f"{walk['durationS']/60:6.1f} min  {walk['points']:5d} pts")
        if not walk["found"]:
            failures.append(f"trails only: no route ({walk['status']})")
        else:
            # 5 km/h vs 60-ish: the walk must be many times slower over a
            # comparable distance. This is the assertion a driving profile
            # wearing a trails label cannot pass.
            drive_speed = drive["distanceM"] / max(drive["durationS"], 1)
            walk_speed = walk["distanceM"] / max(walk["durationS"], 1)
            print(f"               drive {drive_speed*3.6:.0f} km/h vs walk {walk_speed*3.6:.0f} km/h")
            if walk_speed > 2.5:  # 9 km/h -- faster than anyone walks
                failures.append(f"trails only: {walk_speed*3.6:.0f} km/h is not walking speed")

        long_drive = route(LONG_A, LONG_B)
        print(f"  long drive   {long_drive['distanceM']/1000:7.1f} km  "
              f"{long_drive['durationS']/60:6.1f} min  {long_drive['points']:5d} pts")
        print(f"               {long_drive['status']} · {long_drive.get('corridor')}")
        if not long_drive["found"]:
            failures.append(
                f"long driving: no Nashville-Chattanooga route "
                f"({long_drive.get('failure')}; {long_drive['status']})")

        ev_crow = haversine_m(*EV_A, *EV_B)
        plain = route(EV_A, EV_B)
        print(f"  ev: no range {plain['distanceM']/1000:7.1f} km  "
              f"{plain['chargeStops']} stops")
        ev = route(EV_A, EV_B, evRangeMi=EV_RANGE_MI)
        print(f"  ev: {EV_RANGE_MI} mi    {ev['distanceM']/1000:7.1f} km  "
              f"{ev['chargeStops']} stops")
        print(f"               {ev['status']}")
        print(f"               plan: {ev.get('plan')}")
        if not ev["found"]:
            failures.append(f"ev: no route ({ev['status']})")
        elif ev["chargeStops"] == 0:
            failures.append(
                f"ev: {EV_RANGE_MI} mi of range over {ev_crow/1609:.0f} mi planned no stops "
                f"({ev['status']})")
        elif ev["distanceM"] <= plain["distanceM"]:
            failures.append("ev: routing through the stops did not change the route, so "
                            "the stops are decoration")
        elif "EV:" not in ev["status"]:
            # The stops are on the map and the route goes through them, but the
            # status line is the plain route text: nothing on screen says the
            # drive was planned around a battery.
            failures.append(f"ev: nothing in the status says charging was planned "
                            f"({ev['status']})")

        for e in errors[:5]:
            failures.append(e)
        if not args.keep_open:
            page.close()
            browser.close()

    print()
    if failures:
        for f in failures:
            print(f"  FAIL {f}")
        sys.exit(1)
    print("  all routing modes ok")


if __name__ == "__main__":
    main()
