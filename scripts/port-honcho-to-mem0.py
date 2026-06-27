#!/usr/bin/env python3
"""Port Honcho documents to dual Mem0 (bge 384-dim + nomic 768-dim)."""
import json, sys, time
import psycopg
import requests
from datetime import datetime, timezone

HONCHO_DSN = "host=localhost port=5433 dbname=postgres user=postgres password=postgres"
MEM0_BGE = "http://localhost:8888"
MEM0_NOMIC = "http://localhost:8889"

BATCH_SIZE = 10
SLEEP_BETWEEN = 0.0  # fast mode since infer=false

def get_honcho_docs():
    conn = psycopg.connect(HONCHO_DSN)
    cur = conn.cursor()
    cur.execute("""
        SELECT id, content, observer, observed, workspace_name, session_name, level, created_at, internal_metadata
        FROM documents
        WHERE deleted_at IS NULL
        ORDER BY created_at ASC
    """)
    rows = cur.fetchall()
    cur.close()
    conn.close()
    return rows

def add_to_mem0(base_url, content, user_id):
    """Add a memory to Mem0. Returns True on success."""
    payload = {
        "messages": [{"role": "user", "content": content}],
        "user_id": user_id or "unknown",
        "infer": False,
    }
    try:
        resp = requests.post(f"{base_url}/memories", json=payload, timeout=30)
        if resp.status_code in (200, 201):
            return True
        else:
            print(f"   ERROR {resp.status_code}: {resp.text[:80]}")
            return False
    except Exception as e:
        print(f"   EXCEPTION: {e}")
        return False

def main():
    docs = get_honcho_docs()
    total = len(docs)
    print(f"Found {total} Honcho documents to port")

    # Track per-user progress
    stats = {"bge_ok": 0, "nomic_ok": 0, "bge_fail": 0, "nomic_fail": 0}

    for i, (doc_id, content, observer, observed, workspace, session, level, created_at, metadata) in enumerate(docs):
        user_id = observer or "unknown"
        content_preview = content[:60].replace("\n", " ")
        print(f"[{i+1}/{total}] user={user_id}: \"{content_preview}...\"", end="")
        sys.stdout.flush()

        # Add to bge Mem0
        ok = add_to_mem0(MEM0_BGE, content, user_id)
        if ok:
            stats["bge_ok"] += 1
        else:
            stats["bge_fail"] += 1

        # Add to nomic Mem0
        ok = add_to_mem0(MEM0_NOMIC, content, user_id)
        if ok:
            stats["nomic_ok"] += 1
        else:
            stats["nomic_fail"] += 1

        print(f"  (bge={'ok' if stats['bge_ok'] > sum([stats[k] for k in stats if 'bge' in k])-1 else 'fail'}, nomic={'ok' if stats['nomic_ok'] > sum([stats[k] for k in stats if 'nomic' in k])-1 else 'fail'})")

        # Progress every N
        if (i + 1) % BATCH_SIZE == 0:
            elapsed = (i + 1) * SLEEP_BETWEEN if SLEEP_BETWEEN > 0 else 0
            eta = (total - i - 1) / max(BATCH_SIZE, 1) * SLEEP_BETWEEN if SLEEP_BETWEEN > 0 else 0
            print(f"  PROGRESS: {i+1}/{total} | bge: {stats['bge_ok']}ok/{stats['bge_fail']}fail | nomic: {stats['nomic_ok']}ok/{stats['nomic_fail']}fail")

        time.sleep(SLEEP_BETWEEN)

    print(f"\nDONE: {total} documents processed")
    print(f"  bge:  {stats['bge_ok']} ok, {stats['bge_fail']} fail")
    print(f"  nomic: {stats['nomic_ok']} ok, {stats['nomic_fail']} fail")

if __name__ == "__main__":
    main()
