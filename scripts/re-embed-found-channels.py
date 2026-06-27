#!/usr/bin/env python3
"""Re-embed found-but-not-yet-embedded videos into NULLVEC with creator tags.

Reads channel_mapping.db for found videos that haven't been embedded yet,
then POSTs their chunks to NULLVEC with corrected metadata.

Usage:
    python3 re-embed-found-channels.py [--dry-run]
"""

import json
import sqlite3
import sys
import time
from pathlib import Path
from urllib.request import Request, urlopen
from urllib.error import URLError

CHUNKS_DB = Path.home() / ".hermes" / "transcript_chunks.db"
MAPPING_DB = Path.home() / ".hermes" / "channel_mapping.db"
NULLVEC_URL = "http://localhost:8900"
USER_ID = "kino"
BATCH_SIZE = 64


def send_batch(requests_payload: list[dict]) -> list[dict]:
    body = json.dumps(requests_payload).encode("utf-8")
    req = Request(f"{NULLVEC_URL}/memories/batch", data=body,
                  headers={"Content-Type": "application/json"}, method="POST")
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


def sanitize_title(title: str) -> str:
    if not title:
        return "unknown"
    t = title.lower().strip()
    t = "".join(c if c.isalnum() or c in " -_" else "" for c in t)
    t = "-".join(part for part in t.replace("_", " ").split() if part)
    return t[:60] if t else "unknown"


def main():
    dry_run = "--dry-run" in sys.argv

    map_db = sqlite3.connect(str(MAPPING_DB))
    chunks_db = sqlite3.connect(str(CHUNKS_DB))
    chunks_db.row_factory = sqlite3.Row

    # Get already-embedded video IDs
    already_sent = {row[0] for row in map_db.execute(
        "SELECT video_id FROM embed_tracker WHERE chunks_sent > 0"
    ).fetchall()}

    # Get found but not embedded
    to_embed = [row for row in map_db.execute(
        "SELECT video_id, channel_name, channel_id FROM channel_mapping "
        "WHERE channel_name IS NOT NULL AND channel_name != '' "
        "AND error IS NULL"
    ).fetchall() if row[0] not in already_sent]

    print(f"Videos to re-embed: {len(to_embed)}", flush=True)
    if not to_embed:
        print("Nothing to do.", flush=True)
        map_db.close()
        chunks_db.close()
        return

    total_sent = 0
    start_time = time.time()

    for vid_row in to_embed:
        video_id, channel_name, channel_id = vid_row
        rows = chunks_db.execute(
            "SELECT * FROM chunks WHERE json_extract(metadata_json, '$.video_id') = ?",
            (video_id,)
        ).fetchall()

        if not rows:
            continue

        tag = f"{sanitize_title(channel_name)},transcript"
        batch = []
        batch_count = 0

        for row in rows:
            row_data = dict(row)
            metadata = json.loads(row_data["metadata_json"])
            metadata["channel_name"] = channel_name
            metadata["channel_handle"] = channel_id
            metadata["tag"] = tag

            req = {"messages": [{"role": "user", "content": row_data["text"]}],
                   "user_id": USER_ID, "metadata": metadata}
            batch.append(req)

            if len(batch) >= BATCH_SIZE:
                if not dry_run:
                    try:
                        results = send_batch(batch)
                        total_sent += len(results)
                    except Exception as e:
                        print(f"    ERROR batch for {video_id}: {e}", flush=True)
                else:
                    total_sent += len(batch)
                batch_count += len(batch)
                batch = []

        if batch:
            if not dry_run:
                try:
                    results = send_batch(batch)
                    total_sent += len(results)
                except Exception as e:
                    print(f"    ERROR final batch for {video_id}: {e}", flush=True)
            else:
                total_sent += len(batch)
            batch_count += len(batch)

        # Mark as sent
        map_db.execute(
            "INSERT OR REPLACE INTO embed_tracker (video_id, chunks_sent, sent_at) VALUES (?, ?, ?)",
            (video_id, batch_count, time.time()),
        )
        map_db.commit()

        elapsed = time.time() - start_time
        rate = total_sent / max(elapsed, 0.01)
        print(f"  [{total_sent}] {video_id}: {batch_count} chunks tagged '{tag}'  ({rate:.0f}/s)", flush=True)

    elapsed = time.time() - start_time
    chunks_db.close()
    map_db.close()

    print(f"\n=== Summary ===", flush=True)
    print(f"  Videos embedded: {len(to_embed)}", flush=True)
    print(f"  Chunks sent: {total_sent}", flush=True)
    print(f"  Elapsed: {elapsed:.1f}s", flush=True)
    print(f"  Rate: {total_sent / max(elapsed, 0.01):.1f} chunks/s", flush=True)

    if dry_run:
        print(f"\nDry run complete. No data sent.", flush=True)


if __name__ == "__main__":
    main()
