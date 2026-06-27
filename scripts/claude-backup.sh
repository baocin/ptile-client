#!/bin/bash
set -euo pipefail

LOG="/home/aoi/kino/meta/claude-backup.log"
LOCAL_BASE="/home/aoi/kino/ssh-backups"

TIMESTAMP=$(date +%Y%m%d_%H%M%S)

backup_claude() {
  local host=$1
  local device_name=$2
  local user="mpedersen"
  local remote="/Users/${user}"

  local host_dir="${LOCAL_BASE}/${device_name}/claude_config_backups/${TIMESTAMP}"
  mkdir -p "${host_dir}"

  echo "$(date): Starting backup for ${device_name} (${host})" | tee -a "${LOG}"

  if rsync -avz --delete -e ssh "${host}:${remote}/.claude-work/" "${host_dir}/.claude-work/" >> "${LOG}" 2>&1; then
    echo "  .claude-work: OK" | tee -a "${LOG}"
  else
    echo "  .claude-work: FAILED" | tee -a "${LOG}"
  fi

  if rsync -avz --delete -e ssh "${host}:${remote}/.claude-personal/" "${host_dir}/.claude-personal/" >> "${LOG}" 2>&1; then
    echo "  .claude-personal: OK" | tee -a "${LOG}"
  else
    echo "  .claude-personal: FAILED" | tee -a "${LOG}"
  fi

  echo "$(date): Backup complete for ${device_name}" | tee -a "${LOG}"
  echo "---" | tee -a "${LOG}"
}

# Targets from homelab.md
backup_claude "conduit-macbook" "conduit-macbook-pro"
backup_claude "100.105.113.81" "dst-macbook-pro"

echo "$(date): Full claude-backup.sh run complete" | tee -a "${LOG}"