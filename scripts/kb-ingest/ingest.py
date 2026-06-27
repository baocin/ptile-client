"""KB Ingest Pipeline — nightly cron for embedding notes + resolved links into nullvec.

Stages:
  1. Scan KB — walk all .md files, parse frontmatter + body + links
  2. Embed notes — per-file (or per-section for daily logs) to nullvec /memories/batch
  3. Resolve links — download YouTube transcripts, browse URLs
  4. Embed link content — resolved content to nullvec
  5. Track state — SQLite for incremental: skip unchanged by hash

Usage:
    python3 ingest.py                          # scan and ingest all new/changed
    python3 ingest.py --full                   # re-ingest everything
    python3 ingest.py --dry-run                # discover what would be ingested
    python3 ingest.py --link-only              # only resolve and embed links
    python3 ingest.py --note-only              # only embed notes

Metadata schema (stored in nullvec payload):
    kb_source_path: str        # relative path in KB
    kb_tags: [str]             # from frontmatter
    kb_aliases: [str]          # from frontmatter
    kb_created: str|None
    kb_modified: str|None
    entity_type: str           # 'kb_note' | 'kb_note_chunk' | 'kb_link_text' | 'kb_yt_transcript_chunk'
    source: str                # 'kb'
    tag: str                   # 'kb,note' | 'kb,note-chunk' | 'kb,link' | 'kb,youtube-transcript'
    url: str|None              # for link/transcript entries, the source URL
    note_title: str|None       # the KB note's heading (first #)
    section_heading: str|None  # for chunked notes, the ## section heading
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import sqlite3
import subprocess
import sys
import time
import traceback
from concurrent.futures import ThreadPoolExecutor, as_completed
from datetime import datetime, timezone
from pathlib import Path
from typing import Any
from urllib.request import Request, urlopen
from urllib.error import URLError

# --- Local imports ---
sys.path.insert(0, str(Path(__file__).parent))
from kb_parser import parse_kb_file, chunk_by_headings, resolve_wikilinks
from link_resolver import (
    _ensure_state_db, already_embedded, mark_embedded,
    get_yt_transcript, resolve_url,
)

# --- Config ---
KB_ROOT = Path.home() / "kino" / "kb"
NULLVEC_URL = "http://localhost:8900"
BATCH_SIZE = 1
LINK_BATCH_SIZE = 1  # slow down links to avoid embedder OOM
BATCH_DELAY_S = 0.5  # delay between batches for links
USER_ID = "kino"
MAX_WORKERS = 2
MAX_CHUNK_CHARS = 768   # bge-small has 512-token context -> ~768 chars max
                         # nomic has 256-token -> even stricter but padded to 4096

# Paths to exclude from scanning
EXCLUDE_DIRS = {
    ".obsidian", ".git", ".raw_parsed", ".raw",
    "_meta", "_templates",
    "files",  # attachment directory
}

STATE_DIR = Path.home() / ".hermes" / "kb-ingest"
STATE_DB = STATE_DIR / "state.db"


def _get_git_head(repo_root: Path) -> str:
    """Get the current git HEAD commit hash for the KB repo."""
    try:
        result = subprocess.run(
            ["git", "rev-parse", "--short", "HEAD"],
            cwd=str(repo_root),
            capture_output=True, text=True, timeout=5,
        )
        return result.stdout.strip() if result.returncode == 0 else ""
    except Exception:
        return ""


# ─────────────────────────────────────────────
# Nullvec helpers
# ─────────────────────────────────────────────

def _send_batch(payload: list[dict]) -> list[dict]:
    """POST /memories/batch to nullvec."""
    body = json.dumps(payload).encode("utf-8")
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
        raise RuntimeError(f"nullvec error: {e} — {err_body}")


def _note_heading(text: str) -> str | None:
    """Extract the first H1 heading from text."""
    for line in text.split('\n'):
        line = line.strip()
        if line.startswith('# ') and not line.startswith('## '):
            return line[2:].strip()
    return None


# ─────────────────────────────────────────────
# Stage 1: Scan KB
# ─────────────────────────────────────────────

def scan_kb() -> list[dict]:
    """Walk KB_ROOT, parse each .md, return list of parsed file dicts."""
    files = []
    for root, dirs, names in os.walk(str(KB_ROOT)):
        # Skip excluded directories
        rel = Path(root).relative_to(KB_ROOT)
        parts = set(rel.parts)
        if EXCLUDE_DIRS & parts:
            continue
        # Skip hidden dirs
        dirs[:] = [d for d in dirs if not d.startswith('.')]

        for name in names:
            if not name.endswith('.md'):
                continue
            path = Path(root) / name
            try:
                parsed = parse_kb_file(path)
                # Normalize path to relative
                parsed["path"] = str(Path(path).relative_to(KB_ROOT))
                files.append(parsed)
            except Exception as e:
                print(f"  [SKIP] {path}: {e}", flush=True)

    print(f"  Found {len(files)} KB files", flush=True)
    return files


# ─────────────────────────────────────────────
# Stage 2: Build note embedding payloads
# ─────────────────────────────────────────────

def _make_note_request(parsed: dict) -> list[dict]:
    """Build nullvec memory requests for a single KB note.

    Daily logs (in Log/) get section-chunked. All others get one entry.
    Long text is capped at MAX_CHUNK_CHARS per request.
    """
    path = parsed["path"]
    text = parsed["text"][:MAX_CHUNK_CHARS]  # hard cap at 768 chars for 512-token embedder
    body = parsed["body"]
    fm = parsed["frontmatter"]

    note_title = _note_heading(text) or Path(path).stem
    common_meta = {
        "source": "kb",
        "kb_source_path": path,
        "kb_tags": json.dumps(parsed["tags"]),
        "kb_aliases": json.dumps(parsed["aliases"]),
        "kb_created": parsed["created"] or "",
        "kb_modified": parsed["modified"] or "",
        "note_title": note_title or "",
        "kb_ingested_at": datetime.now(timezone.utc).isoformat(),
        "kb_git_hash": _get_git_head(KB_ROOT),
    }
    # Add all frontmatter fields prefixed for discoverability
    for k, v in fm.items():
        if k not in ("tags", "aliases", "created", "modified"):
            common_meta[f"fm_{k}"] = str(v) if not isinstance(v, (str, int, float, bool)) else v

    # Daily logs / long files -> section chunks
    is_log = path.startswith("Log/") or path.startswith("Daily/")
    should_chunk = is_log or len(text) > MAX_CHUNK_CHARS

    if should_chunk:
        chunks = chunk_by_headings(text)
        requests = []
        for i, chunk in enumerate(chunks):
            meta = dict(common_meta)
            meta["entity_type"] = "kb_note_chunk"
            meta["tag"] = "kb,note-chunk"
            meta["section_heading"] = chunk["heading"] or ""
            meta["chunk_index"] = i
            meta["chunk_total"] = len(chunks)
            content = chunk["content"][:MAX_CHUNK_CHARS]
            if chunk["heading"]:
                content = f"## {chunk['heading']}\n\n{content}"
            requests.append({
                "messages": [{"role": "user", "content": content}],
                "user_id": USER_ID,
                "metadata": meta,
            })
        return requests
    else:
        meta = dict(common_meta)
        meta["entity_type"] = "kb_note"
        meta["tag"] = "kb,note"
        text = text[:MAX_CHUNK_CHARS]  # single-file cap
        return [{
            "messages": [{"role": "user", "content": text}],

            "user_id": USER_ID,
            "metadata": meta,
        }]


# ─────────────────────────────────────────────
# Stage 3: Link resolution + embed
# ─────────────────────────────────────────────

def process_youtube(video_id: str, source_path: str) -> dict | None:
    """Download transcript, chunk, build requests. Returns None if failed/duplicate."""
    transcript = get_yt_transcript(video_id)
    if not transcript:
        return None

    # Check dedup
    if already_embedded(transcript, {"video_id": video_id, "source": "kb_link", "source_path": source_path}):
        return None

    # Chunk the transcript (~270 words per chunk like the existing pipeline)
    words = transcript.split()
    chunks = []
    chunk_size = 270
    for i in range(0, len(words), chunk_size):
        chunk_text = " ".join(words[i:i + chunk_size])
        chunks.append({
            "messages": [{"role": "user", "content": chunk_text}],
            "user_id": USER_ID,
            "metadata": {
                "entity_type": "kb_yt_transcript_chunk",
                "tag": "kb,youtube-transcript",
                "source": "kb",
                "kb_source_path": source_path,
                "video_id": video_id,
                "chunk_index": i // chunk_size,
                "source_url": f"https://www.youtube.com/watch?v={video_id}",
            },
        })

    return {"chunks": chunks, "count": len(chunks)}


def process_url(url: str, source_path: str) -> dict | None:
    """Browse URL, extract text, build a single request. Dedup by hash."""
    result = resolve_url(url)
    if result["status"] != "ok" or not result["content"]:
        return None

    content = result["content"]
    title = result["title"]

    if already_embedded(content, {"url": url, "source": "kb_link", "source_path": source_path}):
        return None

    text = f"{title}: {content}" if title else content

    return {
        "requests": [{
            "messages": [{"role": "user", "content": text}],
            "user_id": USER_ID,
            "metadata": {
                "entity_type": "kb_link_text",
                "tag": "kb,link",
                "source": "kb",
                "kb_source_path": source_path,
                "source_url": url,
                "link_title": title or "",
            },
        }],
        "count": 1,
    }


# ─────────────────────────────────────────────
# Main pipeline
# ─────────────────────────────────────────────

class Pipeline:
    """Tracks progress and orchestrates stages."""

    def __init__(self, dry_run: bool = False, full: bool = False):
        self.dry_run = dry_run
        self.full = full
        self.db = _ensure_state_db()

        self.notes_embedded = 0
        self.notes_skipped = 0
        self.links_found = 0
        self.links_embedded = 0
        self.links_skipped = 0
        self.yt_found = 0
        self.yt_embedded = 0
        self.yt_skipped = 0
        self.errors = 0
        self.start_time = time.time()

    def _is_unchanged(self, parsed: dict) -> bool:
        """Check if this note has been embedded with the same content hash."""
        if self.full:
            return False
        text = parsed["text"]
        meta = {"source": "kb", "kb_source_path": parsed["path"]}
        return already_embedded(text, meta)

    def run(self, link_only: bool = False, note_only: bool = False):
        print(f"=" * 60, flush=True)
        print(f"KB Ingest Pipeline", flush=True)
        print(f"  KB root:    {KB_ROOT}", flush=True)
        print(f"  Mode:       {'full' if self.full else 'incremental'}", flush=True)
        print(f"  Scope:      {'links only' if link_only else ('notes only' if note_only else 'all')}", flush=True)
        print(f"  Nullvec:    {NULLVEC_URL}", flush=True)
        print(f"  Dry run:    {self.dry_run}", flush=True)
        print(f"=" * 60, flush=True)

        if not link_only:
            self._stage_notes()

        if not note_only:
            self._stage_links()

        self._summary()

    def _stage_notes(self):
        print(f"\n{'─' * 40}", flush=True)
        print("Stage 1: Scan + embed KB notes", flush=True)
        print(f"{'─' * 40}", flush=True)

        files = scan_kb()
        batch: list[dict] = []
        batch_metadata: list[tuple[str, str, str]] = []  # [(text, entity_type, source_path)]

        for parsed in files:
            # Skip if unchanged
            if self._is_unchanged(parsed):
                self.notes_skipped += 1
                continue

            requests = _make_note_request(parsed)

            for req in requests:
                content = req["messages"][0]["content"]
                etype = req["metadata"]["entity_type"]
                spath = parsed["path"]

                # Double-check dedup per-request
                # (handles the case where a chunk was embedded in a prior partial run)
                if not self.full and already_embedded(content, {"source": "kb", "kb_source_path": spath, "entity_type": etype}):
                    self.notes_skipped += 1
                    continue

                batch.append(req)
                batch_metadata.append((content, etype, spath))

            if len(batch) >= BATCH_SIZE:
                self._flush_notes(batch, batch_metadata)
                batch = []
                batch_metadata = []

        if batch:
            self._flush_notes(batch, batch_metadata)

    def _flush_notes(self, batch: list[dict], metadata: list[tuple[str, str, str]]):
        if self.dry_run:
            print(f"  [DRY RUN] Would embed {len(batch)} note items", flush=True)
            self.notes_embedded += len(batch)
            return

        try:
            results = _send_batch(batch)
            for (content, etype, spath) in metadata:
                mark_embedded(content, entity_type=etype, source_path=spath,
                              metadata={"source": "kb", "kb_source_path": spath, "entity_type": etype})
            self.notes_embedded += len(results)
            elapsed = time.time() - self.start_time
            rate = self.notes_embedded / max(elapsed, 0.01)
            print(f"  [notes] {self.notes_embedded} embedded ({rate:.1f}/s, {self.notes_skipped} skipped)", flush=True)
        except Exception as e:
            print(f"  [notes] ERROR: {e}", flush=True)
            self.errors += 1

    def _stage_links(self):
        print(f"\n{'─' * 40}", flush=True)
        print("Stage 2: Scan for links + resolve + embed", flush=True)
        print(f"{'─' * 40}", flush=True)

        files = scan_kb()

        # Collect unique links per file
        yt_to_process: list[tuple[str, str]] = []        # (video_id, source_path)
        url_to_process: list[tuple[str, str]] = []        # (url, source_path)

        for parsed in files:
            for vid in parsed["youtube_ids"]:
                yt_to_process.append((vid, parsed["path"]))
                self.yt_found += 1
            for url in parsed["urls"]:
                url_to_process.append((url, parsed["path"]))
                self.links_found += 1

        # Deduplicate (same video/link across multiple notes = embed once, file tagged)
        yt_unique = list(set(vid for vid, _ in yt_to_process))
        url_unique = list(set(url for url, _ in url_to_process))

        print(f"  Found {len(yt_to_process)} YouTube references ({len(yt_unique)} unique)", flush=True)
        print(f"  Found {len(url_to_process)} non-YouTube URLs ({len(url_unique)} unique)", flush=True)

        # Process YouTube transcripts (sequential, rate-limited)
        if yt_unique:
            print(f"\n  --- YouTube transcripts ---", flush=True)
            for i, video_id in enumerate(yt_unique):
                # Find source paths for dedup metadata
                source_paths = list(set(sp for vid, sp in yt_to_process if vid == video_id))

                if not self.full and already_embedded("", {"video_id": video_id, "source": "kb_link"}):
                    self.yt_skipped += 1
                    continue

                result = process_youtube(video_id, ",".join(source_paths[:3]))
                if result and result["chunks"]:
                    if self.dry_run:
                        print(f"  [YT][{i+1}/{len(yt_unique)}] {video_id}: {result['count']} chunks (DRY RUN)", flush=True)
                        self.yt_embedded += result["count"]
                    else:
                        try:
                            _send_batch(result["chunks"])
                            for chunk in result["chunks"]:
                                mark_embedded(
                                    chunk["messages"][0]["content"],
                                    entity_type="kb_yt_transcript_chunk",
                                    source_path=source_paths[0] if source_paths else "unknown",
                                    metadata={"video_id": video_id, "source": "kb_link"},
                                )
                            self.yt_embedded += result["count"]
                            print(f"  [YT][{i+1}/{len(yt_unique)}] {video_id}: {result['count']} chunks embedded", flush=True)
                        except Exception as e:
                            print(f"  [YT][{i+1}/{len(yt_unique)}] {video_id}: ERROR {e}", flush=True)
                            self.errors += 1
                else:
                    if result is None:
                        self.yt_skipped += 1

                # Rate limit: be nice to YouTube
                time.sleep(1.0)

        # Process URLs with thread pool
        if url_unique:
            print(f"\n  --- General URLs ---", flush=True)
            url_batch: list[dict] = []
            url_meta: list[tuple[str, str, str]] = []

            for url in url_unique:
                source_paths = list(set(sp for u, sp in url_to_process if u == url))
                if not self.full and already_embedded("", {"url": url, "source": "kb_link"}):
                    self.links_skipped += 1
                    continue

                result = process_url(url, ",".join(source_paths[:3]))
                if result and result["requests"]:
                    url_batch.extend(result["requests"])
                    for req in result["requests"]:
                        url_meta.append((
                            req["messages"][0]["content"],
                            "kb_link_text",
                            ",".join(list(set(sp for u, sp in url_to_process if u == url))[:3]),
                        ))
                else:
                    self.links_skipped += 1

            # Flush all resolved URLs in small batches
            if url_batch:
                for i in range(0, len(url_batch), LINK_BATCH_SIZE):
                    mini_batch = url_batch[i:i + LINK_BATCH_SIZE]
                    mini_meta = url_meta[i:i + LINK_BATCH_SIZE]
                    if self.dry_run:
                        print(f"  [URL] Would embed {len(mini_batch)} items", flush=True)
                        self.links_embedded += len(mini_batch)
                    else:
                        try:
                            _send_batch(mini_batch)
                            for (content, etype, spath) in mini_meta:
                                mark_embedded(content, entity_type=etype, source_path=spath,
                                              metadata={"source": "kb_link"})
                            self.links_embedded += len(mini_batch)
                        except Exception as e:
                            print(f"  [URL] ERROR: {e}", flush=True)
                            self.errors += 1
                    time.sleep(BATCH_DELAY_S)

            print(f"  [links] {self.links_embedded} embedded, {self.links_skipped} skipped", flush=True)

    def _summary(self):
        elapsed = time.time() - self.start_time
        print(f"\n{'=' * 60}", flush=True)
        print(f"SUMMARY", flush=True)
        print(f"{'=' * 60}", flush=True)
        print(f"  Notes embedded:      {self.notes_embedded}", flush=True)
        print(f"  Notes skipped:       {self.notes_skipped}", flush=True)
        print(f"  YouTube transcripts: {self.yt_embedded} chunks (found: {self.yt_found}, skipped: {self.yt_skipped})", flush=True)
        print(f"  Link texts:          {self.links_embedded} (found: {self.links_found}, skipped: {self.links_skipped})", flush=True)
        print(f"  Errors:              {self.errors}", flush=True)
        print(f"  Elapsed:             {elapsed:.1f}s", flush=True)
        if self.dry_run:
            print(f"  MODE: DRY RUN — no data was sent.", flush=True)
        print(f"{'=' * 60}", flush=True)


def main():
    parser = argparse.ArgumentParser(description="KB Ingest Pipeline")
    parser.add_argument("--full", action="store_true", help="Re-ingest everything, ignoring hashes")
    parser.add_argument("--dry-run", action="store_true", help="Discover what would be ingested")
    parser.add_argument("--link-only", action="store_true", help="Only resolve and embed links")
    parser.add_argument("--note-only", action="store_true", help="Only embed notes")
    args = parser.parse_args()

    pipeline = Pipeline(dry_run=args.dry_run, full=args.full)
    pipeline.run(link_only=args.link_only, note_only=args.note_only)


if __name__ == "__main__":
    main()
