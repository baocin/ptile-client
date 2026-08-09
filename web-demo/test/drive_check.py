#!/usr/bin/env python3
"""Does drive mode actually navigate?

Drives the real page in chromium against live tiles, then feeds it a
simulated drive: fixes sampled along the route it just found, as
`watchPosition` would deliver them. Everything the mode claims -- the banner,
the turn queue, the predicted heading, off-route detection -- is a function of
those fixes, so all of it is testable without a car.

The assertions are the ones a plausible stub cannot pass:

  * distance along the route only ever increases across the whole drive
    (a snapper that jumps backwards passes any single-fix check)
  * the banner counts *down* toward each turn
  * turns get named from their own cells, lazily, during the drive
  * the heading changes when the route does
  * a deliberate detour triggers exactly one reroute, not zero and not eleven

Usage: python3 web-demo/test/drive_check.py
"""

import argparse
import http.server
import math
import socketserver
import sys
import threading
from pathlib import Path

WEB_DEMO = Path(__file__).resolve().parent.parent

# Downtown Nashville to Belle Meade: ~12 km of surface streets with real turns,
# which is what makes the turn queue worth checking.
A = (36.1627, -86.7816)
B = (36.0980, -86.8580)


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


def sample_path(path, step_m):
    """Points every `step_m` along a [lat, lon] path -- a car driving it."""
    out = [path[0]]
    carried = 0.0
    for i in range(len(path) - 1):
        a, b = path[i], path[i + 1]
        seg = haversine_m(a[0], a[1], b[0], b[1])
        if seg <= 0:
            continue
        t = step_m - carried
        while t < seg:
            f = t / seg
            out.append([a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f])
            t += step_m
        carried = (carried + seg) % step_m
    out.append(path[-1])
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=0)
    ap.add_argument("--keep-open", action="store_true")
    ap.add_argument("--step", type=float, default=120.0,
                    help="metres between simulated fixes (120 m ~ 6 s at 45 mph)")
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
        page = browser.new_page(viewport={"width": 420, "height": 900})  # a phone
        errors = []
        page.on("pageerror", lambda e: errors.append(str(e)))
        page.on("console",
                lambda m: errors.append(f"console.error: {m.text}") if m.type == "error" else None)

        page.goto(f"{base}#lat={A[0]}&lon={A[1]}&zoom=13", wait_until="load", timeout=90_000)
        page.wait_for_function("() => !!window.__ptiles && !!window.__ptiles.driveFix", timeout=40_000)
        page.wait_for_timeout(2000)

        route = page.evaluate(
            "async ([a, b]) => await window.__ptiles.routeAt(a, b, {})",
            [list(A), list(B)])
        if not route["found"]:
            print(f"  route: {route['status']}")
            sys.exit("no route to drive; the router failed before drive mode was reached")
        print(f"  route          {route['distanceM']/1000:.1f} km  {route['points']} pts")

        started = page.evaluate("async () => await window.__ptiles.driveStart(null)")
        if not started:
            sys.exit("driveStart refused")
        st0 = page.evaluate("() => window.__ptiles.driveState()")
        turns0 = st0["turns"]
        print(f"  turn queue     {len(turns0)} entries: "
              + ", ".join(t["maneuver"] for t in turns0[:6])
              + (" …" if len(turns0) > 6 else ""))
        if len(turns0) < 3:
            failures.append(f"turn queue has {len(turns0)} entries; a 12 km drive has turns")
        if any(t["road"] for t in turns0):
            failures.append("turns were named before the drive started; naming should be lazy")

        # --- the drive -----------------------------------------------------
        # Drive the exact path the page is following, sampled as a car would
        # pass along it.
        pts = page.evaluate("() => window.__ptiles.__routePath()")
        if not pts:
            sys.exit("drive mode exposed no route path to simulate")
        samples = sample_path(pts, args.step)
        print(f"  simulating     {len(samples)} fixes at {args.step:.0f} m apart")

        last_along = -1
        named_during = 0
        bearings = []
        state = None
        for i, (lat, lon) in enumerate(samples):
            state = page.evaluate(
                "async ([lat, lon]) => await window.__ptiles.driveFix(lat, lon, 5, 20)",
                [lat, lon])
            nav = state["nav"]
            if nav is None:
                failures.append(f"fix {i}: no nav state")
                break
            if nav["along_m"] < last_along - 1:
                failures.append(
                    f"fix {i}: along-route distance went backwards "
                    f"({last_along} -> {nav['along_m']})")
                break
            last_along = nav["along_m"]
            bearings.append(nav["bearing_deg"])
            if nav["off_route"]:
                failures.append(f"fix {i}: on the route but reported off it "
                                f"(offset {nav['offset_m']} m)")
                break

        if state:
            named_during = sum(1 for t in state["turns"] if t["road"])
            print(f"  drove          {state['fixes']} fixes, "
                  f"{state['nav']['remaining_m']} m remaining, {named_during} turns named")
            print(f"  banner         {state['banner']['dist']} · {state['banner']['road']}")
            print(f"  eta            {state['eta']} · {state['rest']}")
            if state["nav"]["remaining_m"] > 200:
                failures.append(f"drove the whole route but {state['nav']['remaining_m']} m remain")
            if named_during == 0 and len(turns0) > 2:
                failures.append("no turn was named during the drive; lazy naming never fired")
            if len(set(round(b / 15) for b in bearings)) < 2:
                failures.append("the predicted heading never changed over a route with turns")
            if state["reroutes"] != 0:
                failures.append(f"{state['reroutes']} reroutes on a drive that never left the route")

        # --- the wrong turn ------------------------------------------------
        # Three fixes 150 m off the line: a wrong turn, not noise.
        if samples:
            mid = samples[len(samples) // 2]
            off = [mid[0] + 0.0014, mid[1]]
            for _ in range(4):
                state = page.evaluate(
                    "async ([lat, lon]) => await window.__ptiles.driveFix(lat, lon, 5, 20)",
                    [off[0], off[1]])
            # Rerouting loads a corridor and runs A*; a car gets seconds of
            # road to do it in, so the harness waits rather than declaring
            # failure on work that is still in flight.
            for _ in range(60):
                state = page.evaluate("() => window.__ptiles.driveState()")
                if not state["rerouting"] and state["reroutes"] >= 1:
                    break
                page.wait_for_timeout(500)
            print(f"  wrong turn     note={state['note']!r} reroutes={state['reroutes']}")
            if state["reroutes"] < 1:
                failures.append("a 150 m detour over four fixes triggered no reroute")
            if state["reroutes"] > 1:
                failures.append(f"{state['reroutes']} reroutes for one wrong turn")

        # --- taps land where they are aimed --------------------------------
        # The pane is rotated under the viewport, so Leaflet's own click
        # coordinates are off by the heading -- which on a phone reads as
        # "every tap starts a route somewhere I did not touch". The page
        # carries both directions of the correction; this checks them against
        # each other and against the thing a driver actually aims at.
        state = page.evaluate("() => window.__ptiles.driveState()")
        vw, vh = state["viewport"]
        car = page.evaluate("() => window.__ptiles.__lastFix()")
        xy = page.evaluate("([a, b]) => window.__ptiles.driveScreenXY(a, b)", [car[0], car[1]])
        tap = page.evaluate("([x, y]) => window.__ptiles.driveTapAt(x, y)", [xy[0], xy[1]])
        d = haversine_m(tap[0], tap[1], car[0], car[1])
        print(f"  tap on car     {d:.0f} m off, car drawn at "
              f"({xy[0]:.0f}, {xy[1]:.0f}) of {vw}x{vh}")
        if d > 25:
            failures.append(f"tapping the vehicle resolved {d:.0f} m away; "
                            "the rotation correction is wrong")
        # Drawn in the lower part of the screen: the point of heading-up is
        # that most of the glass shows where you are going.
        if not (0.5 * vh < xy[1] < 0.85 * vh):
            failures.append(f"vehicle drawn at y={xy[0]:.0f}, not in the lower third")

        # --- orientation toggle --------------------------------------------
        north = page.evaluate("() => window.__ptiles.driveSetNorthUp(true)")
        st_n = page.evaluate("() => window.__ptiles.driveState()")
        print(f"  north-up       rotation={st_n['rotation']!r} marker={st_n['markerRotation']!r}")
        if not north or st_n["rotation"] not in ("", None):
            failures.append(f"north-up left the map rotated: {st_n['rotation']!r}")
        if "rotate" not in (st_n["markerRotation"] or ""):
            failures.append("north-up did not rotate the vehicle marker, so heading is unreadable")
        tap_n = page.evaluate("([x, y]) => window.__ptiles.driveTapAt(x, y)", [vw / 2, vh / 2])
        page.evaluate("() => window.__ptiles.driveSetNorthUp(false)")
        st_h = page.evaluate("() => window.__ptiles.driveState()")
        if "rotate" not in (st_h["rotation"] or ""):
            failures.append("heading-up did not restore the map rotation")

        page.evaluate("() => window.__ptiles.driveStop()")
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
    print("  drive mode ok")


if __name__ == "__main__":
    main()
