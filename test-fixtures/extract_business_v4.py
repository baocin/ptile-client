#!/usr/bin/env python3
"""
Capture one real v4 business block as a golden fixture.

`extract_golden.py` reads the local ~/kino/data/ptiles files, which are all
schema v3. v4 exists only in the published snapshot, so this script range-reads
the header, the index entry and the block straight off the host -- the same
three requests the browser client makes.

The cell is the res-7 cell containing 36.35605, -86.07246: the rural point that
produced `businesses: unexpected end of input at offset 42 (needed 25392 more
bytes)`, which is what led to the v4 framing bug. It is also conveniently small
(a few tens of KB), so it commits without bloating the repo.

Writes golden/business_v4.block.bin (raw decompressed block bytes) and
golden/business_v4.meta.json (cell id, centre, version, feature count).

Run: python3 test-fixtures/extract_business_v4.py
"""

from __future__ import annotations

import json
import os
import struct
import urllib.request

import h3
import zstandard as zstd

URL = "https://maps.mydatatimeline.com/maps/2026-08-07/TN.business_v4.ptiles"
LAT, LON = 36.35605, -86.07246
H3_RES = 7
OUT_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "golden")


def fetch(start: int, length: int) -> bytes:
    # A plain python UA gets a Cloudflare 403 on this host; the browser client
    # never sees it because it is a browser.
    req = urllib.request.Request(
        URL,
        headers={
            "Range": f"bytes={start}-{start + length - 1}",
            "User-Agent": "Mozilla/5.0 (X11; Linux x86_64) ptile-client fixture capture",
        },
    )
    with urllib.request.urlopen(req) as r:
        if r.status not in (200, 206):
            raise SystemExit(f"HTTP {r.status} for {start}+{length}")
        return r.read()


def main() -> None:
    head = fetch(0, 256)
    if head[:7] != b"PTILESB":
        raise SystemExit(f"not a PTILESB file: {head[:8]!r}")
    # Byte layout per ptiles_core::header::Header::parse.
    version = head[8]
    block_count = struct.unpack_from("<I", head, 36)[0]
    dict_offset = struct.unpack_from("<Q", head, 40)[0]
    dict_length = struct.unpack_from("<I", head, 48)[0]
    index_offset = struct.unpack_from("<Q", head, 52)[0]
    index_length = struct.unpack_from("<I", head, 60)[0]
    blocks_offset = struct.unpack_from("<Q", head, 64)[0]

    index = fetch(index_offset, index_length)
    # Some files prefix the entry array with a u32 count; ptiles_core detects
    # this the same way, by seeing which offset makes the entries divide evenly.
    if index_length % 19 == 4 and struct.unpack_from("<I", index, 0)[0] == block_count:
        index = index[4:]
    dict_data = fetch(dict_offset, dict_length) if dict_length else b""

    # 19-byte v1 entries vs 38-byte merged-block v2, detected the way
    # ptiles_core::index::detect_entry_size does.
    entry = next(
        (n for n in (19, 38) if block_count and len(index) // n == block_count),
        None,
    )
    if entry is None:
        raise SystemExit(f"cannot infer entry size: index_length={index_length} blocks={block_count}")

    def read_entry(off: int):
        if entry == 19:
            cell = struct.unpack_from("<Q", index, off)[0]
            boff = int.from_bytes(index[off + 8 : off + 14], "little")
            blen = int.from_bytes(index[off + 14 : off + 17], "little")
            fc = struct.unpack_from("<H", index, off + 17)[0]
        else:
            cell = struct.unpack_from("<Q", index, off)[0]
            boff = int.from_bytes(index[off + 24 : off + 30], "little") | (index[off + 32] << 48)
            blen = int.from_bytes(index[off + 30 : off + 32], "little") | (index[off + 33] << 16)
            fc = struct.unpack_from("<H", index, off + 34)[0]
        return cell, boff, blen, fc

    cell = int(h3.latlng_to_cell(LAT, LON, H3_RES), 16)
    # Published indexes mask the high mode/resolution bits on some layers, so
    # match on the low 45 bits too rather than assuming.
    mask = (1 << 45) - 1
    found = None
    for i in range(block_count):
        c, boff, blen, fc = read_entry(i * entry)
        if c == cell or (c & mask) == (cell & mask):
            found = (c, boff, blen, fc)
            break
    if not found:
        raise SystemExit(f"cell {cell:x} not in index")
    _c, boff, blen, feature_count = found

    # Offsets are relative to blocks_offset in some files, absolute in others
    # (ptiles_core::file::index_layout). Try absolute first, fall back.
    raw = fetch(boff, blen)
    dctx = (
        zstd.ZstdDecompressor(dict_data=zstd.ZstdCompressionDict(dict_data))
        if dict_data
        else zstd.ZstdDecompressor()
    )

    def inflate(b: bytes):
        for d in (dctx, zstd.ZstdDecompressor()):
            try:
                return d.decompress(b, max_output_size=64 << 20)
            except zstd.ZstdError:
                continue
        return None

    block = inflate(raw)
    if block is None:
        raw = fetch(blocks_offset + boff, blen)
        block = inflate(raw)
    if block is None:
        raise SystemExit("block did not decompress with dict or plain zstd")

    clat, clon = h3.cell_to_latlng(h3.int_to_str(cell))
    os.makedirs(OUT_DIR, exist_ok=True)
    with open(os.path.join(OUT_DIR, "business_v4.block.bin"), "wb") as f:
        f.write(block)
    meta = {
        "cell_center_lat": clat,
        "cell_center_lon": clon,
        "cell_id_hex": h3.int_to_str(cell),
        "cell_id_int": cell,
        "block_length_compressed": len(raw),
        "block_length_decompressed": len(block),
        "file_version": version,
        "index_entry_size": entry,
        "feature_count_in_index": feature_count,
        "layer": "business",
        "query_point": [LAT, LON],
        "source_file": URL.rsplit("/", 1)[-1],
    }
    with open(os.path.join(OUT_DIR, "business_v4.meta.json"), "w") as f:
        json.dump(meta, f, indent=2, sort_keys=True)
        f.write("\n")
    print(json.dumps(meta, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
