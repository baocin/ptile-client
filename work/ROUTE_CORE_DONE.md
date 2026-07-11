# ROUTE_CORE_DONE

PASS (core + wasm + ptiles2 UI)

## Created/modified

- /home/aoi/kino/projects/ptiles-client/core/src/route_graph.rs
- /home/aoi/kino/projects/ptiles-client/core/src/lib.rs (mod + reexport)
- /home/aoi/kino/projects/ptiles-client/wasm/src/lib.rs (`route_from_segments`)
- /home/aoi/kino/projects/steele.red/ptiles2/lib/client/ (wasm-pack rebuild)
- /home/aoi/kino/projects/steele.red/ptiles2/index.html (corridor load + path draw)
- /home/aoi/kino/projects/steele.red/output/ptiles2/ (synced copy)

## Tests

`cargo test -p ptiles-core route_graph --lib` — 4 passed

## UI flow

Route ON → click A → click B → corridorCells (h3-js) → Range decompress →
decode_roads → route_from_segments → solid polyline + km/min status.
One widen (width+2) if null; dashed crow on final fail. Cap 800 cells.

## Notes

- JS owns I/O + corridor cell list; graph/search stay in ptiles-core.
- skipped: highway sidecar, PTILESU, bi-A polish beyond core gate, progressive arterial paint.
