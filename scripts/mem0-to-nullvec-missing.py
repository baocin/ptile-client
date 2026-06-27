#!/usr/bin/env python3
"""Transfer ONLY mem0 entries not yet in NULLVEC.

Uses PostgreSQL temp table for efficient anti-join instead of massive IN clause.
"""

import json
import subprocess
import sys
import time
from urllib.request import Request, urlopen
from urllib.error import URLError

NULLVEC_URL = "http://localhost:8900"
BATCH_SIZE = 16
MAX_TEXT_LEN = 8000
PG_ENV = {"PGPASSWORD": "nullvec", "PATH": "/usr/bin:/bin"}
PG_MEM0_ENV = {"PGPASSWORD": "postgres", "PATH": "/usr/bin:/bin"}


def _psql_nullvec(sql: str) -> str:
    r = subprocess.run(
        ["/usr/bin/psql", "-h", "localhost", "-p", "5433",
         "-U", "nullvec", "-d", "nullvec", "-t", "-A"],
        input=sql, capture_output=True, text=True, env=PG_ENV,
    )
    if r.returncode != 0:
        raise RuntimeError(f"nullvec psql: {r.stderr[:500]}")
    return r.stdout.strip()


def _psql_mem0(sql: str) -> str:
    r = subprocess.run(
        ["/usr/bin/psql", "-h", "localhost", "-p", "8432",
         "-U", "postgres", "-d", "postgres", "-t", "-A"],
        input=sql, capture_output=True, text=True, env=PG_MEM0_ENV,
    )
    if r.returncode != 0:
        raise RuntimeError(f"mem0 psql: {r.stderr[:500]}")
    return r.stdout.strip()


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
    print("Creating temp table with existing NULLVEC mem0_ids...", flush=True)

    # Create temp table loaded with existing mem0_ids
    _psql_nullvec("""
        DROP TABLE IF EXISTS _existing_mem0_ids;
        CREATE TEMP TABLE _existing_mem0_ids (id text);
    """)
    
    # Copy IDs via \COPY from NULLVEC to mem0 (cross-DB)
    # Instead, export to a file and import
    export = _psql_nullvec(
        "SELECT payload->>'mem0_id' FROM embeddings WHERE payload->>'mem0_id' IS NOT NULL"
    )
    ids = [line.strip() for line in export.split("\n") if line.strip()]
    
    # Count total
    total = int(_psql_mem0("SELECT COUNT(*) FROM memories WHERE payload != '{}'::jsonb AND payload->>'data' IS NOT NULL"))
    print(f"Total mem0: {total}, existing NULLVEC: {len(ids)}", flush=True)

    if not ids:
        print("No existing IDs found. Nothing to skip.", flush=True)
        return

    total_sent = 0
    total_errors = 0
    fetched = 0
    start_time = time.time()
    offset = 0

    while True:
        # Fetch missing entries in chunks using a temp table in mem0's DB
        bsize = BATCH_SIZE * 100  # fetch 1600 at a time
        
        sql = f"""
            SELECT m.id, m.payload->>'data' as data, m.payload->>'tag' as tag
            FROM memories m
            WHERE m.payload != '{{}}'::jsonb
              AND m.payload->>'data' IS NOT NULL
            ORDER BY m.id
            OFFSET {offset}
            LIMIT {bsize}
        """
        rows_raw = _psql_mem0(sql)
        
        if not rows_raw:
            break
        
        # Parse rows and filter against existing IDs
        pending = []
        for line in rows_raw.split("\n"):
            parts = line.split("|")
            if len(parts) < 2:
                continue
            rid = parts[0].strip()
            if rid in ids:
                continue
            data = parts[1]
            tag = parts[2] if len(parts) > 2 else ""
            if not data:
                continue
            pending.append({"id": rid, "data": data, "tag": tag.strip() or ""})
        
        offset += bsize
        
        if not pending:
            print(f"  [{offset}/{total}] 0 pending in this chunk, skipping...", flush=True)
            continue
        
        # Batch the pending ones
        for i in range(0, len(pending), BATCH_SIZE):
            batch = [_build_request(r) for r in pending[i:i + BATCH_SIZE]]
            try:
                results = _send_batch(batch)
                total_sent += len(results)
            except Exception as e:
                print(f"  ERROR batch: {e}", flush=True)
                total_errors += len(batch)
            
            fetched += len(batch)
            elapsed = time.time() - start_time
            rate = fetched / max(elapsed, 0.01)
            remaining = max(0, total - len(ids) - fetched)
            eta = remaining / max(rate, 0.01) if rate > 0 else 0
            print(f"  [{offset}/{total}] sent={total_sent} err={total_errors} {rate:.1f}/s eta={eta:.0f}s", flush=True)

    elapsed = time.time() - start_time
    print(f"\n=== Summary ===", flush=True)
    print(f"  Sent:   {total_sent}", flush=True)
    print(f"  Errors: {total_errors}", flush=True)
    print(f"  Time:   {elapsed:.1f}s", flush=True)
    print(f"  Rate:   {total_sent / max(elapsed, 0.01):.1f}/s", flush=True)
    
    # Cleanup
    _psql_nullvec("DROP TABLE IF EXISTS _existing_mem0_ids;")


if __name__ == "__main__":
    main()
