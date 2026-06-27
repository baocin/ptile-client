#!/bin/bash
# DapStack ticket export backup script
# Exports all tickets via the MCP API, compresses, and rotates with tiered retention.
set -euo pipefail

BACKUP_DIR="/home/aoi/kino/backups/dapstack"
LOG_FILE="/home/aoi/kino/backups/dapstack/backup.log"
RETENTION_DAILY=7
RETENTION_WEEKLY=4
MONTHLY_KEEP=0  # keep all monthly backups
# API key for DapStack (also in ~/.hermes/config.yaml)
export MCP_DAPSTACK_API_KEY="dap_vGjkL9YBWFSYJabiMqvJgKaIQm2-uJ5n1EKsca2U0IDsTPZO"

mkdir -p "$BACKUP_DIR"
mkdir -p "${BACKUP_DIR}/weekly"
mkdir -p "${BACKUP_DIR}/monthly"

TIMESTAMP=$(date +%Y%m%d_%H%M%S)
DAY_OF_WEEK=$(date +%u)       # 1=Mon, 7=Sun
DAY_OF_MONTH=$(date +%d)
BACKUP_FILE="${BACKUP_DIR}/dapstack_${TIMESTAMP}.json.gz"

echo "$(date -Iseconds): Starting DapStack backup to ${BACKUP_FILE}" >> "$LOG_FILE"

# Export all tickets using the MCP tool via Python
# We use the hermes MCP client which handles SSE session negotiation
cd /home/aoi/kino/scripts
if python3 /home/aoi/kino/scripts/dapstack-export.py 2>>"$LOG_FILE" | gzip > "$BACKUP_FILE"; then
    FILESIZE=$(du -h "$BACKUP_FILE" | cut -f1)
    echo "$(date -Iseconds): Backup OK (${FILESIZE})" >> "$LOG_FILE"
else
    EXIT_CODE=$?
    echo "$(date -Iseconds): Backup FAILED (exit code ${EXIT_CODE})" >> "$LOG_FILE"
    exit 1
fi

# --- Rotation: daily ---
find "$BACKUP_DIR" -maxdepth 1 -name "dapstack_*.json.gz" -mtime +"${RETENTION_DAILY}" \
  ! -path "*/weekly/*" ! -path "*/monthly/*" -delete 2>/dev/null || true

# --- Weekly (Sunday): copy to weekly/ keep last 4 ---
if [ "$DAY_OF_WEEK" = "7" ]; then
    WEEKLY_FILE="${BACKUP_DIR}/weekly/dapstack_${TIMESTAMP}.json.gz"
    cp "$BACKUP_FILE" "$WEEKLY_FILE"
    ls -1t "${BACKUP_DIR}/weekly"/dapstack_*.json.gz 2>/dev/null \
      | tail -n +$((RETENTION_WEEKLY + 1)) \
      | xargs -r rm
    echo "$(date -Iseconds): Weekly snapshot saved" >> "$LOG_FILE"
fi

# --- Monthly (1st): copy to monthly/ keep all ---
if [ "$DAY_OF_MONTH" = "01" ]; then
    MONTHLY_FILE="${BACKUP_DIR}/monthly/dapstack_${TIMESTAMP}.json.gz"
    cp "$BACKUP_FILE" "$MONTHLY_FILE"
    echo "$(date -Iseconds): Monthly snapshot saved" >> "$LOG_FILE"
fi

echo "$(date -Iseconds): Rotation complete. Daily: $(ls "${BACKUP_DIR}"/dapstack_*.json.gz 2>/dev/null | wc -l), Weekly: $(ls "${BACKUP_DIR}"/weekly/dapstack_*.json.gz 2>/dev/null | wc -l), Monthly: $(ls "${BACKUP_DIR}"/monthly/dapstack_*.json.gz 2>/dev/null | wc -l)" >> "$LOG_FILE"
