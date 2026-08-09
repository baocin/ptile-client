#!/usr/bin/env python3
"""Generate an `address_v2_dict.ptiles` golden fixture: what the real builder
actually emits, which `address.ptiles` (v1, no dictionary, `PTILESA2` magic)
did not resemble closely enough to catch two decoder bugs.

Every published `{STATE}.address_v2.ptiles` is magic `PTILESD`, version 2, and
its blocks are compressed against an 8 KiB zstd dictionary stored in the file.
The Rust `AddressFile` rejected the magic outright and, once past that,
decompressed blocks without the dictionary -- so the whole layer read as
"unexpected end of input" on every real file while the v1 golden fixture passed.

Emits:
  test-fixtures/golden/address_v2_dict.ptiles
  test-fixtures/golden/address_v2_dict.golden.json
"""
import json
import os
import struct
import sys

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

MAGIC = b"PTILESD\x00"
VERSION = 2

HERE = os.path.dirname(os.path.abspath(__file__))
OUT_DIR = os.path.join(HERE, "golden")

CENTER_LON_MICRO = round(-86.783 * 100000)
CENTER_LAT_MICRO = round(36.166 * 100000)

# (cell_id, [(osm_id, housenumber, street, lat, lon)]). Positions are inside the
# cell, so the i16 microdegree offsets cannot overflow -- the same invariant the
# builder relies on.
CELLS = [
    (
        0x87264D106FFFFFF,
        [
            (1440913532, "100", "Broadway", 36.16612, -86.78321),
            (1440913600, "102", "Broadway", 36.16650, -86.78290),
            (1440913700, "5", "2nd Ave N", 36.16410, -86.77980),
        ],
    ),
    (
        0x87264D1040FFFFF,
        [
            (900000001, "1", "Church St", 36.16901, -86.78810),
        ],
    ),
]


def enc_record(osm_id, pid, housenumber, street, lat, lon):
    """One v2 record: delta osm_id, i16 lon/lat offsets, then the strings."""
    b = bytearray()
    b.extend(encode_varint(zigzag_encode(osm_id - pid)))
    b.extend(
        struct.pack(
            "<hh",
            round(lon * 100000) - CENTER_LON_MICRO,
            round(lat * 100000) - CENTER_LAT_MICRO,
        )
    )
    for s in (housenumber, street):
        raw = s.encode("utf-8")
        b.extend(struct.pack("<H", len(raw)))
        b.extend(raw)
    return bytes(b)


def train_dictionary():
    """A real (magic-carrying) zstd dictionary, trained on synthetic blocks.

    A raw-content dictionary would not do: the decoder parses the dictionary
    header, so a dictionary without one falls back to a dict-less decode and
    the fixture would pass for the wrong reason.
    """
    streets = ["Broadway", "Church St", "2nd Ave N", "Woodland St", "Demonbreun St"]
    samples = []
    for i in range(400):
        recs = [
            enc_record(
                1000000 + i * 7 + j,
                0 if j == 0 else 1000000 + i * 7 + j - 1,
                str(100 + j),
                streets[(i + j) % len(streets)],
                36.166 + (j * 0.0003),
                -86.783 + (i % 11) * 0.0004,
            )
            for j in range(6)
        ]
        samples.append(
            encode_merged_block(
                [(0x87264D106FFFFFF + i, recs)], CENTER_LON_MICRO, CENTER_LAT_MICRO
            )
        )
    return zstd.train_dictionary(4096, samples).as_bytes()


def main():
    os.makedirs(OUT_DIR, exist_ok=True)

    cell_records, golden_cells = [], []
    for cell_id, addrs in CELLS:
        recs, pid, gaddrs = [], 0, []
        for osm_id, hn, st, lat, lon in addrs:
            recs.append(enc_record(osm_id, pid, hn, st, lat, lon))
            pid = osm_id
            gaddrs.append(
                {
                    "osm_id": osm_id,
                    "housenumber": hn,
                    "street": st,
                    # What the decoder must reproduce: the microdegree grid the
                    # offsets are stored on, not the input float.
                    "lat": (CENTER_LAT_MICRO + round(lat * 100000) - CENTER_LAT_MICRO)
                    / 100000,
                    "lon": (CENTER_LON_MICRO + round(lon * 100000) - CENTER_LON_MICRO)
                    / 100000,
                }
            )
        cell_records.append((cell_id, recs))
        golden_cells.append({"cell_id": cell_id, "addresses": gaddrs})

    block = encode_merged_block(cell_records, CENTER_LON_MICRO, CENTER_LAT_MICRO)
    dict_bytes = train_dictionary()
    zd = zstd.ZstdCompressionDict(dict_bytes)
    compressed = zstd.ZstdCompressor(level=12, dict_data=zd).compress(block)

    n_index = len(CELLS)
    dict_offset = HEADER_SIZE
    dict_length = len(dict_bytes)
    index_offset = dict_offset + dict_length
    index_length = 4 + n_index * INDEX_ENTRY_SIZE_V2
    blocks_offset = index_offset + index_length

    sorted_block_cells = sorted(c[0] for c in CELLS)
    index_bytes = bytearray(struct.pack("<I", n_index))
    total_features = 0
    for cell_id, addrs in sorted(CELLS, key=lambda c: c[0]):
        total_features += len(addrs)
        index_bytes.extend(
            encode_index_entry_v2(
                h3_cell=cell_id,
                min_lon=CENTER_LON_MICRO,
                min_lat=CENTER_LAT_MICRO,
                max_lon=CENTER_LON_MICRO,
                max_lat=CENTER_LAT_MICRO,
                block_offset=blocks_offset,
                block_length=len(compressed),
                feature_count=len(addrs),
                cell_index=sorted_block_cells.index(cell_id),
            )
        )

    out_path = os.path.join(OUT_DIR, "address_v2_dict.ptiles")
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
        f.write(dict_bytes)
        f.write(bytes(index_bytes))
        f.write(compressed)

    with open(os.path.join(OUT_DIR, "address_v2_dict.golden.json"), "w") as f:
        json.dump(
            {
                "block_count": 1,
                "total_features": total_features,
                "dict_length": dict_length,
                "cells": golden_cells,
            },
            f,
            indent=2,
        )

    print(
        f"wrote {out_path} ({os.path.getsize(out_path)} bytes), "
        f"{n_index} cells, {total_features} addresses, {dict_length} B dictionary"
    )


if __name__ == "__main__":
    main()
