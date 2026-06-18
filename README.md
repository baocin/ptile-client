1|# ptile-client
2|
JavaScript client library for [PTILES](https://github.com/baocin/ptiles) — a compact binary geospatial format for OSM building footprints, roads, waterways, and more.

PTILES is designed for **feature lookup** (what building is at this GPS coordinate) rather than map rendering. Each file is self-contained, offline-readable, and indexed by H3 cell for O(1) spatial lookup. A typical point query decompresses ~1-5KB — vs fetching a whole PMTiles tile (~100KB+) for the same result.

Queries 77M+ US building footprints from a browser or Node.js app. Loads per-state or national files from Cloudflare R2 or local disk.
6|
7|## Features
8|
9|- **Point queries** — find the nearest building at any GPS coordinate
10|- **Bounds queries** — get all buildings in a lat/lon rectangle
11|- **Two modes** — single file or multi-state directory (auto-routes by location)
12|- **Fast** — zstd decompression with dictionary, indexed by H3 resolution 7
13|- **No deps in Node 22+** — uses built-in `node:zlib` zstd support
14|- **Browser support** — native `DecompressionStream('zstd')` or `@bokuweb/zstd-wasm`
15|
16|## Usage
17|
18|### One-liner (define + query in two calls)
19|
20|``js
21|import { definePtiles } from "ptile-client";
22|import * as h3 from "h3-js";
23|
24|const { ptile, bounds, ready } = definePtiles({
25|  source: "https://maps.mydatatimeline.com/maps/", // per-state lazy load
26|  h3,
27|});
28|await ready;
29|
30|// GPS in, building out — single function call
31|const building = await ptile(36.1628, -86.7816);
32|console.log(building?.name || building?.buildingType);
33|// => "The Place" or "commercial"
34|
35|// All buildings within a rectangle
36|const nearby = await bounds(40.7, -74.0, 40.8, -73.9, 50);
37|console.log(`${nearby.length} buildings in lower Manhattan`);
38|``
39|
40|`definePtiles` handles all three source types automatically:
41|
42|- Ends with `/` → multi-state directory (51 state files, lazy-loaded)
43|- Starts with `http` → single URL (HTTP fetch)
44|- Anything else → local file path (Node only)
45|
46|### Full PtilesReader API (for more control)
47|
48|``js
49|import { PtilesReader } from "ptile-client";
50|import * as h3 from "h3-js";
51|
52|// ── From a single state file ──────────────────────────────────────
53|
54|const reader = await PtilesReader.fromUrl(
55|  "https://maps.mydatatimeline.com/maps/TN.buildings_v8.ptiles",
56|);
57|const building = await reader.query(36.1628, -86.7816, { h3 });
58|console.log(building?.name || building?.buildingType);
59|// => "The Place" or "commercial"
60|
61|// ── From a URL prefix (lazy-loads per state) ──────────────────────
62|
63|const national = await PtilesReader.fromStateDir(
64|  "https://maps.mydatatimeline.com/maps/",
65|  { h3 },
66|);
67|
68|// Queries auto-route to the correct state file
69|const bldg = await national.query(34.0522, -118.2437);
70|console.log(bldg?.buildingType); // in Los Angeles, CA
71|
72|// ── From a directory of local files ───────────────────────────────
73|
74|const reader = await PtilesReader.fromStateDir("./data/states/", { h3 });
75|
76|// ── Bounds query (multi-state) ────────────────────────────────────
77|
78|const buildings = await reader.queryBounds(40.7, -74.0, 40.8, -73.9, 50, {
79|  h3,
80|});
81|console.log(`${buildings.length} buildings in lower Manhattan`);
82|
83|// Each building:
84|// { osmId, buildingType, name, category,
85|//   coordinates: [[lon,lat], ...], centroidLat, centroidLon, ... }
86|``
87|
88|## API
89|
90|### `definePtiles(config)` — singleton setup
91|
92|Configure once, then query with a bare function call. Handles all source types automatically.
93|
94|| Return value | Description |
95|| ------------------------------------------------ | --------------------------------------------------------- |
96|| `ptile(lat, lon)` | Nearest building within 50m. Returns `null` or `Building` |
97|| `bounds(minLat, minLon, maxLat, maxLon, limit?)` | All buildings in rectangle |
98|| `header` | File header (single-file mode only) |
99|| `ready` | Promise that resolves when the reader is initialized |
100|
101|Config options:
102|
103|- `source` — path or URL. Trailing `/` = multi-state dir, `http` = HTTP fetch, else local file
104|- `h3` — h3-js library instance (required)
105|- `mode` — force a mode: `'file'`, `'url'`, `'dir'`, `'state-dir'`
106|
107|### `PtilesReader`
108|
109|| Static method | Description |
110|| ------------------------- | ------------------------------------------------------------------- |
111|| `fromUrl(url)` | Load a single .ptiles file from HTTP URL |
112|| `fromFile(path)` | Load a single .ptiles file from local disk (Node only) |
113|| `fromNationalUrl(url)` | Load a US-wide national file from URL |
114|| `fromNationalFile(path)` | Load a US-wide national file from disk |
115|| `fromStateDir(dirPath)` | Load all `{ABBR}.buildings_v8.ptiles` from a local directory (Node) |
116|| `fromStateDir(urlPrefix)` | Lazy-load per-state files from a URL prefix (browser) |
117|
118|| Instance method | Description |
119|| ------------------------------------------------------------ | ----------------------------------------------------------------------- |
120|| `query(lat, lon, opts?)` | Nearest building within 50m at (lat, lon). Returns `null` or `Building` |
121|| `queryBounds(minLat, minLon, maxLat, maxLon, limit?, opts?)` | All buildings in rectangle, up to `limit` (default 5000) |
122|| `header` | File header metadata (single-file mode only) |
123|
124|### Options
125|
126|The `opts` parameter accepts `{ h3 }` to pass the h3-js library. If not provided, the library checks `globalThis.h3`.
127|
128|### Building object
129|
130|`js
131|{
132|  osmId: 828131426,          // OSM way/relation ID
133|  buildingType: 'commercial', // 'yes', 'house', 'residential', 'commercial', etc.
134|  name: 'The Place',         // building name, or null
135|  category: null,            // Overture category, or null
136|  coordinates: [[-86.78, 36.16], ...],  // polygon vertices [lon, lat]
137|  centroidLat: 36.16045,
138|  centroidLon: -86.78006,
139|  nameSource: null,          // where the name came from
140|  poiOsmId: null,            // associated POI OSM ID
141|}
142|`
143|
144|## Hosted Tile Files (Cloudflare R2)
145|
146|The default building tiles are hosted at:
147|
148|`
149|https://maps.mydatatimeline.com/maps/{ABBR}.buildings_v8.ptiles
150|`
151|
152|All 51 states + DC available. Files range from ~1 MB (DC) to ~160 MB (CA), totaling ~1.1 GB.
153|
154|Other layers on the same host (same naming pattern):
155|
156|| Layer | Extension | Files |
157|| --------------- | ---------------------------- | ----- |
158|| Buildings | `{ABBR}.buildings_v8.ptiles` | 51 |
159|| Roads | `{ABBR}.roads.ptiles` | 51 |
160|| Water | `{ABBR}.water.ptiles` | 51 |
161|| Business / POIs | `{ABBR}.business.ptiles` | 51 |
162|| Places | `{ABBR}.places.ptiles` | 54 |
163|| Parks | `{ABBR}.parks.ptiles` | 54 |
164|| Rail | `{ABBR}.rail.ptiles` | 54 |
165|| Admin | `US.admin.ptiles` | 1 |
166|
167|All layers share the same PTILES format and can be read with `PtilesReader`. Note: `query()` and `queryBounds()` only work on the buildings layer (polygon footprints). Other layers serve their own data structures.
168|
169|## Data Sources
170|
171|Building footprints are extracted from [OpenStreetMap](https://www.openstreetmap.org) via [Geofabrik](https://download.geofabrik.de/) state-level PBF extracts, encoded in [PTILES v8 format](https://github.com/baocin/ptiles) with per-cell string tables and zstd dictionary compression.
172|
173|- **77M building footprints** across the continental US + AK, HI, DC
174|- **Field values**: OSM ID, building type, name, polygon geometry (cell-relative i16 + varint zigzag)
175|- **Spatial index**: H3 resolution 7 (cells ~5 km^2 median)
176|- **Compression**: zstd with pre-trained 512 KB dictionary, ~4 bytes per building feature
177|
178|## Requirements
179|
180|- **Node.js**: >= 22.0.0 (built-in zstd support)
181|- **h3-js**: optional peer dependency for spatial queries (`npm install h3-js`)
182|- **Browser**: Chrome 124+ / Firefox 136+ (native `DecompressionStream('zstd')`) or `@bokuweb/zstd-wasm`
183|
184|## License
185|
186|MIT
187|
