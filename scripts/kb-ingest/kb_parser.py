"""Parse Obsidian KB markdown files: frontmatter, body, links, YouTube URLs."""

from __future__ import annotations

import re
import yaml
from pathlib import Path
from typing import Any

# --- Regex patterns ---

YOUTUBE_RE = re.compile(
    r'(?:https?://)?(?:www\.)?'
    r'(?:youtube\.com/watch\?v=|youtu\.be/)'
    r'([\w-]{11})'
)

URL_RE = re.compile(
    r'https?://[^\s\)\]\"\'<>]+'
)

WIKILINK_RE = re.compile(r'\[\[([^\]|]+)(?:\|[^\]]+)?\]\]')

FRONTMATTER_RE = re.compile(r'^---\s*\n(.*?)\n---\s*\n', re.DOTALL)

HEADING_RE = re.compile(r'^##\s+(.+)$', re.MULTILINE)


def parse_frontmatter(text: str) -> dict[str, Any]:
    """Extract YAML frontmatter from markdown text. Returns {} if none."""
    m = FRONTMATTER_RE.match(text)
    if not m:
        return {}
    try:
        return yaml.safe_load(m.group(1)) or {}
    except yaml.YAMLError:
        return {}


def strip_frontmatter(text: str) -> str:
    """Return body text without frontmatter."""
    return FRONTMATTER_RE.sub('', text, count=1)


def resolve_wikilinks(text: str) -> str:
    """Replace [[Wikilink]] with the display text or the link target."""
    def _replace(m: re.Match) -> str:
        target = m.group(1)
        if '|' in m.group(0):
            # [[target|display]] -> display
            return m.group(0).split('|')[1].rstrip(']')
        return target
    return WIKILINK_RE.sub(_replace, text)


def extract_youtube_ids(text: str) -> list[str]:
    """Extract unique YouTube video IDs from text."""
    return list(set(YOUTUBE_RE.findall(text)))


def extract_urls(text: str) -> list[str]:
    """Extract all http/https URLs (excluding localhost/internal)."""
    urls = []
    for url in URL_RE.findall(text):
        url = url.rstrip('.,;:!?)`')
        # Skip internal/local URLs
        if any(skip in url for skip in [
            'localhost', '127.0.', 'gitea', '10.0.', '100.',
            '.local', '192.168.',
        ]):
            continue
        urls.append(url)
    return list(set(urls))


def chunk_by_headings(text: str, min_chars: int = 400) -> list[dict]:
    """Split text into sections by ## headings. Returns [{heading, content}, ...].

    If text is short (< min_chars*2), returns a single chunk with heading=None.
    """
    text = text.strip()
    if not text:
        return []

    sections = list(HEADING_RE.finditer(text))
    if not sections or len(text) < min_chars * 2:
        return [{"heading": None, "content": text}]

    chunks = []
    for i, match in enumerate(sections):
        start = match.start()
        end = sections[i + 1].start() if i + 1 < len(sections) else len(text)
        content = text[start:end].strip()
        if len(content) >= min_chars:
            chunks.append({
                "heading": match.group(1),
                "content": content,
            })
        else:
            # Small section — append to previous chunk if exists
            if chunks:
                chunks[-1]["content"] += "\n\n" + content
            else:
                chunks.append({"heading": match.group(1), "content": content})
    return chunks


def parse_kb_file(path: Path) -> dict[str, Any]:
    """Parse a single KB markdown file into a structured dict.

    Returns:
        {
            "path": str,          # relative path from KB root
            "frontmatter": dict,  # parsed YAML frontmatter
            "body": str,          # full body text (no frontmatter)
            "text": str,          # body text with wikilinks resolved
            "youtube_ids": [str],
            "urls": [str],
            "tags": [str],        # from frontmatter
            "aliases": [str],     # from frontmatter
            "created": str|None,
            "modified": str|None,
        }
    """
    raw = path.read_text(encoding='utf-8', errors='replace')

    fm = parse_frontmatter(raw)
    body = strip_frontmatter(raw)
    clean_text = resolve_wikilinks(body)

    # Normalize tags/aliases
    tags_raw = fm.get('tags', [])
    if isinstance(tags_raw, str):
        tags_raw = [tags_raw]
    tags = [str(t).strip() for t in tags_raw if t]

    aliases_raw = fm.get('aliases', [])
    if isinstance(aliases_raw, str):
        aliases_raw = [aliases_raw]
    aliases = [str(a).strip() for a in aliases_raw if a]

    created = str(fm.get('created', '')) if fm.get('created') else None
    modified = str(fm.get('modified', '')) if fm.get('modified') else None

    return {
        "path": str(path),
        "frontmatter": fm,
        "body": body,
        "text": clean_text,
        "youtube_ids": extract_youtube_ids(raw),
        "urls": extract_urls(raw),
        "tags": tags,
        "aliases": aliases,
        "created": created,
        "modified": modified,
    }
