# GOAL: publish the rebuilt data, then finish the Rookery API

The reader work is done and verified. What blocks everything now is data: the
host still serves files whose index records **one point per cell**, so every
fix below is reading a file that is ~95% empty.

This directory has no git dir of its own — it is tracked inside `~/kino`.
Commit there. New files under `projects/` need `git add -f`: `.gitignore:57`
in `~/kino` ignores `/projects/`, so untracked additions are refused while
tracked files stage normally.

## Task 1 — Publish the rebuilt national files

`~/kino/projects/ptiles/tiles/US.{signals,camera}.ptiles` are built, verified,
and carry the coarse index. The host serves the old ones.

| | published | built |
| --- | --- | --- |
| signals | 108,173 points (1/cell) | **2,107,809** points, 108,166 cells |
| camera | 36,395 points (1/cell) | **129,251** points, 36,632 cells |

Consequences of not shipping them, all currently visible:

- The intersection sidebar's "approaches" row never appears, because the
  published file gives duplicate co-located nodes that dedupe to one. With real
  data a four-way junction shows 4.
- `CoarseReader` in `demo/index.html` always falls back — the published files
  have no aux region, so none of the coarse-index work is live.
- Both point layers resolve through the `AbsoluteCorrected` offset path, which
  exists solely to read around those files' broken headers.

Rebuild with `python3 scripts/build_points.py` in `~/kino/projects/ptiles`
(~8 min, national, both layers). I have no upload path to
`maps.mydatatimeline.com`; that is the missing piece.

## Task 2 — Rookery consumers

`cli/` is the native JSON bridge. Rookery snaps GPS to roads via
`server/src/server/location/ptiles/RoadsReader.ts` -> `/api/location/road` and
wants the same for the new layers:

- `queryNearestSignal(lat, lon)` -> `{ osmId, signalType, distanceMeters }`,
  point haversine, radius ~30 m. Purpose: a stop beside a signal is a
  controlled stop, not an arrival — it suppresses false "you arrived" events.
- `queryNearestCamera(lat, lon)` -> `{ osmId, deviceType, distanceMeters, direction }`.
- `ptilesFetch.ts` `FILE_ALLOWLIST` needs `signals` and `camera`.

Build on `PtilesFile::read_cell`, not `read_block` — the latter returns a
merged block, and a record decoder will parse its cell table as records rather
than erroring. `cameras_near_road` in `core/src/camera.rs` is O(cameras x
segments) with a bbox cull; check it against a dense real cell before calling
it done.

The Rust core does not yet use the coarse index — `PtilesFile::open` still
reads the whole index. For a native consumer doing one lookup per open that is
the same 4 MB the browser used to pay, so it is worth doing here; see
`core/src/file.rs:147` and the `aux_offset`/`aux_length` fields already parsed
into `Header`.

## Already done — do not redo

- `cd23579` / `9a3aaa2` — both readers detect index entry width (19 or 38 B)
  and all three block-offset bases, and slice merged blocks.
- `2ca7dcf` — every layer confirmed rendering in a browser
  (`demo/test/render_check.py`). **PTILES Mode gates all rendering**; with it
  off a layer fetches its index and never requests a block, which reads as a
  dead layer. That cost three harness attempts.
- `1d0dfcd` — intersection sidebar on click (`demo/test/intersection_check.py`).
- `0c888a9` — dict and index cached across page loads, ETag-keyed. Cold open
  ~950 ms, warm ~85 ms, one range request (`demo/test/cache_check.py`).
- `17ee394` — wasm `find_block_for_cell` no longer forces the 19-byte layout.
- `e227cc2` / ptiles `c946069` — coarse index in the aux region; a point lookup
  pulls 12% of a full open (`demo/test/coarse_check.py`).

Measured and deliberately **not** done, with the numbers, so nobody re-derives
them:

- **Typed-array index parse.** Parsing 108,166 entries is ~96 ms against ~460 ms
  to fetch the index and ~210 ms of latency before that. Not worth
  destabilising `BusinessReader`'s five `.entries` call sites. Where it would
  pay is as a *serialisation* format — caching the parsed index in IndexedDB to
  skip the ~85 ms warm open entirely.
- **HTTP-range binary search over the index.** A 38-byte range request costs the
  same ~210 ms as a 14 KiB one on this host, so narrowing takes ~3 round trips
  (~645 ms) against one 460 ms bulk fetch. The coarse index solves this
  properly instead, in one extra request.
- **Shrinking the zstd dictionary.** 4.03x block compression with it, 1.74x
  without. It is now the largest part of a coarse open (512 KiB of 543 KiB), so
  if anything else is attacked, attack that — but per-layer sizing trades
  against Rookery's bulk scans.

## Standing constraints

- Do not change any on-disk format incompatibly. signals and camera are v1 and
  stay v1. Every `.ptiles` kind is versioned independently — the version byte is
  scoped to its magic and there is no release-wide version. See
  `SUPPORTED_FORMATS.md`.
- `demo/index.html` is the only source for the UI. `steele.red/ptiles` is a
  symlink to `demo/` that steele.red's `build.py` dereferences into `output/`.
  There is deliberately no `index.html` at this repo's root; do not recreate one.
- `demo/js/app.js` and `demo/js/ptiles-remote.js` are unreferenced — an
  alternative wasm-first architecture never wired up, and missing offset-base
  handling, merged-block slicing and caching. Read the header on
  `ptiles-remote.js` before reviving either.
- Check `demo/index.html`'s mtime before and after editing — other sessions
  have had it open.
- `maps.mydatatimeline.com` 403s the default python-urllib User-Agent; send a
  browser UA. Blocks carry no content size, so use a streaming zstd
  decompressor, not one-shot `decompress()`.
- `python3 -m http.server` **ignores Range** and answers 200 with the whole
  file. Any byte-level measurement against it is meaningless; `coarse_check.py`
  carries a Range-capable handler to borrow.

## Verification

```sh
cargo test --workspace && node --test "demo/test/*.test.mjs"
# with `python3 -m http.server 8899 --bind 127.0.0.1` running in demo/:
python3 demo/test/render_check.py        # all seven layers draw
python3 demo/test/intersection_check.py  # junction panel, and no leak into it
python3 demo/test/cache_check.py         # warm open makes one request
python3 demo/test/coarse_check.py        # coarse lookup pulls a fraction
```

`render_check.py` treats roads and water as controls: if they report zero, the
harness is broken, not the code.
