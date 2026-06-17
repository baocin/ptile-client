# ptile-client

JavaScript client library for [PTILES](https://github.com/baocin/ptiles) — a compact binary geospatial format for OSM building footprints, roads, waterways, and more.

Queries 77M+ US building footprints from a browser or Node.js app. Loads per-state or national files from Cloudflare R2 or local disk.

## Features

- **Point queries** — find the nearest building at any GPS coordinate
- **Bounds queries** — get all buildings in a lat/lon rectangle
- **Two modes** — single file or multi-state directory (auto-routes by location)
- **Fast** — zstd decompression with dictionary, indexed by H3 resolution 7
- **No deps in Node 22+** — uses built-in `node:zlib` zstd support
- **Browser support** — native `DecompressionStream('zstd')` or `@bokuweb/zstd-wasm`

## Usage

### One-liner (define + query in two calls)

```js
import { definePtiles } from "ptile-client";
import * as h3 from "h3-js";

const { ptile, bounds, ready } = definePtiles({
  source: "https://pub-e46b7d7ee876916fd2db17000245b340.r2.dev/maps/", // per-state lazy load
  h3,
});
await ready;

// GPS in, building out — single function call
const building = await ptile(36.1628, -86.7816);
console.log(building?.name || building?.buildingType);
// => "The Place" or "commercial"

// All buildings within a rectangle
const nearby = await bounds(40.7, -74.0, 40.8, -73.9, 50);
console.log(`${nearby.length} buildings in lower Manhattan`);
```

`definePtiles` handles all three source types automatically:

- Ends with `/` → multi-state directory (51 state files, lazy-loaded)
- Starts with `http` → single URL (HTTP fetch)
- Anything else → local file path (Node only)

### Full PtilesReader API (for more control)

```js
import { PtilesReader } from "ptile-client";
import * as h3 from "h3-js";

// ── From a single state file ──────────────────────────────────────

const reader = await PtilesReader.fromUrl(
  "https://pub-e46b7d7ee876916fd2db17000245b340.r2.dev/maps/TN.buildings_v8.ptiles",
);
const building = await reader.query(36.1628, -86.7816, { h3 });
console.log(building?.name || building?.buildingType);
// => "The Place" or "commercial"

// ── From a URL prefix (lazy-loads per state) ──────────────────────

const national = await PtilesReader.fromStateDir(
  "https://pub-e46b7d7ee876916fd2db17000245b340.r2.dev/maps/",
  { h3 },
);

// Queries auto-route to the correct state file
const bldg = await national.query(34.0522, -118.2437);
console.log(bldg?.buildingType); // in Los Angeles, CA

// ── From a directory of local files ───────────────────────────────

const reader = await PtilesReader.fromStateDir("./data/states/", { h3 });

// ── Bounds query (multi-state) ────────────────────────────────────

const buildings = await reader.queryBounds(40.7, -74.0, 40.8, -73.9, 50, {
  h3,
});
console.log(`${buildings.length} buildings in lower Manhattan`);

// Each building:
// { osmId, buildingType, name, category,
//   coordinates: [[lon,lat], ...], centroidLat, centroidLon, ... }
```

## API

### `definePtiles(config)` — singleton setup

Configure once, then query with a bare function call. Handles all source types automatically.

| Return value                                     | Description                                               |
| ------------------------------------------------ | --------------------------------------------------------- |
| `ptile(lat, lon)`                                | Nearest building within 50m. Returns `null` or `Building` |
| `bounds(minLat, minLon, maxLat, maxLon, limit?)` | All buildings in rectangle                                |
| `header`                                         | File header (single-file mode only)                       |
| `ready`                                          | Promise that resolves when the reader is initialized      |

Config options:

- `source` — path or URL. Trailing `/` = multi-state dir, `http` = HTTP fetch, else local file
- `h3` — h3-js library instance (required)
- `mode` — force a mode: `'file'`, `'url'`, `'dir'`, `'state-dir'`

### `PtilesReader`

| Static method             | Description                                                         |
| ------------------------- | ------------------------------------------------------------------- |
| `fromUrl(url)`            | Load a single .ptiles file from HTTP URL                            |
| `fromFile(path)`          | Load a single .ptiles file from local disk (Node only)              |
| `fromNationalUrl(url)`    | Load a US-wide national file from URL                               |
| `fromNationalFile(path)`  | Load a US-wide national file from disk                              |
| `fromStateDir(dirPath)`   | Load all `{ABBR}.buildings_v8.ptiles` from a local directory (Node) |
| `fromStateDir(urlPrefix)` | Lazy-load per-state files from a URL prefix (browser)               |

| Instance method                                              | Description                                                             |
| ------------------------------------------------------------ | ----------------------------------------------------------------------- |
| `query(lat, lon, opts?)`                                     | Nearest building within 50m at (lat, lon). Returns `null` or `Building` |
| `queryBounds(minLat, minLon, maxLat, maxLon, limit?, opts?)` | All buildings in rectangle, up to `limit` (default 5000)                |
| `header`                                                     | File header metadata (single-file mode only)                            |

### Options

The `opts` parameter accepts `{ h3 }` to pass the h3-js library. If not provided, the library checks `globalThis.h3`.

### Building object

```js
{
  osmId: 828131426,          // OSM way/relation ID
  buildingType: 'commercial', // 'yes', 'house', 'residential', 'commercial', etc.
  name: 'The Place',         // building name, or null
  category: null,            // Overture category, or null
  coordinates: [[-86.78, 36.16], ...],  // polygon vertices [lon, lat]
  centroidLat: 36.16045,
  centroidLon: -86.78006,
  nameSource: null,          // where the name came from
  poiOsmId: null,            // associated POI OSM ID
}
```

## Hosted Tile Files (Cloudflare R2)

The default building tiles are hosted at:

```
https://pub-e46b7d7ee876916fd2db17000245b340.r2.dev/maps/{ABBR}.buildings_v8.ptiles
```

All 51 states + DC available. Files range from ~1 MB (DC) to ~160 MB (CA), totaling ~1.1 GB.

Other layers on the same host (same naming pattern):

| Layer           | Extension                    | Files |
| --------------- | ---------------------------- | ----- |
| Buildings       | `{ABBR}.buildings_v8.ptiles` | 51    |
| Roads           | `{ABBR}.roads.ptiles`        | 51    |
| Water           | `{ABBR}.water.ptiles`        | 51    |
| Business / POIs | `{ABBR}.business.ptiles`     | 51    |
| Places          | `{ABBR}.places.ptiles`       | 54    |
| Parks           | `{ABBR}.parks.ptiles`        | 54    |
| Rail            | `{ABBR}.rail.ptiles`         | 54    |
| Admin           | `US.admin.ptiles`            | 1     |

All layers share the same PTILES format and can be read with `PtilesReader`. Note: `query()` and `queryBounds()` only work on the buildings layer (polygon footprints). Other layers serve their own data structures.

## Data Sources

Building footprints are extracted from [OpenStreetMap](https://www.openstreetmap.org) via [Geofabrik](https://download.geofabrik.de/) state-level PBF extracts, encoded in [PTILES v8 format](https://github.com/baocin/ptiles) with per-cell string tables and zstd dictionary compression.

- **77M building footprints** across the continental US + AK, HI, DC
- **Field values**: OSM ID, building type, name, polygon geometry (cell-relative i16 + varint zigzag)
- **Spatial index**: H3 resolution 7 (cells ~5 km^2 median)
- **Compression**: zstd with pre-trained 512 KB dictionary, ~4 bytes per building feature

## Requirements

- **Node.js**: >= 22.0.0 (built-in zstd support)
- **h3-js**: optional peer dependency for spatial queries (`npm install h3-js`)
- **Browser**: Chrome 124+ / Firefox 136+ (native `DecompressionStream('zstd')`) or `@bokuweb/zstd-wasm`

## License

MIT
