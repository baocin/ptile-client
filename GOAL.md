# GOAL: finish the publish, then ship the client as a library

Two things are in flight and must be finished before anything else is started.
Everything after them is the library-packaging work, which has not begun.

Written 2026-08-04. Sections marked **IN FLIGHT** may already be done — check
before repeating them; each carries the command that settles it.

---

## 1. DONE 2026-08-17 — v2 roads published

Settled by measurement: all 51 states return version byte 2 from the published
host (`curl -r 8-8` over every state, one `2`, 51 times). Nothing below needs
running again; it is kept for the rollback path and the CDN caveat.

## 1. (was IN FLIGHT) — finish publishing v2 roads

All 51 states were upgraded from roads v1 to v2 and are in
`ptiles/tiles/roads-v2/` (1.5 GB). An `aws s3 cp --recursive` to
`s3://mydatatimeline/maps/` was running when this was written.

**Why it matters:** `ptiles-core` supports roads **v2 only**
(`core/src/versions.rs`, `versions: &[2]`). Before this, Tennessee was the only
state published as v2, so every other state's roads failed to open with
`unsupported format version` and rendered nothing — silently, in both demos.

Finish it:

```sh
# 1. is the upload still going?
pgrep -af "aws s3 cp .*roads-v2"

# 2. every published file must report version byte 2
for st in AL AK AZ AR CA CO CT DE DC FL GA HI ID IL IN IA KS KY LA ME MD MA MI \
          MN MS MO MT NE NV NH NJ NM NY NC ND OH OK OR PA RI SC SD TN TX UT VT \
          VA WA WV WI WY; do
  printf "%s " "$st"
  curl -sS -r 8-8 -A "Mozilla/5.0" \
    "https://maps.mydatatimeline.com/maps/$st.roads.ptiles" | od -An -tu1 | tr -d ' '
done
```

**Beware the CDN.** The host is Cloudflare-fronted. A file can be uploaded to R2
and still serve the old bytes for a while. If a state reads v1, re-check against
R2 directly before concluding the upload failed:

```sh
AWS_PROFILE=mdt-r2 aws s3 ls s3://mydatatimeline/maps/CA.roads.ptiles
```

Re-upload any state that genuinely did not land. Transient
`SSL: UNEXPECTED_EOF_WHILE_READING` failures happened twice during the address
upload and both succeeded on a plain retry.

Then confirm a v1-only state actually works end to end, which it could not before:

```sh
cargo build -q -p ptiles-cli
# any state other than TN — this returned "unsupported format version" before
./target/debug/ptiles-cli --path <a downloaded CA.roads.ptiles> \
  --lat 37.7749 --lon -122.4194 --query intersection
```

**Rollback**, if ever needed: the v1 originals are untouched on the NAS at
`/mnt/core/kino/ptiles/data/states/`.

## 2. DONE 2026-08-17 — NC and TN rebuilt, stranded entries gone

`cargo run -p ptiles-core --features http --example intersection_audit`, against
the published files:

    TN: v2, 23,087 cells, types 1/2/3 = 12,428, type 4 = 489,   stranded = 0
    NC: v2, 28,459 cells, types 1/2/3 = 34,378, type 4 = 1,350, stranded = 0

TN's 12,428 closes the May-snapshot gap (12,103 before, against 12,425 in the
July signals for the same cells). Type 4 is unchanged at the predicted 489 and
1,350. All 9 stranded entries are gone — confirmed by re-resolving every
intersection's coordinates against the cell that indexes it, not assumed.

## 2. (was IN FLIGHT) — NC and TN, one snapshot

NC and TN were already v2, so they took a different path: their existing
intersection tables were preserved byte-for-byte and only roundabouts appended.
That left their types 1/2/3 on an **18 May** OSM snapshot while the other 49
states use the **23 July** `US.signals.ptiles`.

Measured on TN: 12,103 type-1/2/3 entries against 12,425 in the July signals for
the same cells — **322 missing**, across 84 blocks.

A rebuild through the same code path as the other 49 was running when this was
written. When it lands:

```sh
# types 1/2/3 should now match the July signals; type 4 unchanged (TN 489, NC 1350)
# then re-upload just those two:
AWS_PROFILE=mdt-r2 aws s3 cp /home/aoi/kino/projects/ptiles/tiles/roads-v2/TN.roads.ptiles \
  s3://mydatatimeline/maps/TN.roads.ptiles
AWS_PROFILE=mdt-r2 aws s3 cp /home/aoi/kino/projects/ptiles/tiles/roads-v2/NC.roads.ptiles \
  s3://mydatatimeline/maps/NC.roads.ptiles
```

Rebuilding should also clear 9 pre-existing stranded entries (1 in TN, 8 in NC)
that sat in the wrong H3 cell in the shipped files and were therefore unreachable
by any cell lookup. Confirm they are gone rather than assuming.

## 3. DONE — staging cache is already gone

`~/.cache/roads-v2-stage/` no longer exists. Disk is at 95% (97 GB free), so
nothing here is holding space.

## 3. (unblocked) Then: delete the staging cache

`~/.cache/roads-v2-stage/` holds ~13 GB (11 GB of state PBFs, 1.5 GB of roads).
Kept only because sections 1 and 2 needed the cached PBFs — the NFS mount was
running at ~878 kB/s and refetching costs ~92 minutes.

Delete it once both are done. Disk was at 96% (80 GB free).

---

## Library packaging — not started

The original goal. `docs/HANDOFF-wasm-only-client.md` covers why the work so far
was about removing duplicate decoders rather than packaging: six language
bindings multiply every format bug, and every bug this project has had came from
two implementations disagreeing.

**Blocking for `cargo publish`:** four crates carry
`ptiles-core = { path = "../core" }` with no `version =` — `cli/Cargo.toml:15`,
`wasm:14`, `ffi:18`, `motion:16`. crates.io rejects a path dependency without one.

| target | what is needed |
| --- | --- |
| Rust | works today (`cargo add --git`) |
| npm / node / HTML | a real `wasm/package.json`. The tracked root one describes the legacy seed crate and points `main` at a gitignored `pkg/`; its build scripts target the virtual workspace manifest and cannot run. Retire it. |
| Python | `pyproject.toml` + a wheel bundling the cdylib. UniFFI bindings already generate and are CI-checked. |
| Kotlin | `.so` via `cargo-ndk`; buildable on this host |
| Swift | bindings only. An `.xcframework` needs macOS, which this host is not. Document the command. |

Also outstanding:

- **`motion/`** — deliberately deferred. Zero consumers today; intended to be
  exposed through ffi and wasm later.
- **`ffi/Cargo.toml:18`** requests `features = ["std", "http"]`, so every Android
  build compiles `ureq` plus a TLS stack. Add an http-free combination if the
  HTTP client is not wanted on mobile.
- **Builder round-trip** — `build_points.py --verify` re-verifies in Python, the
  same language that produced the 42-vs-38 stride bug. Replace its core with
  build → read via `ptiles-cli` → assert.
- **Generated spec tables** — `core/src/index.rs:6-24` and `merged.rs:7-13` carry
  hand-written byte tables, the same thing that let `SPEC.md` claim 37-byte
  entries when they are 38. `core/tests/supported_formats_doc.rs` is the proven
  const → renderer → doc → drift-test pattern to copy.
- **steele.red housekeeping** — `.content-hashes.json` is tracked but looks like
  build output, so it dirties the tree every build. And when `demo/` is finally
  deleted, `/ptiles-legacy` should 404 rather than serve a stale copy.

---

## Already done — do not redo

Commit-referenced so nothing here gets rebuilt from scratch.

### Address layer, 2026-08-17

- `57b2fc2` — **every address position the client produced was wrong** unless
  its cell happened to be first in its merged block. The builder measures each
  record's `i16` offsets from its own cell's centre; `merged_block_cell_slice`
  read them against the block header's centre, which is the first cell's.
  Blocks hold eight cells, so seven in eight decoded kilometres out. Measured on
  the published `TN.address_v2.ptiles`, OSM way 130905893: truth
  36.15770,-86.78416, decoded 36.13647,-86.78984 — 2.4 km south, with the
  number and street perfectly intact, which is why nothing noticed. **This is
  fixed in the client, so deploying the wasm alone corrects the live site
  against the files already published.**
- `57b2fc2` — the golden fixture could not have caught it: it gave every cell
  the same arbitrary centre, so block centre and cell centre were identical, and
  one of its two cell ids (`0x87264D1040FFFFF`) was not a valid H3 index at all.
  The fixture now derives each cell's centre from its id and uses a real
  neighbour. Reverting the decoder alone turns it red — checked.
- `eeb4e4a` — `AddressFile::search_address`: forward geocode with no location.
  `find_address` could only filter cells the caller already named, i.e. a
  viewport filter, not a geocoder. No hint is a full decompress (14.7 s for
  Tennessee's 4M records — the price of a layer with no name index); with a hint
  it is 0.168 s, bounded by distance rather than by count so the early stop
  cannot change the answer.
- `99fc8ce` — **nothing is viewport-scoped any more.** The demo's Find box
  scanned 24 cells on screen and called a miss "no match in the N cells on
  screen". It now walks the state, ordered by `address_cells_by_distance` (core
  owns the ordering; `IndexEntry` drops the bbox, so `js/ptiles.js` keeps the
  raw index bytes). Ordering alone does not bind — "919 Broadway" has two
  matches in all of Tennessee, so the count bound never fires and the first
  version read 20,767 cells and 29.6 MB per keystroke. With a 25 km first pass
  and a stop a few times past the nearest hit: 22 cells, 406 KiB, 188 ms from
  Nashville; a miss offers to widen to the whole state.
- `99fc8ce` — street matching folds type and direction words. It was raw
  substring, which is asymmetric: Memphis carries both `Beale St` and
  `BEALE Street`, so "Beale St" matched 122 records and "Beale Street" matched
  71 — typing the type in full silently dropped 51 addresses.
- `34c5f8a` — first JS-side address coverage that has ever existed (7 tests over
  `js/ptiles.js` + wasm), plus `core/tests/address_v3_states.rs` over all 51
  files. Exhaustive run: **143,749,384 records in 820,514 cells, all pass**
  (osm 3,199,194 / nad 84,977,431 / openaddresses 55,572,759). That is 447 fewer
  than the builder wrote, which reconciles exactly: 432 dropped by its
  polar/antimeridian guard, 15 unassignable to a cell.

### Address measurements — do not re-derive

- **h3 (builder, Python) and h3o (client, Rust) disagree on boundary points, and
  it does not matter.** 84,552 records across six states: 0.0000%–0.0352% per
  state, and *every* disagreement lands in an **adjacent** cell, worst 1,427 m
  from the stored centre — i.e. exactly a res-7 edge. Adjacency is the whole
  point: ring 1 covers all of it, ring 0 does not, so `--query address` now
  defaults to ring 1. Do not "fix" the H3 implementations.
- **v3 costs about 3.4x the bytes per click and answers better.** Nashville,
  same point, demo reverse path: v2 181 KiB / 2,884 records / 24 ms → "919
  Broadway (0 m)"; v3 616 KiB / 94,421 records / 328 ms → "901 Broadway (12 m)",
  the genuinely nearest address.
- **One cell per block beat changing H3 resolution.** Measured on the v2 files,
  an 8-cell block reaches 494,337 B in Manhattan and a click that wants one cell
  pays for all eight. v3 writes one cell per block: Tennessee's worst block fell
  from 70,344 B to 40,392 B *while carrying 29x more addresses*. Resolution was
  left at 7; the index would have grown 7x at res-8 for no gain here.
- **The shared dictionary paid for the lost context.** 110 KB trained across
  states, against 8 KB per state before: 7.45 B/record at v3 versus 8.29 B/record
  at v2, despite one-cell blocks having far less internal redundancy.
- **Address `osm_id` is not a key.** Node and way ids share a value space, the
  layer records neither which nor the element type, and v3's bulk records all
  carry 0. Nothing in this repo resolves it; the two `/node/` links in the demo
  are intersections and businesses.

- `11b8cfe` — `conformance/corpus/`, eleven slices of real published layers
  (266 KB). Covers both index entry widths, all three offset bases, merged
  blocks, dictionary and dictionary-free decompression, a PTCI coarse index, and
  the historical 42-byte-stride bug preserved as bytes. This is what makes CI
  green without a data directory; the guards in `real_layers.rs` and
  `index_reader.test.mjs` fail rather than skip when no fixture is found, and
  that is deliberate.
- `1ba2c50` — the comment claiming wasm business decoding used "wrong framing"
  was wrong. Framing is identical; `osm_id` differs on 100% of records and **JS**
  is the wrong side (uids exceed 2^53 and it accumulates them in Number space).
- `e769b3d` — offset-base selection factored into `ptiles_core::index_layout` and
  exported as `parse_index_layout` / `index_entries_absolute`.
- `78c14c6` / `c9b2d54` — PTCI coarse index ported to `core/src/coarse.rs` and
  exposed through wasm. It previously existed only in `demo/index.html`.
- `5f674a4` / `bf1c341` — `web-demo/`, the wasm-only client. 12.5 KB of
  byte-level JS removed, including the PTILESC camera layout the page carried
  three separate times. Parity verified in chromium: identical feature counts on
  all seven layers.
- `d35f356` — `activatePtilesMode` loaded a hardcoded list, so ticking parks,
  rail or buildings before enabling PTILES mode added a map group with no reader
  behind it and drew nothing. Buildings had no change handler at all.
- `f5ccc62` / steele.red `74cff2d` — `steele.red/ptiles` now serves `web-demo/`;
  `steele.red/ptiles-legacy` serves `demo/`.
- `445883f` — `conformance/check_published.py`. Two range requests per layer,
  exits non-zero if any published file's header contradicts its own index. Use it
  to gate a publish.
- All 51 `{ST}.address.ptiles` published to R2 and serving (verified HTTP 206).

- `0a5b5e7` — `web-demo/test/perf_check.py`, the timing harness. Per layer, at a
  fixed viewport, cold and warm, three runs with the median and spread: the open
  and render phases, range requests and bytes, and the split between network,
  zstd and decode. The per-layer seconds this file used to quote came from no
  script; these come from this one. Same commit made every layer 27-49% faster
  cold and 60-69% faster warm at identical feature counts — prefetching a
  render's blocks instead of one round trip per cell, a promise-keyed block
  cache, dictionary and index fetched together, the national point layers
  through their PTCI aux, a leading-edge viewport debounce, and `preferCanvas`.
  Numbers are in the commit message.
- `bb45979` — `Layer.prefetch` coalesces a viewport's block reads into runs
  separated by less than 64 KiB and slices them locally, which is what took
  roads from 18 requests to 8. Against the original baseline, time to a stable
  render is 0.28-0.68x cold and 0.28-0.36x warm on all seven layers, at
  identical feature counts. Coarse layers keep the per-cell path — they must
  fetch a run of the real index before they know where a block is.
- `01f4aeb` — the viewport's cells are sorted by distance from the map centre
  before the 300 cap, so the centre decides both what draws first and what
  survives capping (it used to be whatever `polygonToCells` returned, with the
  centre ring appended last). The nearest cells are also split into their own
  range where the payload justifies the extra request. Cold first-feature:
  roads 1174 -> 906 ms, buildings 996 -> 771 ms.
- `f8dc14a` — `wasm/test/golden.mjs` now runs in CI. It matched neither of the
  two globs and had never executed on any machine; it passes and gates.

Measured and deliberately **not** done, with the numbers so nobody re-derives them:

- **The ~30x wasm bench does not apply to a real page.** `bench_wasm.html`
  isolates the wasm boundary on synthetic records; on a real page the cost is
  range requests and zstd. Measured time-to-stable-render, legacy vs wasm-only:
  roads 0.95x, water 1.08x, bldgs 1.33x, parks 1.17x, signal 1.07x, camera 1.00x.
- **The 9 stranded intersection entries** in the shipped NC/TN files are left
  alone if section 2's rebuild does not clear them — 9 records out of ~46,000,
  unreachable but harmless.
- **h3-js stays in both demos.** It is not a second implementation of *this*
  format, and mixing core's H3 with h3-js for different calls would recreate the
  duplicate-implementation problem the whole effort removed.
- Typed-array index parse, HTTP-range binary search over the index, and shrinking
  the zstd dictionary: all measured and rejected. See git history of this file.
- **`interactive: false` on the bulk geometry is worth about 4%**, not the bug
  fix it looked like. Measured warm at Nashville z14: roads 704 -> 672 ms,
  buildings 711 -> 687 ms. The theory it came from — 26,000 interactive paths
  eating the map click the inspector depends on — is wrong. A path fires with
  `propagate` and the map is its event parent, so the click arrives either way;
  checked in chromium on both renderers by clicking a pixel taken off a drawn
  road rather than the map centre, which is the only way to test it at all.
- **Coalescing costs 4.5% more bytes and is worth it.** At a 64 KiB gap
  threshold roads pulls 1409 KiB where it pulled 1348, for 10 fewer round trips.
  The threshold is the knob; it has not been swept.
- **Splitting the centre cells into their own range must be gated on size.**
  Applied to every layer it cost water 802 -> 1103 ms and parks 597 -> 843 ms:
  their whole render is one round trip, so a second request buys nothing. Gated
  at 256 KiB of blocks they are back at 829 and 615 ms. A 3-cell centre instead
  of 7 is worse on both counts for roads (first feature 1041 ms against 824,
  total 1550 against 1332).
- **The run-to-run noise floor over the live CDN is 100-140 ms.** Measured on
  camera and signal across `01f4aeb`, which does not touch the coarse path they
  use. Treat any single-layer difference smaller than that as nothing, and note
  that `perf_check.py` samples at 250 ms, so a layer whose whole render fits in
  one tick (rail, parks, water warm) reports in multiples of 253 ms.
- **What is left is not the decoders.** After all of the above, roads cold is
  1312 ms of which 962 ms has a request outstanding, against 10 ms of zstd and
  73 ms of decode. Warm is 414 ms and almost all of it is Leaflet. The open
  phase — header, then dictionary and index together — is 494-673 ms on every
  layer and is now the floor for anything not already cached; on TN.roads that
  is a 512 KiB dictionary and a 428 KiB index, and the dictionary is fetched
  even when the render needs one block.
- **`US.signals` and `US.camera` do carry a PTCI aux.** This file previously
  implied the published layers predate it. Measured 2026-08-05: signals 5.0 KiB
  aux against a 4014 KiB index, camera 1.7 KiB against 1359 KiB. `TN.roads` and
  `TN.buildings_v8` carry none; `TN.water` carries a 793 KiB aux that is not a
  coarse index and has not been identified.

---

## Standing constraints

- Do not change any on-disk format incompatibly. signals and camera are v1 and
  stay v1. Every `.ptiles` kind is versioned independently — the version byte is
  scoped to its magic and there is no release-wide version. See
  `SUPPORTED_FORMATS.md`, which is generated from `ptiles_core::SUPPORTED_FORMATS`
  and asserted by a test.
- Two UIs, on purpose, until one wins. `web-demo/index.html` decodes through
  `ptiles-core` in wasm and is what `steele.red/ptiles` serves. `demo/index.html`
  is the original, hand-decoding in JavaScript, at `steele.red/ptiles-legacy`.
  Both are dereferenced into `output/` by steele.red's `build.py`, which must run
  on `hino-omarchy` — the symlinks are absolute. When `web-demo/` has proven
  itself, `demo/` is deleted and `/ptiles-legacy` should 404.
- There is deliberately no `index.html` at this repo's root; do not recreate one.
- `demo/js/app.js` and `demo/js/ptiles-remote.js` are unreferenced dead
  alternatives. Read the header on `ptiles-remote.js` before reviving either.
- Check `demo/index.html`'s mtime before and after editing — other sessions have
  had it open.
- `maps.mydatatimeline.com` 403s the default python-urllib User-Agent; send a
  browser UA. Blocks carry no content size, so use a streaming zstd decompressor.
- `python3 -m http.server` **ignores Range** and answers 200 with the whole file.
  Any byte-level measurement against it is meaningless; `coarse_check.py` carries
  a Range-capable handler to borrow.
- **PTILES Mode gates all rendering.** With it off, a layer fetches its index and
  never requests a block — reader present, group on the map, zero features, no
  error. This has now cost two separate harnesses three runs each. Click
  `#btnPtiles`.
- Several layer checkboxes ship `checked`, so "set it and dispatch change" only
  when unchecked does nothing. Dispatch unconditionally.
- `/mnt/core` is NFS and can degrade badly — single-stream reads at 131 kB/s and
  directory listings timing out past 900 s were observed. Prefer explicit
  filename construction over globbing, and give listings generous timeouts.
- Do not write `pgrep -f "<pattern>"` inside a script whose own command line
  contains that pattern. It matches itself and waits forever. This happened.

## Verification

```sh
cargo test --workspace                       # 446 passing
node --test "demo/test/*.test.mjs" "web-demo/test/*.test.mjs"   # 43, 0 skipped
node wasm/test/golden.mjs                    # decoders vs the Python reference
cargo build -p ptiles-core --no-default-features --target thumbv7em-none-eabihf
cargo build -p ptiles-wasm --target wasm32-unknown-unknown --release

python3 conformance/check_published.py       # published layers, exits non-zero if broken
python3 web-demo/test/render_check.py        # the real page in chromium
python3 demo/test/render_check.py            # the legacy page, for comparison
python3 web-demo/test/perf_check.py          # how long each layer takes, and why
```

`perf_check.py` takes ~20 minutes for all seven layers at three runs; use
`--layers roads bldgs --runs 1` while iterating and `--json` to keep a
before/after pair. It is measured against the live CDN, so a single run is
noise — that is why it reports the spread.

`render_check.py` treats roads and water as controls: if they report zero, the
harness is broken, not the code. Both harnesses tick the layer checkbox **before**
enabling PTILES mode, which is the order a user works in and the order that was
broken; testing the other order hid the bug for its whole life.
