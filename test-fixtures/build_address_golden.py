#!/usr/bin/env python3
"""Generate a tiny synthetic `address.ptiles` golden fixture + JSON using the
reference PTiles encoders (no OSM/PBF pipeline needed).

Since no real `{STATE}.address.ptiles` sample is hosted, this produces a
byte-accurate fixture from the same encode helpers the real builder uses
(`ptiles/scripts/shared.py`), so the Rust decoder has a real reference to test
against. Emits:
  test-fixtures/golden/address.ptiles       (whole small file)
  test-fixtures/golden/address.golden.json  (decoded expectation)
"""
import json
import os
import struct
import sys

# The reference encoders live in the ptiles repo.
sys.path.insert(0, "/home/aoi/kino/projects/ptiles/scripts")

import zstandard as zstd  # noqa: E402
from shared import (  # noqa: E402
    encode_varint,
    zigzag_encode,
    encode_merged_block,
    encode_index_entry_v2,
    write_header,
    HEADER_SIZE,
    INDEX_ENTRY_SIZE_V2,
)

MAGIC = b"PTILESA2\x00"
VERSION = 1

HERE = os.path.dirname(os.path.abspath(__file__))
OUT_DIR = os.path.join(HERE, "golden")


def enc_record(osm_id, pid, housenumber, street):
    b = bytearray()
    b.extend(encode_varint(zigzag_encode(osm_id - pid)))
    hn = housenumber.encode("utf-8")
    b.extend(struct.pack("<H", len(hn)))
    b.extend(hn)
    st = street.encode("utf-8")
    b.extend(struct.pack("<H", len(st)))
    b.extend(st)
    return bytes(b)


# Two cells in one block. Cell ids are arbitrary but sorted; the block sorts
# them internally. Records carry {osm_id, housenumber, street}; pid resets to 0
# per cell so the first delta is the raw osm_id.
CELLS = [
    (
        0x87264D106FFFFFF,
        [
            (1440913532, "100", "Broadway"),
            (1440913600, "102", "Broadway"),
            (1440913700, "5", "2nd Ave N"),
        ],
    ),
    (
        0x87264D1040FFFFF,
        [
            (900000001, "1", "Church St"),
        ],
    ),
]


def main():
    os.makedirs(OUT_DIR, exist_ok=True)

    # Build the merged block: list[(cell_id, [record_bytes])].
    cell_records = []
    golden_cells = []
    for cell_id, addrs in CELLS:
        recs = []
        pid = 0
        gaddrs = []
        for osm_id, hn, st in addrs:
            recs.append(enc_record(osm_id, pid, hn, st))
            pid = osm_id
            gaddrs.append({"osm_id": osm_id, "housenumber": hn, "street": st})
        cell_records.append((cell_id, recs))
        golden_cells.append({"cell_id": cell_id, "addresses": gaddrs})

    center_lon_micro = round(-86.783 * 100000)
    center_lat_micro = round(36.166 * 100000)
    block = encode_merged_block(cell_records, center_lon_micro, center_lat_micro)
    compressed = zstd.ZstdCompressor(level=1).compress(block)

    # Layout: header | v2 index | compressed block.
    n_index = len(CELLS)
    dict_offset = HEADER_SIZE
    dict_length = 0
    index_offset = HEADER_SIZE
    index_length = 4 + n_index * INDEX_ENTRY_SIZE_V2
    blocks_offset = index_offset + index_length

    # Build v2 index entries (one per cell; all point at the single block). The
    # index must be sorted by h3_cell for binary search, matching the block's
    # internal sort. cell_index is the cell's ordinal in the sorted block.
    sorted_block_cells = sorted(c[0] for c in CELLS)
    index_bytes = bytearray(struct.pack("<I", n_index))
    total_features = 0
    for cell_id, addrs in sorted(CELLS, key=lambda c: c[0]):
        cell_index = sorted_block_cells.index(cell_id)
        total_features += len(addrs)
        index_bytes.extend(
            encode_index_entry_v2(
                h3_cell=cell_id,
                min_lon=center_lon_micro,
                min_lat=center_lat_micro,
                max_lon=center_lon_micro,
                max_lat=center_lat_micro,
                block_offset=blocks_offset,
                block_length=len(compressed),
                feature_count=len(addrs),
                cell_index=cell_index,
            )
        )

    out_path = os.path.join(OUT_DIR, "address.ptiles")
    with open(out_path, "wb") as f:
        write_header(
            f,
            MAGIC,
            VERSION,
            36.0,
            -87.0,
            36.5,
            -86.5,
            total_features,
            1,  # block_count
            dict_offset,
            dict_length,
            index_offset,
            index_length,
            blocks_offset,
        )
        f.write(bytes(index_bytes))
        f.write(compressed)

    golden = {
        "block_count": 1,
        "total_features": total_features,
        "center": [center_lon_micro / 100000, center_lat_micro / 100000],
        "cells": golden_cells,
    }
    with open(os.path.join(OUT_DIR, "address.golden.json"), "w") as f:
        json.dump(golden, f, indent=2)

    print(f"wrote {out_path} ({os.path.getsize(out_path)} bytes), "
          f"{n_index} cells, {total_features} addresses")


if __name__ == "__main__":
    main()
