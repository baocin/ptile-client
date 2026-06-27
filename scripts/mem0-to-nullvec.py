#!/usr/bin/env python3
"""Transfer all local mem0 memories into NULLVEC.

Reads from mem0's pgvector (PostgreSQL on :8432) and writes to
NULLVEC's /memories/batch endpoint (port 8900).

Truncates texts > 8000 chars to avoid embedder context limits.

Usage:
    python3 mem0-to-nullvec.py [--batch-size 16] [--limit N]
"""

import argparse
import json
import subprocess
import sys
import time
from pathlib import Path
from urllib.request import Request, urlopen
from urllib.error import URLError

NULLVEC_URL = "http://localhost:8900"
BATCH_SIZE = 16
MAX_TEXT_LEN = 8000   # llama.cpp context window safety


def _fetch_memories(offset: int, limit: int) -> list[dict]:
    sql = f"""
        SELECT id, payload
        FROM memories
        WHERE payload != '{{}}'::jsonb
          AND payload->>'data' IS NOT NULL
        ORDER BY id
        OFFSET {offset}
        LIMIT {limit}
    """
    result = subprocess.run(
        ["/usr/bin/psql", "-h", "localhost", "-p", "8432",
         "-U", "postgres", "-d", "postgres",
         "-t", "-A", "-F", "\t",
         "-c", sql.strip()],
        capture_output=True, text=True,
        env={"PGPASSWORD": "postgres", "PATH": "/usr/bin:/bin"},
    )
    if result.returncode != 0:
        raise RuntimeError(f"psql error: {result.stderr[:500]}")

    rows = []
    for line in result.stdout.strip().split("\n"):
        line = line.strip()
        if not line:
            continue
        parts = line.split("\t", 1)
        if len(parts) != 2:
            continue
        rid, payload_str = parts
        payload = json.loads(payload_str)
        data = payload.get("data", "")
        tag = payload.get("tag", "")
        if not data.strip():
            continue
        rows.append({"id": rid, "data": data, "tag": tag or ""})
    return rows


def _send_batch(requests_payload: list[dict]) -> list[dict]:
    body = json.dumps(requests_payload).encode("utf-8")
    req = Request(
        f"{NULLVEC_URL}/memories/batch",
        data=body,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urlopen(req, timeout=180) as resp:
            return json.loads(resp.read())
    except URLError as e:
        err_body = ""
        try:
            if hasattr(e, "read"):
                err_body = e.read().decode()[:500]
        except Exception:
            pass
        raise RuntimeError(f"NULLVEC error: {e} -- {err_body}")


def _build_request(row: dict) -> dict:
    data = row["data"]
    if len(data) > MAX_TEXT_LEN:
        data = data[:MAX_TEXT_LEN]
        # Try to break at a sentence boundary
        last_period = data.rfind(".")
        if last_period > MAX_TEXT_LEN // 2:
            data = data[: last_period + 1]
    
    metadata = {"source": "mem0-transfer", "mem0_id": row["id"]}
    if row["tag"]:
        metadata["tag"] = row["tag"]
    
    return {
        "messages": [{"role": "user", "content": data}],
        "user_id": "kino",
        "metadata": metadata,
    }


def main():
    parser = argparse.ArgumentParser(
        description="Transfer mem0 memories to NULLVEC"
    )
    parser.add_argument("--batch-size", type=int, default=BATCH_SIZE)
    parser.add_argument("--limit", type=int, default=None)
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--start-offset", type=int, default=0)
    args = parser.parse_args()

    # Get total count
    count_result = subprocess.run(
        ["/usr/bin/psql", "-h", "localhost", "-p", "8432",
         "-U", "postgres", "-d", "postgres",
         "-t", "-A",
         "-c", "SELECT COUNT(*) FROM memories WHERE payload != '{}'::jsonb AND payload->>'data' IS NOT NULL"],
        capture_output=True, text=True,
        env={"PGPASSWORD": "postgres", "PATH": "/usr/bin:/bin"},
    )
    total_count = int(count_result.stdout.strip())
    
    pending = total_count - args.start_offset
    print(f"Total: {total_count}, starting at offset {args.start_offset}, pending: {pending}", flush=True)

    if pending <= 0:
        print("Nothing to transfer.", flush=True)
        return

    offset = args.start_offset
    total_sent = 0
    total_errors = 0
    start_time = time.time()

    while True:
        if args.limit and total_sent >= args.limit:
            break

        batch_size = min(args.batch_size, (args.limit - total_sent) if args.limit else args.batch_size)
        rows = _fetch_memories(offset, batch_size)
        if not rows:
            break

        batch = [_build_request(r) for r in rows]

        if not args.dry_run:
            try:
                results = _send_batch(batch)
                total_sent += len(results)
            except Exception as e:
                print(f"  ERROR at offset {offset}: {e}", flush=True)
                total_errors += len(batch)
                offset += len(batch)  # skip bad batch so we don't loop forever
        else:
            total_sent += len(batch)

        offset += len(rows)
        remainder = pending - offset + args.start_offset if not args.dry_run else pending - total_sent
        elapsed = time.time() - start_time
        rate = total_sent / max(elapsed, 0.01)
        eta = max(0, remainder) / max(rate, 0.01) if rate > 0 else 0
        print(
            f"  [{total_sent}/{pending}] {rate:.1f}/s err={total_errors} eta={eta:.0f}s",
            flush=True,
        )

    elapsed = time.time() - start_time
    print(f"\n=== Summary ===", flush=True)
    print(f"  Sent:   {total_sent}", flush=True)
    print(f"  Errors: {total_errors}", flush=True)
    print(f"  Time:   {elapsed:.1f}s", flush=True)
    print(f"  Rate:   {total_sent / max(elapsed, 0.01):.1f}/s", flush=True)


if __name__ == "__main__":
    main()
