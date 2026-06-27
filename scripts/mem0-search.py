#!/usr/bin/env python3
"""Search mem0: vector (bge+nomic) + FTS, blended."""

import json, urllib.request, sys, re, subprocess
from collections import OrderedDict

PORTS = [("bge", 8888), ("nomic", 8889)]

META_KEYS = {"entity_type", "title", "video_id", "channel_handle", "channel_name",
             "duration_s", "view_count", "like_count", "count", "avg_watch_progress",
             "author_handle", "author_name", "avg_favs", "avg_rts", "repo", "language",
             "star_count", "chat_id", "sender", "subject", "arxiv_id", "paper_title",
             "source", "category", "tag", "platform", "url", "first_seen", "last_seen",
             "watched_count"}

TYPE_LABELS = {
    "video_with_transcript": "VIDEOS",
    "channel_no_transcript": "CHANNELS",
    "watch_history": "CHANNELS",
    "repo": "REPOS",
    "author": "AUTHORS",
    "chat": "CHATS",
    "email": "EMAILS",
    "tweet": "TWEETS",
    "claude_chat": "CHATS",
    "paper": "PAPERS",
}

def _fts(query: str, user_id: str, top_k: int) -> list:
    """Full-text search directly on PostgreSQL."""
    words = query.strip().split()
    if not words:
        return []
    tsq = " & ".join(words)
    sql = f"""
    SELECT id, payload::text
    FROM memories
    WHERE payload->>'user_id'='{user_id}'
      AND to_tsvector('english', coalesce(payload->>'data',''))
         @@ to_tsquery('english', '{tsq}')
    ORDER BY ts_rank(to_tsvector('english', coalesce(payload->>'data','')),
                     to_tsquery('english', '{tsq}')) DESC
    LIMIT {top_k};
    """
    result = subprocess.run(
        ["docker", "exec", "mem0-dev-postgres-1", "psql", "-U", "postgres", "-d", "postgres",
         "-t", "-A", "-F", "|", "-c", sql],
        capture_output=True, text=True, timeout=15
    )
    items = []
    for line in result.stdout.strip().split('\n'):
        if not line or '|' not in line:
            continue
        mem_id, payload_str = line.split('|', 1)
        try:
            payload = json.loads(payload_str)
        except json.JSONDecodeError:
            continue
        meta = {k: payload.get(k) for k in META_KEYS if k in payload}
        if not meta.get("entity_type"):
            src = payload.get("source", "")
            if src == "arxiv-digest":
                meta["entity_type"] = "paper"
            elif src == "manual-injest":
                plat = payload.get("platform", "")
                if "youtube" in plat:
                    meta["entity_type"] = "video_with_transcript" if payload.get("transcript") else "channel_no_transcript"
                elif plat == "x.com":
                    meta["entity_type"] = "author"
                elif plat == "github":
                    meta["entity_type"] = "repo"
                elif plat == "claude.ai":
                    meta["entity_type"] = "chat"
                elif plat == "fastmail":
                    meta["entity_type"] = "email"
        items.append({
            "id": mem_id,
            "memory": payload.get("data", ""),
            "metadata": meta,
            "score": 0.99,
            "_embedder": "FTS",
        })
    return items


def _vector_search(port: int, query: str, top_k: int, user_id: str) -> list:
    """Vector search via mem0 API."""
    body = json.dumps({
        "query": query,
        "filters": {"user_id": user_id},
        "top_k": top_k,
    }).encode()
    req = urllib.request.Request(
        f"http://localhost:{port}/search", data=body,
        headers={"Content-Type": "application/json"}, method="POST")
    try:
        with urllib.request.urlopen(req, timeout=15) as resp:
            return json.loads(resp.read()).get("results", [])
    except Exception:
        return []

# --- Formatters ---

def _fmt_video(m):
    title = m.get("title", "?")
    vid = m.get("video_id", "")
    url = m.get("url") or (f"https://youtu.be/{vid}" if vid else "")
    dur = m.get("duration_s")
    suffix = f" ({dur//60}m{dur%60}s)" if dur else ""
    return f"{title}{suffix}", url

def _fmt_channel(m):
    name = m.get("channel_name") or m.get("title", "?")
    cnt = m.get("count", "")
    progress = m.get("avg_watch_progress", "")
    parts = [name]
    if cnt != "":
        parts.append(f"[{cnt}v" if progress == "" else f"[{cnt}v, {int(progress)}%]")
    elif progress != "":
        parts.append(f"[{int(progress)}% watched]")
    return " ".join(parts), ""

def _fmt_author(m):
    name = m.get("author_name", "?")
    handle = m.get("author_handle", "")
    cnt = m.get("count", 0)
    favs = m.get("avg_favs", 0)
    rts = m.get("avg_rts", 0)
    info = f" @{handle}" if handle else ""
    if cnt:
        info += f" | {cnt}t{chr(9653)}{int(favs)}f/{int(rts)}rt"
    return f"{name}{info}", ""

def _fmt_repo(m):
    repo = m.get("repo", "?")
    lang = m.get("language") or ""
    stars = m.get("star_count", 0)
    info = ""
    if lang or stars:
        parts = []
        if lang: parts.append(lang)
        if stars: parts.append(f"{stars}{chr(9733)}")
        info = f" [{', '.join(parts)}]"
    url = f"https://github.com/{repo}" if "/" in repo else ""
    return f"{repo}{info}", url

def _fmt_chat(m, memory):
    chat_id = m.get("chat_id", "")
    preview = ""
    if memory:
        text = re.sub(r'^Claude conversation:\s*(\[(Claude|You)\]:\s*)?', "", memory).strip()
        preview = text[:120].replace("\n", " ")
        if len(text) > 120:
            preview += "..."
    display = preview if preview else chat_id[:8] if chat_id else "(chat)"
    return display, ""

def _fmt_email(m, memory):
    subj = m.get("subject") or "(no subject)"
    sender = m.get("sender") or ""
    info = f" from {sender}" if sender else ""
    return f"{subj}{info}", ""

def _fmt_paper(m, memory):
    arxiv_id = m.get("arxiv_id", "")
    title = ""
    if memory:
        for line in memory.split("\n"):
            if line.startswith("Title:") and not line.startswith("Title: http"):
                title = line[6:].strip()[:120]
                break
        if not title:
            found = False
            for line in memory.split("\n"):
                line = line.strip()
                if not line: continue
                if "Abstract and text:" in line:
                    found = True; continue
                if found and line and not line.startswith("arXiv:"):
                    title = line[:120]; break
    if not title:
        title = m.get("paper_title") or m.get("title") or arxiv_id or "(paper)"
    display = title
    url = f"https://arxiv.org/abs/{arxiv_id}" if arxiv_id else ""
    return display, url

def _fmt_tweet(m, memory):
    author = m.get("author_name", m.get("author_handle", "?"))
    url = m.get("url", "")
    text = (memory or "").strip().replace("\n", " ")
    return f"{text} [{author}] - {url}", ""


FORMATTERS = {
    "VIDEOS":   lambda m, _: _fmt_video(m),
    "CHANNELS": lambda m, _: _fmt_channel(m),
    "AUTHORS":  lambda m, _: _fmt_author(m),
    "REPOS":    lambda m, _: _fmt_repo(m),
    "CHATS":    lambda m, mem: _fmt_chat(m, mem),
    "EMAILS":   lambda m, mem: _fmt_email(m, mem),
    "PAPERS":   lambda m, mem: _fmt_paper(m, mem),
    "TWEETS":   lambda m, mem: _fmt_tweet(m, mem),
}


def main():
    args = sys.argv[1:]
    if not args or args[0] in ("-h", "--help"):
        print("Usage: mem0-search <query> [top_k=8] [user_id=kino]")
        print("  Blends vector search (bge 8888 + nomic 8889) + FTS (postgres).")
        print("  Results labeled [BGE] [NOMIC] [FTS].")
        return

    query = args[0]
    top_k = int(args[1]) if len(args) > 1 else 8
    user_id = args[2] if len(args) > 2 else "kino"

    all_results = []

    # Vector search: bge + nomic
    for label, port in PORTS:
        results = _vector_search(port, query, top_k, user_id)
        for r in results:
            r["_embedder"] = label.upper()
        all_results.extend(results)

    # FTS: direct postgres
    fts_results = _fts(query, user_id, top_k)
    all_results.extend(fts_results)

    if not all_results:
        print(f"No results for '{query}'")
        return

    # Dedup by identity key
    seen = OrderedDict()
    for r in all_results:
        m = r.get("metadata", {})
        key = (m.get("arxiv_id") or m.get("repo") or m.get("author_handle")
               or m.get("video_id") or m.get("chat_id") or m.get("email_id")
               or r.get("hash") or r["id"])
        if key not in seen or r.get("score", 0) > seen[key].get("score", 0):
            seen[key] = r

    # Group by type
    groups = OrderedDict()
    for r in sorted(seen.values(), key=lambda x: x.get("score", 0), reverse=True):
        m = r.get("metadata", {})
        etype = m.get("entity_type") or (
            "paper" if m.get("source") == "arxiv-digest" else "other"
        )
        label = TYPE_LABELS.get(etype, "OTHER")
        groups.setdefault(label, []).append(r)

    # Count by source
    src_counts = {}
    for r in all_results:
        src = r.get("_embedder", "?")
        src_counts[src] = src_counts.get(src, 0) + 1
    src_summary = ", ".join(f"{k}: {v}" for k, v in sorted(src_counts.items()))

    print(f"Query: {query}")
    print(f"Results: {len(seen)} ({src_summary})")
    print()

    # Organize: group by source first, then by type within each source
    source_order = ["FTS", "BGE", "NOMIC"]
    source_groups = OrderedDict()
    for r in sorted(seen.values(), key=lambda x: x.get("score", 0), reverse=True):
        src = r.get("_embedder", "?").upper()
        if src not in source_order:
            source_order.append(src)
        source_groups.setdefault(src, []).append(r)

    for src in source_order:
        items = source_groups.get(src)
        if not items:
            continue

        # Group items within this source by type
        type_groups = OrderedDict()
        for r in items:
            m = r.get("metadata", {})
            etype = m.get("entity_type") or ("paper" if m.get("source") == "arxiv-digest" else "other")
            label = TYPE_LABELS.get(etype, "OTHER")
            type_groups.setdefault(label, []).append(r)

        print(f"=== {src} ===")
        for group_label, type_items in type_groups.items():
            fmt = FORMATTERS.get(group_label)
            print(f"  --- {group_label} ---")
            for r in type_items:
                m = r.get("metadata", {})
                memory = r.get("memory", "")
                if fmt:
                    display, url = fmt(m, memory)
                else:
                    display = m.get("title") or m.get("paper_title") or m.get("subject") \
                        or m.get("repo") or m.get("author_name") or m.get("entity_type") or "(?)"
                    url = m.get("url") or (f"https://arxiv.org/abs/{m.get('arxiv_id')}" if m.get("arxiv_id") else "")
                suffix = f" - {url}" if url else ""
                print(f"    {display}{suffix}")
            print()
        print()

if __name__ == "__main__":
    main()
