#!/usr/bin/env python3
"""Merge per-state {ABBR}.<layer>.ptiles into US.<layer>.ptiles.

ponytail: decompress every zstd frame in blocks section, extract records,
rebucket by H3 cell, re-encode. Works because camera/signals are small (~10K records).
"""

import sys
import os
import struct
import time
from pathlib import Path
from collections import defaultdict

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "../../ptiles/scripts"))
import h3
import zstandard as zstd
from shared import (
    HEADER_SIZE,
    HEADER_STRUCT,
    encode_merged_block,
    encode_index_entry_v2,
    write_header,
    decode_varint,
    encode_varint,
    zigzag_encode,
)

H3_RES = 7
MAGIC = {"camera": b"PTILESC", "signals": b"PTILESS"}


def read_block(data, pos):
    """Read one zstd frame from pos, return (raw_bytes, next_pos)."""
    cb = data[pos:]
    # Try dict decompress with a dummy dict first to find frame size
    # Actually just use memoryview and iterate frames
    dctx = zstd.ZstdDecompressor()
    try:
        raw = dctx.decompress(cb)
        return raw, pos + zstd.frame_content_size(cb) + 9999  # approximate
    except zstd.ZstdError:
        pass
    # Need dict, but we don't know which. Try without
    return None, len(data)


def main():
    layer = sys.argv[1]
    output = Path(sys.argv[2]) if len(sys.argv) > 2 else Path("tiles")
    output.mkdir(parents=True, exist_ok=True)
    tiles_dir = Path("tiles")

    magic = MAGIC.get(layer)
    if not magic:
        print(f"Unknown layer {layer!r}", file=sys.stderr)
        sys.exit(1)
    magic_bytes = magic + b"\x00"

    state_files = sorted(tiles_dir.glob(f"[A-Z][A-Z].{layer}.ptiles"))
    if not state_files:
        print(f"No {layer} files in tiles/", file=sys.stderr)
        sys.exit(1)
    print(f"Merging {len(state_files)} state {layer} files")

    t0 = time.time()
    cell_records = defaultdict(list)
    total_features = 0
    bounds = [float("inf"), float("inf"), float("-inf"), float("-inf")]

    for sf in state_files:
        abbr = sf.stem[:2]
        data = sf.read_bytes()
        hdr = HEADER_STRUCT.unpack(data[:HEADER_SIZE])
        bounds[0] = min(bounds[0], hdr[3])
        bounds[1] = min(bounds[1], hdr[4])
        bounds[2] = max(bounds[2], hdr[5])
        bounds[3] = max(bounds[3], hdr[6])
        tf = hdr[7]
        total_features += tf

        blk_off = hdr[13]
        dd = data[hdr[9] : hdr[9] + hdr[10]]
        ddict = zstd.ZstdCompressionDict(dd)
        dctx = zstd.ZstdDecompressor(dict_data=ddict)

        # Read all zstd frames from blocks_offset to EOF
        frames = []
        pos = blk_off
        while pos < len(data):
            try:
                dctx = zstd.ZstdDecompressor(dict_data=ddict)
                raw = dctx.decompress(data[pos:])
                # Frame consumed all remaining data — single frame
                frames.append(raw)
                break
            except zstd.ZstdError:
                # Multiple frames — advance by frame size
                try:
                    # Use memory stream reader
                    reader = zstd.ZstdDecompressor(dict_data=ddict)
                    chunk_reader = reader.stream_reader(data[pos:])
                    raw = chunk_reader.read()
                    frames.append(raw)
                    break
                except Exception:
                    break

        # Actually just decompress each zstd frame one by one by finding frame boundaries
        # Use the frame iterator
        pos = blk_off
        frame_num = 0
        while pos < len(data):
            # Read frame header to find frame size
            # Try decompression with frame-by-frame approach
            # Actually: zstd frames are self-delimiting. Just try decompress on
            # progressively smaller slices... or use the magic
            # zstd frame magic = 0x28 0xB5 0x2F 0xFD
            if data[pos : pos + 4] != b"\x28\xb5\x2f\xfd":
                break  # not a zstd frame
            try:
                dctx = zstd.ZstdDecompressor(dict_data=ddict)
                raw = dctx.decompress(data[pos:])
                frames.append(raw)
                frame_num += 1
                # The frame consumed up to the next frame or EOF
                # We don't know where this frame ends. Try to find the next frame's start
                # zstd magic, searching for 4 bytes starting from pos+1
                next_start = data.find(b"\x28\xb5\x2f\xfd", pos + 4)
                if next_start < 0:
                    pos = len(data)
                else:
                    pos = next_start
            except zstd.ZstdError:
                break

        # Now we have all frames decompressed. Parse merged-block records.
        for raw in frames:
            p = 0
            clon, clat, cc = struct.unpack_from("<iiI", raw, p)
            p += 12
            cells_in_block = []
            for _ in range(cc):
                cid, off = struct.unpack_from("<QI", raw, p)
                cells_in_block.append((cid, off))
                p += 12
            first_data = 12 + 12 * cc
            for j, (cid, off) in enumerate(cells_in_block):
                next_off = (
                    cells_in_block[j + 1][1] if j + 1 < cc else len(raw) - first_data
                )
                start = first_data + off
                end = first_data + next_off if j + 1 < cc else len(raw)
                rp = start
                while rp < end:
                    rl = struct.unpack_from("<I", raw, rp)[0]
                    rp += 4
                    cell_records[cid].append(raw[rp : rp + rl])
                    rp += rl
        print(f"  {abbr}: {tf} features → {len(frames)} frames", flush=True)

    # Re-encode per cell
    print(f"\nRe-sorting {len(cell_records)} H3 cells...", flush=True)
    sorted_cells = sorted(cell_records.keys())
    cell_blocks = {}
    for c in sorted_cells:
        recs = cell_records[c]
        recs.sort(key=lambda r: decode_varint(r, 0)[0])
        pid = 0
        out = bytearray()
        for rec in recs:
            old_delta, dl = decode_varint(rec, 0)
            old_osm = pid + old_delta
            new_delta = zigzag_encode(old_osm - pid)
            out.extend(encode_varint(new_delta))
            out.extend(rec[dl:])
            pid = old_osm
        cell_blocks[c] = bytes(out)
    del cell_records

    # Pack into merged blocks (8 cells per block)
    print("Building merged blocks...", flush=True)
    bs = 8
    merged_blocks = []
    pi = []
    for i in range(0, len(sorted_cells), bs):
        bch = sorted_cells[i : i + bs]
        cr = []
        for cell in bch:
            cr.append((cell, [cell_blocks[cell]]))
        ch = hex(bch[0])[2:]
        cla, clo = h3.cell_to_latlng(ch)
        blk = encode_merged_block(cr, round(clo * 100000), round(cla * 100000))
        merged_blocks.append(blk)
        for cell in bch:
            pi.append({"h3_cell": cell, "feature_count": 1})
    del cell_blocks

    # Train dictionary + compress
    print("Training zstd dictionary...", flush=True)
    samples = [b for b in merged_blocks if b][:10_000]
    dd = zstd.train_dictionary(512 * 1024, samples).as_bytes()
    zd = zstd.ZstdCompressionDict(dd)
    cctx = zstd.ZstdCompressor(level=12, dict_data=zd)
    cbs = [cctx.compress(b) for b in merged_blocks]

    # Build index
    do = HEADER_SIZE
    dl = len(dd)
    io = do + dl
    il = 4 + len(sorted_cells) * 42
    bo = io + il
    cur = bo
    ie = []
    for idx, cell in enumerate(sorted_cells):
        block_idx = idx // bs
        ie.append(
            {
                "h3_cell": cell,
                "block_offset": cur + sum(len(c) for c in cbs[:block_idx]),
                "block_length": len(cbs[block_idx]) if block_idx < len(cbs) else 0,
                "feature_count": 1,
                "cell_index": idx % bs,
            }
        )
    actual_il = 4 + len(sorted_cells) * 42
    bo = io + actual_il
    cur = bo
    for idx, e in enumerate(ie):
        block_idx = idx // bs
        e["block_offset"] = cur + sum(len(c) for c in cbs[:block_idx])
    ie.sort(key=lambda e: e["h3_cell"])

    out_path = output / f"US.{layer}.ptiles"
    tf = len(sorted_cells)
    with open(out_path, "wb") as f:
        write_header(
            f,
            magic_bytes,
            1,
            bounds[0],
            bounds[1],
            bounds[2],
            bounds[3],
            tf,
            len(merged_blocks),
            do,
            dl,
            io,
            actual_il,
            bo,
        )
        f.write(dd)
        f.write(struct.pack("<I", len(ie)))
        for e in ie:
            f.write(
                encode_index_entry_v2(
                    e["h3_cell"],
                    0,
                    0,
                    0,
                    0,
                    e["block_offset"],
                    e["block_length"],
                    e["feature_count"],
                    e["cell_index"],
                )
            )
        for cb in cbs:
            f.write(cb)

    elapsed = time.time() - t0
    mb = out_path.stat().st_size
    print(
        f"\nWritten {out_path} ({mb:,} bytes, {len(sorted_cells):,} cells, {elapsed:.1f}s)",
        flush=True,
    )


if __name__ == "__main__":
    main()
