#!/bin/bash
# NULLVEC pgvector backup script
# Dumps the PostgreSQL database from the running container, compresses, and rotates.
set -euo pipefail

BACKUP_DIR="/home/aoi/kino/backups/nullvec"
LOG_FILE="/home/aoi/kino/backups/nullvec/backup.log"
CONTAINER="nullvec-pgvector"
DB_USER="nullvec"
DB_NAME="nullvec"
RETENTION_DAYS=7
WEEKLY_KEEP=4
MONTHLY_KEEP=0  # keep all monthly snapshots

mkdir -p "$BACKUP_DIR"
mkdir -p "${BACKUP_DIR}/weekly"
mkdir -p "${BACKUP_DIR}/monthly"

TIMESTAMP=$(date +%Y%m%d_%H%M%S)
DAY_OF_WEEK=$(date +%u)       # 1=Mon, 7=Sun
DAY_OF_MONTH=$(date +%d)
BACKUP_FILE="${BACKUP_DIR}/nullvec_${TIMESTAMP}.sql.gz"

echo "$(date -Iseconds): Starting backup to ${BACKUP_FILE}" >> "$LOG_FILE"

# Dump and compress
if docker exec "$CONTAINER" pg_dump -U "$DB_USER" -d "$DB_NAME" --no-owner --no-acl 2>>"$LOG_FILE" \
  | gzip > "$BACKUP_FILE"; then
    echo "$(date -Iseconds): Backup OK ($(du -h "$BACKUP_FILE" | cut -f1))" >> "$LOG_FILE"
else
    echo "$(date -Iseconds): Backup FAILED" >> "$LOG_FILE"
    exit 1
fi

# Rotate: remove dailies older than RETENTION_DAYS, keep weekly snapshots
find "$BACKUP_DIR" -name "nullvec_*.sql.gz" -mtime +"${RETENTION_DAYS}" -delete 2>/dev/null || true

# Tag Sunday backups as weekly (don't auto-delete these)
if [ "$DAY_OF_WEEK" = "7" ]; then
    cp "$BACKUP_FILE" "${BACKUP_DIR}/weekly/nullvec_${TIMESTAMP}.sql.gz"
    mkdir -p "${BACKUP_DIR}/weekly"
    # Keep only the last WEEKLY_KEEP weekly backups
    ls -1t "${BACKUP_DIR}/weekly"/nullvec_*.sql.gz 2>/dev/null \
      | tail -n +$((WEEKLY_KEEP + 1)) \
      | xargs -r rm
    echo "$(date -Iseconds): Weekly snapshot saved" >> "$LOG_FILE"
fi

# Tag 1st-of-month backups as monthly (keep all)
if [ "$DAY_OF_MONTH" = "01" ]; then
    cp "$BACKUP_FILE" "${BACKUP_DIR}/monthly/nullvec_${TIMESTAMP}.sql.gz"
    echo "$(date -Iseconds): Monthly snapshot saved" >> "$LOG_FILE"
fi

echo "$(date -Iseconds): Rotation complete. Current backups: $(ls "${BACKUP_DIR}"/nullvec_*.sql.gz 2>/dev/null | wc -l) files, Weekly: $(ls "${BACKUP_DIR}"/weekly/nullvec_*.sql.gz 2>/dev/null | wc -l), Monthly: $(ls "${BACKUP_DIR}"/monthly/nullvec_*.sql.gz 2>/dev/null | wc -l)" >> "$LOG_FILE"
