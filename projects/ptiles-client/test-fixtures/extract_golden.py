#!/usr/bin/env python3
"""
Extract golden fixtures for the ptiles-client Rust decoders.

For each of 6 layers (roads, buildings_v8, business, water, parks, rail):
  - opens the real TN.<layer>.ptiles file in ~/kino/data/ptiles/
  - finds an H3 res-7 cell with data near downtown Nashville (36.16, -86.78),
    falling back to the first non-empty index entry
  - reads + decompresses that cell's block (dict-then-plain fallback, mirrors
    ptiles/reader.py::BlockFileReader.read_block_raw)
  - writes the raw decompressed block bytes to golden/<layer>.block.bin
  - decodes the block with the Python reference decoder and writes the
    result to golden/<layer>.golden.json (stable key order, full precision)
  - writes golden/<layer>.meta.json with cell id (hex), source file name,
    and the cell center lat/lon (needed by the v8 buildings decoder)

Run: python3 test-fixtures/extract_golden.py
"""

from __future__ import annotations

import json
import os
import struct
import sys
from dataclasses import asdict, is_dataclass
from enum import IntEnum

import h3
import zstandard as zstd

sys.path.insert(0, os.path.expanduser("~/kino/projects/ptiles"))

from ptiles.codec import (  # noqa: E402
    read_header,
    read_index,
    decode_index_v2,
    binary_search_index,
    decode_merged_block_header,
)

DATA_DIR = os.path.expanduser("~/kino/data/ptiles")
OUT_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "golden")

NASHVILLE_LAT = 36.16
NASHVILLE_LON = -86.78
H3_RES = 7

LAYERS = [
    # (layer_name, file_name, decode_kind)
    ("roads", "TN.roads.ptiles", "roads"),
    ("buildings_v8", "TN.buildings_v8.ptiles", "buildings_v8"),
    ("business", "TN.business.ptiles", "business"),
    ("water", "TN.water.ptiles", "generic"),
    ("parks", "TN.parks.ptiles", "generic"),
    ("rail", "TN.rail.ptiles", "generic"),
]


def open_index(path: str):
    """Parse header + index (v1 or v2), return (header, index, dict_data, relative_offsets)."""
    with open(path, "rb") as f:
        header = read_header(f)
        f.seek(header["dict_offset"])
        dict_data = f.read(header["dict_length"])

        bc = header.get("block_count", 0)
        idx_len = header["index_length"]
        est_v1 = 4 + bc * 17
        v2 = idx_len > est_v1 + bc * 5 and est_v1 > 0

        f.seek(header["index_offset"])
        index_bytes = f.read(idx_len)
        index = decode_index_v2(index_bytes) if v2 else read_index(index_bytes)

        relative_offsets = True
        if index:
            first_off = index[0]["block_offset"]
            relative_offsets = first_off < header["blocks_offset"]

    return header, index, dict_data, relative_offsets, v2


def decompress_raw(path: str, header: dict, entry: dict, dict_data: bytes, relative_offsets: bool) -> bytes:
    """Read + zstd-decompress the physical block on disk (dict-then-plain fallback,
    mirrors ptiles/reader.py::BlockFileReader.read_block_raw)."""
    offset = entry["block_offset"]
    file_offset = header["blocks_offset"] + offset if relative_offsets else offset
    with open(path, "rb") as f:
        f.seek(file_offset)
        compressed = f.read(entry["block_length"])

    raw = None
    if dict_data:
        try:
            d = zstd.ZstdCompressionDict(dict_data)
            dctx = zstd.ZstdDecompressor(dict_data=d)
            raw = dctx.decompress(compressed)
        except Exception:
            raw = None
    if raw is None:
        raw = zstd.ZstdDecompressor().decompress(compressed)
    return raw


def read_block(path: str, header: dict, entry: dict, dict_data: bytes,
                relative_offsets: bool, v2_index: bool) -> bytes:
    """Return the decompressed record-stream bytes for one H3 cell.

    v1 index: one physical block == one cell's record stream, returned as-is.
    v2 index: multiple H3 cells can share one merged physical block; slice out
    just this cell's record range (mirrors
    ptiles/reader.py::BlockFileReader._read_merged_block /
    ptiles/business.py::decode_merged_block_for_cell).
    """
    raw = decompress_raw(path, header, entry, dict_data, relative_offsets)
    if not v2_index:
        return raw

    hdr = decode_merged_block_header(raw)
    cell_offsets = hdr["cell_offsets"]
    record_data_start = hdr["record_data_offset"]
    ci = entry.get("cell_index", 0)
    if ci >= len(cell_offsets):
        return b""
    start_rel = cell_offsets[ci][1]
    if ci + 1 < len(cell_offsets):
        end_rel = cell_offsets[ci + 1][1]
    else:
        end_rel = len(raw) - record_data_start
    abs_start = record_data_start + start_rel
    abs_end = record_data_start + end_rel
    return raw[abs_start:abs_end]


def pick_cell(index: list[dict], is_nonempty) -> dict:
    """Pick the index entry for the res-7 cell nearest downtown Nashville that
    actually decodes to at least one feature; else pick a cell within a
    growing grid_disk; else the first entry in the whole index that decodes
    non-empty.

    `is_nonempty(entry) -> bool` does the real decode-and-check (the index's
    own feature_count field is unreliable -- observed all-zero on business.ptiles).
    """
    target = h3.latlng_to_cell(NASHVILLE_LAT, NASHVILLE_LON, H3_RES)
    target_int = int(target, 16)

    entry = binary_search_index(index, target_int)
    if entry is not None and is_nonempty(entry):
        return entry

    # Expand outward in rings looking for a populated cell near downtown.
    for ring in range(1, 6):
        for cell in h3.grid_disk(target, ring):
            cell_int = int(cell, 16)
            entry = binary_search_index(index, cell_int)
            if entry is not None and is_nonempty(entry):
                return entry

    # Fall back: scan the whole index for the first cell that decodes non-empty.
    for entry in index:
        if is_nonempty(entry):
            return entry

    raise RuntimeError("no non-empty index entry found")


def to_jsonable(obj):
    """Recursively convert dataclasses / IntEnum / tuples to JSON-safe values."""
    if is_dataclass(obj) and not isinstance(obj, type):
        return {k: to_jsonable(v) for k, v in asdict(obj).items()}
    if isinstance(obj, IntEnum):
        return int(obj)
    if isinstance(obj, dict):
        return {k: to_jsonable(v) for k, v in obj.items()}
    if isinstance(obj, (list, tuple)):
        return [to_jsonable(v) for v in obj]
    return obj


def decode(kind: str, block: bytes, header: dict, cell_center_lat: float, cell_center_lon: float):
    version = header["version"]
    if kind == "roads":
        from ptiles.roads import decode_block as decode_roads_block

        roads, intersections = decode_roads_block(block, version)
        return {
            "roads": to_jsonable(list(roads)),
            "intersections": to_jsonable(list(intersections)),
        }
    if kind == "buildings_v8":
        from ptiles.buildings import decode_v8_block

        buildings = decode_v8_block(block, cell_center_lon, cell_center_lat)
        return {"buildings": to_jsonable(list(buildings))}
    if kind == "business":
        from ptiles.business import decode_block as decode_business_block

        records = decode_business_block(block)
        return {"businesses": to_jsonable(records)}
    if kind == "generic":
        # water / parks / rail all expose decode_block(data) -> list[dict]
        module_name = {"water": "water", "parks": "parks", "rail": "rail"}
        raise RuntimeError("generic kind requires layer name; use decode_generic")
    raise ValueError(f"unknown decode kind: {kind}")


def decode_generic(layer: str, block: bytes):
    if layer == "water":
        from ptiles.water import decode_block

        return {"features": to_jsonable(decode_block(block))}
    if layer == "parks":
        from ptiles.parks import decode_block

        return {"features": to_jsonable(decode_block(block))}
    if layer == "rail":
        from ptiles.rail import decode_block

        return {"features": to_jsonable(decode_block(block))}
    raise ValueError(f"unknown generic layer: {layer}")


def main():
    os.makedirs(OUT_DIR, exist_ok=True)
    results = []

    for layer, filename, kind in LAYERS:
        path = os.path.join(DATA_DIR, filename)
        if not os.path.isfile(path):
            print(f"[{layer}] SKIP: {path} not found")
            results.append((layer, "SKIP", "file not found"))
            continue

        header, index, dict_data, relative_offsets, v2 = open_index(path)

        def try_decode(entry):
            """Decode a candidate entry's block; return (decoded, n_features) or None on error."""
            try:
                blk = read_block(path, header, entry, dict_data, relative_offsets, v2)
                cell_hex_ = format(entry["h3_cell"], "x")
                clat, clon = h3.cell_to_latlng(cell_hex_)
                if kind == "generic":
                    d = decode_generic(layer, blk)
                else:
                    d = decode(kind, blk, header, clat, clon)
                n = sum(len(v) for v in d.values() if isinstance(v, list))
                return blk, d, n
            except Exception:
                return None

        cache = {}

        def is_nonempty(entry):
            result = try_decode(entry)
            cache[entry["h3_cell"]] = result
            return result is not None and result[2] > 0

        entry = pick_cell(index, is_nonempty)
        cell_int = entry["h3_cell"]
        cell_hex = format(cell_int, "x")
        cell_lat, cell_lon = h3.cell_to_latlng(cell_hex)

        block, decoded, _n = cache[cell_int]

        block_path = os.path.join(OUT_DIR, f"{layer}.block.bin")
        with open(block_path, "wb") as f:
            f.write(block)

        golden_path = os.path.join(OUT_DIR, f"{layer}.golden.json")
        with open(golden_path, "w") as f:
            json.dump(decoded, f, indent=2, sort_keys=True, ensure_ascii=False)
            f.write("\n")

        meta = {
            "layer": layer,
            "source_file": filename,
            "cell_id_hex": cell_hex,
            "cell_id_int": cell_int,
            "cell_center_lat": cell_lat,
            "cell_center_lon": cell_lon,
            "feature_count_in_index": entry.get("feature_count", 0),
            "block_length_compressed": entry["block_length"],
            "block_length_decompressed": len(block),
            "index_version": 2 if v2 else 1,
            "file_version": header["version"],
        }
        meta_path = os.path.join(OUT_DIR, f"{layer}.meta.json")
        with open(meta_path, "w") as f:
            json.dump(meta, f, indent=2, sort_keys=True)
            f.write("\n")

        # Count features for a quick sanity check.
        n_features = sum(len(v) for v in decoded.values() if isinstance(v, list))
        status = "OK" if n_features > 0 else "EMPTY"
        print(
            f"[{layer}] {status} cell={cell_hex} block={len(block)}B "
            f"features={n_features} golden={os.path.getsize(golden_path)}B"
        )
        results.append((layer, status, n_features))

    print("\n--- summary ---")
    for layer, status, extra in results:
        print(f"{layer:16s} {status:6s} {extra}")


if __name__ == "__main__":
    main()
