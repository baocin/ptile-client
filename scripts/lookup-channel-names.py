#!/usr/bin/env python3
"""Look up YouTube channel names for videos missing metadata in transcript_chunks.db.

Uses yt-dlp in parallel to fetch channel info, stores results in a mapping
table, then re-embeds chunks into NULLVEC with corrected creator tags.

Resume-safe: tracks which videos have been looked up and which have been sent.

Usage:
    python3 lookup-channel-names.py [--workers 8] [--limit N] [--dry-run] [--re-embed-only]
"""

import argparse
import json
import sqlite3
import subprocess
import sys
import time
import os
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path
from urllib.request import Request, urlopen
from urllib.error import URLError

# --- Config ---
CHUNKS_DB = Path.home() / ".hermes" / "transcript_chunks.db"
NULLVEC_URL = "http://localhost:8900"
USER_ID = "kino"
BATCH_SIZE = 64
MAPPING_DB = Path.home() / ".hermes" / "channel_mapping.db"

# yt-dlp is IO-bound, not CPU-bound. 8-12 parallel workers is safe.
# YouTube may throttle at higher counts.
DEFAULT_WORKERS = 10


def init_mapping_db():
    """Create mapping table if not exists."""
    db = sqlite3.connect(str(MAPPING_DB))
    db.execute("""
        CREATE TABLE IF NOT EXISTS channel_mapping (
            video_id TEXT PRIMARY KEY,
            channel_name TEXT,
            channel_id TEXT,
            looked_up_at REAL,
            error TEXT
        )
    """)
    db.execute("""
        CREATE TABLE IF NOT EXISTS embed_tracker (
            video_id TEXT PRIMARY KEY,
            chunks_sent INTEGER,
            sent_at REAL
        )
    """)
    db.commit()
    return db


def get_missing_video_ids(limit=None):
    """Get video IDs missing channel info, excluding already-looked-up ones."""
    db = sqlite3.connect(str(CHUNKS_DB))
    query = """
        SELECT DISTINCT json_extract(metadata_json, '$.video_id') as vid
        FROM chunks 
        WHERE (json_extract(metadata_json, '$.channel_name') IS NULL 
           OR json_extract(metadata_json, '$.channel_name') = '')
           AND vid IS NOT NULL
           AND vid != ''
    """
    if limit:
        query += f" LIMIT {limit}"
    missing = {row[0] for row in db.execute(query).fetchall()}
    db.close()

    # Remove already-looked-up (even if errored — don't retry failed)
    map_db = sqlite3.connect(str(MAPPING_DB))
    done = {row[0] for row in map_db.execute("SELECT video_id FROM channel_mapping").fetchall()}
    map_db.close()

    return sorted(missing - done)


def lookup_video(video_id: str) -> dict:
    """Fetch channel info for a single video using yt-dlp.

    Returns dict with video_id, channel_name, channel_id, error (None if ok).
    """
    url = f"https://youtube.com/watch?v={video_id}"
    try:
        result = subprocess.run(
            ["yt-dlp", "--print", "channel", "--print", "channel_id",
             "--skip-download", url, "--no-warnings"],
            capture_output=True, text=True, timeout=30,
            env={**os.environ, "LC_ALL": "C"},
        )
        if result.returncode != 0:
            err = result.stderr.strip()[:200] or "unknown error"
            return {
                "video_id": video_id,
                "channel_name": None,
                "channel_id": None,
                "error": err,
            }

        lines = result.stdout.strip().split("\n")
        channel_name = lines[0].strip() if len(lines) > 0 and lines[0].strip() else None
        channel_id = lines[1].strip() if len(lines) > 1 and lines[1].strip() else None

        return {
            "video_id": video_id,
            "channel_name": channel_name,
            "channel_id": channel_id,
            "error": None,
        }

    except subprocess.TimeoutExpired:
        return {"video_id": video_id, "channel_name": None, "channel_id": None,
                "error": "timeout"}
    except FileNotFoundError:
        return {"video_id": video_id, "channel_name": None, "channel_id": None,
                "error": "yt-dlp not found"}
    except Exception as e:
        return {"video_id": video_id, "channel_name": None, "channel_id": None,
                "error": str(e)[:200]}


def store_results(map_db, results: list[dict]):
    """Batch store lookup results."""
    now = time.time()
    for r in results:
        map_db.execute(
            "INSERT OR REPLACE INTO channel_mapping (video_id, channel_name, channel_id, looked_up_at, error) "
            "VALUES (?, ?, ?, ?, ?)",
            (r["video_id"], r["channel_name"], r["channel_id"], now, r["error"]),
        )
    map_db.commit()


def send_batch(requests_payload: list[dict]) -> list[dict]:
    """Send a batch to NULLVEC."""
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


def maybe_sanitize_title(title: str) -> str:
    """Sanitize channel name for use as a tag."""
    if not title:
        return "unknown"
    # Lowercase, replace spaces with hyphens, remove special chars
    t = title.lower().strip()
    t = "".join(c if c.isalnum() or c in " -_" else "" for c in t)
    t = "-".join(part for part in t.replace("_", " ").split() if part)
    return t[:60] if t else "unknown"


def re_embed_chunks(args, map_db) -> int:
    """Re-embed chunks with corrected channel info. Returns count sent."""
    chunks_db = sqlite3.connect(str(CHUNKS_DB))
    chunks_db.row_factory = sqlite3.Row

    # Get video IDs that have successful lookups and haven't been re-embedded yet
    already_sent = {row[0] for row in map_db.execute(
        "SELECT video_id FROM embed_tracker WHERE chunks_sent > 0"
    ).fetchall()}

    videos_to_embed = [
        row for row in map_db.execute(
            "SELECT video_id, channel_name, channel_id FROM channel_mapping "
            "WHERE channel_name IS NOT NULL AND channel_name != '' "
            "AND error IS NULL"
        ).fetchall()
        if row[0] not in already_sent
    ]

    print(f"  Videos to re-embed: {len(videos_to_embed)}", flush=True)

    total_sent = 0
    for vid_row in videos_to_embed:
        video_id, channel_name, channel_id = vid_row

        rows = chunks_db.execute(
            "SELECT * FROM chunks WHERE json_extract(metadata_json, '$.video_id') = ?",
            (video_id,)
        ).fetchall()

        if not rows:
            continue

        tag = f"{maybe_sanitize_title(channel_name)},transcript"
        batch = []
        batch_count = 0

        for row in rows:
            row_data = dict(row)
            metadata = json.loads(row_data["metadata_json"])
            metadata["channel_name"] = channel_name
            metadata["channel_handle"] = channel_id
            metadata["tag"] = tag

            req = {
                "messages": [{"role": "user", "content": row_data["text"]}],
                "user_id": USER_ID,
                "metadata": metadata,
            }
            batch.append(req)

            if len(batch) >= args.batch_size:
                if not args.dry_run:
                    try:
                        results = send_batch(batch)
                        total_sent += len(results)
                    except Exception as e:
                        print(f"    ERROR batch for {video_id}: {e}", flush=True)
                else:
                    total_sent += len(batch)
                batch_count += len(batch)
                batch = []

        # Flush remaining
        if batch:
            if not args.dry_run:
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

        print(f"    [{total_sent}] {video_id}: {batch_count} chunks tagged '{tag}'", flush=True)

    chunks_db.close()
    return total_sent


def main():
    parser = argparse.ArgumentParser(
        description="Look up YouTube channel names and re-embed chunks"
    )
    parser.add_argument("--workers", type=int, default=DEFAULT_WORKERS,
                        help=f"Parallel yt-dlp workers (default: {DEFAULT_WORKERS})")
    parser.add_argument("--limit", type=int, default=None,
                        help="Max video IDs to look up")
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--skip-lookup", action="store_true",
                        help="Skip yt-dlp lookup, only re-embed with existing mappings")
    parser.add_argument("--batch-size", type=int, default=BATCH_SIZE,
                        help=f"Batch size for NULLVEC POST (default: {BATCH_SIZE})")
    parser.add_argument("--re-embed-only", action="store_true",
                        help="Only do re-embed step (skip lookup)")
    args = parser.parse_args()

    start_time = time.time()
    map_db = init_mapping_db()
    total_sent = 0

    # Step 1: Look up channel names
    if not args.re_embed_only:
        print(f"=== Step 1: Looking up channel names for missing videos ===", flush=True)

        missing_ids = get_missing_video_ids(limit=args.limit)
        total = len(missing_ids)
        print(f"  Videos to look up: {total}", flush=True)

        if total == 0 and not args.skip_lookup:
            print("  No missing videos found. Skipping lookup.", flush=True)

        if args.skip_lookup:
            print("  Skipping lookup (--skip-lookup).", flush=True)
        elif total > 0:
            done_count = 0
            success_count = 0

            # Use ThreadPoolExecutor for parallel lookups
            with ThreadPoolExecutor(max_workers=args.workers) as executor:
                futures = {
                    executor.submit(lookup_video, vid): vid
                    for vid in missing_ids
                }

                buffer = []
                for future in as_completed(futures):
                    result = future.result()
                    buffer.append(result)
                    done_count += 1
                    if result["channel_name"] and not result["error"]:
                        success_count += 1

                    # Flush buffer periodically
                    if len(buffer) >= 50:
                        store_results(map_db, buffer)
                        buffer = []

                    # Progress
                    elapsed = time.time() - start_time
                    rate = done_count / max(elapsed, 0.01)
                    remaining = total - done_count
                    eta = remaining / max(rate, 0.01)
                    print(
                        f"  [{done_count}/{total}] {success_count} found, "
                        f"{rate:.1f}/s eta={eta:.0f}s",
                        flush=True,
                    )

                # Flush remaining buffer
                if buffer:
                    store_results(map_db, buffer)

            print(f"\n  Lookup complete: {total} videos, {success_count} found, "
                  f"{total - success_count} failed/deleted", flush=True)

    # Step 2: Re-embed with corrected tags
    if not args.skip_lookup or args.re_embed_only:
        print(f"\n=== Step 2: Re-embedding chunks with corrected tags ===", flush=True)
        total_sent = re_embed_chunks(args, map_db)

    map_db.close()

    elapsed = time.time() - start_time
    print(f"\n=== Summary ===", flush=True)
    print(f"  Total chunks re-embedded: {total_sent}", flush=True)
    print(f"  Elapsed: {elapsed:.1f}s", flush=True)
    print(f"  Mapping DB: {MAPPING_DB}", flush=True)

    if args.dry_run:
        print(f"\nDry run complete. No data was sent.", flush=True)


if __name__ == "__main__":
    main()
