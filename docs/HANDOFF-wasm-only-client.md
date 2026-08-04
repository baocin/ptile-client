# Handoff: one decoder, a real-bytes corpus, and a wasm-only demo

Written 2026-08-03, for review. Covers eight commits on `master`
(`11b8cfe`..`fbd0b9b`), one commit on `steele.red` (`fa90306`), and what is
left.

Everything described here is on `master` with CI green.

---

## The problem this was addressing

Every format bug this codebase has had came from two implementations of the
same byte layout disagreeing, and each one failed the same way: a **silently
empty layer**, not an error. That is why they were found days or weeks late.

| drift | consequence |
| --- | --- |
| JS reader hardcoded a 19-byte index stride; generator emitted 38 | parks, rail, places, camera, signals rendered blank |
| wasm `find_block_for_cell` forced v1 | same bug, different boundary |
| Python builder computed `index_length` at 42 bytes, encoder wrote 38 | published files unreadable; not one block reachable |
| `SPEC.md` said v2 entries are 37 bytes | they are 38 |

The goal is to ship this as a library for HTML, npm/node, Python, Swift and
Kotlin. Going from two implementations to six multiplies that risk, so the
work was organised around removing implementations rather than packaging.

---

## 1. CI was red, and the reason mattered

`cargo test` and `node tests` were both failing on `master`. Same cause:

- `core/tests/real_layers.rs:261` `layer_coverage_is_asserted_somewhere`
- `demo/test/index_reader.test.mjs:276` `fixtures_were_actually_exercised`

Both deliberately **fail** when no real `.ptiles` fixture is found, so an empty
data directory cannot masquerade as a green run. Both searched hardcoded
machine-local paths (`/home/aoi/kino/data/ptiles`, …) that no runner has.

A comment in `ci.yml` claimed these "skip when their data directory is absent".
That was wrong — the individual cases skip, the guards do not, which is the
whole point of them. The guards are correct; fixtures were what was missing.

### The fix: a committed corpus of real bytes

`conformance/corpus/` — eleven slices of real published layers, 266 KB total.
Each keeps the real header, the real index entries (copied verbatim, only the
offset/length fields repointed), the real aux region and the real block
payloads. Entry width, offset base, declared stride, merged-block cell tables,
bbox bytes and `cell_index` all survive.

Coverage: both entry widths, all three offset bases, merged blocks, dictionary
and dictionary-free decompression, a PTCI coarse index, and the historical
42-vs-38 stride bug.

The two `stride42` files are the ones worth having. They are slices of the
pre-fix published `US.signals`/`US.camera`, whose header declared a 42-byte
stride while the encoder emitted 38 — so `blocks_offset` and every offset
derived from it overshot the block region. Read as 19-byte entries they still
look structurally plausible and report `block_length == 0` for every cell. No
synthetic fixture caught this in time; these bytes do.

Two documented departures, recorded per file in `manifest.json`: six layers'
512 KB zstd dictionaries are stripped and their blocks recompressed without one
(decompressed payload stays byte-identical; `TN.water`'s 11 KB dictionary is
kept so that path stays covered), and `TN.water`'s 812 KB aux region is dropped.

**Verified in both directions.** With `SEARCH_DIRS` reduced to the corpus alone
— the CI condition — Rust goes 11/11 and node goes 21/21 with 0 skipped, from
11 pass / 1 fail / 9 skipped. Emptying the corpus makes all four conformance
tests fail with actionable messages; forcing `TN.buildings_v8`'s `blocks_offset`
to 0 is caught as *"offset base Absolute, manifest says Relative"*.

---

## 2. A claim in the code that turned out backwards

`demo/index.html:687` said:

```js
// Business: JS v3 decoder (wasm business is wrong framing for live files)
```

which is why `decode_business` was imported and never called. Run against real
bytes, that comment is wrong.

Across 48 blocks and 51 records the framing is **identical** — same record count
in every block, and name, lat, lon, phone, website, address, brand and category
all match exactly.

One field differs, on 100% of records, and **JS is the wrong side**. These uids
reach ~6.3e18, past the 2^53 where a double stops representing consecutive
integers, and the JS decoder accumulates them in Number space:

```js
var uid = prevUid + zigzagDecode(dr.value); prevUid = uid;
```

29 of 51 come out as exactly the float64 rounding of the true value; the other
22 drift further, and some flip sign when a delta crosses an i64 boundary
Number arithmetic cannot wrap — one reads `8340008791195242000` where the value
is `-883363245659532896`. Left unfixed, the demo links some businesses to the
wrong OSM object.

Pinned by `demo/test/business_differential.test.mjs`. The `osm_id` case is
asserted as a *known defect* rather than skipped, so fixing the JS decoder makes
the test fail and forces the exception to be removed.

---

## 3. Closing the wasm gaps

Three things blocked a wasm-only client.

**Offset-base selection was not exported.** Core had it, but
`parse_index_entries` takes index bytes with no header, so it cannot see
`blocks_offset` and cannot know which base applies. The caller decided —
and `demo/index.html`'s `pickOffsetBase` is what deciding looked like: a second
implementation of the rule, in the language that got the stride wrong.

Factored the decision out of `PtilesFile::open` into `ptiles_core::index_layout`
and moved the arithmetic onto `IndexLayout::absolute_block_offset`, so there is
one copy. Exposed as `parse_index_layout` and `index_entries_absolute` — the
latter returns entries with offsets already resolved, which removes every chance
to get the three steps wrong.

**The PTCI coarse index did not exist in Rust at all.** It lived only in
`demo/index.html`. Ported to `core/src/coarse.rs`, with two deliberate
improvements over the original: it fails closed on an unknown version (the JS
ignored the version byte, so a future v2 would be read as v1 and silently
mis-bracketed), and it distinguishes "no coarse index here" (normal, every older
layer) from "these bytes are malformed" (a writer bug).

**Business framing** — see §2; no fix needed in core, the JS was wrong.

---

## 4. `web-demo/`: the wasm-only client

`web-demo/js/ptiles.js` knows how to *fetch* bytes and nothing about what is in
them. Ranges, the ETag-keyed Cache API, in-flight dedup and the block cache stay
in JS because they are network concerns. Everything of the form "what do these
bytes mean" goes to `ptiles-core` through wasm.

`web-demo/index.html` is `demo/index.html` with 12.5 KB of byte-level code
removed: header parsing, both index entry widths, offset-base selection,
merged-block slicing, the ETag range cache, varint/zigzag helpers, the PTILESB
v3 record decoder, `decodeSignalRecords`, the coarse index, and the PTILESC
camera layout — which the page carried **three separate times**.

It also drops the vendored zstd build (`decompress_block` in core).

**Leaflet and h3-js stay.** They are not second implementations of *this*
format, and mixing core's H3 with h3-js for different calls would recreate
exactly the problem being removed.

The reader is a real module taking the wasm namespace as a parameter, so the
same file runs under `--target web` in the browser and `--target nodejs` under
`node --test`. That is what makes it testable; the legacy decoders can only be
reached by regex-scraping a 2656-line HTML file.

### Parity

`web-demo/test/render_check.py` drives the real page in chromium against the
live host. Same viewport, legacy vs wasm-only:

| layer | legacy | wasm-only |
| --- | --- | --- |
| roads | 25781 | 25781 |
| water | 141 | 141 |
| bldgs | 26488 | 26488 |
| parks | 110 | 110 |
| rail | 2 | 2 |
| camera | 2 | 2 |
| signal | 762 | 762 |

Identical on all seven.

### Performance — the ~30x concern did not survive contact

`demo/test/bench_wasm.html` measured wasm decode ~30x slower than hand-rolled JS
per cell, which is why the JS decoders were kept. That benchmark isolates the
wasm boundary crossing on synthetic records; on a real page the cost is range
requests and zstd. Time from PTILES Mode to a stable render:

| layer | legacy | wasm-only | ratio |
| --- | --- | --- | --- |
| roads | 7.86s | 7.44s | 0.95x |
| water | 6.49s | 6.99s | 1.08x |
| bldgs | 8.92s | 11.87s | 1.33x |
| parks | 7.67s | 8.97s | 1.17x |
| signal | 8.12s | 8.68s | 1.07x |
| camera | 3.88s | 3.90s | 1.00x |

Worst case is buildings — 26,488 features — at 1.33x.

---

## 5. Bugs found by actually running it

Three, all silent, none catchable by the byte-level tests alone.

**Cell normalisation.** A res-7 H3 id carries filler digits in its low 21 bits.
The index stores them set; `latLngToCell` returns them masked. The reader keyed
its map by one form and looked up by the other, so every lookup missed and every
layer rendered empty. The corpus test could not catch it because it queried
using *stored* ids, where both forms agree — it now queries the way a caller
does, plus an explicit test that raw and masked reach the same entry.

**The same mismatch in `CoarseIndex::bracket`.** A masked query sorted below the
sample naming its own cell, so the search landed one sample early and the run
did not contain the entry. Fixed by normalising both sides, which is
order-preserving. Two existing tests failed on that change and were right to —
they used cell ids of 100, 200 and 300, all below the filler bits and therefore
all normalising to zero. They now use H3-shaped values.

**A stale corpus aux region.** The slicer copied `aux` verbatim while truncating
the index, so the PTCI samples pointed past the surviving entries — each file
individually valid, internally inconsistent. Nothing caught it because nothing
outside the browser could read PTCI. The slicer now takes a contiguous prefix
and retargets the samples, and `conformance_corpus.rs` asserts every sample
names the cell actually at that position.

Also worth recording: the render harness reported zero for every layer three
times before I read the legacy harness's own comments. **PTILES Mode gates all
rendering** — without clicking `#btnPtiles` a layer fetches its index and never
requests a block: reader present, group on map, zero features, no error. And
several layer checkboxes ship `checked`, so "set it and dispatch change"
conditionally does nothing.

---

## 6. Published data health

`conformance/check_published.py` checks every published layer over two Range
requests and exits non-zero if any needs header correction, so it can gate a
publish.

**Result: 37 block-per-cell layers checked, 0 needing correction.** The broken
42-byte-stride `US.signals`/`US.camera` are **not** live — the rebuilt ones are,
declaring 38 bytes correctly with their PTCI aux regions intact.

Host is Cloudflare-fronted with multipart-style ETags, consistent with R2.
Directory listing is disabled, so the checker probes known names.

Two things to note rather than fix:

- `{state}.address` and `{state}.highways` return 404 at `/maps/` for every
  state tried (AK, CA, NY, TX, TN). They may live elsewhere or not be published.
- `US.admin` is a **lookup-grid** layer, not block-per-cell: it repurposes the
  header's section pointers, so `index_offset` names a zstd polygon table. My
  first version of the checker read 4 bytes there as an entry count, got
  4,247,762,216 entries in a 31 MB file, and reported it **ok**. It now
  detects this the same way core does (`block_count == 0 && aux_length > 0`)
  and skips it explicitly.

---

## 7. Deployment

> **Superseded 2026-08-04.** The wasm client has since taken the primary URL:
> `steele.red/ptiles` → `web-demo/`, and `steele.red/ptiles-legacy` → `demo/`.
> `/ptile-wasm` is retired. The rest of this section describes the interim
> arrangement it was proven under.

`steele.red/ptile-wasm` → `ptile-client/web-demo`, a **new** symlink served
alongside `/ptiles`, which still points at `demo/`. Nothing about the existing
page changed.

Checked for conflicts: the two output trees share no files, `index.html` and
both wasm artifacts differ, and the Cache API names are distinct
(`ptiles-regions-v1` vs `v2`). The only byte-identical file is
`h3-js.umd.js`, a static vendor asset with no state. The built
`output/ptile-wasm` was verified in chromium — roads 25781, parks 110, signal
762, no page errors.

Not live until `build.py` runs on `hino-omarchy`; the symlinks are absolute.

`AGENTS.md` and `GOAL.md` previously said "a second copy of this UI is always a
bug". Both updated: that rule was aimed at *orphan* copies that drift silently,
and `web-demo/` is the inverse — it exists to delete a second copy of the
decoders, it is reachable from a URL, and `render_check.py` fails if it stops
matching. When it has proven itself, `demo/` goes away and the count returns to
one.

---

## 8. State

`cargo test --workspace` 446 passed · `node --test` 42 passed, 0 skipped ·
`no_std` and `wasm32` build · CI green on all 7 jobs.

CI gained three jobs' worth of coverage: a `conformance` job, a wasm-pack build
in the node job (the first coverage the npm packaging path has ever had), and
`web-demo` tests.

### Commits

| | |
| --- | --- |
| `11b8cfe` | conformance corpus, CI green |
| `1ba2c50` | business differential |
| `e769b3d` | wasm index layout exports |
| `78c14c6` | PTCI ported to core |
| `c9b2d54` | PTCI through wasm |
| `5f674a4` | web-demo reader module |
| `bf1c341` | web-demo page, at parity |
| `fbd0b9b` | docs |
| `fa90306` | steele.red symlink *(on `origin` only)* |

---

## 9. What is left

**Not started:**

- **Packaging** — `wasm/package.json` for npm (the tracked root one describes
  the legacy seed crate and points `main` at a gitignored `pkg/`), `pyproject.toml`
  + wheel, Kotlin `.so` via `cargo-ndk`, Swift bindings only (no macOS here).
- **Blocking for `cargo publish`:** four crates carry
  `ptiles-core = { path = "../core" }` with no `version =` (`cli:15`, `wasm:14`,
  `ffi:18`, `motion:16`). crates.io rejects that.
- **`motion/`** — deliberately deferred. Zero consumers today; intended to be
  exposed through ffi and wasm later.
- **Builder round-trip** — `build_points.py --verify` re-verifies in Python, the
  same language that got the 42-byte stride wrong. Should build → read via
  `ptiles-cli` → assert.
- **Generated spec tables** — `core/src/index.rs:6-24` and `merged.rs:7-13` carry
  hand-written byte tables, the same thing that let `SPEC.md` claim 37 bytes.
  `core/tests/supported_formats_doc.rs` is the proven pattern to copy.
- **`ffi/Cargo.toml:18`** requests `features = ["std","http"]`, so every Android
  build compiles `ureq` plus a TLS stack.

**Open questions:**

1. `gitea-http` (port 3000) refuses connections, so `steele.red` is pushed to
   `origin` (3001, confirmed up and holding the commit) only. Expected, or does
   that host need restarting?
2. `TN.business.ptiles` contains Memphis and Nashville businesses at coordinates
   in Siberia and northern Quebec. Records match their indexing cells, so the
   decoder is right and the **builder** wrote wrong coordinates. A `ptiles`-repo
   data bug, outside this work.
3. The pre-fix 42-byte-stride files exist only at
   `ptiles/tiles/published-backup/` and as corpus slices. Archive elsewhere?
4. `web-demo/test/` is copied into the published output, same as `demo/test/` is
   today. Harmless, but excludable in `build.py` if unwanted.
