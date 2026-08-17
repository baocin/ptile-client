#!/usr/bin/env python3
"""Rank the ptiles layers by how long they take to draw, and say where the time
goes.

GOAL.md quotes per-layer time-to-stable-render figures that no script in this
repo produces: render_check.py counts features and never looks at a clock, and
the two micro-benchmarks measure the wasm boundary, which GOAL.md itself says
does not transfer to a real page. So every optimisation claim made against
those numbers is unfalsifiable. This is the missing measurement.

Per layer, at a fixed viewport, it reports:

  * wall time from PTILES Mode to a *stable* feature count -- stable for three
    consecutive samples, not merely non-zero. Breaking on the first non-zero
    reading samples mid-render and flatters whichever layer streams first.
  * HTTP requests and bytes over the wire.
  * where that wall time went: network, zstd, record decode, and the residual
    (Leaflet plus the page's own JS).

Cold and warm are both measured, because they answer different questions. Cold
is a fresh browser context -- no HTTP cache, no block cache, what a first
visitor pays. Warm re-renders the same layer in the same page, which is what
panning costs.

Every configuration runs three times and reports the median with the spread.
A single number over a live CDN is noise; the spread is the honest part.

    python3 web-demo/test/bench_render.py                 # Manhattan, all layers
    python3 web-demo/test/bench_render.py --view nashville
    python3 web-demo/test/bench_render.py --runs 5 --layers bldgs,roads
"""
import argparse
import json
import statistics
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from render_check import ENABLE, WEB_DEMO, serve  # noqa: E402

# Buildings is the layer worth ranking against, and it only shows its cost
# where there are a lot of them. Lower Manhattan is the densest published
# ground in the corpus; Nashville is kept because every other measurement in
# GOAL.md was taken there and comparisons need a shared point.
VIEWS = {
    "manhattan": (40.7128, -74.0060, 16),
    "nashville": (36.1627, -86.7816, 16),
    "downtown-la": (34.0430, -118.2530, 16),
}

LAYERS = [
    ("roads", "chkRoads"),
    ("water", "chkWater"),
    ("bldgs", "chkBldgs"),
    ("parks", "chkParks"),
    ("rail", "chkRail"),
    ("ev", "chkEv"),
    ("signal", "chkSignal"),
    ("camera", "chkCamera"),
]

# Leaflet's own map object is not exported, but the page keeps the view in the
# URL hash and re-reads it, so a hashchange is the supported way to move it.
PAN = """([lat, lon, zoom]) => window.__ptiles.setView(lat, lon, zoom)"""

SETTLE = """(key) => {
  const c = window.__ptiles.featureCounts()[key];
  return c ? c.features : -1;
}"""


def settle(page, key, t0, timeout_s=60, require_change=False):
    """Feature count once it stops moving, and when it *last changed*.

    Returning the time the loop finished confirming would bill every layer for
    the three stable samples it takes to be sure -- 300 ms of pure harness, on
    a measurement where the fastest layer is ~300 ms warm. What is wanted is
    the moment the count reached its final value, which is when the user sees
    a finished map.
    """
    last, stable, changed_at = -1, 0, t0
    seen_change = not require_change
    deadline = time.perf_counter() + timeout_s
    while time.perf_counter() < deadline:
        n = page.evaluate(SETTLE, key)
        if n == last and n > 0:
            stable += 1
            # `require_change` exists because renderPtilesForCells debounces
            # 600 ms: for the first samples after a pan the count still reads
            # its old value, three stable samples land immediately, and the
            # measurement comes back as ~12 ms of nothing happening.
            if stable >= 3 and seen_change:
                return n, changed_at - t0
        else:
            stable = 0
            if n != last:
                changed_at = time.perf_counter()
                if last != -1:
                    seen_change = True
        last = n
        page.wait_for_timeout(100)
    return last, changed_at - t0


def perf(page):
    return page.evaluate("() => window.__ptiles.perf(null)")


def delta(after, before):
    return {k: (after.get(k) or 0) - (before.get(k) or 0)
            for k in ("requests", "bytes", "netWallMs", "zstdMs", "decodeMs", "blocks")}


def measure(browser, base, view, key, checkbox):
    """One cold render and one warm re-render of a single layer."""
    lat, lon, zoom = view
    # A fresh context is the only honest "cold": a new page in the same context
    # keeps the HTTP cache, which is most of what cold is measuring.
    ctx = browser.new_context(viewport={"width": 1400, "height": 900})
    page = ctx.new_page()
    errors = []
    page.on("pageerror", lambda e: errors.append(str(e)))

    page.goto(f"{base}#lat={lat}&lon={lon}&zoom={zoom}", wait_until="load", timeout=90_000)
    page.wait_for_function("() => !!window.__ptiles", timeout=30_000)
    page.wait_for_timeout(2500)

    # The clock starts at the tick, not at the PTILES click: ticking the box is
    # what opens the layer and fetches its index, and that is on the layer's
    # bill. Timing from the click alone reported a network total larger than
    # the wall time it was supposedly inside -- roads: 4,430 ms of network in a
    # 1,994 ms render.
    before = perf(page)
    t0 = time.perf_counter()
    if not page.evaluate(ENABLE, checkbox):
        ctx.close()
        return None, f"{key}: no checkbox #{checkbox}"
    page.wait_for_timeout(300)

    # PTILES Mode gates all rendering -- without this click a layer fetches its
    # index, never requests a block, and reports zero features with no error.
    page.click("#btnPtiles")
    cold_n, cold_s = settle(page, key, t0)
    cold = delta(perf(page), before)
    cold["wall_ms"] = round(cold_s * 1000)
    cold["features"] = cold_n

    # Warm: pan ~10 km to ground this page has not drawn yet. Two rejected
    # alternatives, both of which measured the harness rather than the page:
    # toggling the checkbox re-adds a Leaflet group whose geometry already
    # exists (4 ms), and panning away and back finds an unchanged feature
    # count, so there is no transition to time (13 ms). Panning to new cells
    # always changes the count, and it is what moving the map actually costs
    # once the file's header, dictionary and index are already in hand.
    before = perf(page)
    t0 = time.perf_counter()
    page.evaluate(PAN, [lat + 0.09, lon + 0.09, zoom])
    warm_n, warm_s = settle(page, key, t0, require_change=True)
    warm = delta(perf(page), before)
    warm["wall_ms"] = round(warm_s * 1000)
    warm["features"] = warm_n

    ctx.close()
    return {"cold": cold, "warm": warm}, (errors[0] if errors else None)


def med(values):
    return round(statistics.median(values)) if values else 0


def spread(values):
    return f"{min(values):.0f}-{max(values):.0f}" if values else "-"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--view", default="manhattan", choices=sorted(VIEWS))
    ap.add_argument("--runs", type=int, default=3)
    ap.add_argument("--layers", default="all")
    ap.add_argument("--port", type=int, default=0)
    ap.add_argument("--json", help="write the raw per-run readings here")
    args = ap.parse_args()

    try:
        from playwright.sync_api import sync_playwright
    except ImportError:
        sys.exit("needs playwright: pip install playwright && playwright install chromium")

    wanted = LAYERS if args.layers == "all" else [
        l for l in LAYERS if l[0] in args.layers.split(",")]
    view = VIEWS[args.view]

    httpd = serve(WEB_DEMO, args.port)
    base = f"http://127.0.0.1:{httpd.server_address[1]}/index.html"
    print(f"serving {WEB_DEMO} at {base}")
    print(f"view {args.view} {view}, {args.runs} runs per layer\n")

    raw = {}
    with sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        for key, checkbox in wanted:
            runs = []
            for i in range(args.runs):
                got, err = measure(browser, base, view, key, checkbox)
                if got is None:
                    print(f"  {key}: {err}")
                    break
                runs.append(got)
                print(f"  {key} run {i + 1}: cold {got['cold']['wall_ms']} ms "
                      f"({got['cold']['features']} features, "
                      f"{got['cold']['bytes'] / 1024:.0f} KiB, "
                      f"{got['cold']['requests']} req), "
                      f"warm {got['warm']['wall_ms']} ms")
            if runs:
                raw[key] = runs
        browser.close()

    if args.json:
        Path(args.json).write_text(json.dumps(raw, indent=2))

    rows = []
    for key, runs in raw.items():
        cold = [r["cold"] for r in runs]
        warm = [r["warm"] for r in runs]
        rows.append({
            "layer": key,
            "features": med([c["features"] for c in cold]),
            "cold_ms": med([c["wall_ms"] for c in cold]),
            "cold_spread": spread([c["wall_ms"] for c in cold]),
            "warm_ms": med([w["wall_ms"] for w in warm]),
            "req": med([c["requests"] for c in cold]),
            "kib": med([c["bytes"] for c in cold]) // 1024,
            "net": med([c["netWallMs"] for c in cold]),
            "zstd": med([c["zstdMs"] for c in cold]),
            "decode": med([c["decodeMs"] for c in cold]),
        })
    rows.sort(key=lambda r: -r["cold_ms"])

    print(f"\n{'layer':8s} {'cold ms':>8s} {'spread':>10s} {'warm ms':>8s} "
          f"{'feats':>7s} {'req':>4s} {'KiB':>7s} {'net':>6s} {'zstd':>6s} "
          f"{'decode':>7s} {'rest':>6s}")
    for r in rows:
        rest = max(0, r["cold_ms"] - r["net"] - r["zstd"] - r["decode"])
        print(f"{r['layer']:8s} {r['cold_ms']:8d} {r['cold_spread']:>10s} "
              f"{r['warm_ms']:8d} {r['features']:7d} {r['req']:4d} {r['kib']:7d} "
              f"{r['net']:6d} {r['zstd']:6d} {r['decode']:7d} {rest:6d}")

    if rows:
        worst = rows[0]
        print(f"\nslowest: {worst['layer']} at {worst['cold_ms']} ms cold "
              f"for {worst['features']} features")


if __name__ == "__main__":
    main()
