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
