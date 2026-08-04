# Handoff: five-way audit, 2026-08-04

A five-agent audit of `core`/`wasm`, both demo pages, the `ptiles` builder
scripts, tests/CI/conformance/packaging, and `ffi`/`cli`/`motion`/docs.

**Nothing in this document has been fixed** except the two items marked FIXED,
which were regressions introduced during the wasm-only port earlier the same
day. Everything else is reported, not actioned.

Findings marked **[verified]** were reproduced directly against real bytes or
running code, by me, independently of the agent that reported them. Findings
marked **[reported]** come from an agent and are plausible and specific but I
did not re-run them. **[unverified]** means the reporting agent itself could not
confirm it.

The ranking is by this codebase's actual hazard: **a wrong decode returns a
plausible empty or garbage result rather than an error.** Ugliness ranks below
silence.

---

## 1. Confirmed live defects

### 1.1 Every published business file has `feature_count = 0` — [verified]

`ptiles/scripts/build_us_poi.py:536` and `build_us_poi_v2.py:560`:

```python
"feature_count": sum(1 for r in records
                     if h3.latlng_to_cell(r["lat"], r["lon"], H3_RES) == cell),
```

`h3.latlng_to_cell` returns a **`str`**; `cell` is an **`int`**. The comparison
is never true, so the sum is always 0.

Checked against production over HTTP:

```
TN.business  es=19 entries=18162  first 40 feature_counts: 0 non-zero
CA.business  es=19 entries=31338  first 40 feature_counts: 0 non-zero
TN.roads     es=19 entries=23087  first 40: all non-zero, e.g. 32, 6, 33, 11
TN.parks     es=38 entries= 1660  first 40: all non-zero, e.g. 2, 1, 1, 2
```

Silent because `index.rs::is_structurally_valid` only inspects `block_length`,
so the files parse cleanly. Anything reading the count — scoring, a "how many
here" pre-check — sees zero.

The same expression is also `O(cells x records)` with an H3 call per pair: for a
1M-row state with 5k cells that is ~5e9 calls, so these scripts cannot realistically
finish.

**Fix:** `"feature_count": len(cells[cell])` — the grouping dict is already in
scope. `build_full_ptilesb.py` already does this correctly.
**Then:** rebuild and republish all 51 business files, or the published data
keeps the zeros.

### 1.2 `decode_signals` / `decode_cameras` report nonsense as success — [verified]

`core/src/signals.rs:95`, `core/src/camera.rs:142` — both `Err(_) => break`.

Handing `decode_signals` a merged block (what happens if a caller skips
`merged_cell_slice`, and both are merged layers):

```
decode_signals(merged block) -> Ok(2 records)
   lat=167.91342  lon=2516.24336  type=traffic_signals
   lat=1417.14641 lon=1174.40511  type=traffic_signals
decode_signals(3 garbage bytes) -> Ok(0)
```

A latitude that cannot exist, returned as a successful decode. Nothing
distinguishes "this cell has no signals" from "these bytes are not signals".
Every other layer reports framing overruns (`roads.rs:181`, `buildings.rs:218`,
`business.rs:170`); these two have no framing to check.

**Fix:** on a mid-stream record failure, return
`DecodeError::RecordOverrun { offset, len, block_len }` rather than breaking —
i.e. distinguish "clean end of input" from "stopped early".

### 1.3 The 19-vs-38 misdetection is still reachable, and its guard no longer works — [verified]

Two halves, both confirmed.

**The path.** `parse_index_detected(data, None)` probes rather than using the
declared stride. Reached from `wasm/src/lib.rs` `parse_index_entries` and
`find_block_for_cell`, both of which pass `None`. On a 38-byte index with a
non-zero bbox:

```
declared path: entry_size=38  block_offset=100000            block_length=500
probed  path : entry_size=19  block_offset=18764998447377    block_length=1118481
```

No error either way. `parse_header_and_index` (added for the wasm-only port)
does this correctly; these two exports never got the same treatment.

**The guard.** `core/src/index.rs:131-135` claims:

> "squarely inside the 16-byte bbox, which every real builder writes as zeros.
> So a wrong width gives entry 0 a zero length, every time"

`AGENTS.md:58` says the same: *"bbox at bytes 8..24 is written as zeros"*.

**That is false for every current file.** Measured on the corpus:

| file | bbox at bytes 8..24 | block_length if misread at 19 B |
| --- | --- | --- |
| `TN.parks` | `(-8800547, 3564890, -8800376, 3565637)` | 8,912,950 |
| `TN.rail` | `(-8677394, 3616206, -8677373, 3616221)` | 196,663 |
| `US.signals` | `(-16260529, 6689439, -16258969, 6690157)` | 6,750,310 |
| `US.signals.stride42` (pre-fix) | `(0, 0, 0, 0)` | **0** |

So check 1 of `is_structurally_valid` fires only on the *old* files. On current
data a misread yields a large out-of-range length, not zero. `index.rs:17` has
the accurate hedge — *"often all zero"* — and lines 131-135 contradict it.

**Fix:** give `parse_index_entries` and `find_block_for_cell` a `header_bytes`
parameter and route them through `parse_header_and_index`. Separately, correct
the comment and `AGENTS.md:58`, and stop relying on check 1.

---

## 2. Regressions from the wasm-only port

Both mine, both found by the audit, both **FIXED**.

- **FIXED `e51f274`** — the port dropped `if (!raw) return out;` from
  `decodeSignalRecords`. `cellRecords` returns `null` for a cell with no index
  entry, and the wasm decoders throw on null where the JS ones returned `[]`.
  `queryIntersection` walks a 7-cell ring, so one absent cell rejected the whole
  lookup and `doLookup` swallowed it — intersections silently stopped working in
  whole areas. Same guard added to the business and camera paths.
- **FIXED `cc67a94`** — the adapter keyed `cellMap` by the stored H3 id rather
  than the normalised one, so `cellMap.has(cell)` was false for every cell.
  `cellRecords` normalises internally, which is exactly why it hid;
  `BusinessReader.query` reads `cellMap` directly and would have found nothing.

---

## 3. Tests that cannot fail

### 3.1 The cell-normalisation tests are circular, and their premise is wrong — [reported, premise verified]

`web-demo/test/ptiles.test.mjs:44` and `core/src/coarse.rs:322` both define
"what a masked caller has" using **the same constant the production code
normalises with**. The assertion is `f(x) == f(mask(x))` where `f` begins with
`mask()` — idempotent by construction for any mask width. Changing
`CELL_FILLER_BITS` to `0xff_ffff`, `0x3f_ffff` or `0x0f_ffff` leaves both green.
They detect only the total removal of normalisation.

**The documented rationale is also wrong.** `web-demo/js/ptiles.js:39-43`,
`ptiles.test.mjs:40-43`, `docs/HANDOFF-wasm-only-client.md:204-206` and the
commit message for `cc67a94` all say *"the index stores them set;
`latLngToCell` returns them masked"*. It does not:

```
h3.latlng_to_cell(36.1627, -86.7816, 7) -> 87264d106ffffff   (low 24 bits set)
wasm.cell_for_coord(36.1627, -86.7816)  -> 87264d106ffffff   (identical)
```

No wasm H3 export ever returns a masked id. The masking originates in
`demo/index.html:429` / `web-demo/index.html:384`, which mask **both** sides and
are therefore self-consistent whatever the width. A res-7 id has **24** filler
bits (digits 8-15), not the 21 the code uses.

The fix works — both sides use the same constant — but the reasoning behind it
is incorrect and the tests cannot detect the constant being wrong.

**Fix:** widen to `0xff_ffff` / `0xffffffffff000000n`, and make each test assert
using a mask it does *not* share with production. Correct the four places that
state the wrong rationale.

### 3.2 Rust and JS ask different questions of the same corpus — [reported]

`core/src/index.rs::binary_search` is an exact `u64` match with no
normalisation; `web-demo/js/ptiles.js:163,180` normalises. The corpus is billed
as *"one set of bytes, one expected answer, checked from every language"* — but
`conformance_corpus.rs:134` looks up by **stored** id and
`ptiles.test.mjs:81,104,131,155` by **masked** id, so neither ever asks the
other's question and the divergence is structurally invisible.

Intra-Rust too: `CoarseIndex::bracket` normalises, `binary_search` does not — a
Rust caller can get a valid bracket and then fail to find the entry in it.

### 3.3 An assertion guarding the only Relative-offset file that cannot fail — [reported]

`demo/test/index_reader.test.mjs:273`:
`assert.ok(["absolute","relative","corrected"].includes(base.kind))` — the
function can only return those three. `TN.buildings_v8` is the only Relative
layer in existence; if `pickOffsetBase` regressed to always return `"absolute"`,
buildings would render blank and this stays green. `manifest.json` is a sibling
file; assert against it.

### 3.4 Other coverage gaps — [reported]

- `ffi/tests/integration.rs:9` hardcodes a machine-local data dir with **no**
  corpus fallback and **no** coverage guard. 24 of 31 tests skip on CI, silently.
  This is the entire Kotlin/Swift/Python surface.
- `core/src/http_source.rs:284,341` — the only test exercising
  `read_cell` -> merged slicing -> `decode_rail` on real bytes hits a live host
  and `return`s on error, so "no network" is indistinguishable from "broken".
- `wasm/test/golden.mjs` is matched by no CI glob and runs nowhere.
- `conformance_corpus.rs` only ever calls `read_block`, never `read_cell`, so
  merged-block mis-slicing is uncovered for parks/rail/water/roads/business.
- Corpus lacks `address`, `US.admin` (the lookup-grid shape that once misparsed
  as 4,247,762,216 entries), `highways` and `business_name_index`.
- Nine of eleven corpus files have their dictionary stripped, so **only
  `TN.water` distinguishes "the dict path works" from "the dict path is dead
  code that always falls through"**. Worth a line in `conformance/README.md`.

---

## 4. Core and wasm

Ranked, all [reported] unless noted.

1. **`decode_business` sniffs v3-vs-v4 from four bytes and both arms fail
   silently** (`business.rs:306-324`). v3 accepts v4 bytes because
   `decode_business_v3` skips undecodable records and therefore almost always
   returns `Ok`. And `decode_business_v4` hardcodes `cell_center = (0,0)`
   (`business.rs:282`), so v4 coordinates decode near Null Island — the code says
   so itself at `:284-290`, inside the shipping path, and `wasm/src/lib.rs:119`
   exports it to the browser. Fix: dispatch on the header version like
   `decode_buildings` does, and require a cell center.
2. **`CoarseBracket::byte_range` accepts any `entry_size`** including 0 and
   unknown widths (`coarse.rs:109`), while `parse_entry_run` refuses them. A
   wrong width Range-requests half the entries and lands mid-entry.
3. **`AddressFile` never applies the offset-base rule** (`address.rs:262`) — reads
   at `entry.block_offset` verbatim. Latent (address files are absolute today),
   but it is the same rule expressed twice with one copy wrong.
4. **`AddressFile::open` / `AdminFile::open` allocate from unvalidated header
   lengths** (`address.rs:244`, `admin.rs:255`). `PtilesFile::open` guards exactly
   this at `file.rs:233` and has a regression test for it; these two skipped it.
5. **`unwrap_or_default()` erases an index/block disagreement**
   (`address.rs:264`, `wasm/src/lib.rs:820`) — merges "not indexed", "empty" and
   "the block's own cell table contradicts the index" into one answer.
6. **`CoarseBracket::is_empty()` can never be true** (`coarse.rs:98`) — `len()` is
   `saturating_sub().saturating_add(1)`, always >= 1. Exists only to satisfy
   clippy, and lies.
7. **The buildings version gate is dead code** — `decode_buildings_v8` has no
   callers outside its own tests; everything uses the ungated `decode_buildings`,
   so a v6/v7 block decodes into plausible buildings with wrong geometry.

**Duplication that will drift:** `address.rs` is a second implementation of the
v2 index *and* merged blocks (`V2_INDEX_ENTRY_SIZE`, `decode_v2_entry`,
`parse_v2_index`, `index_search`, `merged_block_cell_slice`) — and it has
**already** drifted, being the source of items 3 and 4 above. `wasm` re-implements
the zstd dict fallback with a comment saying "keep the two in sync" where a
function call belongs. The 38-byte layout is now spelled out in bytes in three
places.

---

## 5. Both demo pages

Ranked, all [reported].

1. **`ptilesLayerRevision` does not guard the thing that matters.**
   `switchState` never increments it, and the three `loadPtilesLayer*` functions
   never check it — they capture `currentState` before the await and assign
   `layer.reader` after. Now reachable from an ordinary pan, since moveend
   auto-switches state: pan TN -> GA mid-load and whichever open resolves last
   wins. If it is the TN one, every GA cell misses. Blank map, no error, no retry.
2. **`switchState` blanks camera/signal/buildings and never reloads them.**
   All readers are nulled; only roads/water/parks/rail are rebuilt. **This is a
   live regression from the state-follows-map change** (`7651f0b`): crossing any
   state line now silently kills those three layers until the box is toggled.
   Camera and signal are US-wide files that did not need reloading at all.
3. **`rendered.add(cellInt)` happens before the await**, in all seven branches.
   One transient 503 leaves a permanent hole in the map that panning back never
   repairs, and seven `catch(e) {}` blocks guarantee no diagnostic.
4. **Reader promises cache their own rejection** — one flaky 5xx on the signals
   open memoises a rejected promise forever; intersections are gone for the session.
5. **Info panel keeps the previous lookup's values** when nothing is found —
   `doLookup` shows every row up front and only repopulates inside `if (b)`.
6. **`chkBldgs` handler registered twice in `demo/`** (`:1178` and `:1216`) —
   benign only by ordering.
7. **`BusinessReader` bypasses merged-block slicing** — correct today because
   business is 19-byte, silent garbage the day it is rebuilt at 38.
8. **`openPointReader` swallows a genuine coarse-index error** — `P.openCoarse`
   deliberately distinguishes "no PTCI" (null) from "PTCI that does not hold up"
   (throw); the catch erases it, so a corrupt index degrades to a silent 4 MB
   full open forever.
9. **Viewport render truncates an unordered cell list** to 300 — `polygonToCells`
   order is unspecified, so the kept 300 may exclude the map centre.

**Leftover in `web-demo` that should not be there:** `COARSE_MAGIC` and
`SIGNAL_TYPES` are dead but still present — format constants sitting in the page
waiting to drift. `ptiles.js` hardcodes `entry_size === 38` in two places, in
the module whose stated premise is that only wasm decides.

**A doc claim to correct:** `web-demo/js/ptiles.js:26-27` says the port removed
two vendored libraries, "the zstd build and h3-js". The zstd half is true.
**h3-js is still 192 KB and still loaded**, and the page calls the global `h3.*`
about 30 times; `P.h3` is used only by the test.

---

## 6. Builder scripts (`ptiles` repo)

Beyond §1.1. All [reported] except where noted.

1. **`build_all_roads.py` is dead and writes v1.** It imports three names that do
   not exist in `build_roads.py` (`ImportError` confirmed by the agent), and
   `:262` writes `version=1`, which `versions.rs` refuses.
2. **`build_us_highways.py` disagrees with the reader on five record fields**
   while claiming `VERSION = 2`, which `check_supported` accepts: zigzag vs plain
   varint, u8 vs u16 vertex count, class/flags order inverted, and three flag bits
   meaning different things. `roads.rs:188` skips undecodable records without a
   count, so the result is an empty or partly-garbage layer with no error.
3. **`build_roads.py` has the same class/flags inversion**, plus `ref` as u16
   where the reader reads u8 — and it is live (`batch_build_roads.py`,
   `run_us_build.sh`). It also writes a completely different container: magic
   `PTLR`, version at byte 4 (the header has it at byte 8), no H3 index at all.
4. **`ptiles/codec.py` defines `INDEX_ENTRY_SIZE_V2` twice** — as **37** at `:548`
   and **38** at `:612`. The later wins, so it is correct today. This is the
   42-vs-38 landmine in its purest form.
5. **`shared.py:343` `index_entry_v2_format()` returns a 32-byte format string**
   for a 38-byte entry. Currently unused.
6. **`shared.py:391` `decode_merged_block` disagrees with `encode_merged_block`
   in the same file** — the decoder reads a length prefix the encoder never writes.
   `merged.rs:22-25` already documents that the encoder is the authority.
7. **`shared.py:359` silently truncates `block_length` above 2^24.**
   `upgrade_roads_v2.py` gets this right with an explicit guard and a raise.
8. **`build_full_ptilesb.py:239` picks i16 or i32 coordinates with no flag bit
   recording which.** `business.rs` always reads i16. Unreachable today
   (res-7 offsets are ~1,200 units), but if it fires every subsequent record in
   the block desyncs. Should raise rather than branch.
9. **Cell derived at full precision, coordinates stored quantised** in
   `build_places.py`, `build_address.py`, `build_parks.py`, `build_rail.py` — a
   point within 1e-5 deg of an edge is indexed under a cell its own payload says it
   is not in. `build_points.py:258` already does it correctly.
10. **Three state-bbox tables with different values and opposite coordinate
    order** (`states.py` lon-first; `build_us_poi.py` lat-first;
    `build_water.py` a third). Nine states disagree between the first two.
11. **`build_us_poi_v2.py:121` points at a path that no longer exists**, so every
    state returns `[]` and `main()` exits 0 — a clean-looking run that builds nothing.
12. **`build_national_parquet.py:913` `latlon_to_state()` is first-match-wins over
    overlapping padded bboxes** — (36.0, -89.5), Reelfoot Lake in Tennessee,
    returns `AR`.
13. **`rebuild_buildings_index.py` has three `NameError`s** and is non-functional.
14. **39 `.pyc` files tracked in git.**

**Checked and sound:** `build_points.py` (the only builder that verifies its own
output — stride, offsets, sort order, coarse-index consistency, zstd magic, cell
containment), its PTCI writer against `coarse.rs`, `shared.py::HEADER_STRUCT`
against `header.rs` field-by-field, and `upgrade_roads_v2.py`.

---

## 7. FFI, CLI, motion

1. **`PtilesStack` decodes any layer with any decoder** — [reported, measured by
   the agent]. `new` takes three `Option<Arc<PtilesLayer>>` of the same opaque
   type; in Swift/Kotlin/Python nothing distinguishes the slots, and `score` has
   no `kind` check where every `PtilesLayer` method has one. Swapping the roads
   and business slots returned **4,727 candidates instead of 14,294, with no
   error**. Fix is one `if` per slot in `score`.
2. **CLI swallows every read error into "no data"** (`main.rs:546`,
   `.ok().flatten()`). A truncated file returns `{"candidate_count": 0,
   "nearest_road": null}` and exit 0. The FFI propagates the same failure correctly.
3. **CLI silently ignores unknown flags** — `args.finish()` is never called, so
   `--rng 1` is accepted as ring 0 and `--bogus` exits 0.
4. **`motion`'s `max_gap_ms` reset is bypassed** whenever the platform supplies a
   speed, so a 10-minute gap leaves a stale 16 m/s that biases scoring toward roads.
5. **FFI reports I/O failures as `Decode`** — a dropped connection and a corrupt
   block are the same variant.
6. **Both CLI and FFI call `read_block`, not `read_cell`** — latent, becomes
   silent garbage the day any of their three layers ships with a 38-byte index.
   `read_cell` is byte-identical on v1, so the fix is free.

**Verified clean, and worth recording:** no accidental exports in the FFI; no
panics reachable across the boundary; the committed bindings are genuinely
current (all 36 symbols and 14 checksums identical across the three languages);
and the CI freshness check is real — it regenerates in place and `git diff
--exit-code`s, with no path filter or `|| true`. One gap: it compares tracked
files only, so a newly-emitted *untracked* binding would slip through.

**`motion/`** has zero consumers workspace-wide. The code is sound apart from
item 4. Either expose it through `ffi` — the CoreLocation story in
`ffi/src/lib.rs:11-15` is exactly its use case — or delete it. It should not stay
as it is.

---

## 8. Documentation

19 inaccuracies. The byte-layout ones matter most, because a spec claiming
37-byte entries is how a reader gets written wrong in the first place.

| where | says | actually |
| --- | --- | --- |
| `AGENTS.md:58` | 38 B bbox "is written as zeros" | holds a real bbox — see §1.3 **[verified]** |
| `AGENTS.md:65` | a 38-as-19 misread gives zero length | gives a large out-of-range length **[verified]** |
| `AGENTS.md:62` | US.signals/camera "overshoot by count*4" | rebuilt; true only of the corpus `stride42` slices |
| `ffi/README.md:3` | bindings for "Swift and Kotlin" | **three** languages; CI gates Python too |
| `README.md:103` | `/maps/v4-20260711/{ST}.{layer}.ptiles` | no version directory; that path 404s |
| `README.md:107` | `buildings_v9`, `business_v4`, `address_v1`, ... | `roads`, `water`, `business`, `buildings_v8`, `parks`, `rail`, `places`, `address` — every `_vN` name 404s, and `roads` is missing |
| `README.md:13` | core is "zero-alloc ... for all PTiles layers" | `alloc` throughout; no `places` decoder exists |
| `README.md:16` | `ffi` is a "C ABI surface" | UniFFI proc-macro mode; no `extern "C"` anywhere |
| `README.md:4,18,26` | `fuzz` is a workspace member | it has its own `[workspace]`; `--workspace` never builds it |
| `demo/README.md:6` | "No PTILES decoder logic is duplicated in JavaScript" | `demo/index.html` **is** the hand-rolled JS decoder |
| `demo/README.md:20` | `demo/js/app.js` imports the module | dead code, referenced by nothing |
| `demo/README.md:93` | "`.github/` is now empty" | 7 CI jobs |
| `README.md:92` | `steele.red/ptiles` -> `demo/` | -> `web-demo/`; `ptiles-legacy` -> `demo/` |
| `docs/INTEGRATION.md:17` | "one whole-file GET per layer, no Range requests" | Range-backed throughout, with an ETag Cache API layer |
| `fuzz/README.md:3` | "Three libFuzzer targets" | twelve |
| `AGENTS.md:44` | `render_check.py` "fails if it stops matching `demo/`" | fails only on zero features or a page error; there is no comparison |
| `SUPPORTED_FORMATS.md:32` | cites `versions.rs::tests::doc_matches_generated_table` | lives in `core/tests/supported_formats_doc.rs` |
| `versions.rs:14` | `PTILESA` "deliberately absent from this table" | it is in the table, and tested |
| `HANDOFF-wasm-only-client.md:249` | address 404s | address is live; highways still 404s |

`ffi/README.md` is the most actively harmful: anyone following its regeneration
block leaves `bindings/python/` stale and reddens CI.

---

## 9. Simplification

- `core/tests/_scratch_golden.rs` — scratch committed as a test, duplicating
  `golden.rs` with weaker assertions. Delete.
- `index::parse_index` and its private `ENTRY_SIZE` — no callers outside their own
  tests, but re-exported. Its own doc says it "reads a 38-byte index as garbage
  rather than failing, which is the historical bug". Deleting removes an exported
  footgun.
- `merged::cell_ids` — exported, zero callers.
- Root `src/lib.rs` (819 lines) — header says "LEGACY SEED — superseded"; the port
  is done, and `package.json:8` ships it to every npm consumer.
- `package.json:5` points npm at `pkg/ptiles_client.js` — the **superseded**
  decoder, not `wasm/`'s output. The npm half of the contract is currently wrong.
- Dead JS: `demo/js/{app,ptiles-remote}.js` plus undocumented root-level
  duplicates `js/{app,ptiles-remote}.js`.
- `ffi/Cargo.toml:28` pulls `uniffi` with `bindgen-tests`, but
  `build_foreign_language_testcases!` appears nowhere — that macro is the only
  thing that would actually execute the generated Swift/Kotlin/Python.
- Clippy: 8 warnings, none touching a decode path. `-D warnings` would go red on
  day one for unrelated reasons.

---

## 10. Judgement on CI

**Worth adding:** `cargo publish --dry-run` (four crates cannot publish today and
nothing catches it); anything touching `package.json`; `cargo test -p ptiles-core
--no-default-features` (the `no_std` job only *builds*, so `coarse.rs`,
`merged.rs` and `index.rs` unit tests never run under it); one real
`aarch64-linux-android` link for `ffi`.

**Not worth it:** clippy as a gate, `fmt`, and a browser render check — the render
harness reported zero for every layer three separate times because PTILES Mode
gates rendering, and a check that can report zero without failing is worse than
none.

---

## 11. Open questions

1. Business `feature_count` is a one-line fix but needs all 51 files rebuilt and
   republished to take effect. Nothing currently reads the field. Worth doing?
2. `motion/` — expose through ffi, or delete?
3. Does any real caller emit a 24-bit-masked cell id, or is `normalize_cell`
   guarding a form only `index.html` invents? Decides whether §3.1's fix is
   corrective or defensive.
4. Is `bindings_fresh` a *required* status check in branch protection? Not
   determinable from the tree; if it is not, a red run is still mergeable.
5. Is the CLI's 3-layer scope a deliberate Rookery contract, or should
   water/parks/rail/signals/camera be wired up? It fails loudly today, so this is
   a roadmap question, not a correctness one.
6. What was in `TN.water`'s 812 KB aux region? It is dropped from the corpus and
   nothing records what it held.
