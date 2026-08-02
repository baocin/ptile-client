// The index layout, across the wasm boundary, against real bytes.
//
// Reading a .ptiles file means getting three things right before a single
// record is decoded: the index entry width (19 or 38), the offset base
// (absolute, relative, or absolute-but-overshooting), and the arithmetic that
// turns a stored offset into a file offset. Each has been wrong at least once,
// and all three fail the same way -- a plausible offset that reads the wrong
// bytes, or a zero-length block that renders as "no data here" instead of as
// an error.
//
// Until now the wasm boundary could only answer the first. `parse_index_entries`
// takes index bytes with no header, so it cannot see `blocks_offset` and cannot
// know which base applies; the caller had to decide. `demo/index.html`'s
// `pickOffsetBase` is what that decision looked like in practice -- a second
// implementation of the rule, written in the language that got the entry stride
// wrong in the first place.
//
// `parse_index_layout` and `index_entries_absolute` close that. Both call
// `ptiles_core::index_layout`, which is the same function `PtilesFile::open`
// uses, so a JS caller and a Rust caller cannot reach different conclusions
// about the same file.
//
// Two things are checked here:
//   1. wasm resolves every entry in every corpus file onto a real zstd frame.
//      This is end-to-end: width, base and arithmetic all have to be right, and
//      a zstd magic is hard to hit by accident.
//   2. index.html's own pickOffsetBase agrees with core on every file. That is
//      the migration harness for replacing it -- when it stops being called,
//      this test is the record that it agreed at the time.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, existsSync, readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { createRequire } from "node:module";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = join(HERE, "..", "..");
const WASM_PKG = join(ROOT, "wasm-pkg", "ptiles_wasm.js");
const CORPUS = join(ROOT, "conformance", "corpus");

if (!existsSync(WASM_PKG)) {
  throw new Error(
    `wasm-pkg/ not built -- this test cannot run.\n` +
    `  PATH="$HOME/.cargo/bin:$PATH" wasm-pack build wasm --target nodejs --out-dir ../wasm-pkg --release`,
  );
}

const require = createRequire(import.meta.url);
const wasm = require(WASM_PKG);
const html = readFileSync(join(ROOT, "demo", "index.html"), "utf8");
const manifest = JSON.parse(readFileSync(join(ROOT, "conformance", "manifest.json"), "utf8"));

function grabFunction(name) {
  const start = html.indexOf(`function ${name}(`);
  if (start < 0) throw new Error(`function ${name} not found in index.html`);
  let i = html.indexOf("{", start), depth = 0;
  for (; i < html.length; i++) {
    if (html[i] === "{") depth++;
    else if (html[i] === "}") { depth--; if (depth === 0) return html.slice(start, i + 1); }
  }
  throw new Error("unbalanced braces");
}

const JS = (() => {
  const src = [
    html.match(/var ENTRY_SIZE_V1 = [^\n]*\n/)[0],
    html.match(/var KNOWN_ENTRY_SIZES = [^\n]*\n/)[0],
  ];
  for (const f of [
    "u16", "u32", "u64", "f32", "readPacked", "parsePtilesHeader", "readIndexEntry",
    "indexIsStructurallyValid", "parsePtilesIndex", "pickOffsetBase",
  ]) src.push(grabFunction(f));
  src.push("return { parsePtilesHeader, parsePtilesIndex, pickOffsetBase };");
  return new Function(src.join("\n"))();
})();

const FILES = readdirSync(CORPUS).filter((f) => f.endsWith(".ptiles")).sort();

/** header bytes + index bytes for one corpus file, the two Range reads a client makes. */
function sections(name) {
  const fd = readFileSync(join(CORPUS, name));
  const all = new Uint8Array(fd.buffer, fd.byteOffset, fd.length);
  const header = all.subarray(0, 256);
  const h = wasm.parse_header(header);
  const start = Number(h.index_offset);
  return { all, header, index: all.subarray(start, start + h.index_length), h };
}

const ZSTD_MAGIC = [0x28, 0xb5, 0x2f, 0xfd];

test("the corpus is present", () => {
  assert.ok(FILES.length >= 8, `only ${FILES.length} corpus files found`);
  assert.equal(FILES.length, Object.keys(manifest.files).length);
});

test("wasm resolves every index entry onto a real zstd frame", () => {
  let total = 0;
  for (const name of FILES) {
    const { all, header, index } = sections(name);
    const entries = wasm.index_entries_absolute(header, index);
    assert.ok(entries.length > 0, `${name}: no entries resolved`);

    for (const e of entries) {
      const off = Number(e.block_offset);
      const magic = Array.from(all.subarray(off, off + 4));
      assert.deepEqual(
        magic, ZSTD_MAGIC,
        `${name}: cell ${e.h3_cell.toString(16)} resolved to offset ${off}, which is ` +
        `not the start of a zstd frame (got ${magic.map((b) => b.toString(16))}). ` +
        `The entry width, the offset base, or the arithmetic applying it is wrong.`,
      );
      total++;
    }
  }
  assert.ok(total > 100, `only ${total} entries checked`);
});

test("wasm reports the layout the manifest records", () => {
  for (const name of FILES) {
    const { header, index } = sections(name);
    const got = wasm.parse_index_layout(header, index);
    const want = manifest.files[name];

    assert.equal(got.entry_size, want.entry_size, `${name}: entry width`);
    assert.equal(got.entry_size_source, want.entry_size_source, `${name}: how the width was chosen`);
    assert.equal(got.declared_stride ?? null, want.declared_stride ?? null, `${name}: declared stride`);

    const base = typeof got.offset_base === "string"
      ? got.offset_base
      : Object.keys(got.offset_base)[0];
    assert.equal(base, want.offset_base, `${name}: offset base`);
    if (base === "AbsoluteCorrected") {
      assert.equal(
        Number(got.offset_base.AbsoluteCorrected.overshoot), want.overshoot,
        `${name}: overshoot`,
      );
    }
  }
});

test("the two known-broken files are reported as inconsistent, not merely unusual", () => {
  const broken = FILES.filter((f) => f.includes("stride42"));
  assert.equal(broken.length, 2, "expected both stride42 files");

  for (const name of broken) {
    const { header, index } = sections(name);
    const got = wasm.parse_index_layout(header, index);
    assert.equal(got.declared_stride, 42, `${name}: declared stride`);
    assert.equal(
      got.entry_size_source, "Probed",
      `${name}: 42 divides evenly but is not a width we know, so the width has ` +
      `to come from probing the bytes rather than from the header`,
    );
    assert.ok(
      got.header_is_inconsistent,
      `${name}: the header contradicts its own index and the layout should say so`,
    );
  }

  // And the files that were rebuilt to fix this must NOT be flagged.
  for (const name of ["US.signals.ptiles", "US.camera.ptiles"]) {
    const { header, index } = sections(name);
    assert.equal(
      wasm.parse_index_layout(header, index).header_is_inconsistent, false,
      `${name} is the rebuilt file; if this fires, the builder has regressed`,
    );
  }
});

// The coarse index is the reason a client can open US.signals without pulling
// 4014 KiB of index first. These check the whole path through wasm: parse the
// aux region, bracket a cell, and confirm the byte range that comes back really
// does contain that cell's entry -- which is what a partial open relies on and
// what nothing outside the browser could verify before core learned PTCI.
test("wasm parses the coarse index, and brackets land on the right entries", () => {
  const withAux = FILES.filter((f) => (manifest.files[f].aux_length ?? 0) > 0);
  assert.ok(withAux.length >= 2, `expected 2+ files with a coarse index, got ${withAux.length}`);

  for (const name of withAux) {
    const { all, header, index, h } = sections(name);
    const auxAt = Number(h.aux_offset);
    const aux = all.subarray(auxAt, auxAt + h.aux_length);

    const coarse = wasm.parse_coarse_index(aux);
    assert.ok(coarse, `${name}: aux holds a coarse index but wasm returned null`);
    assert.ok(coarse.samples.length > 0, `${name}: no samples`);

    const entries = wasm.index_entries_absolute(header, index);
    assert.equal(
      Number(coarse.entry_count), entries.length,
      `${name}: coarse entry_count disagrees with the real index length`,
    );

    // Every sample must name the cell actually at that position.
    for (const s of coarse.samples) {
      const at = Number(s.entry_index);
      assert.equal(
        entries[at].h3_cell, s.h3_cell,
        `${name}: sample says entry ${at} is cell ${s.h3_cell.toString(16)}, ` +
        `but it is ${entries[at].h3_cell.toString(16)}`,
      );
    }

    // And a bracket must produce a byte range that contains the wanted entry.
    // This is the partial-open path end to end.
    const layout = wasm.parse_index_layout(header, index);
    for (let i = 0; i < entries.length; i += 37) {
      const cell = entries[i].h3_cell;
      const br = wasm.coarse_bracket(
        aux, cell.toString(16), h.index_offset, layout.entry_size,
      );
      assert.ok(br, `${name}: cell ${cell.toString(16)} bracketed to nothing`);
      assert.ok(
        br.start <= i && i <= br.end,
        `${name}: entry ${i} is outside its own bracket ${br.start}..${br.end}`,
      );

      // Read only the bracketed bytes, exactly as a Range request would, and
      // confirm the cell is findable in them.
      const from = Number(br.byte_from), to = Number(br.byte_to);
      const run = all.subarray(from, to + 1);
      assert.equal(
        run.length, br.entries * layout.entry_size,
        `${name}: bracket byte range is not a whole number of entries`,
      );
      let found = false;
      for (let off = 0; off + 8 <= run.length; off += layout.entry_size) {
        const dv = new DataView(run.buffer, run.byteOffset + off, 8);
        if (dv.getBigUint64(0, true) === cell) { found = true; break; }
      }
      assert.ok(
        found,
        `${name}: cell ${cell.toString(16)} is not inside the ${br.entries}-entry ` +
        `run its own bracket named -- a partial open would report it missing`,
      );
    }
  }
});

test("a file with no coarse index returns null rather than throwing", () => {
  // Every layer built before PTCI existed is in this state; it is the normal
  // case, and a client has to be able to tell it from a malformed one.
  const { all, h } = sections("TN.roads.ptiles");
  assert.equal(h.aux_length, 0, "TN.roads is expected to carry no aux region");
  assert.equal(wasm.parse_coarse_index(new Uint8Array(0)), null);
  assert.equal(wasm.parse_coarse_index(all.subarray(0, 64)), null);
});

// index.html decides the offset base itself, in pickOffsetBase. This pins that
// copy against core. When the wasm-only port removes it, this test is the
// record that the two agreed beforehand -- and until then, it fails if either
// side drifts.
test("index.html's pickOffsetBase agrees with core on every corpus file", () => {
  const NAMES = { absolute: "Absolute", relative: "Relative", corrected: "AbsoluteCorrected" };
  let checked = 0;

  for (const name of FILES) {
    const { all, header, index } = sections(name);
    const h = JS.parsePtilesHeader(all.subarray(0, 256));
    const parsed = JS.parsePtilesIndex(all, h);
    const jsBase = JS.pickOffsetBase(parsed, h);

    const rs = wasm.parse_index_layout(header, index);
    const rsBase = typeof rs.offset_base === "string"
      ? rs.offset_base
      : Object.keys(rs.offset_base)[0];

    assert.equal(parsed.entrySize, rs.entry_size, `${name}: entry width JS vs core`);
    assert.equal(
      NAMES[jsBase.kind], rsBase,
      `${name}: JS picked ${jsBase.kind}, core picked ${rsBase}`,
    );

    // Compare the resolved offsets, not just the labels. The two sides use
    // different conventions -- JS carries a signed `adjust` to add, core
    // carries an unsigned `overshoot` to subtract -- so equal labels alone
    // would not prove they land on the same byte.
    const rsEntries = wasm.index_entries_absolute(header, index);
    assert.equal(rsEntries.length, parsed.entries.length, `${name}: entry count`);
    for (let i = 0; i < parsed.entries.length; i++) {
      assert.equal(
        parsed.entries[i].blockOffset + jsBase.adjust,
        Number(rsEntries[i].block_offset),
        `${name}: entry ${i} resolves to a different byte in JS than in core`,
      );
      checked++;
    }
  }

  assert.ok(checked > 100, `only ${checked} offsets compared`);
});
