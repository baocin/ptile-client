# Syncthing

Docker compose setup for Syncthing file sync.

## Quick Start

Already running. The ingest folder is at:

  /home/aoi/data/syncthing/ingest/

## Connecting from Phone/Laptop

1. Install Syncthing on your device (https://syncthing.net)
2. Add remote device with this device ID:

   IFTVITX-WLRQHPC-OEXGKTL-L6Y54HJ-GAWCD6J-CLQVK2E-NW5JVK2-2T6C4QE

3. Share the "Ingest" folder with your device
4. Files dropped into the Ingest folder from anywhere will sync everywhere

## Web UI

  http://omarchy:8384  (or http://localhost:8384 on this machine)

Currently has no GUI auth set. The API key is in the config if needed.

## Management

  cd /home/aoi/kino/docker/syncthing
  docker compose down    # Stop
  docker compose up -d   # Start
  docker compose logs    # View logs

## Storage

- Config: /home/aoi/data/syncthing/config/
- Ingest: /home/aoi/data/syncthing/ingest/
