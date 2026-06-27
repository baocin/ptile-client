# docker/containers

## Purpose

Per-service Docker build contexts for self-hosted inference servers, monitoring tools, and internal services.

## Ownership

- Kino infrastructure. Each subdirectory is a build context for one service.

## Local Contracts

- Each subdirectory contains a Dockerfile and any service-specific files.
- Containers are referenced by docker-compose.yml at the parent level.
- Do not add containers here without adding them to the docker-compose.yml.

## Child DOX Index

No subdirectories with their own AGENTS.md.
