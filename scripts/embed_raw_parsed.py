#!/usr/bin/env python3
"""
Embed .raw_parsed files into nullvec via POST /memories/batch.

Reads all markdown files from the raw_parsed directory tree, extracts
frontmatter + body, and sends them as batches to nullvec (localhost:8900).

Runs in phases: each top-level category is a phase.
Checkpoints progress so it's resumable.

Usage:
    python3 embed_raw_parsed.py                         # all phases
    python3 embed_raw_parsed.py --phase sessions         # single phase
    python3 embed_raw_parsed.py --dry-run                # count only
"""

import os
import sys
import json
import time
import re
import argparse
from pathlib import Path
from collections import OrderedDict

import requests

NULLVEC = "http://localhost:8900"
BATCH_SIZE = 50  # items per POST
SLEEP_BETWEEN_BATCHES = 1.0  # seconds
PROGRESS_FILE = "/tmp/embed_raw_parsed_progress.json"

RAW_PARSED = Path(os.path.expanduser("~/kino/kb/.raw_parsed"))

# Phase definitions: (name, glob_pattern, metadata_overrides)
# Each phase = a set of files with consistent metadata
PHASES = OrderedDict()

# personal-cli sessions — full claude CLI session transcripts
PHASES["personal-cli-sessions"] = {
    "glob": "personal-cli/sessions/**/*.md",
    "metadata": {"source": "raw_parsed", "category": "personal-cli", "type": "session"},
}

# personal-cli plans — claude CLI generated plans
PHASES["personal-cli-plans"] = {
    "glob": "personal-cli/plans/**/*.md",
    "metadata": {"source": "raw_parsed", "category": "personal-cli", "type": "plan"},
}

# web-export misc — claude.ai web conversations
PHASES["web-misc"] = {
    "glob": "web-export/misc/**/*.md",
    "metadata": {
        "source": "raw_parsed",
        "category": "web-export",
        "type": "conversation",
    },
}

# web-export project dirs
PHASES["web-conduit"] = {
    "glob": "web-export/conduit/**/*.md",
    "metadata": {
        "source": "raw_parsed",
        "category": "web-export",
        "project": "conduit",
        "type": "conversation",
    },
}
PHASES["web-dst"] = {
    "glob": "web-export/dst/**/*.md",
    "metadata": {
        "source": "raw_parsed",
        "category": "web-export",
        "project": "dst",
        "type": "conversation",
    },
}
PHASES["web-xlikes"] = {
    "glob": "web-export/xlikes/**/*.md",
    "metadata": {
        "source": "raw_parsed",
        "category": "web-export",
        "project": "xlikes",
        "type": "conversation",
    },
}
PHASES["web-protectgrandma"] = {
    "glob": "web-export/protectgrandma/**/*.md",
    "metadata": {
        "source": "raw_parsed",
        "category": "web-export",
        "project": "protectgrandma",
        "type": "conversation",
    },
}
PHASES["web-puck"] = {
    "glob": "web-export/puck/**/*.md",
    "metadata": {
        "source": "raw_parsed",
        "category": "web-export",
        "project": "puck",
        "type": "conversation",
    },
}

# work-cli sessions + plans
PHASES["work-cli-sessions"] = {
    "glob": "work-cli/sessions/**/*.md",
    "metadata": {"source": "raw_parsed", "category": "work-cli", "type": "session"},
}
PHASES["work-cli-plans"] = {
    "glob": "work-cli/plans/**/*.md",
    "metadata": {"source": "raw_parsed", "category": "work-cli", "type": "plan"},
}


def extract_content(filepath: Path) -> tuple[str, dict] | None:
    """Extract frontmatter + body from a raw_parsed markdown file.
    Returns (full_text, metadata) or None if empty."""
    text = filepath.read_text(encoding="utf-8", errors="replace")

    # Strip frontmatter
    fm = {}
    body = text
    if text.startswith("---"):
        parts = text.split("---", 2)
        if len(parts) >= 3:
            fm_text = parts[1]
            body = parts[2].strip()
            # Parse frontmatter
            for line in fm_text.strip().split("\n"):
                m = re.match(r"^(\w+):\s*(.*)", line)
                if m:
                    k, v = m.group(1), m.group(2).strip().strip("'\"")
                    fm[k] = v

    if not body or not body.strip():
        return None

    # Full text for embedding = frontmatter title + body
    title = fm.get("title", filepath.stem)
    full_text = f"# {title}\n\n{body}"

    meta = {
        "title": title,
        "path": str(filepath.relative_to(RAW_PARSED)),
        "uuid": fm.get("uuid", ""),
        "created": fm.get("created", ""),
        "message_count": fm.get("message_count", ""),
    }

    return full_text, meta


def load_progress() -> dict:
    if os.path.exists(PROGRESS_FILE):
        try:
            return json.load(open(PROGRESS_FILE))
        except (json.JSONDecodeError, OSError):
            pass
    return {}


def save_progress(progress: dict):
    with open(PROGRESS_FILE, "w") as f:
        json.dump(progress, f, indent=2)


def run_phase(phase: str, config: dict, progress: dict, dry_run: bool = False):
    """Process a single phase, respecting checkpoint."""
    checkpoint_key = f"done_{phase}"
    if progress.get(checkpoint_key):
        print(f"  [{phase}] already complete, skipping")
        return 0

    glob_pattern = config["glob"]
    base_meta = config["metadata"]

    files = sorted(RAW_PARSED.glob(glob_pattern))
    print(f"  [{phase}] {len(files)} files found")

    if dry_run:
        return len(files)

    # Build items
    items = []
    for fpath in files:
        result = extract_content(fpath)
        if result is None:
            continue
        full_text, file_meta = result

        metadata = {**base_meta, **file_meta}
        # Clean up empties
        metadata = {k: v for k, v in metadata.items() if v}

        items.append(
            {
                "messages": [{"role": "user", "content": full_text}],
                "user_id": "kino",
                "metadata": metadata,
            }
        )

    total = len(items)
    print(f"  [{phase}] {total} items to embed (after filtering)")

    # Send in batches
    sent = 0
    errors = 0
    for i in range(0, total, BATCH_SIZE):
        batch = items[i : i + BATCH_SIZE]
        try:
            resp = requests.post(
                f"{NULLVEC}/memories/batch",
                json=batch,
                timeout=30,
            )
            if resp.status_code == 202:
                sent += len(batch)
                result = resp.json()
                dupes = sum(1 for r in result if r.get("dedupe_skipped"))
                accepted = sum(1 for r in result if r.get("message") == "accepted")
                if dupes or accepted != len(batch):
                    print(
                        f"  [{phase}] batch {i // BATCH_SIZE + 1}/{(total + BATCH_SIZE - 1) // BATCH_SIZE}: {accepted} accepted, {dupes} dupes, {len(batch) - accepted - dupes} other"
                    )
                else:
                    print(
                        f"  [{phase}] batch {i // BATCH_SIZE + 1}/{(total + BATCH_SIZE - 1) // BATCH_SIZE}: {len(batch)} ok"
                    )
            else:
                errors += len(batch)
                print(
                    f"  [{phase}] batch {i // BATCH_SIZE + 1} FAILED: HTTP {resp.status_code} {resp.text[:200]}"
                )
        except requests.exceptions.RequestException as e:
            errors += len(batch)
            print(f"  [{phase}] batch {i // BATCH_SIZE + 1} ERROR: {e}")

        if i + BATCH_SIZE < total:
            time.sleep(SLEEP_BETWEEN_BATCHES)

    # Checkpoint
    status = "ok" if errors == 0 else f"partial ({errors} errors)"
    progress[checkpoint_key] = {
        "total": total,
        "sent": sent,
        "errors": errors,
        "status": status,
    }
    save_progress(progress)

    print(f"  [{phase}] done: {sent}/{total} sent ({errors} errors)")
    return total


def main():
    parser = argparse.ArgumentParser(description="Embed .raw_parsed to nullvec")
    parser.add_argument("--phase", help="Run a single phase only")
    parser.add_argument("--dry-run", action="store_true", help="Count only, don't send")
    parser.add_argument("--reset", action="store_true", help="Reset progress")
    args = parser.parse_args()

    if args.reset:
        if os.path.exists(PROGRESS_FILE):
            os.remove(PROGRESS_FILE)
            print("Progress reset")
        else:
            print("No progress file to reset")

    progress = load_progress()

    phases_to_run = PHASES
    if args.phase:
        if args.phase not in PHASES:
            print(f"Unknown phase: {args.phase}")
            print(f"Available: {', '.join(PHASES.keys())}")
            sys.exit(1)
        phases_to_run = {args.phase: PHASES[args.phase]}

    # Verify nullvec is up
    if not args.dry_run:
        try:
            r = requests.get(f"{NULLVEC}/health", timeout=5)
            r.raise_for_status()
            health = r.json()
            queue = health.get("pending_queue", {})
            print(
                f"nullvec healthy | embeddings: {health.get('embeddings_count', '?')} | queue: {queue.get('pending', 0)} pending"
            )
            print()
        except requests.exceptions.RequestException as e:
            print(f"ERROR: nullvec unreachable at {NULLVEC}: {e}")
            sys.exit(1)

    grand_total = 0
    for phase, config in phases_to_run.items():
        n = run_phase(phase, config, progress, args.dry_run)
        grand_total += n

    print()
    print(f"Total: {grand_total} files processed")
    if not args.dry_run:
        progress = load_progress()
        total_sent = sum(
            p.get("sent", 0) for p in progress.values() if isinstance(p, dict)
        )
        total_errors = sum(
            p.get("errors", 0) for p in progress.values() if isinstance(p, dict)
        )
        print(f"Total sent: {total_sent}, errors: {total_errors}")


if __name__ == "__main__":
    main()
