"""Resolve URLs to text content for KB link ingestion.

- YouTube: download transcript via youtube-transcript-api
- General URLs: browse and extract text content
- Failed attempts logged and skipped on next run
"""

from __future__ import annotations

import hashlib
import json
import sqlite3
import sys
import time
import traceback
from pathlib import Path
from typing import Any

import urllib.request
from urllib.error import URLError, HTTPError

try:
    from youtube_transcript_api import YouTubeTranscriptApi
    YT_AVAILABLE = True
except ImportError:
    YT_AVAILABLE = False


STATE_DIR = Path.home() / ".hermes" / "kb-ingest"
STATE_DB = STATE_DIR / "state.db"


def _ensure_state_db():
    STATE_DIR.mkdir(parents=True, exist_ok=True)
    db = sqlite3.connect(str(STATE_DB))
    db.execute("PRAGMA journal_mode=WAL")
    db.execute("""
        CREATE TABLE IF NOT EXISTS link_cache (
            url TEXT PRIMARY KEY,
            content TEXT,
            title TEXT,
            status TEXT,          -- 'ok', 'failed', 'skipped'
            error TEXT,
            attempted_at TEXT DEFAULT (datetime('now'))
        )
    """)
    db.execute("""
        CREATE TABLE IF NOT EXISTS embed_tracking (
            hash TEXT PRIMARY KEY,
            entity_type TEXT,      -- 'kb_note', 'kb_note_chunk', 'kb_link_text', 'kb_yt_transcript_chunk'
            source TEXT,           -- 'kb'
            source_path TEXT,      -- KB file path
            embedded_at TEXT DEFAULT (datetime('now'))
        )
    """)
    db.commit()
    return db


def _content_hash(content: str, metadata: dict | None = None) -> str:
    h = hashlib.sha256(content.encode('utf-8'))
    if metadata:
        h.update(json.dumps(metadata, sort_keys=True).encode())
    return h.hexdigest()


def already_embedded(content: str, metadata: dict | None = None) -> bool:
    """Check hash-tracked dedup in state DB."""
    h = _content_hash(content, metadata)
    db = _ensure_state_db()
    row = db.execute("SELECT 1 FROM embed_tracking WHERE hash = ?", (h,)).fetchone()
    db.close()
    return row is not None


def mark_embedded(content: str, entity_type: str, source_path: str,
                  metadata: dict | None = None) -> str:
    """Record in state DB that we embedded this content."""
    h = _content_hash(content, metadata)
    db = _ensure_state_db()
    db.execute(
        "INSERT OR IGNORE INTO embed_tracking (hash, entity_type, source, source_path) VALUES (?, ?, 'kb', ?)",
        (h, entity_type, source_path),
    )
    db.commit()
    db.close()
    return h


def get_yt_transcript(video_id: str) -> str | None:
    """Download YouTube transcript via youtube-transcript-api. Returns text or None."""
    if not YT_AVAILABLE:
        print("  [YT] youtube_transcript_api not installed, skipping", flush=True)
        return None

    try:
        api = YouTubeTranscriptApi()
        transcript = api.fetch(video_id, languages=['en'])
        if not transcript or not transcript.snippets:
            return None
        parts = []
        for s in transcript.snippets:
            text = s.text.strip()
            if text:
                parts.append(text)
        return " ".join(parts) if parts else None
    except Exception as e:
        print(f"  [YT] Failed to get transcript for {video_id}: {e}", flush=True)
        return None


def resolve_url(url: str) -> dict[str, Any]:
    """Resolve a single URL to text content. Returns {content, title, status, error}.

    Uses link_cache to avoid re-browsing already-resolved URLs.
    """
    db = _ensure_state_db()

    # Check cache
    row = db.execute(
        "SELECT content, title, status, error FROM link_cache WHERE url = ?",
        (url,),
    ).fetchone()
    if row:
        return {
            "content": row[0],
            "title": row[1],
            "status": row[2],
            "error": row[3],
        }

    db.close()

    print(f"  [URL] Resolving: {url}", flush=True)

    try:
        req = urllib.request.Request(
            url,
            headers={
                "User-Agent": "Mozilla/5.0 (compatible; KbIngest/1.0)",
                "Accept": "text/html,text/plain,*/*",
            },
        )
        with urllib.request.urlopen(req, timeout=15) as resp:
            raw = resp.read().decode('utf-8', errors='replace')
            title = _extract_title(raw)
            content = _extract_plaintext(raw)

            result = {
                "content": content[:10000],  # cap at 10K chars
                "title": title,
                "status": "ok",
                "error": None,
            }

            # Cache it
            db = _ensure_state_db()
            db.execute(
                "INSERT OR REPLACE INTO link_cache (url, content, title, status) VALUES (?, ?, ?, 'ok')",
                (url, result["content"], result["title"]),
            )
            db.commit()
            db.close()
            return result

    except (HTTPError, URLError, OSError, ValueError) as e:
        error_msg = str(e)[:200]
        print(f"  [URL] Failed: {error_msg}", flush=True)
        result = {
            "content": None,
            "title": None,
            "status": "failed",
            "error": error_msg,
        }
        db = _ensure_state_db()
        db.execute(
            "INSERT OR REPLACE INTO link_cache (url, content, title, status, error) VALUES (?, ?, ?, 'failed', ?)",
            (url, None, None, error_msg),
        )
        db.commit()
        db.close()
        return result


def _extract_title(html: str) -> str | None:
    """Extract <title> from HTML."""
    import re
    m = re.search(r'<title[^>]*>(.*?)</title>', html, re.IGNORECASE | re.DOTALL)
    if m:
        return m.group(1).strip()
    return None


def _extract_plaintext(html: str) -> str:
    """Very basic HTML-to-text extraction."""
    import re
    # Remove script/style
    text = re.sub(r'<(script|style)[^>]*>.*?</\1>', '', html, flags=re.DOTALL | re.IGNORECASE)
    # Remove tags
    text = re.sub(r'<[^>]+>', ' ', text)
    # Decode entities
    text = text.replace('&nbsp;', ' ').replace('&amp;', '&').replace('&lt;', '<').replace('&gt;', '>')
    # Collapse whitespace
    text = re.sub(r'\s+', ' ', text).strip()
    return text[:10000]
