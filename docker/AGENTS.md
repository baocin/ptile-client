# docker

## Purpose

Docker Compose configurations and container build definitions for self-hosted services.

## Ownership

- Kino infrastructure. All containers run on this host.

## Local Contracts

- docker-compose.yml — primary compose file for active services (Syncthing, inference servers, monitoring).
- containers/ — per-service Docker build contexts.
- syncthing/ — Syncthing-specific configuration and data bind mounts.
- volumes/ — Docker volume mount points on host filesystem.
- Run `docker compose` from this directory. Do not run random containers outside this compose file without adding them here.

## Child DOX Index

| Directory                 | Has AGENTS.md |
| ------------------------- | ------------- |
| containers/               | yes           |
| containers/unlimited-ocr/ | yes           |
| syncthing/                | no            |
| volumes/                  | no            |
