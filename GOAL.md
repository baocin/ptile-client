# GOAL: finish the publish, then ship the client as a library

Two things are in flight and must be finished before anything else is started.
Everything after them is the library-packaging work, which has not begun.

Written 2026-08-04. Sections marked **IN FLIGHT** may already be done — check
before repeating them; each carries the command that settles it.

---

## 1. IN FLIGHT — finish publishing v2 roads

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

## 2. IN FLIGHT — NC and TN, one snapshot

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

## 3. Then: delete the staging cache

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
node --test "demo/test/*.test.mjs" "web-demo/test/*.test.mjs"   # 42, 0 skipped
cargo build -p ptiles-core --no-default-features --target thumbv7em-none-eabihf
cargo build -p ptiles-wasm --target wasm32-unknown-unknown --release

python3 conformance/check_published.py       # published layers, exits non-zero if broken
python3 web-demo/test/render_check.py        # the real page in chromium
python3 demo/test/render_check.py            # the legacy page, for comparison
```

`render_check.py` treats roads and water as controls: if they report zero, the
harness is broken, not the code. Both harnesses tick the layer checkbox **before**
enabling PTILES mode, which is the order a user works in and the order that was
broken; testing the other order hid the bug for its whole life.
