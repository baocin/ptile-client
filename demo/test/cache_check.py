#!/usr/bin/env python3
"""Cold vs warm open cost, and proof the warm load doesn't refetch.

Opening one layer costs ~4.5 MB against the live host -- header 256 B, a
512 KiB zstd dictionary, and a 4014 KiB index -- and none of it used to survive
a reload. This measures what a second visit actually costs now, and asserts the
dict and index are served from the Cache API rather than the network.

The ETag is part of the cache key, so a rebuilt file misses instead of serving
stale bytes. `etag_change_busts_cache` covers that by rewriting the key.

Usage:
    python3 -m http.server 8899 --bind 127.0.0.1   # from demo/
    python3 demo/test/cache_check.py
"""
import sys
import urllib.request

from playwright.sync_api import sync_playwright

URL = "http://127.0.0.1:8899/index.html#lat=36.1627&lon=-86.7816&zoom=14"
# Signals is the worst case: 4014 KiB index, 108,166 entries.
LAYER_URL = "https://maps.mydatatimeline.com/maps/US.signals.ptiles"

OPEN_JS = """(u) => {
  const t0 = performance.now();
  return window.__ptiles.openRemote(u).then(f => ({
    ms: Math.round(performance.now() - t0),
    entries: f.entries.length,
    dictBytes: f.dict.length,
  }));
}"""


def ranges_for(reqs, url):
    """Bytes actually pulled over the network for that file."""
    return [r for r in reqs if r == url]


def run():
    with sync_playwright() as p:
        browser = p.chromium.launch()
        ctx = browser.new_context()  # one context => one Cache Storage
        results = {}

        for phase in ("cold", "warm"):
            page = ctx.new_page()
            got = []
            page.on("request", lambda r: got.append(r.url) if "ptiles" in r.url else None)
            page.goto(URL, wait_until="load", timeout=90_000)
            page.wait_for_function("() => !!window.__ptiles", timeout=30_000)
            page.wait_for_timeout(1500)
            got.clear()  # only count what the open itself pulls

            info = page.evaluate(OPEN_JS, LAYER_URL)
            page.wait_for_timeout(1500)
            results[phase] = {**info, "requests": len(ranges_for(got, LAYER_URL))}
            page.close()

        # A changed ETag must miss: rewrite the stored keys to a bogus tag and
        # confirm the next open goes back to the network.
        page = ctx.new_page()
        page.goto(URL, wait_until="load", timeout=90_000)
        page.wait_for_function("() => !!window.__ptiles", timeout=30_000)
        moved = page.evaluate("""async () => {
          const c = await caches.open("ptiles-regions-v1");
          const keys = await c.keys();
          let n = 0;
          for (const k of keys) {
            const body = await (await c.match(k)).arrayBuffer();
            await c.put(k.url.replace(/\\/[^/]+\\/(\\d+-\\d+)$/, "/STALE-ETAG/$1"), new Response(body));
            await c.delete(k);
            n++;
          }
          return n;
        }""")
        got = []
        page.on("request", lambda r: got.append(r.url) if "ptiles" in r.url else None)
        info = page.evaluate(OPEN_JS, LAYER_URL)
        page.wait_for_timeout(1500)
        results["stale-etag"] = {**info, "requests": len(ranges_for(got, LAYER_URL)),
                                 "rekeyed": moved}
        page.close()
        browser.close()

    print(f"layer: {LAYER_URL}\n")
    print(f"  {'phase':11s} {'open ms':>8s} {'range GETs':>11s}  entries")
    print("  " + "-" * 46)
    for k in ("cold", "warm", "stale-etag"):
        r = results[k]
        print(f"  {k:11s} {r['ms']:>8d} {r['requests']:>11d}  {r['entries']:,}")

    cold, warm, stale = results["cold"], results["warm"], results["stale-etag"]
    ok = True
    if warm["requests"] > cold["requests"]:
        print("\nFAIL: warm load issued more range requests than cold")
        ok = False
    if warm["requests"] > 1:
        print(f"\nFAIL: warm load made {warm['requests']} range requests; only the "
              f"256-byte header should hit the network")
        ok = False
    if stale["requests"] <= warm["requests"]:
        print(f"\nFAIL: a changed ETag must refetch, but stale-etag made "
              f"{stale['requests']} requests vs warm {warm['requests']}")
        ok = False
    if cold["entries"] != warm["entries"]:
        print("\nFAIL: cached open produced a different index than the cold one")
        ok = False

    if ok:
        saved = cold["ms"] - warm["ms"]
        print(f"\nwarm open saves {saved} ms and {cold['requests'] - warm['requests']} "
              f"range requests; a changed ETag correctly refetches")
    return 0 if ok else 1


if __name__ == "__main__":
    try:
        with urllib.request.urlopen(URL.split("#")[0], timeout=10) as r:
            if r.status != 200:
                sys.exit(f"server returned {r.status}")
    except Exception as e:
        sys.exit(f"cannot reach the demo: {e}\n"
                 f"run: python3 -m http.server 8899 --bind 127.0.0.1   (from demo/)")
    sys.exit(run())
