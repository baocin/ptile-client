/* tslint:disable */
/* eslint-disable */

export class AdminReader {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Jurisdiction covering `(lat, lon)` as `{country, state, county, zip,
     * timezone, boundary_flags}`, or `null` if the grid has no entry.
     */
    admin_at(lat: number, lon: number): any;
    /**
     * `grid_bytes` = the raw (uncompressed) `aux` section; `string_tables_bytes`
     * = the *decompressed* `dict` section. Throws on malformed input.
     */
    constructor(grid_bytes: Uint8Array, string_tables_bytes: Uint8Array);
}

/**
 * Stateful motion classifier: per-fix vote (speed + road tiles + accel) fed
 * through the CHRE-style debouncer, so `movement` only changes when the
 * evidence actually persists.
 *
 * The road half is what disambiguates the awkward cases: stopped in a traffic
 * lane vs standing on the sidewalk. Pass the output of [`nearest_road`]
 * straight through as `road` — its `road_class`/`distance_m` are the two
 * fields read, extras are ignored.
 */
export class MovementTracker {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * `config` is optional (`null`/`undefined` = CHRE defaults): any subset of
     * `{majority_window, rapid_latency_ms, default_latency_ms,
     * vehicle_sticky_ms, min_continuous}`.
     */
    constructor(config: any);
    /**
     * Ingest one fix. `t_ms` is a monotonic timestamp; `speed_mps` and
     * `accuracy_m` are optional (pass `undefined` when the platform omits
     * them — speed is then derived from consecutive positions); `accel` is an
     * [`accel_stats`] result or `null`; `road` is a [`nearest_road`] result or
     * `null`; `intersection` is a [`nearest_intersection`] result or `null` —
     * at a signal/stop/give-way the "still driving" grace period stretches
     * from 150 s to 5 min, so a long light stops reading as an arrival.
     *
     * Returns `{movement, vote: {movement, confidence}, smoothed_speed_mps,
     * at_traffic_control}` where `movement` is the debounced state and `vote`
     * is this fix alone.
     */
    push(t_ms: number, lat: number, lon: number, speed_mps: number | null | undefined, accuracy_m: number | null | undefined, accel: any, road: any, intersection: any): any;
    /**
     * Current debounced movement type as a lowercase string.
     */
    readonly movement: string;
    /**
     * Smoothed position-derived speed (m/s), or `undefined` before enough fixes.
     */
    readonly smoothedSpeedMps: number | undefined;
}

/**
 * Accelerometer window summary from three same-length `Float32Array`s (raw
 * m/s^2 per axis, no gravity removal needed — magnitude is used). Returns
 * `{variance, mean_magnitude, dominant_frequency, step_count,
 * window_duration_s}`, the shape [`MovementTracker::push`] takes.
 */
export function accel_stats(x: Float32Array, y: Float32Array, z: Float32Array, sample_rate_hz: number): any;

/**
 * Decode the addresses for one H3 cell from an already-decompressed merged
 * block (address layer). JS fetches the block bytes (via the v2 index) and
 * decompresses them (`decompress_block`, empty dict), then calls this per
 * cell. Returns a JS array of `{osm_id, housenumber, street, lat, lon}`
 * (empty if the cell isn't in the block). `cell_hex` is a lowercase hex H3
 * cell string.
 *
 * `version` is the file header's version. v2 and later put an `i16` position
 * offset on every record; the block does not announce it, so passing the
 * wrong number here reads the coordinate bytes as a string length. Callers
 * already have `parse_header(...)`.
 */
export function address_cell(block_bytes: Uint8Array, cell_hex: string, version: number): any;

/**
 * `[lat, lon]` center of an H3 res-7 cell (hex string). Demo/browser
 * boundary for `ptiles_core::cell_center` -- replaces `h3-js`'s
 * `cellToLatLng`.
 */
export function cell_center(cell_hex: string): Float64Array;

/**
 * The filler-bit mask itself, for callers that must mask in their own code
 * (a `Map` keyed by cell id, say) rather than call across the boundary per id.
 */
export function cell_filler_mask(): bigint;

/**
 * H3 res-7 cell (lowercase hex string) containing `(lat, lon)`. Demo/browser
 * boundary for `ptiles_core::cell_for_coord` -- replaces `h3-js`'s
 * `latLngToCell` for every caller in this workspace.
 */
export function cell_for_coord(lat: number, lon: number): string;

/**
 * H3 res-7 cells covering a viewport bbox -- the wasm boundary for
 * `ptiles_core::cells_for_bounds` (see docs/INTEGRATION.md's "viewport ->
 * cells" step). Returns lowercase hex cell strings (a JS array of
 * `string`), matching how the demo/`h3-js` represents cells everywhere
 * (`h3.latLngToCell(...)` returns a lowercase hex string, and the demo's
 * `cellMap`/`renderPtilesForCells` consume cells in that same string form,
 * see steele.red/ptiles/index.html) -- not `u64`/BigInt, so callers can
 * pass results straight into existing `h3-js`-shaped code without a
 * conversion step.
 *
 * Errors (as a JS exception, matching every other export's rejection
 * pattern) if any coordinate is non-finite, `min` is not `<=` `max`, or the
 * box would cover more than `ptiles_core::MAX_BOUNDS_CELLS` cells.
 */
export function cells_for_bounds(min_lat: number, min_lon: number, max_lat: number, max_lon: number): any[];

/**
 * The run of index entries that may contain `cell_hex`, as a byte range to
 * Range-request.
 *
 * This is the point of the coarse index: `US.signals` carries a 4014 KiB
 * index, and locating one cell in it otherwise means fetching all of it,
 * because entries are only findable by position. With the samples, a lookup
 * is header+aux in one request and then this range -- 256 entries, under
 * 10 KiB.
 *
 * Returns `null` if the cell sorts below the first sample, i.e. the file does
 * not contain it.
 */
export function coarse_bracket(aux: Uint8Array, cell_hex: string, index_offset: bigint, entry_size: number): any;

export function decode_buildings(data: Uint8Array, cell_center_lat: number, cell_center_lon: number): any;

/**
 * Decode a buildings block for the cell it came from.
 *
 * Prefer this to [`decode_buildings`]: v8/v9 coordinates are deltas from the
 * cell centre, and passing the wrong centre produces a full set of well-formed
 * buildings in the wrong place with nothing to notice. Deriving the centre from
 * the cell id here removes the chance to get it wrong -- including the common
 * case of handing over a *masked* lookup key, which is not a valid H3 index and
 * used to answer null island.
 */
export function decode_buildings_for_cell(block_bytes: Uint8Array, cell_hex: string): any;

export function decode_business(data: Uint8Array): any;

/**
 * Decode a `camera` cell's records (PTILESC v1). Same merged-block caveat as
 * `decode_signals`.
 */
export function decode_cameras(data: Uint8Array): any;

export function decode_parks(data: Uint8Array): any;

export function decode_rail(data: Uint8Array): any;

export function decode_roads(data: Uint8Array): any;

/**
 * Decode a `signals` cell's records (PTILESS v1). Input is the byte range for
 * one cell, i.e. the output of `merged_cell_slice` -- signals files carry a
 * 38-byte index and therefore merged blocks, so passing a whole decompressed
 * block here decodes its cell table as records.
 */
export function decode_signals(data: Uint8Array): any;

export function decode_trails(data: Uint8Array): any;

export function decode_water(data: Uint8Array): any;

/**
 * internal fallback (see module doc above for why this isn't a direct call
 * into core). Pass an empty `dict` slice for dict-less layers (parks/address).
 */
export function decompress_block(compressed: Uint8Array, dict: Uint8Array): Uint8Array;

/**
 * Whether a trail type is built infrastructure (cycleway, footway) rather
 * than a natural way. Exposed so a renderer styles the two apart without
 * re-listing the layer's type vocabulary in JavaScript.
 * Great-circle distance in metres.
 *
 * The page had 31 sites doing this by hand in JavaScript -- each one its own
 * chance to use the wrong earth radius or drop the cos(lat) term. One
 * implementation, in core, is the point of this client.
 */
export function distance_m(lat1: number, lon1: number, lat2: number, lon2: number): number;

/**
 * The height this crate would assume for a building type with no published
 * height. Exposed so a UI can explain a guess rather than just draw it.
 */
export function estimated_height_for(building_type: string): number;

/**
 * Find the block offset/length covering `cell_hex` (lowercase hex H3 res-7
 * cell, the string form `cells_for_bounds`/`cell_for_coord` return). Returns
 * `null` if the cell has no block in this file (sparse coverage).
 *
 * Takes the raw index bytes and re-parses them. The search itself is
 * O(log n), but the parse is O(n) and happens on **every call** -- an earlier
 * doc comment here claimed the call was O(log n) with no network cost, which
 * was only ever true of the search half. Callers doing more than an occasional
 * lookup should use `parse_index_entries` once and search the result, or hold
 * a `PtilesFile`, which parses on open.
 *
 * Entry width is detected rather than assumed; see `parse_index_entries`.
 */
export function find_block_for_cell(index_bytes: Uint8Array, cell_hex: string): any;

/**
 * Forward geocode over already-decoded address records: "400 Broadway".
 */
export function geocode_addresses(query: string, addresses_js: any, limit?: number | null): any;

/**
 * Every index entry with `block_offset` already resolved to an absolute file
 * offset -- the byte range to Range-request, with no further arithmetic.
 *
 * This is the export a client should reach for. Between choosing the entry
 * width, choosing the offset base and applying it, there are three chances to
 * be wrong, and each one fails the same silent way: a plausible-looking offset
 * that reads the wrong bytes, or a zero-length block that renders as "no data
 * here" rather than as an error. All three happen in `ptiles-core` here.
 *
 * Entries whose offset arithmetic would wrap (only reachable with a corrupt
 * index) are dropped rather than returned with a bogus value.
 */
export function index_entries_absolute(header_bytes: Uint8Array, index_bytes: Uint8Array): any;

/**
 * Whether an `intersection_type` is a node traffic waits at (signals, stop,
 * give-way) rather than flows through. This is the distinction
 * `MovementTracker` uses to stretch its "still driving" window.
 */
export function intersection_holds_traffic(intersection_type: number): boolean;

/**
 * Name for an `intersection_type` byte, from the format's own vocabulary:
 * `traffic_signals` | `stop` | `give_way` | `roundabout` | `junction`.
 *
 * `nearest_intersection` returns the raw integer because that is what the block
 * stores; this is how JS names it without keeping a second copy of the mapping
 * that can drift from the Rust one.
 */
export function intersection_type_name(intersection_type: number): string;

/**
 * Business name search, JS-owns-fetch flavor.
 *
 * The `{STATE}.business_name_index.ptiles` sidecar (see
 * `ptiles_core::business_search`'s module doc for its first-letter-bucket
 * format) is a normal `.ptiles`-shaped file: header, dict, index, blocks.
 * wasm does no I/O, so it doesn't open this file itself -- the intended JS
 * flow mirrors what the demo already does for spatial layers
 * (docs/INTEGRATION.md's "single whole-file fetch, parse header/index once,
 * cache" notes) plus these two pure calls:
 *
 *   1. JS fetches (or already has, whole-file) the name-index file's bytes
 *      and parses its header/index once, same as any other `.ptiles` file
 *      -- the sidecar's index entries key on a 0-27 bucket value stored in
 *      the normal `h3_cell` index field, not a real H3 cell.
 *   2. `key_for_business_name_query(query)` -- call this wasm export to get
 *      that 0-27 key without reimplementing the bucketing rule in JS.
 *   3. JS looks up the index entry for that key, slices out its compressed
 *      block, and decompresses it with `decompress_block` (dict-less, per
 *      the builder -- pass an empty `dict`).
 *   4. `match_business_name_block(block_bytes, query, limit)` -- call this
 *      to decode the block's records and get back ranked `BusinessHit`s
 *      (`{name, category_idx, lat, lon, cell: null, score}`), same scoring
 *      (`2`=exact, `1`=prefix, `0`=substring) as the native
 *      `search_business_indexed`/`search_business_brute_force` paths.
 *
 * No block ever needs re-fetching for a different query against the same
 * state: once JS has cached the file's index it only pays for step 3's one
 * block per distinct first-letter key.
 */
export function key_for_business_name_query(query: string): number;

/**
 * Reverse geocode a point against already-decoded features.
 *
 * `roads_js` / `trails_js` / `addresses_js` are the arrays this module's
 * `decode_roads`, `decode_trails` and `address_cell` return, for whatever
 * cells the caller fetched. Returns
 * `{nearest_way, on_way, address}` — see `ptiles_core::locate`.
 */
export function locate_point(lat: number, lon: number, roads_js: any, trails_js: any, addresses_js: any): any;

/**
 * See [`key_for_business_name_query`]'s doc comment for the full JS-side
 * flow this is step 4 of. Pure decode-and-match over an already-fetched,
 * already-decompressed name-index block -- no I/O, no H3 lookup.
 */
export function match_business_name_block(block_bytes: Uint8Array, query: string, limit: number): any;

/**
 * The record bytes for one cell inside a decompressed merged block.
 *
 * Layers with a 38-byte index pack several cells per block behind a cell
 * table; a record decoder handed the whole block parses that table as
 * records and yields plausible garbage rather than an error. Returns `null`
 * if the block does not contain the cell.
 */
export function merged_cell_slice(block: Uint8Array, cell_hex: string): Uint8Array | undefined;

/**
 * The nearest address to a point, or null. Separate from `locate_point` for
 * callers that hold only the address layer.
 */
export function nearest_address_to(lat: number, lon: number, addresses_js: any, threshold_m?: number | null): any;

/**
 * Decode a roads block and return the nearest labeled intersection to
 * `(lat, lon)` — the "am I at an intersection?" query. `threshold_m` is
 * optional (omit/`undefined` from JS for the SPEC.md default of 50 m).
 * Returns `null` when nothing is within the threshold. Reports a mapped
 * intersection point + its control type, not junction degree (the format
 * stores no topology). JS supplies `block_bytes` already decompressed
 * (no-I/O contract, same as `nearest_road`).
 */
export function nearest_intersection(block_bytes: Uint8Array, lat: number, lon: number, threshold_m?: number | null): any;

/**
 * Decode a roads block and return the single nearest road segment to
 * `(lat, lon)`, per plan addendum item 1: `{osm_id, name, road_class,
 * snapped, distance_m, geometry}`. `threshold_m` is optional; omit (pass
 * `None`/`undefined` from JS) to use the SPEC.md default of 50 m.
 *
 * JS supplies `block_bytes` already decompressed (no-fetch contract is
 * unchanged — this does not do any I/O or H3 lookup itself). Returns
 * `null` if no road is within the threshold.
 */
export function nearest_road(block_bytes: Uint8Array, lat: number, lon: number, threshold_m?: number | null): any;

/**
 * Ring-1 (6 cells) H3 neighbors of `cell_hex`, as lowercase hex strings.
 * Demo/browser boundary for `ptiles_core::neighbor_cells` -- replaces
 * `h3-js`'s `gridRing(cell, 1)` (used by the deployed demo's
 * `BusinessReader.query` for nearby-business radius search).
 */
export function neighbor_cells(cell_hex: string): any[];

/**
 * Drop an H3 id's unused low digits, so two ids naming the same res-7 cell
 * compare equal. The mask is a property of the id layout, not of any caller.
 */
export function normalize_cell(cell: bigint): bigint;

/**
 * Parse the PTCI sampled index from a file's `aux` region.
 *
 * Returns `null` when `aux` is not a coarse index -- empty, too short, or
 * holding something else. That is the normal case for every layer built
 * before PTCI existed, and a caller should fall back to reading the full
 * index. It *throws* when the region announces itself as PTCI and then does
 * not hold up (unknown version, impossible sample count), because that means
 * whatever wrote the file has a bug and is worth surfacing rather than
 * silently degrading.
 *
 * The JS original (`parseCoarseIndex` in demo/index.html) returned null for
 * both, and ignored the version byte entirely.
 */
export function parse_coarse_index(aux: Uint8Array): any;

/**
 * Decode a bare run of index entries -- no count prefix, just entries -- at a
 * known width, with `block_offset` left exactly as stored.
 *
 * This is the shape a PTCI partial read returns: `coarse_bracket` names a byte
 * range that lands mid-index, so there is no count in front of it. Files
 * carrying a coarse index are written by the current builder, which verifies
 * its own offsets, so the stored values are already absolute and need no base
 * applied -- but that is the caller's knowledge, not something derivable from
 * a run, which is why this returns them unmodified.
 *
 * Trailing bytes that do not complete an entry are ignored.
 */
export function parse_entry_run(entries: Uint8Array, entry_size: number): any;

/**
 * Parse a `.ptiles` file's 256-byte header (demo/browser boundary for
 * `ptiles_core::Header::parse`). Lets JS learn `dict_offset`/`dict_length`/
 * `index_offset`/`index_length`/`blocks_offset` from just the first 256
 * bytes of a Range request, without JS re-implementing the fixed-offset
 * layout from SPEC.md itself (`ptiles_core::header` is the single source of
 * truth for that layout parity-checked against `ptiles/codec.py`).
 */
export function parse_header(data: Uint8Array): any;

/**
 * Parse a `.ptiles` file's spatial index section (the `index_offset`..
 * `index_offset+index_length` byte range from the header) into its full
 * entry list. Demo/browser boundary for `ptiles_core::parse_index_detected`
 * so JS never has to hand-roll either entry layout.
 *
 * Entry width is detected, not assumed. This used to call `parse_index`,
 * which forces the 19-byte v1 layout: on the 38-byte layers (parks, rail,
 * places, signals, camera) that reads `block_offset` and `block_length` out
 * of the zeroed bbox field, so every cell came back with a zero-length block
 * and the caller saw "no data here" rather than an error.
 */
export function parse_index_entries(data: Uint8Array): any;

/**
 * What the reader concludes about a file's index layout, from its header and
 * index bytes: entry width, why that width was chosen, offset base, and the
 * stride the header declared.
 *
 * This existed nowhere on the JS side of the boundary. `parse_index_entries`
 * takes index bytes alone, so it cannot see `blocks_offset` and cannot tell
 * whether the offsets it returns are absolute, relative to the block region,
 * or absolute-but-overshooting. Callers had to decide that themselves, and
 * `demo/index.html`'s `pickOffsetBase` is what "themselves" meant -- a second
 * implementation of the rule, in the language that got the index stride wrong.
 *
 * Prefer [`index_entries_absolute`] when all you want is offsets you can
 * fetch. Use this when you need to *report* the layout, e.g. to warn that a
 * file's header contradicts its own index.
 */
export function parse_index_layout(header_bytes: Uint8Array, index_bytes: Uint8Array): any;

/**
 * The height to draw a building at: the published one, or this crate's guess
 * when none was published.
 *
 * Returns a bare `f64`, not a struct. serde-wasm-bindgen hands objects to the
 * browser as a `Map` in some engines, where `r.height_m` reads `undefined` --
 * which would silently extrude every guessed building to `NaN`. The caller
 * already knows whether it passed a height, so the flag adds nothing here;
 * `height_or_estimate` in core still returns it for Rust callers.
 */
export function resolved_height(height_m: number | null | undefined, building_type: string): number;

/**
 * Decode a roads block into its full segment list (geometry + name +
 * every other `RoadSegment` field), identical shape to `decode_roads`.
 * Exists as its own export (plan addendum item 1's "roads" query) so
 * callers reach for a query-shaped name rather than the raw decoder;
 * ring-1 neighbor-cell expansion is NOT done here -- JS owns block
 * fetching (which cell(s) to fetch bytes for), so ring handling stays in
 * JS per the plan's `query.rs` split (`neighbor_cells` in core, calling
 * convention in JS/CLI).
 */
export function roads_in_block(block_bytes: Uint8Array): any;

/**
 * Route on pre-decoded road segments (JS owns fetch + zstd + corridor).
 * `segments_js`: `[{coords:[[lon,lat],...], road_class, oneway?, speed_limit_kmh?}, ...]`
 * `zone_middle`: bool[] same length (true = arterial-only middle); empty/null = all end-cap.
 * Returns `{distance_m, duration_s, path:[[lat,lon],...]}` or null.
 */
export function route_from_segments(segments_js: any, zone_middle: any, lat1: number, lon1: number, lat2: number, lon2: number, snap_m?: number | null, avoid_highways?: boolean | null, avoid_intersections?: boolean | null): any;

/**
 * Rank road/building/business candidates for a GPS fix (plan addendum
 * item 2: emission-probability scoring lives in core, this is just the
 * wasm boundary). `buildings_block`/`business_block` are optional --
 * pass an empty slice (`new Uint8Array()`) from JS when a layer isn't
 * available for the current cell; buildings need `cell_center_lat`/
 * `cell_center_lon` to decode (v8 buildings are cell-relative-delta
 * encoded, see `buildings.rs`), which is also why callers must supply
 * them even when `buildings_block` is empty.
 */
export function score_candidates(fix_json: string, roads_block: Uint8Array, buildings_block: Uint8Array, business_block: Uint8Array, cell_center_lat: number, cell_center_lon: number): any;

/**
 * Statistically significant changes in a speed series.
 *
 * `t_ms` and `speed_mps` are parallel arrays in time order (a `Float64Array`
 * each). `config` is optional (`null` = defaults): any subset of
 * `{window, alpha, min_separation, min_delta_mps}`.
 *
 * Returns `[{index, t_ms, t_stat, p_value, alpha_corrected, before_mps,
 * after_mps}, ...]` in index order. This is a different question from the
 * classifier's transitions -- Welch's t-test on adjacent windows, no thresholds
 * and no movement vocabulary involved -- so the two disagreeing is information
 * rather than a bug. See `motion/src/shifts.rs`.
 */
export function significant_shifts(t_ms: Float64Array, speed_mps: Float64Array, config: any): any;

export function trail_is_developed(trail_type: string): boolean;

/**
 * Decompress a compressed `.ptiles` block, trying the layer's zstd
 * dictionary first and falling back to plain (dict-less) decompress on
 * failure. Mirrors `ptiles/compression.py`'s `decompress_block` /
 * `decompress_fallback` pair and `ptiles-core::file::PtilesFile::read_block`'s
 * Which buildings are in line of sight from a point on the ground.
 *
 * `buildings`: `[{coords:[[lon,lat],...], height_m: number|null,
 * building_type: string}, ...]` -- the shape `decode_buildings` already
 * returns, so a caller can pass its own decoded records straight back in.
 * Filter to a sensible radius first: this is geometry over a few hundred
 * footprints, not over a whole cell's 18k.
 *
 * Returns one entry per input, in the same order:
 * `{visible, height_m, estimated, distance_m}`. `estimated` marks a height
 * that came from the building type rather than the file, which matters
 * because most published buildings carry no height at all.
 */
export function viewshed(buildings: any, lat: number, lon: number, eye_m: number, radius_m: number): any;

/**
 * The reverse of [`viewshed`]: which of these buildings can see *any* of these
 * points. Line of sight is reciprocal, so running the ordinary viewshed from
 * each target point and taking the union answers "find me somewhere with a
 * view of the river" without any new geometry.
 *
 * `origins` is `[[lat, lon], ...]` -- one point for a shop, a sampled run
 * along the bank for a river. `buildings` is deserialized once and reused for
 * every origin, which is the whole reason this is not a JS loop over
 * [`viewshed`]: a few hundred footprints crossing the wasm boundary two dozen
 * times costs more than the geometry does.
 *
 * Returns one entry per building, in input order.
 */
export function viewshed_multi(buildings: any, origins: any, eye_m: number, radius_m: number): any;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly __wbg_adminreader_free: (a: number, b: number) => void;
    readonly __wbg_movementtracker_free: (a: number, b: number) => void;
    readonly accel_stats: (a: number, b: number, c: number, d: number, e: number, f: number, g: number) => [number, number, number];
    readonly address_cell: (a: number, b: number, c: number, d: number, e: number) => [number, number, number];
    readonly adminreader_admin_at: (a: number, b: number, c: number) => [number, number, number];
    readonly adminreader_new: (a: number, b: number, c: number, d: number) => [number, number, number];
    readonly cell_center: (a: number, b: number) => [number, number, number, number];
    readonly cell_filler_mask: () => bigint;
    readonly cell_for_coord: (a: number, b: number) => [number, number];
    readonly cells_for_bounds: (a: number, b: number, c: number, d: number) => [number, number, number, number];
    readonly coarse_bracket: (a: number, b: number, c: number, d: number, e: bigint, f: number) => [number, number, number];
    readonly decode_buildings: (a: number, b: number, c: number, d: number) => [number, number, number];
    readonly decode_buildings_for_cell: (a: number, b: number, c: number, d: number) => [number, number, number];
    readonly decode_business: (a: number, b: number) => [number, number, number];
    readonly decode_cameras: (a: number, b: number) => [number, number, number];
    readonly decode_parks: (a: number, b: number) => [number, number, number];
    readonly decode_rail: (a: number, b: number) => [number, number, number];
    readonly decode_roads: (a: number, b: number) => [number, number, number];
    readonly decode_signals: (a: number, b: number) => [number, number, number];
    readonly decode_trails: (a: number, b: number) => [number, number, number];
    readonly decode_water: (a: number, b: number) => [number, number, number];
    readonly decompress_block: (a: number, b: number, c: number, d: number) => [number, number, number, number];
    readonly estimated_height_for: (a: number, b: number) => number;
    readonly find_block_for_cell: (a: number, b: number, c: number, d: number) => [number, number, number];
    readonly geocode_addresses: (a: number, b: number, c: any, d: number) => [number, number, number];
    readonly index_entries_absolute: (a: number, b: number, c: number, d: number) => [number, number, number];
    readonly intersection_holds_traffic: (a: number) => number;
    readonly intersection_type_name: (a: number) => [number, number];
    readonly key_for_business_name_query: (a: number, b: number) => number;
    readonly locate_point: (a: number, b: number, c: any, d: any, e: any) => [number, number, number];
    readonly match_business_name_block: (a: number, b: number, c: number, d: number, e: number) => [number, number, number];
    readonly merged_cell_slice: (a: number, b: number, c: number, d: number) => [number, number, number, number];
    readonly movementtracker_movement: (a: number) => [number, number];
    readonly movementtracker_new: (a: any) => [number, number, number];
    readonly movementtracker_push: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: any, j: any, k: any) => [number, number, number];
    readonly movementtracker_smoothedSpeedMps: (a: number) => [number, number];
    readonly nearest_address_to: (a: number, b: number, c: any, d: number, e: number) => [number, number, number];
    readonly nearest_intersection: (a: number, b: number, c: number, d: number, e: number, f: number) => [number, number, number];
    readonly nearest_road: (a: number, b: number, c: number, d: number, e: number, f: number) => [number, number, number];
    readonly neighbor_cells: (a: number, b: number) => [number, number, number, number];
    readonly normalize_cell: (a: bigint) => bigint;
    readonly parse_coarse_index: (a: number, b: number) => [number, number, number];
    readonly parse_entry_run: (a: number, b: number, c: number) => [number, number, number];
    readonly parse_header: (a: number, b: number) => [number, number, number];
    readonly parse_index_entries: (a: number, b: number) => [number, number, number];
    readonly parse_index_layout: (a: number, b: number, c: number, d: number) => [number, number, number];
    readonly resolved_height: (a: number, b: number, c: number, d: number) => number;
    readonly route_from_segments: (a: any, b: any, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number) => [number, number, number];
    readonly score_candidates: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number) => [number, number, number];
    readonly significant_shifts: (a: number, b: number, c: number, d: number, e: any) => [number, number, number];
    readonly trail_is_developed: (a: number, b: number) => number;
    readonly viewshed: (a: any, b: number, c: number, d: number, e: number) => [number, number, number];
    readonly viewshed_multi: (a: any, b: any, c: number, d: number) => [number, number, number];
    readonly distance_m: (a: number, b: number, c: number, d: number) => number;
    readonly roads_in_block: (a: number, b: number) => [number, number, number];
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
    readonly __wbindgen_exn_store: (a: number) => void;
    readonly __externref_table_alloc: () => number;
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __externref_table_dealloc: (a: number) => void;
    readonly __wbindgen_free: (a: number, b: number, c: number) => void;
    readonly __externref_drop_slice: (a: number, b: number) => void;
    readonly __wbindgen_start: () => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;

/**
 * Instantiates the given `module`, which can either be bytes or
 * a precompiled `WebAssembly.Module`.
 *
 * @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
 *
 * @returns {InitOutput}
 */
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
 * If `module_or_path` is {RequestInfo} or {URL}, makes a request and
 * for everything else, calls `WebAssembly.instantiate` directly.
 *
 * @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
 *
 * @returns {Promise<InitOutput>}
 */
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
