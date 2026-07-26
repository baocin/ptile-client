// Benchmark the JS index parse and block decompress against the largest real
// layer, so the "108k object literals" question gets answered with numbers
// instead of intuition.
//
// The comparison that matters is parse time vs the range fetch that has to
// happen first: openPtilesRemote fetches header, dict and index up front over
// HTTP before a single block is read. If parsing is small next to that, the
// typed-array rewrite is not worth destabilising BusinessReader for.
//
// Usage: node demo/test/bench_index.mjs [path-to.ptiles]

import { readFileSync, existsSync } from "node:fs";
import { performance } from "node:perf_hooks";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const INDEX_HTML = join(HERE, "..", "index.html");

const CANDIDATES = [
  process.argv[2],
  "/home/aoi/kino/projects/ptiles/tiles/US.signals.ptiles",
  "/home/aoi/kino/data/ptiles/TN.business.ptiles",
].filter(Boolean);

const path = CANDIDATES.find((p) => existsSync(p));
if (!path) {
  console.error("no fixture found; pass one as argv[1]");
  process.exit(1);
}

function grabFunction(html, name) {
  const start = html.indexOf(`function ${name}(`);
  if (start < 0) throw new Error(`${name} not found`);
  let i = html.indexOf("{", start), depth = 0;
  for (; i < html.length; i++) {
    if (html[i] === "{") depth++;
    else if (html[i] === "}" && --depth === 0) return html.slice(start, i + 1);
  }
  throw new Error(`unbalanced ${name}`);
}

const html = readFileSync(INDEX_HTML, "utf8");
const names = ["u16", "u32", "u64", "f32", "readPacked", "parsePtilesHeader",
  "readIndexEntry", "indexIsStructurallyValid", "parsePtilesIndex"];
const R = new Function([
  html.match(/var ENTRY_SIZE_V1 = [^\n]*\n/)[0],
  html.match(/var KNOWN_ENTRY_SIZES = [^\n]*\n/)[0],
  ...names.map((n) => grabFunction(html, n)),
  `return { ${names.join(", ")} };`,
].join("\n"))();

const fd = readFileSync(path);
const head = new Uint8Array(fd.buffer, fd.byteOffset, 256);
const h = R.parsePtilesHeader(head);
const buf = new Uint8Array(fd.buffer, fd.byteOffset, h.indexOffset + h.indexLength);

const first = R.parsePtilesIndex(buf, h);
const N = 7;
const times = [];
for (let i = 0; i < N; i++) {
  const t = performance.now();
  R.parsePtilesIndex(buf, h);
  times.push(performance.now() - t);
}
times.sort((a, b) => a - b);
const median = times[Math.floor(N / 2)];

const heapBefore = process.memoryUsage().heapUsed;
const kept = R.parsePtilesIndex(buf, h);
const heapAfter = process.memoryUsage().heapUsed;

const indexKiB = h.indexLength / 1024;
console.log(`file          ${path.split("/").pop()}`);
console.log(`entries       ${kept.entries.length.toLocaleString()} x ${kept.entrySize} B ` +
  `(${kept.entrySizeSource})`);
console.log(`index section ${indexKiB.toFixed(0)} KiB`);
console.log(`parse         median ${median.toFixed(1)} ms  ` +
  `(min ${times[0].toFixed(1)}, max ${times[N - 1].toFixed(1)}, n=${N})`);
console.log(`heap for entries  ~${((heapAfter - heapBefore) / 1048576).toFixed(1)} MiB`);
console.log(`\nthe index must be fetched before it can be parsed:`);
console.log(`  ${indexKiB.toFixed(0)} KiB over a 50 Mbps link  ~${(indexKiB * 8 / 51200 * 1000).toFixed(0)} ms transfer, plus one RTT`);
console.log(`  parse is ${median.toFixed(1)} ms of that`);
void first;
