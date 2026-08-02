#!/usr/bin/env python3
"""Build the conformance corpus by slicing real published `.ptiles` files.

Why slices and not synthetic files: `core/tests/index_layout.rs` already proves
the detection logic against a synthetic layout matrix. What it cannot prove is
that a *generator* still writes what the reader expects. That needs bytes a
generator actually wrote -- but the published layers run 0.35 MB to 54 MB, and
their header+index sections alone total ~9.9 MB, which is too much to commit.

So each case keeps the real header, the real index entries (copied verbatim,
with only the offset/length fields repointed), the real aux region, and the
real block payloads -- just fewer of them. Every property the readers detect
survives: entry width, offset base, declared stride, merged-block cell tables,
bbox and cell_index bytes.

Two deliberate departures, both recorded per file in `manifest.json`:

  dict: "stripped"  Six layers carry a 512 KB zstd dictionary, which would
                    dominate a corpus otherwise measured in kilobytes. For
                    those, blocks are decompressed with the real dictionary
                    and recompressed without one. The *decompressed* payload
                    stays byte-identical to the generator's; only the
                    compression framing differs. `TN.water` (11 KB dict) is
                    kept intact so the dictionary path stays covered, and
                    `TN.parks`/`TN.rail` never had a dictionary at all.

  entries: N        Only the first N index entries are kept, deduplicated by
                    the block they name (merged layers point many cells at one
                    block).

This script is not run in CI. It reads machine-local published data; CI
consumes only its committed output. Re-run it when the published files change:

    python3 conformance/slice.py

It verifies every file it writes by reopening it and checking the detected
layout matches what the source file had. A slice that no longer reproduces its
source's layout is a failure, not a warning -- silently testing the wrong thing
is the failure mode this corpus exists to prevent.
"""

import json
import os
import struct
import sys

try:
    import zstandard
except ImportError:
    sys.exit("need `zstandard` (pip install zstandard)")

HEADER_SIZE = 256
ENTRY_SIZE_V1 = 19
ENTRY_SIZE_V2 = 38
KNOWN_ENTRY_SIZES = (ENTRY_SIZE_V1, ENTRY_SIZE_V2)

HERE = os.path.dirname(os.path.abspath(__file__))
CORPUS = os.path.join(HERE, "corpus")

# Where published layers live on this machine. Same list the Rust and node
# suites search, plus the pre-fix backups that are the only surviving source
# of the 42-byte-stride case.
SEARCH_DIRS = [
    "/home/aoi/kino/data/ptiles",
    "/home/aoi/kino/projects/ptiles/tiles",
    "/home/aoi/kino/projects/ptiles/tiles/published-backup",
    "/mnt/core/kino/ptiles/data/states",
]

# name -> (source filename, source dir hint or None, entries to keep, why)
CASES = [
    ("TN.roads.ptiles", None, 48,
     "v2 roads: intersection table, 19-byte entries, absolute offsets"),
    ("TN.water.ptiles", None, 48,
     "only layer with a small enough dictionary to keep intact"),
    ("TN.business.ptiles", None, 48,
     "PTILESB v3 records; the layer whose wasm framing is disputed"),
    ("TN.buildings_v8.ptiles", None, 48,
     "the only layer using RELATIVE block offsets"),
    ("TN.parks.ptiles", None, 48,
     "38-byte entries over merged blocks, no dictionary"),
    ("TN.rail.ptiles", None, 64,
     "14 entries total, 2 KB: kept whole, so the slicer is identity here"),
    ("TN.places.ptiles", None, 48,
     "38-byte entries; no decoder yet, so this is an index-only case"),
    ("US.signals.ptiles", "/home/aoi/kino/projects/ptiles/tiles", 48,
     "rebuilt signals: 38-byte stride now declared correctly, plus a PTCI aux region"),
    ("US.camera.ptiles", "/home/aoi/kino/projects/ptiles/tiles", 48,
     "rebuilt camera, with its PTCI aux region"),
    ("US.signals.stride42.ptiles", "/home/aoi/kino/projects/ptiles/tiles/published-backup", 48,
     "THE historical bug: index_length computed at 42 bytes, entries emitted at 38"),
    ("US.camera.stride42.ptiles", "/home/aoi/kino/projects/ptiles/tiles/published-backup", 48,
     "same 42-vs-38 skew on camera"),
]

# Keep the dictionary verbatim only when it is small enough not to dominate.
MAX_KEPT_DICT = 64 * 1024
# Same for the aux region. `US.signals`/`US.camera` carry a few KB of PTCI
# coarse index, worth keeping; `TN.water` carries 812 KB, which is not.
MAX_KEPT_AUX = 32 * 1024


# --------------------------------------------------------------------- header

HEADER_FIELDS = {
    "magic": (0, "7s"), "version": (8, "B"),
    "min_lat": (12, "f"), "min_lon": (16, "f"),
    "max_lat": (20, "f"), "max_lon": (24, "f"),
    "feature_count": (28, "Q"), "block_count": (36, "I"),
    "dict_offset": (40, "Q"), "dict_length": (48, "I"),
    "index_offset": (52, "Q"), "index_length": (60, "I"),
    "blocks_offset": (64, "Q"), "aux_offset": (72, "Q"), "aux_length": (80, "I"),
}


def parse_header(buf):
    return {k: struct.unpack_from("<" + f, buf, off)[0]
            for k, (off, f) in HEADER_FIELDS.items()}


def build_header(src_bytes, fields):
    """Copy the source header verbatim, then overwrite the fields we moved.

    Copying rather than rebuilding keeps the bbox floats, the magic null and
    the 172 reserved bytes exactly as the generator wrote them.
    """
    h = bytearray(src_bytes[:HEADER_SIZE])
    for k, v in fields.items():
        off, f = HEADER_FIELDS[k]
        struct.pack_into("<" + f, h, off, v)
    return bytes(h)


# ---------------------------------------------------------------------- index

def read_uint_le(d, off, n):
    v = 0
    for i in range(n):
        v |= d[off + i] << (8 * i)
    return v


def read_entry(d, pos, es):
    """Mirror of `core/src/index.rs::read_entry`."""
    if es == ENTRY_SIZE_V2:
        return {
            "h3_cell": struct.unpack_from("<Q", d, pos)[0],
            "block_offset": read_uint_le(d, pos + 24, 6) | (d[pos + 32] << 48),
            "block_length": read_uint_le(d, pos + 30, 2) | (d[pos + 33] << 16),
            "feature_count": struct.unpack_from("<H", d, pos + 34)[0],
        }
    return {
        "h3_cell": struct.unpack_from("<Q", d, pos)[0],
        "block_offset": read_uint_le(d, pos + 8, 6),
        "block_length": read_uint_le(d, pos + 14, 3),
        "feature_count": struct.unpack_from("<H", d, pos + 17)[0],
    }


def patch_entry(raw, es, block_offset, block_length):
    """Rewrite only offset/length in a verbatim-copied entry.

    Everything else -- the bbox at 8..24 and cell_index at 36..38 on v2, the
    feature_count on both -- stays as the generator wrote it.
    """
    e = bytearray(raw)
    if es == ENTRY_SIZE_V2:
        for i in range(6):
            e[24 + i] = (block_offset >> (8 * i)) & 0xFF
        e[32] = (block_offset >> 48) & 0xFF
        e[30] = block_length & 0xFF
        e[31] = (block_length >> 8) & 0xFF
        e[33] = (block_length >> 16) & 0xFF
    else:
        for i in range(6):
            e[8 + i] = (block_offset >> (8 * i)) & 0xFF
        for i in range(3):
            e[14 + i] = (block_length >> (8 * i)) & 0xFF
    return bytes(e)


def detect_entry_size(index_region, count, index_length):
    """Mirror of `core/src/index.rs::detect_entry_size`, enough for our inputs.

    Returns (entry_size, source, declared_stride).
    """
    declared = None
    if count and (index_length - 4) % count == 0:
        declared = (index_length - 4) // count
    if declared in KNOWN_ENTRY_SIZES and structurally_valid(index_region, count, declared):
        return declared, "DeclaredLength", declared
    for es in KNOWN_ENTRY_SIZES:
        if structurally_valid(index_region, count, es):
            return es, "Probed", declared
    raise SystemExit(f"no known entry width fits: count={count} index_length={index_length}")


def structurally_valid(region, count, es):
    """Entry 0 must name a real block and cells must not descend."""
    if count == 0 or len(region) < count * es:
        return False
    first = read_entry(region, 0, es)
    if first["block_length"] == 0:
        return False
    prev = 0
    for i in range(min(count, 64)):
        c = read_entry(region, i * es, es)["h3_cell"]
        if c < prev:
            return False
        prev = c
    return True


def offset_base_of(entries, header, count, es):
    """Mirror of `core/src/file.rs::open`'s offset-base decision."""
    real_end = header["index_offset"] + 4 + count * es
    if entries and entries[0]["block_offset"] < header["blocks_offset"]:
        return ("Relative", 0)
    if header["blocks_offset"] > real_end:
        return ("AbsoluteCorrected", header["blocks_offset"] - real_end)
    return ("Absolute", 0)


def resolve(entry, header, base, overshoot):
    if base == "Relative":
        return header["blocks_offset"] + entry["block_offset"]
    if base == "AbsoluteCorrected":
        return entry["block_offset"] - overshoot
    return entry["block_offset"]


# ------------------------------------------------------------------- slicing

def find_source(name, hint):
    """Locate a source file. `stride42` names map back to their real filename."""
    real = name.replace(".stride42", "")
    dirs = [hint] if hint else SEARCH_DIRS
    for d in dirs:
        p = os.path.join(d, real)
        if os.path.exists(p):
            return p
    return None


def slice_file(src_path, out_path, want_entries):
    with open(src_path, "rb") as f:
        blob = f.read()

    h = parse_header(blob)
    count = struct.unpack_from("<I", blob, h["index_offset"])[0]
    # `index_length` counts the 4-byte entry count, so the entries themselves
    # end at index_offset + index_length. Reading further would be more
    # permissive than `core/src/file.rs::open`, which sees exactly this window.
    region = blob[h["index_offset"] + 4: h["index_offset"] + h["index_length"]]
    es, es_source, declared = detect_entry_size(region, count, h["index_length"])

    entries = [read_entry(region, i * es, es) for i in range(count)]
    base, overshoot = offset_base_of(entries, h, count, es)

    # Pick the first `want_entries` entries that name a real block, then
    # deduplicate the blocks they point at -- merged layers aim many cells at
    # one block, and storing it once is the whole size win.
    picked, blocks, block_pos = [], [], {}
    for i, e in enumerate(entries):
        if len(picked) >= want_entries:
            break
        if e["block_length"] == 0:
            continue
        key = (e["block_offset"], e["block_length"])
        if key not in block_pos:
            abs_off = resolve(e, h, base, overshoot)
            payload = blob[abs_off: abs_off + e["block_length"]]
            if len(payload) != e["block_length"]:
                continue  # entry points past EOF; skip rather than emit garbage
            block_pos[key] = len(blocks)
            blocks.append(payload)
        picked.append((i, e, region[i * es:(i + 1) * es], key))

    if not picked:
        raise SystemExit(f"{src_path}: no usable entries")

    # Dictionary: keep small ones verbatim, strip large ones by recompressing.
    dict_bytes = blob[h["dict_offset"]: h["dict_offset"] + h["dict_length"]]
    keep_dict = 0 < len(dict_bytes) <= MAX_KEPT_DICT
    stripped = bool(dict_bytes) and not keep_dict
    if stripped:
        blocks = [recompress(b, dict_bytes) for b in blocks]
        dict_bytes = b""

    aux = blob[h["aux_offset"]: h["aux_offset"] + h["aux_length"]] if h["aux_length"] else b""
    aux_dropped = len(aux) > MAX_KEPT_AUX
    if aux_dropped:
        aux = b""

    # Lay the new file out in the source's own section order: aux, dict,
    # index, blocks.
    pos = HEADER_SIZE
    aux_off = pos if aux else 0
    pos += len(aux)
    dict_off = pos if dict_bytes else 0
    pos += len(dict_bytes)
    index_off = pos

    n = len(picked)
    # `declared_stride` is a property of the header's arithmetic, not of the
    # entries. Reproduce it so the 42-vs-38 skew survives slicing.
    stride = declared if declared and declared not in KNOWN_ENTRY_SIZES else es
    index_length = 4 + stride * n
    real_end = index_off + 4 + es * n
    new_overshoot = (stride - es) * n
    blocks_off = real_end + new_overshoot

    # Blocks are laid out from `real_end`, which is where the index truly ends
    # regardless of what the header claims about it.
    offsets, cur = [], real_end
    for b in blocks:
        offsets.append(cur)
        cur += len(b)

    out_entries = []
    for (_, e, raw, key) in picked:
        idx = block_pos[key]
        abs_pos = offsets[idx]
        length = len(blocks[idx])
        if base == "Relative":
            stored = abs_pos - blocks_off
        elif base == "AbsoluteCorrected":
            stored = abs_pos + new_overshoot
        else:
            stored = abs_pos
        out_entries.append(patch_entry(raw, es, stored, length))

    header = build_header(blob, {
        "feature_count": sum(e["feature_count"] for (_, e, _, _) in picked),
        "block_count": len(blocks),
        "dict_offset": dict_off, "dict_length": len(dict_bytes),
        "index_offset": index_off, "index_length": index_length,
        "blocks_offset": blocks_off,
        "aux_offset": aux_off, "aux_length": len(aux),
    })

    out = bytearray()
    out += header
    out += aux
    out += dict_bytes
    out += struct.pack("<I", n)
    for e in out_entries:
        out += e
    # For the stride-42 cases the declared index region overlaps the first
    # block bytes, exactly as it does in the published file. Nothing is
    # inserted to pad it; the overlap *is* the bug being preserved.
    for b in blocks:
        out += b

    with open(out_path, "wb") as f:
        f.write(bytes(out))

    return {
        "source": os.path.basename(src_path),
        "magic": h["magic"].decode("ascii", "replace").rstrip("\x00"),
        "version": h["version"],
        "entry_size": es,
        "entry_size_source": es_source,
        "declared_stride": declared,
        "offset_base": base,
        "overshoot": new_overshoot if base == "AbsoluteCorrected" else 0,
        "source_overshoot": overshoot,
        "entry_count": n,
        "block_count": len(blocks),
        "feature_count": sum(e["feature_count"] for (_, e, _, _) in picked),
        "dict": "kept" if keep_dict else ("stripped" if stripped else "none"),
        "source_dict_length": h["dict_length"],
        "aux_length": len(aux),
        "aux": "dropped" if aux_dropped else ("kept" if aux else "none"),
        "source_aux_length": h["aux_length"],
        "first_cell": f'{picked[0][1]["h3_cell"]:x}',
        "bytes": len(out),
    }


def recompress(payload, dict_bytes):
    """Decompress with the real dictionary, recompress without one."""
    d = zstandard.ZstdCompressionDict(dict_bytes)
    dctx = zstandard.ZstdDecompressor(dict_data=d)
    try:
        raw = dctx.decompress(payload)
    except zstandard.ZstdError:
        # No embedded content size -- stream it instead.
        raw = dctx.decompressobj().decompress(payload)
    return zstandard.ZstdCompressor(level=10).compress(raw)


# ----------------------------------------------------------------- verification

def verify(path, expect):
    """Reopen a written slice and confirm it still detects as its source did."""
    with open(path, "rb") as f:
        blob = f.read()
    h = parse_header(blob)
    count = struct.unpack_from("<I", blob, h["index_offset"])[0]
    region = blob[h["index_offset"] + 4: h["index_offset"] + 4 + h["index_length"]]
    es, es_source, declared = detect_entry_size(region, count, h["index_length"])
    entries = [read_entry(region, i * es, es) for i in range(count)]
    base, overshoot = offset_base_of(entries, h, count, es)

    problems = []
    if es != expect["entry_size"]:
        problems.append(f"entry_size {es} != {expect['entry_size']}")
    if base != expect["offset_base"]:
        problems.append(f"offset_base {base} != {expect['offset_base']}")
    if declared != expect["declared_stride"]:
        problems.append(f"declared_stride {declared} != {expect['declared_stride']}")
    if base == "AbsoluteCorrected" and overshoot != expect["overshoot"]:
        problems.append(f"overshoot {overshoot} != {expect['overshoot']}")
    if count != expect["entry_count"]:
        problems.append(f"count {count} != {expect['entry_count']}")

    # Every entry must resolve to bytes inside the file, and every block must
    # still start with a zstd frame magic.
    for i, e in enumerate(entries):
        off = resolve(e, h, base, overshoot)
        if off + e["block_length"] > len(blob):
            problems.append(f"entry {i} resolves past EOF")
            break
        if blob[off:off + 4] != b"\x28\xb5\x2f\xfd":
            problems.append(f"entry {i} does not point at a zstd frame")
            break
    return problems


def main():
    os.makedirs(CORPUS, exist_ok=True)
    manifest, missing = {}, []

    for name, hint, want, why in CASES:
        src = find_source(name, hint)
        if not src:
            missing.append(name)
            print(f"  SKIP {name}: source not found")
            continue
        out = os.path.join(CORPUS, name)
        info = slice_file(src, out, want)
        info["why"] = why
        problems = verify(out, info)
        if problems:
            raise SystemExit(f"{name}: slice does not reproduce its source layout: "
                             + "; ".join(problems))
        manifest[name] = info
        print(f"  {name:32s} {info['bytes']:7d} B  {info['entry_size']}B entries, "
              f"{info['offset_base']}"
              + (f"+{info['overshoot']}" if info["overshoot"] else "")
              + f", dict={info['dict']}, {info['entry_count']} entries")

    with open(os.path.join(HERE, "manifest.json"), "w") as f:
        json.dump({
            "note": "Generated by conformance/slice.py. Every value here is "
                    "re-derived from the committed bytes and asserted by the "
                    "conformance runners; do not hand-edit.",
            "files": manifest,
        }, f, indent=2, sort_keys=True)
        f.write("\n")

    total = sum(i["bytes"] for i in manifest.values())
    print(f"\n{len(manifest)} files, {total} bytes total")
    if missing:
        print(f"missing sources: {', '.join(missing)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
