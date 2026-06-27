#!/bin/bash
# Continuously run scanner.py scan in a loop until all mounts are done
# The scanner uses INSERT OR IGNORE so re-running is safe
while true; do
    python3 -u ~/kino/projects/nas-reorg/scanner/scanner.py scan 2>&1
    # Check if we got past the first mount (meaning we completed old external)
    result=$(sqlite3 ~/kino/projects/snac/scanner/nas_index.db "SELECT COUNT(DISTINCT mount_name) FROM files WHERE is_dir=0;" 2>/dev/null)
    if [ "$result" -ge 10 ] 2>/dev/null; then
        echo "All 10 mounts scanned. Done."
        break
    fi
    echo "--- Restarting scan loop ---"
    sleep 2
done
