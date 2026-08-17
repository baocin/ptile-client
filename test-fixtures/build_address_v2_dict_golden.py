#!/usr/bin/env python3
"""Generate the `address_v{2,3}_dict.ptiles` golden fixtures: what the real
builder emits, which `address.ptiles` (v1, no dictionary, `PTILESA2` magic) did
not resemble closely enough to catch two decoder bugs.

Every published `{STATE}.address_v2.ptiles` is magic `PTILESD`, version 2, and
its blocks are compressed against an 8 KiB zstd dictionary stored in the file.
The Rust `AddressFile` rejected the magic outright and, once past that,
decompressed blocks without the dictionary -- so the whole layer read as
"unexpected end of input" on every real file while the v1 golden fixture passed.

v3 is v2 plus a one-byte provenance field after the coordinate offsets, for the
merged OSM + NAD + OpenAddresses layer.

Emits:
  test-fixtures/golden/address_v2_dict.ptiles  + .golden.json
  test-fixtures/golden/address_v3_dict.ptiles  + .golden.json
"""
import json
import os
import struct
import sys

sys.path.insert(0, "/home/aoi/kino/projects/ptiles/scripts")

import h3  # noqa: E402
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

HERE = os.path.dirname(os.path.abspath(__file__))
OUT_DIR = os.path.join(HERE, "golden")

def cell_centre_micro(cell_id):
    """Each cell's own centre, in 1e-5 degrees -- what the real builder
    measures its offsets from, and what the decoder must reconstruct.

    The fixture used to use one arbitrary constant for every cell, which made
    the block header's centre and the per-cell centre identical. That is the
    one thing a real merged block never is, and it is why a decoder reading
    offsets against the block header passed this fixture while putting seven of
    every eight published cells kilometres off.
    """
    clat, clon = h3.cell_to_latlng(hex(cell_id)[2:])
    return round(clon * 100000), round(clat * 100000)

# (cell_id, [(osm_id, housenumber, street, lat, lon, source)]). Positions are
# inside the cell, so the i16 microdegree offsets cannot overflow -- the same
# invariant the builder relies on. `source` is the v3 provenance byte
# (0=osm, 1=nad, 2=openaddresses) and is dropped when writing v2.
CELLS = [
    (
        0x87264D106FFFFFF,
        [
            (1440913532, "100", "Broadway", 36.16612, -86.78321, 0),
            (1440913600, "102", "Broadway", 36.16650, -86.78290, 1),
            (1440913700, "5", "2nd Ave N", 36.16410, -86.77980, 2),
        ],
    ),
    (
        # A real neighbour of the cell above, not an invented id: the fixture
        # used to name 0x87264D1040FFFFF, which is not a valid H3 index at all,
        # so no cell centre could be derived for it and the offsets had to be
        # measured from an arbitrary constant instead.
        0x87264D131FFFFFF,
        [
            (900000001, "1", "Church St", 36.18373, -86.78151, 1),
        ],
    ),
]


def enc_record(osm_id, pid, housenumber, street, lat, lon, source, version, centre):
    """One record: delta osm_id, i16 lon/lat offsets, v3's source byte, strings."""
    b = bytearray()
    b.extend(encode_varint(zigzag_encode(osm_id - pid)))
    b.extend(struct.pack("<hh", round(lon * 100000) - centre[0], round(lat * 100000) - centre[1]))
    if version >= 3:
        b.append(source)
    for s in (housenumber, street):
        raw = s.encode("utf-8")
        b.extend(struct.pack("<H", len(raw)))
        b.extend(raw)
    return bytes(b)


def train_dictionary(version):
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
                j % 3,
                version,
                cell_centre_micro(0x87264D106FFFFFF),
            )
            for j in range(6)
        ]
        samples.append(
            encode_merged_block(
                [(0x87264D106FFFFFF + i, recs)], *cell_centre_micro(0x87264D106FFFFFF)
            )
        )
    return zstd.train_dictionary(4096, samples).as_bytes()


def build(version, stem):
    cell_records, golden_cells = [], []
    for cell_id, addrs in CELLS:
        recs, pid, gaddrs = [], 0, []
        centre = cell_centre_micro(cell_id)
        for osm_id, hn, st, lat, lon, source in addrs:
            recs.append(enc_record(osm_id, pid, hn, st, lat, lon, source, version, centre))
            pid = osm_id
            gaddrs.append(
                {
                    "osm_id": osm_id,
                    "housenumber": hn,
                    "street": st,
                    # What the decoder must reproduce: the microdegree grid the
                    # offsets are stored on, not the input float.
                    "lat": round(lat * 100000) / 100000,
                    "lon": round(lon * 100000) / 100000,
                    "source": ["osm", "nad", "openaddresses"][source]
                    if version >= 3
                    else "osm",
                }
            )
        cell_records.append((cell_id, recs))
        golden_cells.append({"cell_id": cell_id, "addresses": gaddrs})

    # The block header carries the *first* cell's centre, exactly as the real
    # builder writes it -- so the second cell's records only decode correctly
    # if the reader uses that cell's own centre instead of this one.
    block = encode_merged_block(cell_records, *cell_centre_micro(sorted(c[0] for c in CELLS)[0]))
    dict_bytes = train_dictionary(version)
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
                # The real bounds of this cell's records, as the builder
                # writes them. A degenerate bbox (centre repeated) would make
                # every record read as "outside" its own cell, which is what a
                # whole-file search uses to order cells by distance.
                min_lon=min(round(a[4] * 100000) for a in addrs),
                min_lat=min(round(a[3] * 100000) for a in addrs),
                max_lon=max(round(a[4] * 100000) for a in addrs),
                max_lat=max(round(a[3] * 100000) for a in addrs),
                block_offset=blocks_offset,
                block_length=len(compressed),
                feature_count=len(addrs),
                cell_index=sorted_block_cells.index(cell_id),
            )
        )

    out_path = os.path.join(OUT_DIR, f"{stem}.ptiles")
    with open(out_path, "wb") as f:
        write_header(
            f,
            MAGIC,
            version,
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

    with open(os.path.join(OUT_DIR, f"{stem}.golden.json"), "w") as f:
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
        f"wrote {out_path} (v{version}, {os.path.getsize(out_path)} bytes), "
        f"{n_index} cells, {total_features} addresses, {dict_length} B dictionary"
    )


def main():
    os.makedirs(OUT_DIR, exist_ok=True)
    build(2, "address_v2_dict")
    build(3, "address_v3_dict")


if __name__ == "__main__":
    main()
