#!/usr/bin/env python3
"""
Self-contained NAS scanner loop.
Walks /run/media/aoi/haze, appends to nas_index.db,
and keeps going until all mounts are done.
Uses INSERT OR IGNORE so re-runs are safe.
"""
import os
import sys
import time
import sqlite3
import json

BASE = "/run/media/aoi/haze"
DB_PATH = os.path.expanduser("~/kino/projects/snac/scanner/nas_index.db")

MOUNTS = [
    "old external",
    "p51_8-25-25",
    "torrents",
    "unified",
    "imac",
    "hino_backup_5-20-25",
    "old backups",
    "dst-macbook-offload",
    "core_mirror",
    "DST iMAC",
]

SKIP_DIRS = {
    ".Trash-1000", ".Trash-0", "$RECYCLE.BIN",
    "System Volume Information", ".stfolder", ".stversions",
    "@Recycle", "@Recently-Snapshot",
}

BATCH_SIZE = 5000

def get_db():
    db = sqlite3.connect(DB_PATH)
    db.execute("PRAGMA journal_mode=WAL")
    db.execute("PRAGMA synchronous=NORMAL")
    return db

def init_db():
    db = get_db()
    db.executescript("""
        CREATE TABLE IF NOT EXISTS files (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            mount_name    TEXT NOT NULL,
            relative_path TEXT NOT NULL,
            full_path     TEXT NOT NULL UNIQUE,
            size          INTEGER NOT NULL,
            mtime         REAL NOT NULL,
            md5           TEXT,
            phash         TEXT,
            image_width   INTEGER,
            image_height  INTEGER,
            is_dir        INTEGER NOT NULL DEFAULT 0,
            scanned_at    TEXT DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_files_mount ON files(mount_name);
        CREATE INDEX IF NOT EXISTS idx_files_size ON files(size);
        CREATE INDEX IF NOT EXISTS idx_files_md5 ON files(md5);
        CREATE INDEX IF NOT EXISTS idx_files_phash ON files(phash);
        CREATE INDEX IF NOT EXISTS idx_files_fullpath ON files(full_path);
    """)
    db.commit()
    db.close()

def walk_mount(db, mount_name):
    mount_path = os.path.join(BASE, mount_name)
    if not os.path.isdir(mount_path):
        print(f"  SKIP {mount_name}: not found")
        return 0

    count = 0
    batch = []
    for root, dirs, files in os.walk(mount_path, followlinks=False):
        dirs[:] = [d for d in dirs if d not in SKIP_DIRS and not d.startswith(".")]

        rel_root = os.path.relpath(root, mount_path)
        if rel_root == ".":
            rel_root = ""

        for name in files:
            full_path = os.path.join(root, name)
            try:
                st = os.lstat(full_path)
                if not os.path.isfile(full_path):
                    continue
                rel_path = os.path.join(rel_root, name) if rel_root else name
                batch.append((mount_name, rel_path, full_path, st.st_size, st.st_mtime, 0))
                count += 1
            except (OSError, PermissionError):
                continue
            if len(batch) >= BATCH_SIZE:
                db.executemany(
                    "INSERT OR IGNORE INTO files (mount_name, relative_path, full_path, size, mtime, is_dir) VALUES (?,?,?,?,?,?)",
                    batch
                )
                db.commit()
                batch = []
            if count % 10000 == 0:
                print(f"    {count} files so far...", flush=True)

        for d in dirs:
            full_path = os.path.join(root, d)
            try:
                st = os.lstat(full_path)
                rel_path = os.path.join(rel_root, d) if rel_root else d
                batch.append((mount_name, rel_path, full_path, st.st_size, st.st_mtime, 1))
            except (OSError, PermissionError):
                continue
            if len(batch) >= BATCH_SIZE:
                db.executemany(
                    "INSERT OR IGNORE INTO files (mount_name, relative_path, full_path, size, mtime, is_dir) VALUES (?,?,?,?,?,?)",
                    batch
                )
                db.commit()
                batch = []

    if batch:
        db.executemany(
            "INSERT OR IGNORE INTO files (mount_name, relative_path, full_path, size, mtime, is_dir) VALUES (?,?,?,?,?,?)",
            batch
        )
        db.commit()

    total = db.execute("SELECT COUNT(*) FROM files WHERE mount_name=?", (mount_name,)).fetchone()[0]
    return total

def main():
    init_db()
    print(f"Scanning {BASE}...")
    print(f"Mounts: {MOUNTS}")
    print()

    for mount in MOUNTS:
        start = time.time()
        print(f"Scanning {mount}...", end=" ", flush=True)
        total = walk_mount(get_db(), mount)
        elapsed = time.time() - start
        print(f"{total} files in {elapsed:.1f}s")

    # Summary
    db = get_db()
    total = db.execute("SELECT COUNT(*) FROM files WHERE is_dir=0").fetchone()[0]
    total_size = db.execute("SELECT COALESCE(SUM(size),0) FROM files WHERE is_dir=0").fetchone()[0]
    mounts = db.execute("SELECT COUNT(DISTINCT mount_name) FROM files WHERE is_dir=0").fetchone()[0]
    print(f"\nDone. {total} files, {total_size/(1024**4):.1f} TB, {mounts} mounts indexed.")
    db.close()

if __name__ == "__main__":
    main()
