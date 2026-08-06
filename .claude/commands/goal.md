---
description: Optimize ptile-client and widen its test coverage, measured against the steele.red/ptiles demo
argument-hint: "[focus, e.g. 'buildings render time' or 'dict path coverage' — omit to pick the biggest unclaimed win]"
---

Make <https://steele.red/ptiles> as fast as this repo can make it, and make the
test suite able to prove it. Target: **$ARGUMENTS** (empty means pick the
largest measured win that is not already ruled out below).

**Scope fence: `ptile-client` only.** Not the `ptiles` builder repo, not the
`steele.red` repo. Several things that look like client bugs are upstream and
are listed in section 6 — recognise them, do not try to fix them here.

## 1. Orient, then distrust what you read

```
git log --oneline -15
git status --porcelain
```

Read `GOAL.md`, `AGENTS.md`, and `docs/HANDOFF-audit-2026-08-04.md`.

Treat all three as claims, not evidence. They have been wrong repeatedly, and
the errors were the kind that waste a whole session:

- `README.md` asserted a tile path (`maps/v4-20260711/…`) that 404s, eight layer
  filenames that do not exist, and a C ABI on a crate that has no `extern "C"`
  in it. All four were fixed in `07edebe` only after being checked against the
  live host.
- `GOAL.md`'s "IN FLIGHT" sections are dated 2026-08-04 and may be long done.
  Each carries the command that settles it. Run it.
- The audit's §1 defects are mostly still open, but §1.2 was fixed in `62bdf4b`.
  Check before re-reporting anything as broken.

Re-derive every number and path from the repo or the live host. Cite what you
measured. When a doc turns out to be wrong, fix the doc in the same commit.

## 2. You cannot optimize this yet — build the harness first

`GOAL.md` quotes time-to-stable-render per layer (roads 7.44s, water 6.99s,
bldgs 11.87s, parks 8.97s, signal 8.68s, camera 3.90s). **No script in this repo
produces those numbers.** `render_check.py` counts features and never looks at a
clock; `bench_index.mjs` and `bench_wasm.html` are micro-benchmarks, and
`bench_wasm.html` is the one whose result GOAL.md explicitly says does not
transfer to a real page.

So the first deliverable of any optimization work is a repeatable measurement,
or every claim after it is unfalsifiable. It needs to report, per layer, at a
fixed viewport and a cold and warm cache:

- wall time from PTILES Mode to a stable feature count
- number of HTTP requests and total bytes over the wire
- time split between fetch, zstd, decode and Leaflet

Borrow the settle loop from `render_check.py` (stable for 3 samples, not merely
non-zero) and the Range-capable handler from `demo/test/coarse_check.py`.
Run each measurement at least three times and report the median with the spread;
a single number over a live CDN is noise.

Then optimize against it, and put the before/after in the commit message.

## 3. Already measured and rejected — do not re-derive

Every one of these is where an optimizer's instinct goes first. The numbers are
in this file's git history.

- **The ~30x wasm-boundary penalty does not apply to a real page.** Measured
  legacy vs wasm-only, time to stable render: roads 0.95x, water 1.08x, bldgs
  1.33x, parks 1.17x, signal 1.07x, camera 1.00x. The cost is range requests and
  zstd, not the boundary. Do not "fix" the boundary.
- **Typed-array index parsing** — measured, rejected.
- **HTTP-range binary search over a layer's index** — measured, rejected.
- **Shrinking the zstd dictionary** — measured, rejected.
- **h3-js stays** in both demos. It is not a second implementation of *this*
  format, and mixing it with core's H3 for different calls recreates the exact
  duplicate-implementation problem that the whole wasm effort removed.

One near-miss worth stating plainly: the rejected range binary search was over a
**layer index**. The 28 MB `US.admin.ptiles` lookup grid is a different
structure — a flat, sorted, fixed-16-byte-stride table of 1,785,304 entries —
and a range search over *that* is not ruled out. It is the documented upgrade
path for the opt-in jurisdiction load in `web-demo/index.html`. It needs a wasm
export that resolves one grid entry against the string tables instead of
requiring the whole grid.

## 4. Where the time plausibly is

Unverified until you measure them, listed so you start somewhere sensible:

- **Buildings is the worst layer** at 26,488 features. Leaflet's default SVG
  renderer creates a DOM node per polygon; `L.canvas()` is the obvious lever and
  has never been tried here. 3D mode adds a second path per building plus a
  painter's-algorithm sort.
- `renderPtilesForCells` caps at 300 cells and debounces 600 ms. Both are round
  numbers nobody has justified with a measurement.
- The block cache is an unbounded in-memory `Map` keyed by block offset
  (`web-demo/js/ptiles.js`). Never evicted.
- **`ptiles-core` does not read the coarse index at all.** `core/src/coarse.rs`
  exists and is exported, but nothing in `core/src/file.rs` uses it, so the Rust
  side reads whole 4 MB indexes where the browser fetches ~5 KiB plus one short
  run. This is the largest untouched asymmetry between the two readers.
- Business search brute-forces every block when the name index is missing —
  measured at over 180 s for one query without finishing. See section 6.

## 5. Test coverage: the gaps that matter

The suite is large and still has holes where a wrong decode returns a plausible
empty result rather than an error. That is this codebase's characteristic
failure, and it is what coverage here is *for* — ugliness ranks below silence.

Verify each of these before acting; they come from the audit, which is itself a
claim:

- **`wasm/test/golden.mjs` runs nowhere.** CI globs `demo/test/*.test.mjs` and
  `web-demo/test/*.test.mjs`; that file matches neither and is not named
  `*.test.mjs`. 12 KB of tests executing on no machine.
- **`ffi/tests/integration.rs` skips most of itself on CI** when the local data
  directory is absent. That is the entire Kotlin/Swift/Python surface. Count how
  many actually run before deciding what to do.
- **Only `TN.water` covers the zstd dictionary path** — 9 of the 11 corpus files
  had their dictionaries stripped.
- **The cell-normalisation tests are circular**: they assert `f(x) == f(mask(x))`
  where `f` begins with `mask()`. Changing `CELL_FILLER_BITS` leaves them green.
- **No native Rust encoder**, so there is no `decode(encode(x)) == x` property
  anywhere. `docs/ROADMAP.md` suggests water (simplest framing) first.

When you add a test, make it one that **fails on a plausible stub**. The two
worth copying already exist: raising the observer's eye must reveal *more*
buildings, and shrinking the view finder's radius must reveal *fewer*. A bare
count passes on a 2D shadow test that ignores height entirely.

## 6. Not this repo's problem — recognise and move on

Do not attempt these here. Note them if they block you, then work around them.

- **Every published business file reports `feature_count = 0`** — a builder bug
  (`build_us_poi.py` compares a str to an int). Records decode fine.
- **`{ST}.business_name_index.ptiles` is unpublished** — every state checked
  404s. The client's indexed search is written and tested against a local copy;
  it activates when the file ships.
- **No business categories sidecar exists**, so `category_idx` can only ever
  render as a number.
- **`PTILESP` places** passes the version gate with no decoder written.
- **Roads v2 carries no intersection degree or node id**, so
  `nearest_intersection` cannot tell a junction from a road endpoint.

## 7. Traps that have each cost a session

- **PTILES Mode gates all rendering.** With it off, a layer fetches its index,
  never requests a block, and shows a reader, a map group, zero features and no
  error. Click `#btnPtiles`. This has cost two harnesses three runs each.
- **Several layer checkboxes ship `checked`**, so "set the value and dispatch
  change only if it changed" does nothing. Dispatch unconditionally. Same for
  `<select>` elements driven from a test hook.
- **`python3 -m http.server` ignores Range** and answers 200 with the whole
  file. Any byte-level measurement against it is meaningless.
  `demo/test/coarse_check.py` carries a Range-capable handler to borrow.
- **`maps.mydatatimeline.com` 403s the default python-urllib User-Agent.** Send
  a browser UA. Blocks carry no content size, so use streaming zstd.
- **Rebuild the wasm before any browser measurement.** `web-demo/lib/client/` is
  a committed build artifact and goes stale silently:
  ```sh
  PATH="$HOME/.cargo/bin:$PATH" wasm-pack build wasm --target web \
    --out-dir ../web-demo/lib/client --out-name ptiles_client --release
  PATH="$HOME/.cargo/bin:$PATH" wasm-pack build wasm --target nodejs \
    --out-dir ../wasm-pkg --release      # node --test needs this one
  ```
- **Never `pgrep -f "<pattern>"` inside a script whose own command line contains
  that pattern.** It matches itself and waits forever.
- `/mnt/core` is NFS and has been observed at 131 kB/s with directory listings
  timing out past 900 s. Construct filenames explicitly rather than globbing.

## 8. Verify by observation

Reading your own diff is not verification, and neither is a green count from a
harness that cannot fail.

```sh
cargo test --workspace
cargo build -p ptiles-core --no-default-features --target thumbv7em-none-eabihf
cargo build -p ptiles-wasm --target wasm32-unknown-unknown --release
node --test web-demo/test/ptiles.test.mjs
node --test "demo/test/*.test.mjs"
python3 web-demo/test/render_check.py        # the real page, live tiles, ~7 min
python3 conformance/check_published.py       # published headers vs their indexes
```

Paste real output. If something fails, say so with the output. If a step was
skipped, say that.

## 9. Commit, and deploy only if asked

One commit per coherent change: what changed, **why it was wrong or slow
before**, and the measured evidence, with numbers. End with:

```
Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
```

Then update `GOAL.md`: move anything finished into "Already done — do not
redo" with its hash, and add anything you measured and rejected to the rejected
list *with the number*, so the next session does not re-derive it.

Deploying is a separate ask. When asked:

```sh
cd ~/kino/projects/steele.red && python3 build.py       # must run on hino; symlinks are absolute
AWS_PROFILE=steele-red-deploy aws s3 sync output/ptiles/ s3://steele.red/ptiles/
AWS_PROFILE=steele-red-deploy aws cloudfront create-invalidation \
  --distribution-id E1X2E2N30TVNGX --paths '/ptiles/*'
```

Those credentials can create an invalidation but cannot read one back
(`cloudfront:GetInvalidation` is denied), so poll the live URL to confirm
propagation rather than the invalidation's status.

## 10. Report

Lead with what is faster or better covered than it was, and the measured
evidence for it. Then continue to the next unblocked item automatically.

Stop and ask only when proceeding either way would waste real work. Give a
recommendation, not a survey.

Close with "Unresolved Questions" if anything is genuinely open. No emojis.
