"""Fast bulk-loader: embed KB notes via nullvec API, then COPY directly to pgvector.

Uses the nullvec embedder HTTP API to get embeddings, then inserts via
PostgreSQL COPY for maximum speed. Avoids nullvec's UPSERT deadlock issue
by going single-connection, no concurrent writers.
"""

from __future__ import annotations

import hashlib
import json
import sqlite3
import sys
import time
import uuid as uuid_mod
from datetime import datetime, timezone
from pathlib import Path
from urllib.request import Request, urlopen

sys.path.insert(0, str(Path(__file__).parent))
from kb_parser import parse_kb_file, chunk_by_headings
from link_resolver import get_yt_transcript, resolve_url

KB_ROOT = Path.home() / "kino" / "kb"
EMBED_URL = "http://localhost:8084/v1/embeddings"
PG_DSN = "host=localhost port=5433 dbname=nullvec user=nullvec password=nullvec"

MAX_CHUNK_CHARS = 768


def embed_text(text: str) -> list[float]:
    """Embed a single text. Returns 4096-d vector (padded)."""
    body = json.dumps({"input": text, "model": "bge-small-en-v1.5"}).encode()
    req = Request(EMBED_URL, data=body, headers={"Content-Type": "application/json"})
    with urlopen(req, timeout=120) as resp:
        data = json.loads(resp.read())
    vec = data["data"][0]["embedding"]
    # Pad to 4096
    vec += [0.0] * (4096 - len(vec))
    return vec


def quantize(vec: list[float]) -> list[int]:
    """Scalar quantize to int8."""
    import math
    fmin, fmax = min(vec), max(vec)
    scale = 127.0 / max(abs(fmin), abs(fmax), 1e-8)
    return [max(-128, min(127, int(round(v * scale)))) for v in vec]


def build_note_requests(parsed: dict) -> list[dict]:
    """Build memory objects, same as ingest.py's _make_note_request."""
    path = parsed["path"]
    text = parsed["text"][:MAX_CHUNK_CHARS]
    fm = parsed["frontmatter"]
    is_log = str(path).startswith("Log/") or str(path).startswith("Daily/")
    should_chunk = is_log or len(text) > MAX_CHUNK_CHARS

    if should_chunk:
        chunks = chunk_by_headings(text)
        requests = []
        for i, chunk in enumerate(chunks):
            content = chunk["content"][:MAX_CHUNK_CHARS]
            if chunk["heading"]:
                content = f"## {chunk['heading']}\n\n{content}"
            requests.append({
                "content": content,
                "metadata": {
                    "entity_type": "kb_note_chunk",
                    "tag": "kb,note-chunk",
                    "source": "kb",
                    "kb_source_path": str(path),
                    "section_heading": chunk["heading"] or "",
                    "chunk_index": i,
                    "chunk_total": len(chunks),
                },
            })
        return requests
    else:
        return [{
            "content": text[:MAX_CHUNK_CHARS],
            "metadata": {
                "entity_type": "kb_note",
                "tag": "kb,note",
                "source": "kb",
                "kb_source_path": str(path),
            },
        }]


def main():
    import psycopg2
    from psycopg2.extras import execute_values

    print("Scanning KB...", flush=True)
    files = []
    import os
    for root, dirs, names in os.walk(str(KB_ROOT)):
        parts = Path(root).relative_to(KB_ROOT).parts
        if ".obsidian" in parts or ".git" in parts or ".raw" in parts or "_meta" in parts or "_templates" in parts or "files" in parts:
            continue
        dirs[:] = [d for d in dirs if not d.startswith(".")]
        for name in names:
            if not name.endswith(".md"):
                continue
            p = Path(root) / name
            files.append(parse_kb_file(p))

    print(f"Found {len(files)} files, building requests...", flush=True)

    # Build all requests
    all_reqs = []
    for parsed in files:
        reqs = build_note_requests(parsed)
        for r in reqs:
            r["kb_source_path"] = str(Path(parsed["path"]).relative_to(KB_ROOT))
        all_reqs.extend(reqs)

    print(f"Built {len(all_reqs)} items to embed", flush=True)

    # Batch embed via bge-small
    print("Embedding...", flush=True)
    t0 = time.time()
    conn = psycopg2.connect(PG_DSN)
    cur = conn.cursor()

    count = 0
    batch_size = 8

    for i in range(0, len(all_reqs), batch_size):
        batch = all_reqs[i:i+batch_size]
        texts = [r["content"] for r in batch]

        # Batch embed via single HTTP call
        body = json.dumps({"input": texts, "model": "bge-small-en-v1.5"}).encode()
        req = Request(EMBED_URL, data=body, headers={"Content-Type": "application/json"})
        try:
            with urlopen(req, timeout=120) as resp:
                data = json.loads(resp.read())
        except Exception as e:
            print(f"  ERR embed at {i}: {e}", flush=True)
            time.sleep(2)
            continue
        vectors = [d["embedding"] for d in data["data"]]
        # Pad each to 4096
        vectors = [v + [0.0] * (4096 - len(v)) for v in vectors]

        # Build COPY rows
        rows = []
        for req, vec in zip(batch, vectors):
            vid = str(uuid_mod.uuid4())
            h = hashlib.sha256(req['content'].encode()).hexdigest()
            vec_str = "[" + ",".join(f"{v:.8f}" for v in vec) + "]"
            vec_int8 = "{" + ",".join(str(v) for v in quantize(vec)) + "}"
            payload = json.dumps({
                "content": req["content"],
                **req["metadata"],
                "user_id": "kino",
                "hash": h,
            })
            # Check for existing by hash before insert
            cur.execute("SELECT 1 FROM embeddings WHERE hash = %s", (h,))
            if cur.fetchone():
                continue
            created_at = datetime.now(timezone.utc).isoformat()
            rows.append((vid, h, vec_str, vec_int8, payload, None, created_at))

        if rows:
            execute_values(
                cur,
                "INSERT INTO embeddings (id, hash, vector, vector_int8, payload, parent_id, created_at) VALUES %s ON CONFLICT (hash) DO NOTHING",
                rows,
                template="(%s, %s, %s::vector, %s::int8[], %s::jsonb, %s::uuid, %s::timestamptz)",
            )
            conn.commit()
            count += len(rows)

        elapsed = time.time() - t0
        rate = count / max(elapsed, 0.01)
        print(f"  [{count}/{len(all_reqs)}] {rate:.1f}/s", flush=True)

    cur.close()
    conn.close()
    elapsed = time.time() - t0
    print(f"Done: {count} items in {elapsed:.0f}s ({count/max(elapsed,0.01):.1f}/s)", flush=True)


if __name__ == "__main__":
    main()
