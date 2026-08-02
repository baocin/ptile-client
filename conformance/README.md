# Conformance corpus

Committed `.ptiles` files that every client must read identically. A new client
is "correct" exactly when it passes this corpus.

## Why it exists

Every format bug this codebase has had came from two implementations of the
same format disagreeing — a JS reader hardcoding a 19-byte index stride while
the generator emitted 38, a builder computing `index_length` at 42 bytes while
the encoder wrote 38, a spec claiming 37. Each was found days or weeks late,
because the failure mode is a *silently empty* layer rather than an error.

Going from two implementations to six multiplies that. The corpus is the
mechanism that catches it: one set of bytes, one expected answer, checked from
every language.

It also fixes a concrete CI failure. `core/tests/real_layers.rs` and
`demo/test/index_reader.test.mjs` both carry a guard that *fails* when no real
fixture is found, so that an empty data directory cannot masquerade as a green
run. Those guards are correct, and on a runner — which has no
`/home/aoi/kino/data/ptiles` — they had nothing to assert against. The corpus
is what they assert against now. The guards were not weakened.

## What is in it

Eleven slices of real published layers, 143 KB total. Between them they cover
both index entry widths, all three offset bases, merged blocks, dictionary and
dictionary-free decompression, a PTCI aux region, and the historical
42-vs-38-byte stride bug.

| file | entry width | offset base | why it is here |
| --- | --- | --- | --- |
| `TN.roads.ptiles` | 19 B | Absolute | v2 roads, intersection table |
| `TN.water.ptiles` | 19 B | Absolute | the one layer whose dictionary is small enough to keep intact |
| `TN.business.ptiles` | 19 B | Absolute | PTILESB v3 records |
| `TN.buildings_v8.ptiles` | 19 B | **Relative** | the only layer observed storing relative offsets |
| `TN.parks.ptiles` | 38 B | Absolute | merged blocks, no dictionary |
| `TN.rail.ptiles` | 38 B | Absolute | 14 entries, 2 KB — kept whole |
| `TN.places.ptiles` | 38 B | Absolute | index-only case; no decoder yet |
| `US.signals.ptiles` | 38 B | Absolute | rebuilt: stride now declared correctly, plus a PTCI aux region |
| `US.camera.ptiles` | 38 B | Absolute | rebuilt, with its PTCI aux region |
| `US.signals.stride42.ptiles` | 38 B | **AbsoluteCorrected** | the published bug, preserved as bytes |
| `US.camera.stride42.ptiles` | 38 B | **AbsoluteCorrected** | the same skew on camera |

The two `stride42` files are the important ones. They are slices of the
pre-fix published `US.signals`/`US.camera`, whose header declared a 42-byte
stride while the encoder emitted 38 — so `blocks_offset` and every absolute
offset derived from it overshot the real block region and not one block was
reachable. Read as 19-byte entries they still look structurally plausible and
report `block_length == 0` for every cell, which is the silent-empty failure
in its original form. No synthetic fixture caught this in time; these bytes do.

`manifest.json` records the expected layout for each file. It is generated, not
hand-written — every value in it is re-derived from the committed bytes and
asserted by the runners.

## Runners

| language | runner | status |
| --- | --- | --- |
| Rust | `core/tests/conformance_corpus.rs` | in place |
| JS | `demo/test/index_reader.test.mjs` (via `SEARCH_DIRS`) | index layout only |
| Python, Kotlin, Swift | — | not yet written |

The Rust runner pins the *layout decision*, not just decoded output: entry
width, why that width was chosen, offset base, declared stride. A reader that
lands on the right bytes via the wrong reasoning passes an output-only test and
fails this one — which is the point, since a reader that is right by accident
is one generator change from being wrong.

## Regenerating

```sh
python3 conformance/slice.py
```

Requires `zstandard` and read access to the published layers on this machine.
**CI never runs this** — it consumes only the committed output.

Each slice keeps the real header, the real index entries (copied verbatim, with
only the offset/length fields repointed), the real aux region, and the real
block payloads — just fewer of them. Entry width, offset base, declared stride,
merged-block cell tables, bbox bytes and `cell_index` all survive. The script
verifies this by reopening every file it writes and confirming the detected
layout still matches the source; a slice that no longer reproduces its source
is an error, not a warning.

Two deliberate departures, both recorded per file in `manifest.json`:

- **`dict: "stripped"`** — six layers carry a 512 KB zstd dictionary, which
  would dominate a corpus otherwise measured in kilobytes. For those, blocks
  are decompressed with the real dictionary and recompressed without one. The
  *decompressed* payload stays byte-identical to the generator's; only the
  compression framing differs. `TN.water` (11 KB dictionary) is kept intact so
  the dictionary path stays covered, and `TN.parks`/`TN.rail` never had one.
- **`aux: "dropped"`** — `TN.water`'s 812 KB aux region is dropped. The few-KB
  PTCI regions on `US.signals`/`US.camera` are kept, since those are what a
  coarse-index reader needs.

## Adding a case

Add it to `CASES` in `slice.py` and re-run. Prefer a real file that exhibits
something no existing case does — a new entry width, a new offset base, a
generator quirk. A case that only re-covers ground `core/tests/index_layout.rs`
already covers synthetically is not worth the bytes.
