#!/usr/bin/env python3
"""Chunk and embed Professor Jiang Xueqin's transcripts into NULLVEC.

Reads from ~/kino/kb/Projects/Jiang-Xueqin/transcripts/, chunks into
~300-word segments, and embeds with tag "professor jiang,transcript".

Also re-embeds existing Jiang chunks from transcript_chunks.db with corrected tags.

Usage:
    python3 embed-jiang-transcripts.py [--batch-size 32] [--limit N] [--dry-run]
"""

import argparse
import hashlib
import json
import os
import re
import sqlite3
import sys
import time
from pathlib import Path
from urllib.request import Request, urlopen
from urllib.error import URLError

# --- Config ---
NULLVEC_URL = "http://localhost:8900"
TRANSCRIPTS_DIR = Path.home() / "kino" / "kb" / "Projects" / "Jiang-Xueqin" / "transcripts"
CHUNKS_DB = Path.home() / ".hermes" / "transcript_chunks.db"
BATCH_SIZE = 32
USER_ID = "kino"
TARGET_WORDS = 270
OVERLAP_WORDS = 25

# Known Jiang video IDs in the chunks DB (some may lack channel info in metadata)
JIANG_VIDEO_IDS = [
    "Q69YBuJrToM",   # Prof. Jiang Clips channel
    "RH_ycZYH8-s",   # WW3 interview (confirmed by user)
]


def _parse_timestamp(ts: str) -> float:
    """Convert MM:SS or HH:MM:SS to float seconds."""
    parts = ts.strip().split(":")
    if len(parts) == 3:
        return int(parts[0]) * 3600 + int(parts[1]) * 60 + float(parts[2])
    elif len(parts) == 2:
        return int(parts[0]) * 60 + float(parts[1])
    return float(parts[0])


def _format_timestamp(seconds: float) -> str:
    m = int(seconds // 60)
    s = int(seconds % 60)
    return f"{m}:{s:02d}"


def _parse_transcript_file(filepath: Path) -> list[dict]:
    """Parse a Jiang transcript text file into cue dicts.

    Format: each line starts with MM:SS or HH:MM:SS timestamp,
    optionally followed by >> speaker indicator, then text.
    """
    cues = []
    with open(filepath, "r", encoding="utf-8", errors="replace") as f:
        lines = f.readlines()

    # Detect timestamp format
    ts_pattern = re.compile(r"^(\d{1,2}:\d{2}(?::\d{2})?)\s*(>>)?\s*(.*)")

    current_ts = None
    current_text = ""

    for line in lines:
        line = line.strip()
        if not line:
            continue

        m = ts_pattern.match(line)
        if m:
            # Save previous cue
            if current_ts is not None and current_text.strip():
                cues.append({
                    "start_sec": current_ts,
                    "text": current_text.strip(),
                })

            ts_str = m.group(1)
            speaker = m.group(2) or ""
            text = (m.group(3) or "").strip()
            current_ts = _parse_timestamp(ts_str)
            current_text = text
        else:
            # Continuation of previous line
            if current_text:
                current_text += " " + line
            else:
                current_text = line

    # Save final cue
    if current_ts is not None and current_text.strip():
        cues.append({
            "start_sec": current_ts,
            "text": current_text.strip(),
        })

    return cues


def _chunk_transcript(cues: list[dict]) -> list[dict]:
    """Group cues into ~TARGET_WORDS chunks with OVERLAP_WORDS overlap.

    Returns list of {text, start_sec, end_sec, chunk_index, chunk_count}.
    """
    if not cues:
        return []

    chunks = []
    current_cues = []
    current_words = 0
    overlap_cues = []  # trailing cues for overlap

    def _build_chunk(cue_list, idx, total):
        if not cue_list:
            return None
        texts = []
        start_sec = cue_list[0]["start_sec"]
        end_sec = cue_list[-1]["start_sec"]
        for c in cue_list:
            texts.append(c["text"])
        full_text = " ".join(texts)

        return {
            "text": full_text,
            "start_sec": start_sec,
            "end_sec": end_sec,
            "start_time_string": _format_timestamp(start_sec),
            "end_time_string": _format_timestamp(end_sec),
            "chunk_index": idx,
            "chunk_count": total,
        }

    i = 0
    while i < len(cues):
        cue = cues[i]
        cue_words = len(cue["text"].split())

        if current_words + cue_words <= TARGET_WORDS:
            current_cues.append(cue)
            current_words += cue_words
            i += 1
        else:
            if current_cues:
                chunks.append(_build_chunk(current_cues, 0, 0))

            # Build overlap: walk backward to collect OVERLAP_WORDS trailing words
            overlap_words = 0
            overlap_cues = []
            for c in reversed(current_cues):
                wc = len(c["text"].split())
                if overlap_words + wc <= OVERLAP_WORDS:
                    overlap_cues.insert(0, c)
                    overlap_words += wc
                else:
                    break

            current_cues = overlap_cues.copy()
            current_words = overlap_words
            # Don't increment i — re-process current cue with overlap

    # Final chunk
    if current_cues:
        chunks.append(_build_chunk(current_cues, 0, 0))

    # Assign chunk indices and counts
    for idx, c in enumerate(chunks):
        c["chunk_index"] = idx
        c["chunk_count"] = len(chunks)

    return chunks


def _send_batch(requests_payload: list[dict]) -> list[dict]:
    body = json.dumps(requests_payload).encode("utf-8")
    req = Request(
        f"{NULLVEC_URL}/memories/batch",
        data=body,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urlopen(req, timeout=120) as resp:
            return json.loads(resp.read())
    except URLError as e:
        err_body = ""
        try:
            if hasattr(e, "read"):
                err_body = e.read().decode()[:500]
        except Exception:
            pass
        raise RuntimeError(f"NULLVEC error: {e} — {err_body}")


def _build_jiang_request(text: str, metadata: dict) -> dict:
    """Build a MemoryWriteRequest with proper tags for Professor Jiang content."""
    metadata["tag"] = "professor jiang,transcript"
    return {
        "messages": [{"role": "user", "content": text}],
        "user_id": USER_ID,
        "metadata": metadata,
    }


def process_transcript_file(filepath: Path, args) -> int:
    """Process a single Jiang transcript file. Returns number of chunks."""
    cues = _parse_transcript_file(filepath)
    if not cues:
        print(f"  SKIP {filepath.name}: no cues parsed", flush=True)
        return 0

    chunks = _chunk_transcript(cues)
    if not chunks:
        return 0

    # Derive video_id from filename or content
    filename = filepath.stem  # e.g. "2026-05-07-Interview-WW3-Has-Already-Begun"
    video_id = filename  # Use filename as fallback identifier

    metadata_base = {
        "entity_type": "video_transcript_chunk",
        "platform": "youtube",
        "source": "jiang-xueqin-download",
        "video_id": video_id,
        "channel_name": "Prof. Jiang Xueqin",
        "channel_handle": "Professor Jiang",
    }

    batch = []
    sent = 0

    for chunk in chunks:
        meta = {**metadata_base, **chunk}
        # Remove keys that shouldn't be in metadata (used in chunk but not metadata)
        for k in ["text"]:
            meta.pop(k, None)

        req = _build_jiang_request(chunk["text"], meta)
        batch.append(req)

        if len(batch) >= args.batch_size:
            if not args.dry_run:
                try:
                    results = _send_batch(batch)
                    sent += len(results)
                except Exception as e:
                    print(f"  ERROR sending batch for {filepath.name}: {e}", flush=True)
            else:
                sent += len(batch)
            print(f"  [{sent}] {filepath.name}: sent batch of {len(batch)}", flush=True)
            batch = []

    # Flush remaining
    if batch:
        if not args.dry_run:
            try:
                results = _send_batch(batch)
                sent += len(results)
            except Exception as e:
                print(f"  ERROR final batch for {filepath.name}: {e}", flush=True)
        else:
            sent += len(batch)
        print(f"  [{sent}] {filepath.name}: sent final batch of {len(batch)}", flush=True)

    return sent


def re_embed_existing_jiang_chunks(args) -> int:
    """Re-embed existing Jiang chunks from transcript_chunks.db with corrected tags."""
    if not CHUNKS_DB.exists():
        print("No chunks DB found, skipping re-embed", flush=True)
        return 0

    db = sqlite3.connect(str(CHUNKS_DB))
    db.row_factory = sqlite3.Row

    total_sent = 0
    for vid in JIANG_VIDEO_IDS:
        rows = db.execute(
            "SELECT * FROM chunks WHERE json_extract(metadata_json, '$.video_id') = ?",
            (vid,)
        ).fetchall()

        if not rows:
            print(f"  No chunks found for video {vid}", flush=True)
            continue

        print(f"  Found {len(rows)} existing chunks for video {vid}", flush=True)
        batch = []
        batch_count = 0

        for row in rows:
            row = dict(row)
            metadata = json.loads(row["metadata_json"])
            # Add enriched metadata
            metadata["channel_name"] = "Prof. Jiang Xueqin"
            metadata["channel_handle"] = "Professor Jiang"
            metadata["source"] = "jiang-xueqin-download"

            req = _build_jiang_request(row["text"], metadata)
            batch.append(req)

            if len(batch) >= args.batch_size:
                if not args.dry_run:
                    try:
                        results = _send_batch(batch)
                        total_sent += len(results)
                    except Exception as e:
                        print(f"    ERROR batch: {e}", flush=True)
                else:
                    total_sent += len(batch)
                batch_count += len(batch)
                print(f"    [{batch_count}/{len(rows)}] re-embedded {vid}", flush=True)
                batch = []

        # Flush
        if batch:
            if not args.dry_run:
                try:
                    results = _send_batch(batch)
                    total_sent += len(results)
                except Exception as e:
                    print(f"    ERROR final batch: {e}", flush=True)
            else:
                total_sent += len(batch)
            batch_count += len(batch)

        print(f"  Done with video {vid}: {batch_count} chunks re-embedded", flush=True)

    db.close()
    return total_sent


def main():
    parser = argparse.ArgumentParser(
        description="Embed Professor Jiang transcripts into NULLVEC"
    )
    parser.add_argument("--batch-size", type=int, default=BATCH_SIZE)
    parser.add_argument("--limit", type=int, default=None,
                        help="Max transcript files to process")
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--skip-existing", action="store_true",
                        help="Skip re-embedding existing chunks from chunks DB")
    args = parser.parse_args()

    start_time = time.time()
    total_sent = 0

    # Step 1: Re-embed existing Jiang chunks with corrected tags
    if not args.skip_existing:
        print("=== Step 1: Re-embedding existing Jiang chunks with corrected tags ===", flush=True)
        existing_sent = re_embed_existing_jiang_chunks(args)
        total_sent += existing_sent
        print(f"  Re-embedded: {existing_sent} chunks\n", flush=True)

    # Step 2: Process new Jiang transcript files
    print("=== Step 2: Processing Jiang transcript files ===", flush=True)

    if not TRANSCRIPTS_DIR.exists():
        print(f"ERROR: Transcripts dir not found: {TRANSCRIPTS_DIR}", flush=True)
        sys.exit(1)

    txt_files = sorted(TRANSCRIPTS_DIR.glob("*.txt"))
    print(f"Found {len(txt_files)} transcript files", flush=True)

    if args.limit:
        txt_files = txt_files[:args.limit]

    files_sent = 0
    for filepath in txt_files:
        sent = process_transcript_file(filepath, args)
        total_sent += sent
        files_sent += 1
        if sent > 0:
            print(f"  => {filepath.name}: {sent} chunks", flush=True)

    elapsed = time.time() - start_time
    print(f"\n=== Summary ===", flush=True)
    print(f"  Files processed: {files_sent}", flush=True)
    print(f"  Total chunks sent: {total_sent}", flush=True)
    print(f"  Elapsed: {elapsed:.1f}s", flush=True)
    print(f"  Rate: {total_sent / max(elapsed, 0.01):.1f} chunks/s", flush=True)

    if args.dry_run:
        print(f"\nDry run complete. No data was sent.", flush=True)


if __name__ == "__main__":
    main()
