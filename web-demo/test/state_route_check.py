#!/usr/bin/env python3
"""Does the map follow the ground it is over, or the bbox it started in?

State files are chosen by bounding box, and bounding boxes overlap. NJ's
reaches -73.89, so all of Manhattan is inside it: a map opened at -74.006
picked NJ and, because the picker preferred whatever state was already loaded,
stayed on NJ across the whole of New York City. Every cell in the viewport was
missing from the NJ index, so the page drew nothing, fetched nothing, and said
nothing -- the silent-empty failure this format keeps producing.

The fix releases that stickiness on evidence: if the loaded index has no entry
for the point, the state is re-chosen without it. This checks the fix from the
outside -- pan into Manhattan, and buildings must appear.

    python3 web-demo/test/state_route_check.py
"""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from render_check import ENABLE, WEB_DEMO, serve  # noqa: E402
from bench_render import SETTLE  # noqa: E402

START = (40.7128, -74.0060, 16)   # Hudson waterfront: inside NJ's bbox
INTO_NY = (40.7900, -73.9300, 16)  # upper Manhattan: NJ holds none of this


def check_pan_into_ny():
    from playwright.sync_api import sync_playwright

    httpd = serve(WEB_DEMO, 0)
    base = f"http://127.0.0.1:{httpd.server_address[1]}/index.html"
    lat, lon, zoom = START

    with sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        page = browser.new_context(viewport={"width": 1400, "height": 900}).new_page()
        page.goto(f"{base}#lat={lat}&lon={lon}&zoom={zoom}", wait_until="load", timeout=90_000)
        page.wait_for_function("() => !!window.__ptiles", timeout=30_000)
        page.wait_for_timeout(2500)
        assert page.evaluate(ENABLE, "chkBldgs"), "no buildings checkbox"
        # Signals on purpose: it is a US-wide file, so it holds the cell
        # wherever the map is. Counting it as coverage would answer "NJ has
        # this" over Manhattan and restore the bug -- with this layer ticked
        # and the guard removed, the assertions below fail.
        assert page.evaluate(ENABLE, "chkSignal"), "no signals checkbox"
        page.click("#btnPtiles")
        for _ in range(300):
            if page.evaluate(SETTLE, "bldgs") > 0:
                break
            page.wait_for_timeout(100)
        started_in = page.evaluate("() => window.__ptiles.state()")

        page.evaluate("([a, b, c]) => window.__ptiles.setView(a, b, c)", list(INTO_NY))
        # The switch reloads every layer, so give it room; poll rather than
        # sleeping a fixed time, or this measures the CDN on a bad day.
        count, state = 0, started_in
        for _ in range(300):
            state = page.evaluate("() => window.__ptiles.state()")
            count = page.evaluate(SETTLE, "bldgs")
            if state == "NY" and count > 0:
                break
            page.wait_for_timeout(100)
        browser.close()

    print(f"opened in {started_in}, panned into Manhattan -> {state}, {count} buildings")
    assert state == "NY", f"still serving {state} over Manhattan"
    assert count > 0, "switched state but drew nothing"
    print("ok: released the wrong state")


def check_no_flap():
    """Standing still must not keep changing the answer.

    The release is driven by a coverage miss, and a miss is recorded against
    whichever state was current when it happened -- so a rule that only ever
    rejects the current state can hand the map back and forth for as long as
    the user leaves it alone. This sits on one point over the NJ/NY line and
    nudges it, which fires moveend without meaningfully moving.
    """
    httpd = serve(WEB_DEMO, 0)
    base = f"http://127.0.0.1:{httpd.server_address[1]}/index.html"
    lat, lon, zoom = START

    from playwright.sync_api import sync_playwright
    with sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        page = browser.new_context(viewport={"width": 1400, "height": 900}).new_page()
        page.goto(f"{base}#lat={lat}&lon={lon}&zoom={zoom}", wait_until="load", timeout=90_000)
        page.wait_for_function("() => !!window.__ptiles", timeout=30_000)
        page.wait_for_timeout(2500)
        page.evaluate(ENABLE, "chkBldgs")
        page.click("#btnPtiles")
        page.wait_for_timeout(4000)

        seen = [page.evaluate("() => window.__ptiles.state()")]
        for i in range(8):
            # Under a metre each time: same cell, same ground, same answer.
            page.evaluate("([a, b, c]) => window.__ptiles.setView(a, b, c)",
                          [lat + 0.000005 * (i % 2), lon, zoom])
            page.wait_for_timeout(1500)
            s = page.evaluate("() => window.__ptiles.state()")
            if s != seen[-1]:
                seen.append(s)
        browser.close()

    print("states while standing still:", " -> ".join(seen))
    # One settling change is legitimate -- the first coverage answer arrives
    # only once an index is open. A second means it is oscillating.
    assert len(seen) <= 2, f"state flapped: {seen}"
    print("ok: no flap")


def main():
    try:
        import playwright.sync_api  # noqa: F401
    except ImportError:
        sys.exit("needs playwright: pip install playwright && playwright install chromium")
    check_pan_into_ny()
    check_no_flap()


if __name__ == "__main__":
    main()
