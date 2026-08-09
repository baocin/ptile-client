import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { createPtiles } from "../js/ptiles.js";
import { crawlRoadRefCells } from "../js/road-ref-crawler.js";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = join(HERE, "..", "..");
const ROADS = "/home/aoi/kino/data/ptiles/TN.roads.ptiles";
const require = createRequire(import.meta.url);
const wasm = require(join(ROOT, "wasm-pkg", "ptiles_wasm.js"));
const P = createPtiles(wasm);

function gridDisk(cell, radius) {
  const seen = new Set([cell]);
  let edge = [cell];
  for (let ring = 0; ring < radius; ring++) {
    const next = [];
    for (const current of edge) {
      for (const neighbor of wasm.neighbor_cells(current)) {
        if (!seen.has(neighbor)) { seen.add(neighbor); next.push(neighbor); }
      }
    }
    edge = next;
  }
  return [...seen];
}

function shortGridPath(from, to) {
  if (from === to) return [from];
  const first = wasm.neighbor_cells(from);
  if (first.includes(to)) return [from, to];
  for (const middle of first) {
    if (wasm.neighbor_cells(middle).includes(to)) return [from, middle, to];
  }
  throw new Error(`${from} and ${to} are more than two cells apart`);
}

test("a highway ref is followed through changing OSM way ids", async () => {
  assert.ok(existsSync(ROADS), `real roads fixture missing: ${ROADS}`);
  const bytes = readFileSync(ROADS);
  const layer = await P.open(P.bytesSource(
    new Uint8Array(bytes.buffer, bytes.byteOffset, bytes.length)));

  const result = await crawlRoadRefCells({
    start: [36.1627, -86.7816],
    end: [35.0456, -85.3097],
    maxProbeCells: 900,
    neighborRadius: 2,
    h3: {
      cellFor: (lat, lon) => wasm.cell_for_coord(lat, lon),
      gridDisk,
      gridPath: shortGridPath,
      center: (cell) => wasm.cell_center(cell),
    },
    readRefs: async (cell) => {
      const raw = await layer.cellRecords(BigInt(`0x${cell}`));
      if (!raw) return [];
      return wasm.decode_roads(raw)
        .filter((road) => ["motorway", "motorway_link", "trunk", "trunk_link",
          "primary", "primary_link"].includes(road.road_class))
        .map((road) => road.ref_tag)
        .filter(Boolean);
    },
  });

  assert.equal(result.ref, "I 24");
  assert.ok(result.path.length > 20, `implausibly short path: ${result.path.length}`);
  assert.ok(result.path.length < 100, `implausibly long path: ${result.path.length}`);
  assert.ok(result.spine.length >= result.path.length);
  assert.ok(result.probedCells <= 900, `${result.probedCells} probes exceeded the budget`);
});
