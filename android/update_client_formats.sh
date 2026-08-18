#!/usr/bin/env bash
# Record which layer formats this APK is built against.
#
#   ./update_client_formats.sh                       # against the default build
#   PTILES_BUILD=/path/to/states ./update_client_formats.sh 2026-08-18
#
# Writes app/src/main/assets/client_formats.json, which FormatCheck compares
# against the snapshot's manifest.json before MapPackDownloader downloads
# anything. Run it whenever the app is built for release, and whenever the
# snapshot it targets (MapPackDownloader.CURRENT_DATE) changes -- a stale copy
# claims the app can decode formats it has never seen, which is exactly the
# failure the check exists to prevent.
set -euo pipefail

cd "$(dirname "$0")"
PTILES_REPO="${PTILES_REPO:-$HOME/kino/projects/ptiles}"
BUILD_DIR="${PTILES_BUILD:-/mnt/core/kino/ptiles/data/v5/states}"
SNAPSHOT="${1:-$(grep -o 'CURRENT_DATE = "[0-9-]*"' \
  app/src/main/java/com/steele/looky/offline/MapPackDownloader.kt |
  head -1 | grep -o '[0-9-]\{10\}')}"

python3 "$PTILES_REPO/scripts/write_client_manifest.py" "$BUILD_DIR" \
  --client looky-android \
  --snapshot "$SNAPSHOT" \
  --out app/src/main/assets/client_formats.json
