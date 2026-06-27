# scripts

## Purpose

Utility scripts and cron job helpers — Python and shell scripts for infrastructure maintenance, embedding, backups, health checks, and data processing.

## Ownership

- Kino infrastructure. Some scripts are cron job targets; others are manual utilities.

## Local Contracts

- Python scripts with .py extension. Shell scripts with .sh extension.
- Scripts are standalone unless they import from a project venv.
- embed-\*.py — embedding pipeline scripts (various iterations for mem0, nullvec, transcripts).
- backup scripts (claude-backup.sh, dapstack-backup.sh, nullvec-backup.sh) run on cron.
- health check scripts (site-health-check.sh, local-svc-health-check.sh) run on cron.
- gen-dashboard.py — regenerates dashboard HTML.
- Scripts should be refactored into proper project repos when they become production-grade.
- **pycache**/ — Python cache artifacts, safe to delete.

## Child DOX Index

No subdirectories with their own AGENTS.md.
