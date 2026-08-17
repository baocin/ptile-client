#!/usr/bin/env python3
"""What panning and zooming actually cost, per layer.

bench_render.py measures a first render. Its "warm" column was worse than
useless: it panned, waited for the feature count to settle, and reported ~13 ms
without ever checking that a redraw had happened. If a move loads no new cells
the count never moves, the settle loop returns immediately, and the harness
reports the time it took to ask -- not the time it took to draw. Every number
here is therefore gated on evidence that work occurred: new HTTP requests, new
blocks, or a changed feature count. A move that produces none is reported as
"no redraw", never as a fast one.

Moves, in the order a user makes them:

  pan-near   ~60% of a viewport east -- mostly new cells, some already drawn
  pan-far    ~4 viewports away       -- all new ground, cold blocks
  zoom-in    one level               -- same ground, fewer cells, redraw
  zoom-out   one level               -- more cells, more features on screen
  revisit    back to the start       -- everything cached; the true warm case

Cost is split into the wait before anything happens (the 600 ms debounce in
renderPtilesForCells is most of it) and the work after, so a slow move can be
told from a late one.

    python3 web-demo/test/bench_pan.py --layers bldgs
    python3 web-demo/test/bench_pan.py --view manhattan --runs 3
"""
import argparse
import json
import statistics
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from bench_render import PAN, SETTLE, VIEWS, perf, delta  # noqa: E402
from render_check import ENABLE, WEB_DEMO, serve  # noqa: E402

# Degrees at the benchmark zoom. A 1400px viewport at z16 spans roughly 0.02
# degrees of longitude, so "near" is a little over half a screen and "far" is
# ground this page has never drawn.
NEAR_DEG = 0.012
FAR_DEG = 0.08

MOVES = ["pan-near", "pan-far", "zoom-in", "zoom-out", "revisit"]

# Redraw cost is invisible to the network counters: zooming back to ground the
# page has already drawn issues no request and changes no feature count, yet
# Leaflet still rebuilds every path. This watches the main thread instead --
# long tasks (>50 ms of blocked JS) and the worst frame gap -- so "no data
# work" and "no work" stop being the same reading.
FRAME_PROBE = """
window.__frames = { longTasks: [], gaps: [], last: performance.now() };
try {
  new PerformanceObserver((list) => {
    for (const e of list.getEntries()) window.__frames.longTasks.push(e.duration);
  }).observe({ entryTypes: ["longtask"] });
} catch (e) { /* long-task timing is chromium-only; gaps still work */ }
(function tick() {
  const now = performance.now();
  window.__frames.gaps.push(now - window.__frames.last);
  window.__frames.last = now;
  requestAnimationFrame(tick);
})();
"""

FRAME_RESET = """() => {
  window.__frames.longTasks = [];
  window.__frames.gaps = [];
  window.__frames.last = performance.now();
}"""

FRAME_READ = """() => ({
  longTaskMs: Math.round(window.__frames.longTasks.reduce((a, b) => a + b, 0)),
  longTasks: window.__frames.longTasks.length,
  maxGapMs: Math.round(Math.max(0, ...window.__frames.gaps)),
})"""


# A pan across a state line reloads every layer, and a layer that is still
# loading reports zero features -- three zeros in a row look exactly like a
# settled count. Panning into Manhattan therefore reported 0 features and a
# 338 ms "redraw" that was really the harness sampling mid-switch.
LOADING = """(key) => {
  const c = window.__ptiles.featureCounts()[key];
  return !c || c.loading || !c.hasReader;
}"""


def wait_for_work(page, key, baseline, before_count, timeout_s=15):
    """Block until the page visibly starts redrawing. Returns seconds waited,
    or None if nothing happened -- which is a result, not a failure."""
    t0 = time.perf_counter()
    while time.perf_counter() - t0 < timeout_s:
        d = delta(perf(page), baseline)
        if d["requests"] > 0 or d["blocks"] > 0:
            return time.perf_counter() - t0
        if page.evaluate(SETTLE, key) != before_count:
            return time.perf_counter() - t0
        page.wait_for_timeout(50)
    return None


def settle_after_work(page, key, timeout_s=60):
    """Feature count once it stops moving, measured from now."""
    t0 = time.perf_counter()
    last, stable, changed_at = None, 0, t0
    while time.perf_counter() - t0 < timeout_s:
        if page.evaluate(LOADING, key):
            stable, changed_at, last = 0, time.perf_counter(), None
            page.wait_for_timeout(100)
            continue
        n = page.evaluate(SETTLE, key)
        if n == last:
            stable += 1
            # Zero is the count a reloaded layer reports in the gap between its
            # reader arriving and the render running, and 300 ms of it looks
            # settled. Nothing draws zero features on purpose here, so make zero
            # earn 3 s before it counts as an answer.
            if stable >= (3 if n else 30):
                return n, changed_at - t0
        else:
            stable = 0
            changed_at = time.perf_counter()
        last = n
        page.wait_for_timeout(100)
    return last, changed_at - t0


def do_move(page, key, move, view):
    """One move, fully measured. Returns None when nothing redrew."""
    lat, lon, zoom = view
    target = {
        "pan-near": (lat, lon + NEAR_DEG, zoom),
        "pan-far": (lat + FAR_DEG, lon + FAR_DEG, zoom),
        "zoom-in": (lat, lon, zoom + 1),
        "zoom-out": (lat, lon, zoom - 1),
        "revisit": (lat, lon, zoom),
    }[move]

    before_count = page.evaluate(SETTLE, key)
    before_state = page.evaluate("() => window.__ptiles.state()")
    baseline = perf(page)
    page.evaluate(FRAME_RESET)
    t0 = time.perf_counter()
    page.evaluate(PAN, list(target))

    waited = wait_for_work(page, key, baseline, before_count)
    if waited is None:
        # No new data. Leaflet may still have rebuilt every path, so give it a
        # moment and report what the main thread did rather than calling it
        # nothing.
        page.wait_for_timeout(1500)
        frames = page.evaluate(FRAME_READ)
        return {"move": move, "redrew": False, "features": before_count, **frames}

    n, settle_s = settle_after_work(page, key)
    d = delta(perf(page), baseline)
    frames = page.evaluate(FRAME_READ)
    total_ms = round((time.perf_counter() - t0) * 1000)
    after_state = page.evaluate("() => window.__ptiles.state()")
    return {
        "move": move,
        "redrew": True,
        # A move that crossed a state line paid for a whole layer reload, not a
        # pan; its delta_features is the new state's total, not what the move
        # added. Say so rather than letting it be read as pan cost.
        "switched": None if after_state == before_state else f"{before_state}>{after_state}",
        **frames,
        "wait_ms": round(waited * 1000),
        "draw_ms": round(settle_s * 1000),
        "total_ms": total_ms,
        "features": n,
        "delta_features": n - before_count,
        "requests": d["requests"],
        "kib": round(d["bytes"] / 1024),
        "zstd_ms": d["zstdMs"],
        "decode_ms": d["decodeMs"],
    }


def run_layer(browser, base, view, key, checkbox):
    lat, lon, zoom = view
    ctx = browser.new_context(viewport={"width": 1400, "height": 900})
    page = ctx.new_page()
    page.add_init_script(FRAME_PROBE)
    page.goto(f"{base}#lat={lat}&lon={lon}&zoom={zoom}", wait_until="load", timeout=90_000)
    page.wait_for_function("() => !!window.__ptiles", timeout=30_000)
    page.wait_for_timeout(2500)
    if not page.evaluate(ENABLE, checkbox):
        ctx.close()
        return None
    page.click("#btnPtiles")
    # Let the first render *actually* finish. settle_after_work accepts a
    # stable zero, so calling it alone returned immediately with nothing drawn
    # and billed the first render to the first pan.
    for _ in range(600):
        if page.evaluate(SETTLE, key) > 0:
            break
        page.wait_for_timeout(100)
    settle_after_work(page, key)

    # Which state file is answering. A view near a state line routes to the
    # neighbour (Manhattan is inside NJ's bbox), whose file holds none of the
    # ground being panned over, so every move reports "no data" and the
    # harness looks broken when the routing is what is wrong.
    serving = page.evaluate("() => window.__ptiles.state()")

    out = [do_move(page, key, m, view) for m in MOVES]
    ctx.close()
    return out, serving


def med(vals):
    return round(statistics.median(vals)) if vals else 0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--view", default="manhattan", choices=sorted(VIEWS))
    ap.add_argument("--runs", type=int, default=3)
    ap.add_argument("--layers", default="bldgs,roads,signal")
    ap.add_argument("--port", type=int, default=0)
    ap.add_argument("--json")
    args = ap.parse_args()

    try:
        from playwright.sync_api import sync_playwright
    except ImportError:
        sys.exit("needs playwright: pip install playwright && playwright install chromium")

    boxes = {"roads": "chkRoads", "water": "chkWater", "bldgs": "chkBldgs",
             "parks": "chkParks", "rail": "chkRail", "ev": "chkEv",
             "signal": "chkSignal", "camera": "chkCamera"}
    wanted = [(k, boxes[k]) for k in args.layers.split(",") if k in boxes]
    view = VIEWS[args.view]

    httpd = serve(WEB_DEMO, args.port)
    base = f"http://127.0.0.1:{httpd.server_address[1]}/index.html"
    print(f"view {args.view} {view}, {args.runs} runs\n", flush=True)

    raw = {}
    with sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        for key, checkbox in wanted:
            runs = []
            for i in range(args.runs):
                res = run_layer(browser, base, view, key, checkbox)
                if res is None:
                    print(f"  {key}: no checkbox", flush=True)
                    break
                got, serving = res
                runs.append(got)
                shown = " ".join(
                    (f"{m['move']}={m['total_ms']}ms"
                     + (f"({m['switched']})" if m.get("switched") else ""))
                    if m["redrew"] else f"{m['move']}=none"
                    for m in got)
                print(f"  {key} run {i + 1} [{serving}]: {shown}", flush=True)
            if runs:
                raw[key] = runs
        browser.close()

    if args.json:
        Path(args.json).write_text(json.dumps(raw, indent=2))

    print(f"\n{'layer':8s} {'move':10s} {'total':>7s} {'wait':>6s} {'draw':>6s} "
          f"{'req':>4s} {'KiB':>6s} {'decode':>7s} {'block':>6s} {'gap':>5s} "
          f"{'feats':>7s} {'d.feats':>8s}")
    for key, runs in raw.items():
        for i, move in enumerate(MOVES):
            got = [r[i] for r in runs if r[i]["redrew"]]
            if not got:
                idle = [r[i] for r in runs]
                print(f"{key:8s} {move:10s} {'no data':>7s} {'':6s} {'':6s} {'':4s} "
                      f"{'':6s} {'':7s} {med([g.get('longTaskMs', 0) for g in idle]):6d} "
                      f"{med([g.get('maxGapMs', 0) for g in idle]):5d} "
                      f"{med([g.get('features', 0) for g in idle]):7d}")
                continue
            print(f"{key:8s} {move:10s} "
                  f"{med([g['total_ms'] for g in got]):7d} "
                  f"{med([g['wait_ms'] for g in got]):6d} "
                  f"{med([g['draw_ms'] for g in got]):6d} "
                  f"{med([g['requests'] for g in got]):4d} "
                  f"{med([g['kib'] for g in got]):6d} "
                  f"{med([g['decode_ms'] for g in got]):7d} "
                  f"{med([g.get('longTaskMs', 0) for g in got]):6d} "
                  f"{med([g.get('maxGapMs', 0) for g in got]):5d} "
                  f"{med([g['features'] for g in got]):7d} "
                  f"{med([g['delta_features'] for g in got]):8d}")


if __name__ == "__main__":
    main()
