# Kino — Directory MANIFEST

Root of `/home/aoi/kino`. Core infra, projects, data, and config.

## Top-Level Structure

| Path | Purpose |
|------|---------|
| `kb/` | Obsidian knowledge base vault (primary note-taking) |
| `projects/` | All project repos (timeline, preperc, snac, etc.) |
| `docker/` | Docker compose files + container volumes |
| `template/` | Design system DESIGN.md files, templates |
| `data/` | Geospatial datasets, model files, raw inputs |
| `backups/` | nullvec pg_dump, dapstack exports, app backups |
| `scripts/` | Utility scripts, kb-ingest pipeline |
| `research/` | Exploration notes (numbered topics) |
| `references/` | External reference collections |
| `financial/` | Budget spreadsheets, subscription tracking |
| `services/` | Service configs |
| `ssh-backups/` | Remote machine backup archives |
| `ssh-mounts/` | Remote filesystem mount points |
| `contacts/` | Contact info / address book |
| `reports/` | Generated reports |
| `dashboard/` | Dashboard assets |
| `transcripts/` | Meeting/chat transcripts |
| `prompts/` | Prompt templates |
| `archive/` | Old/archived work |
| `hermes-repo/` | Hermes agent source (local clone) |
| `agent-skills-eval/` | Agent skill evaluation framework |

## Key Project Paths

| Project | Location |
|---------|----------|
| Timeline (Android app) | `projects/timeline/` |
| Timeline worktrees | `projects/timeline-worktrees/` |
| PrePerc (land evaluation) | `projects/preperc/` |
| SNAC (NAS dedup) | `projects/snac/` |
| Ticket-Q (queue dispatch) | `projects/ticket-q/` |
| Steele.Red (website) | `projects/steele.red/` |
| Jiang Xueqin (Chinese learning) | `projects/jiang-xueqin/` |
| X Likes Ingest | `projects/xlikes-ingest/` |
| YT Transcript Pipeline | `projects/yt-transcript-pipeline/` |
| Whimper | `projects/whimper/` |
| Xanadu | `projects/xanadu/` |

## Design Systems

| Path | Contents |
|------|----------|
| `template/designs/` | 71 DESIGN.md files from getdesign.md (YAML frontmatter + prose) |
| `template/designs/{slug}/DESIGN.md` | Per-brand design tokens: colors, typography, components, spacing |

## Infrastructure

| Path | Purpose |
|------|---------|
| `docker/syncthing/` | Syncthing container (port 8384) |
| `docker/volumes/` | Persistent Docker volumes |
| `backups/nullvec/` | Daily pg_dump of pgvector DB (7d + 4 weekly + monthly) |
| `backups/dapstack/` | Daily DapStack ticket exports (same retention scheme) |
| `ssh-backups/conduit-macbook-pro/` | Conduit MacBook backups |
| `ssh-backups/dst-macbook-pro/` | DST MacBook backups |

## Data (Geospatial / ML)

| Path | Contents |
|------|----------|
| `data/sentinel2-tn/` | Sentinel-2 satellite imagery (Tennessee) |
| `data/sentinel2-tn-dry/` | Dry-season S2 imagery |
| `data/sentinel1-tn/` | Sentinel-1 radar imagery |
| `data/srtm-tn/` | SRTM elevation data |
| `data/ssurgo-tn/` | SSURGO soil survey data |
| `data/soilgrids-tn/` | SoilGrids predictions |
| `data/nlcd-tn/` | NLCD land cover |
| `data/openaddresses-tn/` | OpenAddresses point data |
| `data/roads/` | Road network data |
| `data/docling-models/` | Document parsing models |

## Research Log

| Topic | Dir |
|-------|-----|
| Air-to-water extraction | `research/01-air-water/` |
| Muon tomography | `research/02-muon-tomography/` |
| SDR resonances | `research/03-sdr-resonances/` |
| Hermes cron jobs | `research/04-hermes-cron/` |
| Obsidian sync | `research/05-obsidian-sync/` |
| Hermes memory | `research/06-hermes-memory/` |
| MCP server | `research/07-mcp-server/` |
| Claude Code ACP | `research/08-claude-code-acp/` |
| NAS dedup | `research/09-nas-dedup/` |
| Kino voice extract | `research/10-kino-voice-extract/` |

## Knowledge Base (Obsidian)

`kb/` structure:

- `00-inbox/` — Quick captures
- `05-jobs/` — Job applications (conduit, dst)
- `10-projects/` — Project notes (timeline, jiang-xueqin, preperc, snac, hermes)
- `20-areas/` — Area-of-responsibility notes
- `30-resources/` — Reference, papers, talks
- `35-people/` — People notes
- `40-archive/` — Old archived notes
- `mocs/` — Maps of content
- `Log/` — Daily logs (MM-DD-YY.md)
- `kanban/` — Kanban board (folder-as-state markdown)
- `_templates/` — Note templates
- `_meta/` — Meta notes and plans
