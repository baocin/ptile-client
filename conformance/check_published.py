#!/usr/bin/env python3
"""Health-check the published .ptiles layers over HTTP Range.

Answers one question per file: would a reader have to correct for a broken
header? The published US.signals/US.camera once declared a 42-byte index
stride while their encoder emitted 38 bytes, so `blocks_offset` and every
absolute offset derived from it overshot the real block region and not one
block was reachable. Read as 19-byte entries those files still look
structurally plausible and report a zero-length block for every cell, which
renders as "no data here" rather than as an error.

Costs two range requests per layer -- the 256-byte header, then the front of
the index -- so it is cheap enough to run against the whole published set.

    python3 conformance/check_published.py
    python3 conformance/check_published.py --base https://host/maps --states TN,CA

Exit status is non-zero if any reachable layer needs header correction, so
this can gate a publish.
"""
import argparse
import concurrent.futures
import struct
import os
import re
import sys
import urllib.error
import urllib.request

# The host rejects urllib's default User-Agent.
UA = "Mozilla/5.0 (ptiles-client conformance check)"

ENTRY_SIZE_V1, ENTRY_SIZE_V2 = 19, 38
KNOWN = (ENTRY_SIZE_V1, ENTRY_SIZE_V2)

INDEX_HTML = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                          "..", "web-demo", "index.html")


def demo_config():
    """The base URL and layer filenames the *page* uses, read from the page.

    These were hardcoded here and drifted: this file checked
    `maps/TN.address.ptiles` (PTILESA v1) while index.html fetched
    `maps/2026-08-07/TN.address_v2.ptiles` (PTILESD v2). A conformance run that
    verifies files nobody serves is worse than no run at all, and a deploy
    checked against the wrong snapshot is how the wrong version ships.

    Parsed rather than duplicated so the page stays the single source of truth;
    it is the thing users actually load.
    """
    with open(INDEX_HTML, encoding="utf-8") as f:
        html = f.read()
    m = re.search(r'var\s+PTILES_BASE\s*=\s*"([^"]+)"', html)
    if not m:
        sys.exit(f"could not find PTILES_BASE in {INDEX_HTML}")
    base = m.group(1).rstrip("/")
    m = re.search(r"var\s+LAYER_FILES\s*=\s*\{(.*?)\};", html, re.S)
    if not m:
        sys.exit(f"could not find LAYER_FILES in {INDEX_HTML}")
    stems = dict(re.findall(r'(\w+)\s*:\s*"([^"]+)"', m.group(1)))
    return base, stems


DEMO_BASE, DEMO_LAYERS = demo_config()
DEFAULT_BASE = DEMO_BASE

# Per-state layers, named as the page names them (roads_v2, address_v2, ...).
PER_STATE = [DEMO_LAYERS[k] for k in
             ["roads", "water", "business", "buildings", "parks", "rail",
              "places", "address", "highways"] if k in DEMO_LAYERS]
NATIONAL = [f"US.{DEMO_LAYERS.get(k, k)}" for k in ["signals", "camera", "admin"]]


def fetch(url, start, end):
    req = urllib.request.Request(
        url, headers={"Range": f"bytes={start}-{end}", "User-Agent": UA})
    with urllib.request.urlopen(req, timeout=30) as r:
        return r.read()


def read_uint_le(d, off, n):
    v = 0
    for i in range(n):
        v |= d[off + i] << (8 * i)
    return v


def entry_at(d, pos, es):
    """Mirror of core/src/index.rs::read_entry, offset+length only."""
    if es == ENTRY_SIZE_V2:
        return (struct.unpack_from("<Q", d, pos)[0],
                read_uint_le(d, pos + 24, 6) | (d[pos + 32] << 48),
                read_uint_le(d, pos + 30, 2) | (d[pos + 33] << 16))
    return (struct.unpack_from("<Q", d, pos)[0],
            read_uint_le(d, pos + 8, 6),
            read_uint_le(d, pos + 14, 3))


def plausible(idx, count, es, n=4):
    """Entry 0 names a real block and cells do not descend."""
    checkable = min(count, n, (len(idx) - 4) // es)
    if checkable < 1:
        return False
    if entry_at(idx, 4, es)[2] == 0:
        return False
    prev = 0
    for i in range(checkable):
        cell = entry_at(idx, 4 + i * es, es)[0]
        if cell < prev:
            return False
        prev = cell
    return True


def check(base, name):
    url = f"{base}/{name}.ptiles"
    try:
        h = fetch(url, 0, 255)
    except urllib.error.HTTPError as e:
        return {"name": name, "status": e.code}
    except Exception as e:
        return {"name": name, "status": f"{type(e).__name__}"}

    if len(h) < 256 or h[0:6] != b"PTILES":
        return {"name": name, "status": "not a .ptiles"}

    magic = h[0:7].decode("ascii", "replace")
    version = h[8]
    block_count = struct.unpack_from("<I", h, 36)[0]
    index_offset = struct.unpack_from("<Q", h, 52)[0]
    index_length = struct.unpack_from("<I", h, 60)[0]
    blocks_offset = struct.unpack_from("<Q", h, 64)[0]
    aux_length = struct.unpack_from("<I", h, 80)[0]

    # Admin (and address) repurpose the header's section pointers: they are
    # lookup-grid layers, not block-per-cell, so `index_offset` names a
    # zstd-compressed polygon table rather than a cell index. Reading 4 bytes
    # there as an entry count yields whatever the zstd frame happens to start
    # with -- US.admin reported 4,247,762,216 entries in a 31 MB file before
    # this check existed, and the script called it "ok". Core distinguishes
    # them the same way (core/src/admin.rs): block_count == 0 and aux_length > 0.
    if block_count == 0 and aux_length > 0:
        return {"name": name, "status": 200, "magic": magic, "version": version,
                "kind": "lookup-grid", "skipped": True, "aux": aux_length,
                "broken": False}

    # Count plus a few entries, enough to validate a width structurally.
    idx = fetch(url, index_offset, index_offset + 4 + ENTRY_SIZE_V2 * 4 - 1)
    count = struct.unpack_from("<I", idx, 0)[0]

    declared = None
    if count and (index_length - 4) % count == 0:
        declared = (index_length - 4) // count

    if declared in KNOWN and plausible(idx, count, declared):
        es, how = declared, "declared"
    else:
        es, how = None, "probed"
        for cand in KNOWN:
            if plausible(idx, count, cand):
                es = cand
                break
    if es is None:
        return {"name": name, "status": "no known entry width fits",
                "declared": declared, "count": count}

    real_end = index_offset + 4 + count * es
    first_off = entry_at(idx, 4, es)[1]
    if first_off < blocks_offset:
        base_kind, overshoot = "relative", 0
    elif blocks_offset > real_end:
        base_kind, overshoot = "corrected", blocks_offset - real_end
    else:
        base_kind, overshoot = "absolute", 0

    inconsistent = base_kind == "corrected" or (declared is not None and declared != es)

    return {"name": name, "status": 200, "magic": magic, "version": version,
            "entry_size": es, "how": how, "declared": declared, "count": count,
            "base": base_kind, "overshoot": overshoot, "aux": aux_length,
            "broken": inconsistent}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--base", default=DEFAULT_BASE)
    ap.add_argument("--states", default="TN,CA,NY,TX,AK",
                    help="comma-separated, or 'none'")
    ap.add_argument("--jobs", type=int, default=8)
    args = ap.parse_args()

    names = list(NATIONAL)
    if args.states.lower() != "none":
        for st in args.states.split(","):
            names += [f"{st.strip()}.{layer}" for layer in PER_STATE]

    rows = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as ex:
        for r in ex.map(lambda n: check(args.base, n), names):
            rows.append(r)

    present = [r for r in rows if r.get("status") == 200 and not r.get("skipped")]
    skipped = [r for r in rows if r.get("skipped")]
    missing = [r for r in rows if r.get("status") != 200]
    broken = [r for r in present if r.get("broken")]

    print(f"{'layer':24s} {'magic':9s} {'v':>2s} {'entries':>8s} {'w':>3s} "
          f"{'how':9s} {'base':10s} {'aux':>6s}  state")
    print("-" * 92)
    for r in sorted(present, key=lambda r: r["name"]):
        note = "BROKEN" if r["broken"] else "ok"
        if r["broken"] and r["base"] == "corrected":
            note += f" (+{r['overshoot']})"
        if r["declared"] is not None and r["declared"] != r["entry_size"]:
            note += f" declared {r['declared']}B"
        print(f"{r['name']:24s} {r['magic']:9s} {r['version']:2d} "
              f"{r['count']:8d} {r['entry_size']:3d} {r['how']:9s} "
              f"{r['base']:10s} {r['aux']:6d}  {note}")

    for r in sorted(skipped, key=lambda r: r["name"]):
        print(f"{r['name']:24s} {r['magic']:9s} {r['version']:2d} "
              f"{'--':>8s} {'--':>3s} {'--':9s} {'--':10s} {r['aux']:6d}  "
              f"lookup-grid, not block-per-cell (no cell index to check)")

    print(f"\n{len(present)} block-per-cell layers checked, {len(skipped)} "
          f"lookup-grid skipped, {len(missing)} not found, "
          f"{len(broken)} needing header correction")
    if missing:
        print("not found: " + ", ".join(sorted(r["name"] for r in missing)))
    if broken:
        print("\nBROKEN -- these were published from a generator whose header "
              "contradicts its own index:")
        for r in broken:
            print(f"  {r['name']}: {r['base']}"
                  + (f" +{r['overshoot']}" if r["overshoot"] else "")
                  + (f", declared {r['declared']}B vs {r['entry_size']}B actual"
                     if r["declared"] != r["entry_size"] else ""))
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
