#!/usr/bin/env python3
"""Does the coarse index actually avoid fetching the whole index?

Opening a layer the ordinary way pulls its entire index first -- 4014 KiB for
US.signals -- because entries are only locatable by position. Builders now
write a sampled index to the header's `aux` region, so a point lookup can
fetch ~5 KiB of samples plus one short run of the real index instead.

This serves a locally built file (the published ones predate aux, so the
reader would silently fall back against them) and compares bytes pulled by
each path for the same lookup.

Needs `python3 scripts/build_points.py --states DC` in ~/kino/projects/ptiles
to have been run. Skips cleanly if that file is absent.

Usage:
    python3 -m http.server 8899 --bind 127.0.0.1   # from demo/
    python3 demo/test/coarse_check.py
"""
import http.server
import os
import re
import shutil
import socketserver
import sys
import threading
import urllib.request
from pathlib import Path

from playwright.sync_api import sync_playwright


class RangeHandler(http.server.SimpleHTTPRequestHandler):
    """Serve files with real Range support and CORS.

    `python3 -m http.server` ignores Range entirely and answers 200 with the
    whole file, so measuring "bytes pulled" against it compares two identical
    full downloads. That is how the first version of this test managed to
    report the coarse path pulling *more* than the full one.
    """

    def do_GET(self):
        path = self.translate_path(self.path)
        if not os.path.isfile(path):
            self.send_error(404)
            return
        size = os.path.getsize(path)
        rng = self.headers.get("Range")
        m = re.match(r"bytes=(\d+)-(\d*)", rng or "")
        with open(path, "rb") as f:
            if m:
                start = int(m.group(1))
                end = int(m.group(2)) if m.group(2) else size - 1
                end = min(end, size - 1)
                f.seek(start)
                body = f.read(end - start + 1)
                self.send_response(206)
                self.send_header("Content-Range", f"bytes {start}-{end}/{size}")
            else:
                body = f.read()
                self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Accept-Ranges", "bytes")
        self.send_header("ETag", '"coarse-fixture"')
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Access-Control-Expose-Headers", "Content-Range, ETag")
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *a):
        pass


def serve(directory):
    handler = lambda *a, **kw: RangeHandler(*a, directory=str(directory), **kw)
    httpd = socketserver.ThreadingTCPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=httpd.serve_forever, daemon=True).start()
    return httpd, httpd.server_address[1]

DEMO = Path(__file__).resolve().parent.parent
TILES = Path("/home/aoi/kino/projects/ptiles/tiles")
# The national file has 108k entries, so sampling actually matters. DC has 54
# and its whole index is 2 KiB -- nothing to save.
NATIONAL = TILES / "US.signals.ptiles"
FALLBACK = TILES / "DC.signals.ptiles"
PAGE = "http://127.0.0.1:8899/index.html"

# Downtown Nashville for the national file, downtown DC for the fallback.
LAT, LON = 36.1627, -86.7816
FALLBACK_LATLON = (38.9007, -77.0377)


def bytes_pulled(reqs):
    return sum(n for _, n in reqs)


def run(fixture_url):
    global FIXTURE_URL
    FIXTURE_URL = fixture_url
    with sync_playwright() as p:
        browser = p.chromium.launch()
        page = browser.new_page()
        pulled = []
        page.on("response", lambda r: pulled.append(
            (r.url, int(r.headers.get("content-length") or 0)))
            if fixture_url in r.url else None)

        page.goto(PAGE, wait_until="load", timeout=90_000)
        page.wait_for_function("() => !!window.__ptiles", timeout=30_000)
        page.wait_for_timeout(1500)

        # Coarse path
        pulled.clear()
        coarse = page.evaluate("""async ([u, la, lo]) => {
          const r = await window.__ptiles.openCoarse(u);
          if (!r) return null;
          const cell = BigInt("0x" + h3.latLngToCell(la, lo, 7)) & 0xffffffffffe00000n;
          const bytes = await r.decompressCell(cell);
          return { samples: r.coarse.cells.length, stride: r.coarse.stride,
                   entryCount: r.coarse.entryCount, got: bytes ? bytes.length : 0 };
        }""", [FIXTURE_URL, LAT, LON])
        page.wait_for_timeout(800)
        coarse_reqs = list(pulled)

        # Full path, fresh page so nothing is cached from the run above
        page.close()
        page = browser.new_page()
        pulled2 = []
        page.on("response", lambda r: pulled2.append(
            (r.url, int(r.headers.get("content-length") or 0)))
            if fixture_url in r.url else None)
        page.goto(PAGE, wait_until="load", timeout=90_000)
        page.wait_for_function("() => !!window.__ptiles", timeout=30_000)
        page.wait_for_timeout(1500)
        pulled2.clear()
        full = page.evaluate("""async ([u, la, lo]) => {
          const f = await window.__ptiles.openRemote(u);
          return { entries: f.entries.length };
        }""", [FIXTURE_URL, LAT, LON])
        page.wait_for_timeout(800)
        full_reqs = list(pulled2)
        browser.close()

    if coarse is None:
        print("FAIL: the fixture has no coarse index -- rebuild it")
        return 1

    cb, fb = bytes_pulled(coarse_reqs), bytes_pulled(full_reqs)
    print(f"fixture: {FIXTURE.name} ({FIXTURE.stat().st_size:,} B on disk)")
    print(f"  coarse index: {coarse['samples']} samples, stride {coarse['stride']}, "
          f"{coarse['entryCount']:,} entries")
    print()
    print(f"  {'path':8s} {'requests':>9s} {'bytes pulled':>13s}")
    print("  " + "-" * 34)
    print(f"  {'coarse':8s} {len(coarse_reqs):>9d} {cb:>13,d}")
    print(f"  {'full':8s} {len(full_reqs):>9d} {fb:>13,d}")

    ok = True
    if coarse["got"] <= 0:
        print("\nFAIL: coarse lookup returned no records for a cell that has them")
        ok = False
    if cb >= fb:
        print(f"\nFAIL: coarse path pulled {cb:,} B, no better than the full path's {fb:,} B")
        ok = False
    if ok:
        print(f"\ncoarse lookup pulled {fb - cb:,} B less ({cb / fb:.0%} of the full open) "
              f"and decoded {coarse['got']} bytes of records")
    return 0 if ok else 1


if __name__ == "__main__":
    FIXTURE = NATIONAL if NATIONAL.exists() else FALLBACK
    if not FIXTURE.exists():
        print(f"skipping: neither {NATIONAL.name} nor {FALLBACK.name} is built")
        sys.exit(0)
    if FIXTURE is FALLBACK:
        LAT, LON = FALLBACK_LATLON
        print(f"note: using {FIXTURE.name}; its index is small, so the saving "
              f"will look modest")
    try:
        with urllib.request.urlopen(PAGE, timeout=10) as r:
            if r.status != 200:
                sys.exit(f"server returned {r.status}")
    except Exception as e:
        sys.exit(f"cannot reach the demo: {e}\n"
                 f"run: python3 -m http.server 8899 --bind 127.0.0.1   (from demo/)")
    httpd, port = serve(FIXTURE.parent)
    try:
        sys.exit(run(f"http://127.0.0.1:{port}/{FIXTURE.name}"))
    finally:
        httpd.shutdown()
