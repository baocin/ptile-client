/* @ts-self-types="./ptiles_client.d.ts" */

/**
 * Portable adaptive motion and sensor-sampling session.
 *
 * This class never touches browser hardware. `observe()` and `tick()` return
 * a `sampling` record plus `sampling_changed`; the JavaScript adapter maps
 * that advice to Geolocation, DeviceMotion, a desktop bridge, or any other
 * host service and can emit its preferred callback/event/stream locally.
 */
export class AdaptiveMotionSession {
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        AdaptiveMotionSessionFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_adaptivemotionsession_free(ptr, 0);
    }
    /**
     * @returns {any}
     */
    get currentAdvice() {
        const ret = wasm.adaptivemotionsession_currentAdvice(this.__wbg_ptr);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return takeFromExternrefTable0(ret[0]);
    }
    /**
     * @returns {any}
     */
    get lastAppliedSampling() {
        const ret = wasm.adaptivemotionsession_lastAppliedSampling(this.__wbg_ptr);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return takeFromExternrefTable0(ret[0]);
    }
    /**
     * @returns {string}
     */
    get movement() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.adaptivemotionsession_movement(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Both arguments are optional. `config` has the nested
     * `{motion, debounce, sampling}` shape returned by the Rust defaults;
     * `capabilities` describes what this host can actually collect.
     * @param {any} config
     * @param {any} capabilities
     */
    constructor(config, capabilities) {
        const ret = wasm.adaptivemotionsession_new(config, capabilities);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        this.__wbg_ptr = ret[0];
        AdaptiveMotionSessionFinalization.register(this, this.__wbg_ptr, this);
        return this;
    }
    /**
     * Ingest `{t_ms, location?, accelerometer?, road?, traffic_control?}`.
     * The timestamp is monotonic milliseconds supplied by the caller.
     * @param {any} observation
     * @returns {any}
     */
    observe(observation) {
        const ret = wasm.adaptivemotionsession_observe(this.__wbg_ptr, observation);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return takeFromExternrefTable0(ret[0]);
    }
    /**
     * Tell the policy what the host actually configured. This is feedback,
     * not permission for the library to control hardware.
     * @param {any} applied
     */
    reportAppliedSampling(applied) {
        const ret = wasm.adaptivemotionsession_reportAppliedSampling(this.__wbg_ptr, applied);
        if (ret[1]) {
            throw takeFromExternrefTable0(ret[0]);
        }
    }
    reset() {
        wasm.adaptivemotionsession_reset(this.__wbg_ptr);
    }
    /**
     * Replace host capabilities. Returns true when hardware may need to be
     * reconfigured immediately.
     * @param {any} capabilities
     * @param {number} now_ms
     * @returns {boolean}
     */
    setCapabilities(capabilities, now_ms) {
        const ret = wasm.adaptivemotionsession_setCapabilities(this.__wbg_ptr, capabilities, now_ms);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return ret[0] !== 0;
    }
    /**
     * `background`, `tracking`, or `navigation`.
     * @param {string} intent
     * @param {number} now_ms
     * @returns {boolean}
     */
    setIntent(intent, now_ms) {
        const ptr0 = passStringToWasm0(intent, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.adaptivemotionsession_setIntent(this.__wbg_ptr, ptr0, len0, now_ms);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return ret[0] !== 0;
    }
    /**
     * Reevaluate an advice deadline without inventing a sensor sample.
     * @param {number} now_ms
     * @returns {any}
     */
    tick(now_ms) {
        const ret = wasm.adaptivemotionsession_tick(this.__wbg_ptr, now_ms);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return takeFromExternrefTable0(ret[0]);
    }
}
if (Symbol.dispose) AdaptiveMotionSession.prototype[Symbol.dispose] = AdaptiveMotionSession.prototype.free;

export class AdminReader {
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        AdminReaderFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_adminreader_free(ptr, 0);
    }
    /**
     * Jurisdiction covering `(lat, lon)` as `{country, state, county, zip,
     * timezone, boundary_flags}`, or `null` if the grid has no entry.
     * @param {number} lat
     * @param {number} lon
     * @returns {any}
     */
    admin_at(lat, lon) {
        const ret = wasm.adminreader_admin_at(this.__wbg_ptr, lat, lon);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return takeFromExternrefTable0(ret[0]);
    }
    /**
     * `grid_bytes` = the raw (uncompressed) `aux` section; `string_tables_bytes`
     * = the *decompressed* `dict` section. Throws on malformed input.
     * @param {Uint8Array} grid_bytes
     * @param {Uint8Array} string_tables_bytes
     */
    constructor(grid_bytes, string_tables_bytes) {
        const ptr0 = passArray8ToWasm0(grid_bytes, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passArray8ToWasm0(string_tables_bytes, wasm.__wbindgen_malloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.adminreader_new(ptr0, len0, ptr1, len1);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        this.__wbg_ptr = ret[0];
        AdminReaderFinalization.register(this, this.__wbg_ptr, this);
        return this;
    }
}
if (Symbol.dispose) AdminReader.prototype[Symbol.dispose] = AdminReader.prototype.free;

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
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        MovementTrackerFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_movementtracker_free(ptr, 0);
    }
    /**
     * Current debounced movement type as a lowercase string.
     * @returns {string}
     */
    get movement() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.movementtracker_movement(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * `config` is optional (`null`/`undefined` = CHRE defaults): any subset of
     * `{majority_window, rapid_latency_ms, default_latency_ms,
     * vehicle_sticky_ms, min_continuous}`.
     * @param {any} config
     */
    constructor(config) {
        const ret = wasm.movementtracker_new(config);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        this.__wbg_ptr = ret[0];
        MovementTrackerFinalization.register(this, this.__wbg_ptr, this);
        return this;
    }
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
     * @param {number} t_ms
     * @param {number} lat
     * @param {number} lon
     * @param {number | null | undefined} speed_mps
     * @param {number | null | undefined} accuracy_m
     * @param {any} accel
     * @param {any} road
     * @param {any} intersection
     * @returns {any}
     */
    push(t_ms, lat, lon, speed_mps, accuracy_m, accel, road, intersection) {
        const ret = wasm.movementtracker_push(this.__wbg_ptr, t_ms, lat, lon, !isLikeNone(speed_mps), isLikeNone(speed_mps) ? 0 : speed_mps, !isLikeNone(accuracy_m), isLikeNone(accuracy_m) ? 0 : accuracy_m, accel, road, intersection);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return takeFromExternrefTable0(ret[0]);
    }
    /**
     * Smoothed position-derived speed (m/s), or `undefined` before enough fixes.
     * @returns {number | undefined}
     */
    get smoothedSpeedMps() {
        const ret = wasm.movementtracker_smoothedSpeedMps(this.__wbg_ptr);
        return ret[0] === 0 ? undefined : ret[1];
    }
}
if (Symbol.dispose) MovementTracker.prototype[Symbol.dispose] = MovementTracker.prototype.free;

/**
 * A route being followed. Holds the path, its cumulative distances and its
 * turn queue on the Rust side so a position update costs one small call
 * rather than re-serialising the whole route at every GPS fix -- which, at
 * 1 Hz on a 600-point route, is the difference between free and not.
 *
 * ```js
 * const nav = Navigator.new(route.path.map(p => [p[1], p[0]]), corridorRoads);
 * const turns = nav.turns();            // the queue, once
 * const st = nav.update(lat, lon, acc);  // every fix
 * map.setBearing(st.bearing_deg);        // the predicted heading
 * ```
 */
export class Navigator {
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        NavigatorFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_navigator_free(ptr, 0);
    }
    /**
     * Total route length in metres.
     * @returns {number}
     */
    get length_m() {
        const ret = wasm.navigator_length_m(this.__wbg_ptr);
        return ret;
    }
    /**
     * Name one turn from roads decoded near it, the lazy alternative to
     * naming the whole queue when the route is built.
     *
     * Build the `Navigator` with no roads, then as each turn comes within
     * announcing distance: read `probe_lat`/`probe_lon` off the turn, fetch
     * the one cell holding that point, decode it, and pass the segments here.
     * That is one block per turn -- almost always already cached, since it is
     * a cell the route drives through -- instead of keeping a whole
     * corridor's roads alive for the trip.
     *
     * Name a turn *before* its first announcement: "turn left" at 2 km
     * followed by "turn left onto Broadway" at 200 m reads as two turns.
     *
     * Returns the named turn, or null when nothing was near enough.
     * @param {number} index
     * @param {any} roads_js
     * @param {number | null} [radius_m]
     * @returns {any}
     */
    name_turn(index, roads_js, radius_m) {
        const ret = wasm.navigator_name_turn(this.__wbg_ptr, index, roads_js, !isLikeNone(radius_m), isLikeNone(radius_m) ? 0 : radius_m);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return takeFromExternrefTable0(ret[0]);
    }
    /**
     * `path` is `[lon, lat]` pairs -- the decoders' order. `RouteResult.path`
     * is `[lat, lon]` for Leaflet, so flip it on the way in.
     *
     * `roads` is the corridor the route was found in, used only to name the
     * turns; pass null for an unnamed queue. `name_radius_m` defaults to 30.
     * @param {any} path_js
     * @param {any} roads_js
     * @param {number | null} [name_radius_m]
     */
    constructor(path_js, roads_js, name_radius_m) {
        const ret = wasm.navigator_new(path_js, roads_js, !isLikeNone(name_radius_m), isLikeNone(name_radius_m) ? 0 : name_radius_m);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        this.__wbg_ptr = ret[0];
        NavigatorFinalization.register(this, this.__wbg_ptr, this);
        return this;
    }
    /**
     * The point to fetch a cell for when naming turn `index`: `[lat, lon]`,
     * 15 m past the corner on the road being joined. Null for an index that
     * is not in the queue.
     * @param {number} index
     * @returns {Float64Array | undefined}
     */
    probe(index) {
        const ret = wasm.navigator_probe(this.__wbg_ptr, index);
        let v1;
        if (ret[0] !== 0) {
            v1 = getArrayF64FromWasm0(ret[0], ret[1]).slice();
            wasm.__wbindgen_free(ret[0], ret[1] * 8, 8);
        }
        return v1;
    }
    /**
     * The turn queue: `Depart`, every manoeuvre, `Arrive`. Each carries the
     * manoeuvre, the signed bearing change, where it is, how far along the
     * route, and the road it turns onto when one could be named.
     * @returns {any}
     */
    turns() {
        const ret = wasm.navigator_turns(this.__wbg_ptr);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return takeFromExternrefTable0(ret[0]);
    }
    /**
     * Where a fix puts you: snapped position, distance along and remaining,
     * the predicted heading, the next turn and how far to it, and whether
     * this fix is off the route.
     *
     * `off_route` describes one fix, not a decision. Require it on several
     * consecutive fixes before rerouting -- a single bad fix in a parking
     * garage is not a wrong turn.
     *
     * Null when the route is too short to follow.
     * @param {number} lat
     * @param {number} lon
     * @param {number} accuracy_m
     * @returns {any}
     */
    update(lat, lon, accuracy_m) {
        const ret = wasm.navigator_update(this.__wbg_ptr, lat, lon, accuracy_m);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return takeFromExternrefTable0(ret[0]);
    }
}
if (Symbol.dispose) Navigator.prototype[Symbol.dispose] = Navigator.prototype.free;

/**
 * Accelerometer window summary from three same-length `Float32Array`s (raw
 * m/s^2 per axis, no gravity removal needed — magnitude is used). Returns
 * `{variance, mean_magnitude, dominant_frequency, step_count,
 * window_duration_s}`, the shape [`MovementTracker::push`] takes.
 * @param {Float32Array} x
 * @param {Float32Array} y
 * @param {Float32Array} z
 * @param {number} sample_rate_hz
 * @returns {any}
 */
export function accel_stats(x, y, z, sample_rate_hz) {
    const ptr0 = passArrayF32ToWasm0(x, wasm.__wbindgen_malloc);
    const len0 = WASM_VECTOR_LEN;
    const ptr1 = passArrayF32ToWasm0(y, wasm.__wbindgen_malloc);
    const len1 = WASM_VECTOR_LEN;
    const ptr2 = passArrayF32ToWasm0(z, wasm.__wbindgen_malloc);
    const len2 = WASM_VECTOR_LEN;
    const ret = wasm.accel_stats(ptr0, len0, ptr1, len1, ptr2, len2, sample_rate_hz);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

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
 * @param {Uint8Array} block_bytes
 * @param {string} cell_hex
 * @param {number} version
 * @returns {any}
 */
export function address_cell(block_bytes, cell_hex, version) {
    const ptr0 = passArray8ToWasm0(block_bytes, wasm.__wbindgen_malloc);
    const len0 = WASM_VECTOR_LEN;
    const ptr1 = passStringToWasm0(cell_hex, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len1 = WASM_VECTOR_LEN;
    const ret = wasm.address_cell(ptr0, len0, ptr1, len1, version);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * Signed difference between two bearings, degrees, positive to the right.
 * @param {number} from_deg
 * @param {number} to_deg
 * @returns {number}
 */
export function bearing_delta(from_deg, to_deg) {
    const ret = wasm.bearing_delta(from_deg, to_deg);
    return ret;
}

/**
 * Bearing from one point to another, degrees clockwise from north -- the
 * convention a camera's own `direction` tag uses, so the two are directly
 * comparable.
 * @param {number} from_lat
 * @param {number} from_lon
 * @param {number} to_lat
 * @param {number} to_lon
 * @returns {number}
 */
export function bearing_to(from_lat, from_lon, to_lat, to_lon) {
    const ret = wasm.bearing_to(from_lat, from_lon, to_lat, to_lon);
    return ret;
}

/**
 * Which cameras can see `(lat, lon)`, nearest first -- "is anything pointed
 * at me right now".
 *
 * `cameras_js` is what `decode_cameras` returned; `buildings_js` is a
 * `ViewBuilding` array (`{coords, height_m, building_type}`), the same input
 * `viewshed` takes, and may be null when the caller has no buildings loaded.
 * `range_m` defaults to `ptiles_core::CAMERA_RANGE_M` (50 m).
 *
 * Each answer carries `sees` plus the three reasons behind it
 * (`aimed_at_you`, `aim_assumed`, `line_of_sight`, `blocked_by`). Every
 * assumption leans toward reporting a camera rather than omitting one: an
 * untagged aim is assumed to point at you, a dome rotates, and an unmeasured
 * building is credited with the low end of its height range. Passing no
 * buildings therefore gives every in-range camera a clear sight line.
 *
 * `index` and `blocked_by` arrive as BigInt -- they are `usize` in Rust, and
 * serde carries 64-bit integers across as BigInt rather than silently
 * narrowing them to a Number.
 * @param {number} lat
 * @param {number} lon
 * @param {any} cameras_js
 * @param {any} buildings_js
 * @param {number | null} [range_m]
 * @returns {any}
 */
export function cameras_seeing(lat, lon, cameras_js, buildings_js, range_m) {
    const ret = wasm.cameras_seeing(lat, lon, cameras_js, buildings_js, !isLikeNone(range_m), isLikeNone(range_m) ? 0 : range_m);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * `[lat, lon]` center of an H3 res-7 cell (hex string). Demo/browser
 * boundary for `ptiles_core::cell_center` -- replaces `h3-js`'s
 * `cellToLatLng`.
 * @param {string} cell_hex
 * @returns {Float64Array}
 */
export function cell_center(cell_hex) {
    const ptr0 = passStringToWasm0(cell_hex, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.cell_center(ptr0, len0);
    if (ret[3]) {
        throw takeFromExternrefTable0(ret[2]);
    }
    var v2 = getArrayF64FromWasm0(ret[0], ret[1]).slice();
    wasm.__wbindgen_free(ret[0], ret[1] * 8, 8);
    return v2;
}

/**
 * The filler-bit mask itself, for callers that must mask in their own code
 * (a `Map` keyed by cell id, say) rather than call across the boundary per id.
 * @returns {bigint}
 */
export function cell_filler_mask() {
    const ret = wasm.cell_filler_mask();
    return BigInt.asUintN(64, ret);
}

/**
 * H3 res-7 cell (lowercase hex string) containing `(lat, lon)`. Demo/browser
 * boundary for `ptiles_core::cell_for_coord` -- replaces `h3-js`'s
 * `latLngToCell` for every caller in this workspace.
 * @param {number} lat
 * @param {number} lon
 * @returns {string}
 */
export function cell_for_coord(lat, lon) {
    let deferred1_0;
    let deferred1_1;
    try {
        const ret = wasm.cell_for_coord(lat, lon);
        deferred1_0 = ret[0];
        deferred1_1 = ret[1];
        return getStringFromWasm0(ret[0], ret[1]);
    } finally {
        wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
    }
}

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
 * @param {number} min_lat
 * @param {number} min_lon
 * @param {number} max_lat
 * @param {number} max_lon
 * @returns {any[]}
 */
export function cells_for_bounds(min_lat, min_lon, max_lat, max_lon) {
    const ret = wasm.cells_for_bounds(min_lat, min_lon, max_lat, max_lon);
    if (ret[3]) {
        throw takeFromExternrefTable0(ret[2]);
    }
    var v1 = getArrayJsValueFromWasm0(ret[0], ret[1]).slice();
    wasm.__wbindgen_free(ret[0], ret[1] * 4, 4);
    return v1;
}

/**
 * The fraction of range the charge planner holds back (0.2).
 * @returns {number}
 */
export function charge_reserve() {
    const ret = wasm.charge_reserve();
    return ret;
}

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
 * @param {Uint8Array} aux
 * @param {string} cell_hex
 * @param {bigint} index_offset
 * @param {number} entry_size
 * @returns {any}
 */
export function coarse_bracket(aux, cell_hex, index_offset, entry_size) {
    const ptr0 = passArray8ToWasm0(aux, wasm.__wbindgen_malloc);
    const len0 = WASM_VECTOR_LEN;
    const ptr1 = passStringToWasm0(cell_hex, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len1 = WASM_VECTOR_LEN;
    const ret = wasm.coarse_bracket(ptr0, len0, ptr1, len1, index_offset, entry_size);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * @param {Uint8Array} data
 * @param {number} cell_center_lat
 * @param {number} cell_center_lon
 * @returns {any}
 */
export function decode_buildings(data, cell_center_lat, cell_center_lon) {
    const ptr0 = passArray8ToWasm0(data, wasm.__wbindgen_malloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.decode_buildings(ptr0, len0, cell_center_lat, cell_center_lon);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * Decode a buildings block for the cell it came from.
 *
 * Prefer this to [`decode_buildings`]: v8/v9 coordinates are deltas from the
 * cell centre, and passing the wrong centre produces a full set of well-formed
 * buildings in the wrong place with nothing to notice. Deriving the centre from
 * the cell id here removes the chance to get it wrong -- including the common
 * case of handing over a *masked* lookup key, which is not a valid H3 index and
 * used to answer null island.
 * @param {Uint8Array} block_bytes
 * @param {string} cell_hex
 * @returns {any}
 */
export function decode_buildings_for_cell(block_bytes, cell_hex) {
    const ptr0 = passArray8ToWasm0(block_bytes, wasm.__wbindgen_malloc);
    const len0 = WASM_VECTOR_LEN;
    const ptr1 = passStringToWasm0(cell_hex, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len1 = WASM_VECTOR_LEN;
    const ret = wasm.decode_buildings_for_cell(ptr0, len0, ptr1, len1);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * Decode a business block without knowing its version or its cell.
 *
 * **v3 only.** A v4 block is rejected rather than decoded: its coordinates are
 * offsets from the block's cell centre, and this entry point has no cell. Use
 * [`decode_business_versioned`] or [`decode_business_for_cell`] instead.
 * @param {Uint8Array} data
 * @returns {any}
 */
export function decode_business(data) {
    const ptr0 = passArray8ToWasm0(data, wasm.__wbindgen_malloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.decode_business(ptr0, len0);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * Decode a business block for the cell it came from.
 *
 * Prefer this to [`decode_business`], for the same reason
 * `decode_buildings_for_cell` exists: v4 stores coordinates as `i16` offsets
 * from the cell centre, and `decode_business`'s version sniff decodes v4 with a
 * centre of `(0, 0)` -- every record a few hundred metres off Null Island.
 * @param {Uint8Array} block_bytes
 * @param {string} cell_hex
 * @returns {any}
 */
export function decode_business_for_cell(block_bytes, cell_hex) {
    const ptr0 = passArray8ToWasm0(block_bytes, wasm.__wbindgen_malloc);
    const len0 = WASM_VECTOR_LEN;
    const ptr1 = passStringToWasm0(cell_hex, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len1 = WASM_VECTOR_LEN;
    const ret = wasm.decode_business_for_cell(ptr0, len0, ptr1, len1);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * Decode a business block whose file version is known (from the header).
 *
 * `version >= 4` reads v4 framing against the cell centre; anything lower reads
 * v3's length-prefixed framing. Use this over [`decode_business`] whenever the
 * header is at hand -- the sniff cannot tell the two apart reliably, because a
 * v4 block starts with a small zigzag uid that is also a plausible v3 length.
 * @param {Uint8Array} block_bytes
 * @param {number} version
 * @param {string} cell_hex
 * @returns {any}
 */
export function decode_business_versioned(block_bytes, version, cell_hex) {
    const ptr0 = passArray8ToWasm0(block_bytes, wasm.__wbindgen_malloc);
    const len0 = WASM_VECTOR_LEN;
    const ptr1 = passStringToWasm0(cell_hex, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len1 = WASM_VECTOR_LEN;
    const ret = wasm.decode_business_versioned(ptr0, len0, version, ptr1, len1);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * Decode a `camera` cell's records (PTILESC v1). Same merged-block caveat as
 * `decode_signals`.
 * @param {Uint8Array} data
 * @returns {any}
 */
export function decode_cameras(data) {
    const ptr0 = passArray8ToWasm0(data, wasm.__wbindgen_malloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.decode_cameras(ptr0, len0);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * Decode an EV charging block (`{ST}.ev_v1.ptiles`, PTILESE v1).
 *
 * `power_kw` and `connectors` are null/empty when OSM does not say, which is
 * most of them -- that is an unknown, not a zero.
 * @param {Uint8Array} data
 * @returns {any}
 */
export function decode_chargers(data) {
    const ptr0 = passArray8ToWasm0(data, wasm.__wbindgen_malloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.decode_chargers(ptr0, len0);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * @param {Uint8Array} data
 * @returns {any}
 */
export function decode_parks(data) {
    const ptr0 = passArray8ToWasm0(data, wasm.__wbindgen_malloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.decode_parks(ptr0, len0);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * @param {Uint8Array} data
 * @returns {any}
 */
export function decode_rail(data) {
    const ptr0 = passArray8ToWasm0(data, wasm.__wbindgen_malloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.decode_rail(ptr0, len0);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * @param {Uint8Array} data
 * @returns {any}
 */
export function decode_roads(data) {
    const ptr0 = passArray8ToWasm0(data, wasm.__wbindgen_malloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.decode_roads(ptr0, len0);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * Decode a `signals` cell's records (PTILESS v1). Input is the byte range for
 * one cell, i.e. the output of `merged_cell_slice` -- signals files carry a
 * 38-byte index and therefore merged blocks, so passing a whole decompressed
 * block here decodes its cell table as records.
 * @param {Uint8Array} data
 * @returns {any}
 */
export function decode_signals(data) {
    const ptr0 = passArray8ToWasm0(data, wasm.__wbindgen_malloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.decode_signals(ptr0, len0);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * @param {Uint8Array} data
 * @returns {any}
 */
export function decode_trails(data) {
    const ptr0 = passArray8ToWasm0(data, wasm.__wbindgen_malloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.decode_trails(ptr0, len0);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * @param {Uint8Array} data
 * @returns {any}
 */
export function decode_water(data) {
    const ptr0 = passArray8ToWasm0(data, wasm.__wbindgen_malloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.decode_water(ptr0, len0);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * internal fallback (see module doc above for why this isn't a direct call
 * into core). Pass an empty `dict` slice for dict-less layers (parks/address).
 * @param {Uint8Array} compressed
 * @param {Uint8Array} dict
 * @returns {Uint8Array}
 */
export function decompress_block(compressed, dict) {
    const ptr0 = passArray8ToWasm0(compressed, wasm.__wbindgen_malloc);
    const len0 = WASM_VECTOR_LEN;
    const ptr1 = passArray8ToWasm0(dict, wasm.__wbindgen_malloc);
    const len1 = WASM_VECTOR_LEN;
    const ret = wasm.decompress_block(ptr0, len0, ptr1, len1);
    if (ret[3]) {
        throw takeFromExternrefTable0(ret[2]);
    }
    var v3 = getArrayU8FromWasm0(ret[0], ret[1]).slice();
    wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
    return v3;
}

/**
 * Whether a trail type is built infrastructure (cycleway, footway) rather
 * than a natural way. Exposed so a renderer styles the two apart without
 * re-listing the layer's type vocabulary in JavaScript.
 * Great-circle distance in metres.
 *
 * The page had 31 sites doing this by hand in JavaScript -- each one its own
 * chance to use the wrong earth radius or drop the cos(lat) term. One
 * implementation, in core, is the point of this client.
 * @param {number} lat1
 * @param {number} lon1
 * @param {number} lat2
 * @param {number} lon2
 * @returns {number}
 */
export function distance_m(lat1, lon1, lat2, lon2) {
    const ret = wasm.distance_m(lat1, lon1, lat2, lon2);
    return ret;
}

/**
 * The height this crate would assume for a building type with no published
 * height. Exposed so a UI can explain a guess rather than just draw it.
 * @param {string} building_type
 * @returns {number}
 */
export function estimated_height_for(building_type) {
    const ptr0 = passStringToWasm0(building_type, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.estimated_height_for(ptr0, len0);
    return ret;
}

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
 * @param {Uint8Array} index_bytes
 * @param {string} cell_hex
 * @returns {any}
 */
export function find_block_for_cell(index_bytes, cell_hex) {
    const ptr0 = passArray8ToWasm0(index_bytes, wasm.__wbindgen_malloc);
    const len0 = WASM_VECTOR_LEN;
    const ptr1 = passStringToWasm0(cell_hex, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len1 = WASM_VECTOR_LEN;
    const ret = wasm.find_block_for_cell(ptr0, len0, ptr1, len1);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * Forward geocode over already-decoded address records: "400 Broadway".
 * @param {string} query
 * @param {any} addresses_js
 * @param {number | null} [limit]
 * @returns {any}
 */
export function geocode_addresses(query, addresses_js, limit) {
    const ptr0 = passStringToWasm0(query, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.geocode_addresses(ptr0, len0, addresses_js, isLikeNone(limit) ? Number.MAX_SAFE_INTEGER : (limit) >>> 0);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

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
 * @param {Uint8Array} header_bytes
 * @param {Uint8Array} index_bytes
 * @returns {any}
 */
export function index_entries_absolute(header_bytes, index_bytes) {
    const ptr0 = passArray8ToWasm0(header_bytes, wasm.__wbindgen_malloc);
    const len0 = WASM_VECTOR_LEN;
    const ptr1 = passArray8ToWasm0(index_bytes, wasm.__wbindgen_malloc);
    const len1 = WASM_VECTOR_LEN;
    const ret = wasm.index_entries_absolute(ptr0, len0, ptr1, len1);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * Whether an `intersection_type` is a node traffic waits at (signals, stop,
 * give-way) rather than flows through. This is the distinction
 * `MovementTracker` uses to stretch its "still driving" window.
 * @param {number} intersection_type
 * @returns {boolean}
 */
export function intersection_holds_traffic(intersection_type) {
    const ret = wasm.intersection_holds_traffic(intersection_type);
    return ret !== 0;
}

/**
 * Name for an `intersection_type` byte, from the format's own vocabulary:
 * `traffic_signals` | `stop` | `give_way` | `roundabout` | `junction`.
 *
 * `nearest_intersection` returns the raw integer because that is what the block
 * stores; this is how JS names it without keeping a second copy of the mapping
 * that can drift from the Rust one.
 * @param {number} intersection_type
 * @returns {string}
 */
export function intersection_type_name(intersection_type) {
    let deferred1_0;
    let deferred1_1;
    try {
        const ret = wasm.intersection_type_name(intersection_type);
        deferred1_0 = ret[0];
        deferred1_1 = ret[1];
        return getStringFromWasm0(ret[0], ret[1]);
    } finally {
        wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
    }
}

/**
 * Whether a connector charges at DC speed in North America (CCS1, CCS2,
 * CHAdeMO, Tesla). The difference between a twenty-minute stop and an
 * afternoon, and a property of the format's own connector vocabulary, so it
 * comes from core rather than being re-listed in each renderer.
 * @param {string} connector
 * @returns {boolean}
 */
export function is_fast_connector(connector) {
    const ptr0 = passStringToWasm0(connector, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.is_fast_connector(ptr0, len0);
    return ret !== 0;
}

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
 * @param {string} query
 * @returns {number}
 */
export function key_for_business_name_query(query) {
    const ptr0 = passStringToWasm0(query, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.key_for_business_name_query(ptr0, len0);
    return ret;
}

/**
 * Reverse geocode a point against already-decoded features.
 *
 * `roads_js` / `trails_js` / `addresses_js` are the arrays this module's
 * `decode_roads`, `decode_trails` and `address_cell` return, for whatever
 * cells the caller fetched. Returns
 * `{nearest_way, on_way, address}` — see `ptiles_core::locate`.
 * @param {number} lat
 * @param {number} lon
 * @param {any} roads_js
 * @param {any} trails_js
 * @param {any} addresses_js
 * @returns {any}
 */
export function locate_point(lat, lon, roads_js, trails_js, addresses_js) {
    const ret = wasm.locate_point(lat, lon, roads_js, trails_js, addresses_js);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * See [`key_for_business_name_query`]'s doc comment for the full JS-side
 * flow this is step 4 of. Pure decode-and-match over an already-fetched,
 * already-decompressed name-index block -- no I/O, no H3 lookup.
 * @param {Uint8Array} block_bytes
 * @param {string} query
 * @param {number} limit
 * @returns {any}
 */
export function match_business_name_block(block_bytes, query, limit) {
    const ptr0 = passArray8ToWasm0(block_bytes, wasm.__wbindgen_malloc);
    const len0 = WASM_VECTOR_LEN;
    const ptr1 = passStringToWasm0(query, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len1 = WASM_VECTOR_LEN;
    const ret = wasm.match_business_name_block(ptr0, len0, ptr1, len1, limit);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * The record bytes for one cell inside a decompressed merged block.
 *
 * Layers with a 38-byte index pack several cells per block behind a cell
 * table; a record decoder handed the whole block parses that table as
 * records and yields plausible garbage rather than an error. Returns `null`
 * if the block does not contain the cell.
 * @param {Uint8Array} block
 * @param {string} cell_hex
 * @returns {Uint8Array | undefined}
 */
export function merged_cell_slice(block, cell_hex) {
    const ptr0 = passArray8ToWasm0(block, wasm.__wbindgen_malloc);
    const len0 = WASM_VECTOR_LEN;
    const ptr1 = passStringToWasm0(cell_hex, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len1 = WASM_VECTOR_LEN;
    const ret = wasm.merged_cell_slice(ptr0, len0, ptr1, len1);
    if (ret[3]) {
        throw takeFromExternrefTable0(ret[2]);
    }
    let v3;
    if (ret[0] !== 0) {
        v3 = getArrayU8FromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
    }
    return v3;
}

/**
 * The speed thresholds the classifier judges a smoothed speed by, and the
 * stateless tree's own floors, so a UI can draw them without keeping a second
 * copy that drifts.
 *
 * `stationary_max_mps` / `driving_min_mps` are `MotionConfig`'s bands, which is
 * what the smoothed speed series is actually classified against.
 * `walking_ceiling_mps` / `driving_floor_mps` are the stateless `classify`
 * tree's floors -- higher, because that path has no smoothing behind it and a
 * single fast fix should not read as driving. Both matter to a reader: the first
 * pair explains the bands, the second explains the votes.
 *
 * `running_hint_mps` is the odd one out and is labelled as such wherever it is
 * shown: the classifier never infers `Running` from speed (it needs cadence), so
 * this is only where a *person* marking up a speed chart would draw the
 * walking/running line. It is here so every such tool uses the same documented
 * number instead of inventing one.
 * @returns {any}
 */
export function motion_thresholds() {
    const ret = wasm.motion_thresholds();
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * The nearest address to a point, or null. Separate from `locate_point` for
 * callers that hold only the address layer.
 * @param {number} lat
 * @param {number} lon
 * @param {any} addresses_js
 * @param {number | null} [threshold_m]
 * @returns {any}
 */
export function nearest_address_to(lat, lon, addresses_js, threshold_m) {
    const ret = wasm.nearest_address_to(lat, lon, addresses_js, !isLikeNone(threshold_m), isLikeNone(threshold_m) ? 0 : threshold_m);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * Decode a roads block and return the nearest labeled intersection to
 * `(lat, lon)` — the "am I at an intersection?" query. `threshold_m` is
 * optional (omit/`undefined` from JS for the SPEC.md default of 50 m).
 * Returns `null` when nothing is within the threshold. Reports a mapped
 * intersection point + its control type, not junction degree (the format
 * stores no topology). JS supplies `block_bytes` already decompressed
 * (no-I/O contract, same as `nearest_road`).
 * @param {Uint8Array} block_bytes
 * @param {number} lat
 * @param {number} lon
 * @param {number | null} [threshold_m]
 * @returns {any}
 */
export function nearest_intersection(block_bytes, lat, lon, threshold_m) {
    const ptr0 = passArray8ToWasm0(block_bytes, wasm.__wbindgen_malloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.nearest_intersection(ptr0, len0, lat, lon, !isLikeNone(threshold_m), isLikeNone(threshold_m) ? 0 : threshold_m);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * The rail track under a point, or null. Station points are skipped; use
 * `nearest_station` for those. `rail_js` is what `decode_rail` returned.
 * @param {number} lat
 * @param {number} lon
 * @param {any} rail_js
 * @returns {any}
 */
export function nearest_rail(lat, lon, rail_js) {
    const ret = wasm.nearest_rail(lat, lon, rail_js);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * Decode a roads block and return the single nearest road segment to
 * `(lat, lon)`, per plan addendum item 1: `{osm_id, name, road_class,
 * snapped, distance_m, geometry}`. `threshold_m` is optional; omit (pass
 * `None`/`undefined` from JS) to use the SPEC.md default of 50 m.
 *
 * JS supplies `block_bytes` already decompressed (no-fetch contract is
 * unchanged — this does not do any I/O or H3 lookup itself). Returns
 * `null` if no road is within the threshold.
 * @param {Uint8Array} block_bytes
 * @param {number} lat
 * @param {number} lon
 * @param {number | null} [threshold_m]
 * @returns {any}
 */
export function nearest_road(block_bytes, lat, lon, threshold_m) {
    const ptr0 = passArray8ToWasm0(block_bytes, wasm.__wbindgen_malloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.nearest_road(ptr0, len0, lat, lon, !isLikeNone(threshold_m), isLikeNone(threshold_m) ? 0 : threshold_m);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * The nearest station or halt point, or null.
 * @param {number} lat
 * @param {number} lon
 * @param {any} rail_js
 * @returns {any}
 */
export function nearest_station(lat, lon, rail_js) {
    const ret = wasm.nearest_station(lat, lon, rail_js);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * The trail under a point, or null: "which path am I walking on".
 *
 * `trails_js` is what `decode_trails` returned. Trailhead points are skipped
 * -- a point has no centreline to be on -- so ask `nearest_trailhead` for
 * those. Returns a `NearbyWay`: `{kind, name, class, distance_m, snapped,
 * on_it}`, with `on_it` true within 25 m.
 * @param {number} lat
 * @param {number} lon
 * @param {any} trails_js
 * @returns {any}
 */
export function nearest_trail(lat, lon, trails_js) {
    const ret = wasm.nearest_trail(lat, lon, trails_js);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * The nearest trailhead -- where a trail network is entered, which is what a
 * caller planning to *start* a walk wants. Returns a `NearbyPoint`:
 * `{kind, name, class, lat, lon, distance_m}`, or null.
 * @param {number} lat
 * @param {number} lon
 * @param {any} trails_js
 * @returns {any}
 */
export function nearest_trailhead(lat, lon, trails_js) {
    const ret = wasm.nearest_trailhead(lat, lon, trails_js);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * Ring-1 (6 cells) H3 neighbors of `cell_hex`, as lowercase hex strings.
 * Demo/browser boundary for `ptiles_core::neighbor_cells` -- replaces
 * `h3-js`'s `gridRing(cell, 1)` (used by the deployed demo's
 * `BusinessReader.query` for nearby-business radius search).
 * @param {string} cell_hex
 * @returns {any[]}
 */
export function neighbor_cells(cell_hex) {
    const ptr0 = passStringToWasm0(cell_hex, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.neighbor_cells(ptr0, len0);
    if (ret[3]) {
        throw takeFromExternrefTable0(ret[2]);
    }
    var v2 = getArrayJsValueFromWasm0(ret[0], ret[1]).slice();
    wasm.__wbindgen_free(ret[0], ret[1] * 4, 4);
    return v2;
}

/**
 * Drop an H3 id's unused low digits, so two ids naming the same res-7 cell
 * compare equal. The mask is a property of the id layout, not of any caller.
 * @param {bigint} cell
 * @returns {bigint}
 */
export function normalize_cell(cell) {
    const ret = wasm.normalize_cell(cell);
    return BigInt.asUintN(64, ret);
}

/**
 * The park at a point: the polygon containing it, else the nearest park
 * boundary. Returns a `NearbyArea`: `{kind, name, class, distance_m,
 * inside}`, or null. Check `inside` before telling a user they are in it --
 * `distance_m` is 0 exactly when they are.
 * @param {number} lat
 * @param {number} lon
 * @param {any} parks_js
 * @returns {any}
 */
export function park_at(lat, lon, parks_js) {
    const ret = wasm.park_at(lat, lon, parks_js);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

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
 * @param {Uint8Array} aux
 * @returns {any}
 */
export function parse_coarse_index(aux) {
    const ptr0 = passArray8ToWasm0(aux, wasm.__wbindgen_malloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.parse_coarse_index(ptr0, len0);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

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
 * @param {Uint8Array} entries
 * @param {number} entry_size
 * @returns {any}
 */
export function parse_entry_run(entries, entry_size) {
    const ptr0 = passArray8ToWasm0(entries, wasm.__wbindgen_malloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.parse_entry_run(ptr0, len0, entry_size);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * Parse a `.ptiles` file's 256-byte header (demo/browser boundary for
 * `ptiles_core::Header::parse`). Lets JS learn `dict_offset`/`dict_length`/
 * `index_offset`/`index_length`/`blocks_offset` from just the first 256
 * bytes of a Range request, without JS re-implementing the fixed-offset
 * layout from SPEC.md itself (`ptiles_core::header` is the single source of
 * truth for that layout parity-checked against `ptiles/codec.py`).
 * @param {Uint8Array} data
 * @returns {any}
 */
export function parse_header(data) {
    const ptr0 = passArray8ToWasm0(data, wasm.__wbindgen_malloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.parse_header(ptr0, len0);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

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
 * @param {Uint8Array} data
 * @returns {any}
 */
export function parse_index_entries(data) {
    const ptr0 = passArray8ToWasm0(data, wasm.__wbindgen_malloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.parse_index_entries(ptr0, len0);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

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
 * @param {Uint8Array} header_bytes
 * @param {Uint8Array} index_bytes
 * @returns {any}
 */
export function parse_index_layout(header_bytes, index_bytes) {
    const ptr0 = passArray8ToWasm0(header_bytes, wasm.__wbindgen_malloc);
    const len0 = WASM_VECTOR_LEN;
    const ptr1 = passArray8ToWasm0(index_bytes, wasm.__wbindgen_malloc);
    const len1 = WASM_VECTOR_LEN;
    const ret = wasm.parse_index_layout(ptr0, len0, ptr1, len1);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * Plan the charging stops a drive needs.
 *
 * `path_js` is the route as `[lon, lat]` pairs, `chargers_js` is what
 * `decode_chargers` returned for the corridor, and `range_m` is what the car
 * says it has *now*. The plan drives only
 * `range_m * (1 - CHARGE_RESERVE)` -- 80% -- so the driver reaches each stop
 * with something in reserve, and prefers a stop in the far half of each leg
 * so one stop does not become three. Returns
 * `{stops, reachable, shortfall_m, usable_range_m, route_m}`; `stops[].index`
 * points back into the chargers array.
 * @param {any} path_js
 * @param {any} chargers_js
 * @param {number} range_m
 * @param {number | null} [max_detour_m]
 * @returns {any}
 */
export function plan_charge_stops(path_js, chargers_js, range_m, max_detour_m) {
    const ret = wasm.plan_charge_stops(path_js, chargers_js, range_m, !isLikeNone(max_detour_m), isLikeNone(max_detour_m) ? 0 : max_detour_m);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * Whether a point falls inside a closed ring. `coords` is a flat
 * `[lon, lat, lon, lat, ...]` array -- the decoders' coordinate order,
 * flattened because a nested array costs a full serde round-trip per vertex.
 *
 * Exposed because the demo hand-rolled ray casting in JavaScript, where an
 * off-by-one in the wrap-around index silently mis-answers points near the
 * first vertex.
 * @param {number} lat
 * @param {number} lon
 * @param {Float64Array} coords
 * @returns {boolean}
 */
export function point_in_polygon(lat, lon, coords) {
    const ptr0 = passArrayF64ToWasm0(coords, wasm.__wbindgen_malloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.point_in_polygon(lat, lon, ptr0, len0);
    return ret !== 0;
}

/**
 * The height to draw a building at: the published one, or this crate's guess
 * when none was published.
 *
 * Returns a bare `f64`, not a struct. serde-wasm-bindgen hands objects to the
 * browser as a `Map` in some engines, where `r.height_m` reads `undefined` --
 * which would silently extrude every guessed building to `NaN`. The caller
 * already knows whether it passed a height, so the flag adds nothing here;
 * `height_or_estimate` in core still returns it for Rust callers.
 * @param {number | null | undefined} height_m
 * @param {string} building_type
 * @returns {number}
 */
export function resolved_height(height_m, building_type) {
    const ptr0 = passStringToWasm0(building_type, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.resolved_height(!isLikeNone(height_m), isLikeNone(height_m) ? 0 : height_m, ptr0, len0);
    return ret;
}

/**
 * Decode a roads block into its full segment list (geometry + name +
 * every other `RoadSegment` field), identical shape to `decode_roads`.
 * Exists as its own export (plan addendum item 1's "roads" query) so
 * callers reach for a query-shaped name rather than the raw decoder;
 * ring-1 neighbor-cell expansion is NOT done here -- JS owns block
 * fetching (which cell(s) to fetch bytes for), so ring handling stays in
 * JS per the plan's `query.rs` split (`neighbor_cells` in core, calling
 * convention in JS/CLI).
 * @param {Uint8Array} block_bytes
 * @returns {any}
 */
export function roads_in_block(block_bytes) {
    const ptr0 = passArray8ToWasm0(block_bytes, wasm.__wbindgen_malloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.roads_in_block(ptr0, len0);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * Route on pre-decoded road segments (JS owns fetch + zstd + corridor).
 * `segments_js`: `[{coords:[[lon,lat],...], road_class, oneway?, speed_limit_kmh?}, ...]`
 * `zone_middle`: bool[] same length (true = arterial-only middle); empty/null = all end-cap.
 * Returns `{distance_m, duration_s, path:[[lat,lon],...]}` or null.
 * @param {any} segments_js
 * @param {any} zone_middle
 * @param {number} lat1
 * @param {number} lon1
 * @param {number} lat2
 * @param {number} lon2
 * @param {number | null} [snap_m]
 * @param {boolean | null} [avoid_highways]
 * @param {boolean | null} [avoid_intersections]
 * @returns {any}
 */
export function route_from_segments(segments_js, zone_middle, lat1, lon1, lat2, lon2, snap_m, avoid_highways, avoid_intersections) {
    const ret = wasm.route_from_segments(segments_js, zone_middle, lat1, lon1, lat2, lon2, !isLikeNone(snap_m), isLikeNone(snap_m) ? 0 : snap_m, isLikeNone(avoid_highways) ? 0xFFFFFF : avoid_highways ? 1 : 0, isLikeNone(avoid_intersections) ? 0xFFFFFF : avoid_intersections ? 1 : 0);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * Diagnostic form of [`route_from_segments`]. It always returns an object:
 * `{result, failure}` with exactly one field non-null. The nullable export
 * remains intact for existing callers.
 * @param {any} segments_js
 * @param {any} zone_middle
 * @param {number} lat1
 * @param {number} lon1
 * @param {number} lat2
 * @param {number} lon2
 * @param {number | null} [snap_m]
 * @param {boolean | null} [avoid_highways]
 * @param {boolean | null} [avoid_intersections]
 * @returns {any}
 */
export function route_from_segments_diagnostic(segments_js, zone_middle, lat1, lon1, lat2, lon2, snap_m, avoid_highways, avoid_intersections) {
    const ret = wasm.route_from_segments_diagnostic(segments_js, zone_middle, lat1, lon1, lat2, lon2, !isLikeNone(snap_m), isLikeNone(snap_m) ? 0 : snap_m, isLikeNone(avoid_highways) ? 0xFFFFFF : avoid_highways ? 1 : 0, isLikeNone(avoid_intersections) ? 0xFFFFFF : avoid_intersections ? 1 : 0);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * Route on foot over decoded trails: "get me there on paths, not roads".
 *
 * `trails_js` is what `decode_trails` returned. Trails are converted to the
 * router's segment shape by `core::trail_segments` -- a trail and a road are
 * both a named linestring with a class, so one graph builder serves both --
 * and routed under the foot profile: paths, tracks, footways, steps and the
 * quiet street classes a walker actually uses are routable, motorways and
 * trunk roads are not, one-way tags do not apply, and speeds are walking
 * speeds rather than posted limits.
 *
 * Pass `roads_js` as well to let the walk use quiet streets between paths;
 * the trails layer alone is a set of disconnected fragments in most places,
 * because the path through the park does not touch the path in the next
 * park. Null when no walkable route exists within `snap_m` of both ends.
 * @param {any} trails_js
 * @param {any} roads_js
 * @param {number} lat1
 * @param {number} lon1
 * @param {number} lat2
 * @param {number} lon2
 * @param {number | null} [snap_m]
 * @returns {any}
 */
export function route_trails(trails_js, roads_js, lat1, lon1, lat2, lon2, snap_m) {
    const ret = wasm.route_trails(trails_js, roads_js, lat1, lon1, lat2, lon2, !isLikeNone(snap_m), isLikeNone(snap_m) ? 0 : snap_m);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * Rank road/building/business candidates for a GPS fix (plan addendum
 * item 2: emission-probability scoring lives in core, this is just the
 * wasm boundary). `buildings_block`/`business_block` are optional --
 * pass an empty slice (`new Uint8Array()`) from JS when a layer isn't
 * available for the current cell; buildings need `cell_center_lat`/
 * `cell_center_lon` to decode (v8 buildings are cell-relative-delta
 * encoded, see `buildings.rs`), which is also why callers must supply
 * them even when `buildings_block` is empty.
 * @param {string} fix_json
 * @param {Uint8Array} roads_block
 * @param {Uint8Array} buildings_block
 * @param {Uint8Array} business_block
 * @param {number} cell_center_lat
 * @param {number} cell_center_lon
 * @returns {any}
 */
export function score_candidates(fix_json, roads_block, buildings_block, business_block, cell_center_lat, cell_center_lon) {
    const ptr0 = passStringToWasm0(fix_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ptr1 = passArray8ToWasm0(roads_block, wasm.__wbindgen_malloc);
    const len1 = WASM_VECTOR_LEN;
    const ptr2 = passArray8ToWasm0(buildings_block, wasm.__wbindgen_malloc);
    const len2 = WASM_VECTOR_LEN;
    const ptr3 = passArray8ToWasm0(business_block, wasm.__wbindgen_malloc);
    const len3 = WASM_VECTOR_LEN;
    const ret = wasm.score_candidates(ptr0, len0, ptr1, len1, ptr2, len2, ptr3, len3, cell_center_lat, cell_center_lon);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

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
 * @param {Float64Array} t_ms
 * @param {Float64Array} speed_mps
 * @param {any} config
 * @returns {any}
 */
export function significant_shifts(t_ms, speed_mps, config) {
    const ptr0 = passArrayF64ToWasm0(t_ms, wasm.__wbindgen_malloc);
    const len0 = WASM_VECTOR_LEN;
    const ptr1 = passArrayF64ToWasm0(speed_mps, wasm.__wbindgen_malloc);
    const len1 = WASM_VECTOR_LEN;
    const ret = wasm.significant_shifts(ptr0, len0, ptr1, len1, config);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * Which band a smoothed speed falls in, as a lowercase `MovementType` name.
 *
 * The same function the classifier uses, exported so a caller can bucket a
 * series without re-implementing the comparison -- which is how a UI's idea of
 * "walking" drifts from the library's.
 * @param {number} smoothed_mps
 * @returns {string}
 */
export function speed_band(smoothed_mps) {
    let deferred1_0;
    let deferred1_1;
    try {
        const ret = wasm.speed_band(smoothed_mps);
        deferred1_0 = ret[0];
        deferred1_1 = ret[1];
        return getStringFromWasm0(ret[0], ret[1]);
    } finally {
        wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
    }
}

/**
 * @param {string} trail_type
 * @returns {boolean}
 */
export function trail_is_developed(trail_type) {
    const ptr0 = passStringToWasm0(trail_type, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.trail_is_developed(ptr0, len0);
    return ret !== 0;
}

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
 * @param {any} buildings
 * @param {number} lat
 * @param {number} lon
 * @param {number} eye_m
 * @param {number} radius_m
 * @returns {any}
 */
export function viewshed(buildings, lat, lon, eye_m, radius_m) {
    const ret = wasm.viewshed(buildings, lat, lon, eye_m, radius_m);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

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
 * @param {any} buildings
 * @param {any} origins
 * @param {number} eye_m
 * @param {number} radius_m
 * @returns {any}
 */
export function viewshed_multi(buildings, origins, eye_m, radius_m) {
    const ret = wasm.viewshed_multi(buildings, origins, eye_m, radius_m);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}

/**
 * The water at a point: the polygon containing it, else the nearest water
 * feature. A river centreline is a linestring and never reports `inside`;
 * reference geometries (`geom_type == 2`, coordinates held elsewhere in the
 * file) are skipped rather than reported at a position they do not carry.
 * @param {number} lat
 * @param {number} lon
 * @param {any} water_js
 * @returns {any}
 */
export function water_at(lat, lon, water_js) {
    const ret = wasm.water_at(lat, lon, water_js);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return takeFromExternrefTable0(ret[0]);
}
function __wbg_get_imports() {
    const import0 = {
        __proto__: null,
        __wbg_Error_fdd633d4bb5dd76a: function(arg0, arg1) {
            const ret = Error(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_Number_c4bdf66bb78f7977: function(arg0) {
            const ret = Number(arg0);
            return ret;
        },
        __wbg_String_8564e559799eccda: function(arg0, arg1) {
            const ret = String(arg1);
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg___wbindgen_bigint_get_as_i64_d9e915702856f831: function(arg0, arg1) {
            const v = arg1;
            const ret = typeof(v) === 'bigint' ? v : undefined;
            getDataViewMemory0().setBigInt64(arg0 + 8 * 1, isLikeNone(ret) ? BigInt(0) : ret, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, !isLikeNone(ret), true);
        },
        __wbg___wbindgen_boolean_get_edaed31a367ce1bd: function(arg0) {
            const v = arg0;
            const ret = typeof(v) === 'boolean' ? v : undefined;
            return isLikeNone(ret) ? 0xFFFFFF : ret ? 1 : 0;
        },
        __wbg___wbindgen_debug_string_8a447059637473e2: function(arg0, arg1) {
            const ret = debugString(arg1);
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg___wbindgen_in_4990f46af709e33c: function(arg0, arg1) {
            const ret = arg0 in arg1;
            return ret;
        },
        __wbg___wbindgen_is_bigint_90b5ccfe67c78460: function(arg0) {
            const ret = typeof(arg0) === 'bigint';
            return ret;
        },
        __wbg___wbindgen_is_function_acc5528be2b923f2: function(arg0) {
            const ret = typeof(arg0) === 'function';
            return ret;
        },
        __wbg___wbindgen_is_null_6d937fbfb6478470: function(arg0) {
            const ret = arg0 === null;
            return ret;
        },
        __wbg___wbindgen_is_object_0beba4a1980d3eea: function(arg0) {
            const val = arg0;
            const ret = typeof(val) === 'object' && val !== null;
            return ret;
        },
        __wbg___wbindgen_is_string_1fca8072260dd261: function(arg0) {
            const ret = typeof(arg0) === 'string';
            return ret;
        },
        __wbg___wbindgen_is_undefined_721f8decd50c87a3: function(arg0) {
            const ret = arg0 === undefined;
            return ret;
        },
        __wbg___wbindgen_jsval_eq_4e8c38722cb8ff51: function(arg0, arg1) {
            const ret = arg0 === arg1;
            return ret;
        },
        __wbg___wbindgen_jsval_loose_eq_4b9aba9e5b3c4582: function(arg0, arg1) {
            const ret = arg0 == arg1;
            return ret;
        },
        __wbg___wbindgen_number_get_1cc01dd708740256: function(arg0, arg1) {
            const obj = arg1;
            const ret = typeof(obj) === 'number' ? obj : undefined;
            getDataViewMemory0().setFloat64(arg0 + 8 * 1, isLikeNone(ret) ? 0 : ret, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, !isLikeNone(ret), true);
        },
        __wbg___wbindgen_string_get_71bb4348194e31f0: function(arg0, arg1) {
            const obj = arg1;
            const ret = typeof(obj) === 'string' ? obj : undefined;
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg___wbindgen_throw_ea4887a5f8f9a9db: function(arg0, arg1) {
            throw new Error(getStringFromWasm0(arg0, arg1));
        },
        __wbg_call_8e98ed2f3c86c4b5: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.call(arg1);
            return ret;
        }, arguments); },
        __wbg_done_b62d4a7d2286852a: function(arg0) {
            const ret = arg0.done;
            return ret;
        },
        __wbg_entries_c261c3fa1f281256: function(arg0) {
            const ret = Object.entries(arg0);
            return ret;
        },
        __wbg_get_197a3fe98f169e38: function(arg0, arg1) {
            const ret = arg0[arg1 >>> 0];
            return ret;
        },
        __wbg_get_9a29be2cb383ed9a: function() { return handleError(function (arg0, arg1) {
            const ret = Reflect.get(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_get_unchecked_54a4374c38e08460: function(arg0, arg1) {
            const ret = arg0[arg1 >>> 0];
            return ret;
        },
        __wbg_get_with_ref_key_6412cf3094599694: function(arg0, arg1) {
            const ret = arg0[arg1];
            return ret;
        },
        __wbg_instanceof_ArrayBuffer_2a7bb09fee70c2da: function(arg0) {
            let result;
            try {
                result = arg0 instanceof ArrayBuffer;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Uint8Array_f080092dc70f5d58: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Uint8Array;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_isArray_145a34fd0a38d37b: function(arg0) {
            const ret = Array.isArray(arg0);
            return ret;
        },
        __wbg_isSafeInteger_a3389a198582f5f6: function(arg0) {
            const ret = Number.isSafeInteger(arg0);
            return ret;
        },
        __wbg_iterator_cc47ba25a2be735a: function() {
            const ret = Symbol.iterator;
            return ret;
        },
        __wbg_length_589238bdcf171f0e: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_length_c6054974c0a6cdb9: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_new_2e117a478906f062: function() {
            const ret = new Object();
            return ret;
        },
        __wbg_new_36e147a8ced3c6e0: function() {
            const ret = new Array();
            return ret;
        },
        __wbg_new_81880fb5002cb255: function(arg0) {
            const ret = new Uint8Array(arg0);
            return ret;
        },
        __wbg_next_0c4066e251d2eff9: function() { return handleError(function (arg0) {
            const ret = arg0.next();
            return ret;
        }, arguments); },
        __wbg_next_402fa10b59ab20c3: function(arg0) {
            const ret = arg0.next;
            return ret;
        },
        __wbg_prototypesetcall_d721637c7ca66eb8: function(arg0, arg1, arg2) {
            Uint8Array.prototype.set.call(getArrayU8FromWasm0(arg0, arg1), arg2);
        },
        __wbg_set_6be42768c690e380: function(arg0, arg1, arg2) {
            arg0[arg1] = arg2;
        },
        __wbg_set_dc601f4a69da0bc2: function(arg0, arg1, arg2) {
            arg0[arg1 >>> 0] = arg2;
        },
        __wbg_value_49f783bb59765962: function(arg0) {
            const ret = arg0.value;
            return ret;
        },
        __wbindgen_cast_0000000000000001: function(arg0) {
            // Cast intrinsic for `F64 -> Externref`.
            const ret = arg0;
            return ret;
        },
        __wbindgen_cast_0000000000000002: function(arg0) {
            // Cast intrinsic for `I64 -> Externref`.
            const ret = arg0;
            return ret;
        },
        __wbindgen_cast_0000000000000003: function(arg0, arg1) {
            // Cast intrinsic for `Ref(String) -> Externref`.
            const ret = getStringFromWasm0(arg0, arg1);
            return ret;
        },
        __wbindgen_cast_0000000000000004: function(arg0) {
            // Cast intrinsic for `U64 -> Externref`.
            const ret = BigInt.asUintN(64, arg0);
            return ret;
        },
        __wbindgen_init_externref_table: function() {
            const table = wasm.__wbindgen_externrefs;
            const offset = table.grow(4);
            table.set(0, undefined);
            table.set(offset + 0, undefined);
            table.set(offset + 1, null);
            table.set(offset + 2, true);
            table.set(offset + 3, false);
        },
    };
    return {
        __proto__: null,
        "./ptiles_client_bg.js": import0,
    };
}

const AdaptiveMotionSessionFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_adaptivemotionsession_free(ptr, 1));
const AdminReaderFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_adminreader_free(ptr, 1));
const MovementTrackerFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_movementtracker_free(ptr, 1));
const NavigatorFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_navigator_free(ptr, 1));

function addToExternrefTable0(obj) {
    const idx = wasm.__externref_table_alloc();
    wasm.__wbindgen_externrefs.set(idx, obj);
    return idx;
}

function debugString(val) {
    // primitive types
    const type = typeof val;
    if (type == 'number' || type == 'boolean' || val == null) {
        return  `${val}`;
    }
    if (type == 'string') {
        return `"${val}"`;
    }
    if (type == 'symbol') {
        const description = val.description;
        if (description == null) {
            return 'Symbol';
        } else {
            return `Symbol(${description})`;
        }
    }
    if (type == 'function') {
        const name = val.name;
        if (typeof name == 'string' && name.length > 0) {
            return `Function(${name})`;
        } else {
            return 'Function';
        }
    }
    // objects
    if (Array.isArray(val)) {
        const length = val.length;
        let debug = '[';
        if (length > 0) {
            debug += debugString(val[0]);
        }
        for(let i = 1; i < length; i++) {
            debug += ', ' + debugString(val[i]);
        }
        debug += ']';
        return debug;
    }
    // Test for built-in
    const builtInMatches = /\[object ([^\]]+)\]/.exec(toString.call(val));
    let className;
    if (builtInMatches && builtInMatches.length > 1) {
        className = builtInMatches[1];
    } else {
        // Failed to match the standard '[object ClassName]'
        return toString.call(val);
    }
    if (className == 'Object') {
        // we're a user defined class or Object
        // JSON.stringify avoids problems with cycles, and is generally much
        // easier than looping through ownProperties of `val`.
        try {
            return 'Object(' + JSON.stringify(val) + ')';
        } catch (_) {
            return 'Object';
        }
    }
    // errors
    if (val instanceof Error) {
        return `${val.name}: ${val.message}\n${val.stack}`;
    }
    // TODO we could test for more things here, like `Set`s and `Map`s.
    return className;
}

function getArrayF64FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getFloat64ArrayMemory0().subarray(ptr / 8, ptr / 8 + len);
}

function getArrayJsValueFromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    const mem = getDataViewMemory0();
    const result = [];
    for (let i = ptr; i < ptr + 4 * len; i += 4) {
        result.push(wasm.__wbindgen_externrefs.get(mem.getUint32(i, true)));
    }
    wasm.__externref_drop_slice(ptr, len);
    return result;
}

function getArrayU8FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getUint8ArrayMemory0().subarray(ptr / 1, ptr / 1 + len);
}

let cachedDataViewMemory0 = null;
function getDataViewMemory0() {
    if (cachedDataViewMemory0 === null || cachedDataViewMemory0.buffer.detached === true || (cachedDataViewMemory0.buffer.detached === undefined && cachedDataViewMemory0.buffer !== wasm.memory.buffer)) {
        cachedDataViewMemory0 = new DataView(wasm.memory.buffer);
    }
    return cachedDataViewMemory0;
}

let cachedFloat32ArrayMemory0 = null;
function getFloat32ArrayMemory0() {
    if (cachedFloat32ArrayMemory0 === null || cachedFloat32ArrayMemory0.byteLength === 0) {
        cachedFloat32ArrayMemory0 = new Float32Array(wasm.memory.buffer);
    }
    return cachedFloat32ArrayMemory0;
}

let cachedFloat64ArrayMemory0 = null;
function getFloat64ArrayMemory0() {
    if (cachedFloat64ArrayMemory0 === null || cachedFloat64ArrayMemory0.byteLength === 0) {
        cachedFloat64ArrayMemory0 = new Float64Array(wasm.memory.buffer);
    }
    return cachedFloat64ArrayMemory0;
}

function getStringFromWasm0(ptr, len) {
    return decodeText(ptr >>> 0, len);
}

let cachedUint8ArrayMemory0 = null;
function getUint8ArrayMemory0() {
    if (cachedUint8ArrayMemory0 === null || cachedUint8ArrayMemory0.byteLength === 0) {
        cachedUint8ArrayMemory0 = new Uint8Array(wasm.memory.buffer);
    }
    return cachedUint8ArrayMemory0;
}

function handleError(f, args) {
    try {
        return f.apply(this, args);
    } catch (e) {
        const idx = addToExternrefTable0(e);
        wasm.__wbindgen_exn_store(idx);
    }
}

function isLikeNone(x) {
    return x === undefined || x === null;
}

function passArray8ToWasm0(arg, malloc) {
    const ptr = malloc(arg.length * 1, 1) >>> 0;
    getUint8ArrayMemory0().set(arg, ptr / 1);
    WASM_VECTOR_LEN = arg.length;
    return ptr;
}

function passArrayF32ToWasm0(arg, malloc) {
    const ptr = malloc(arg.length * 4, 4) >>> 0;
    getFloat32ArrayMemory0().set(arg, ptr / 4);
    WASM_VECTOR_LEN = arg.length;
    return ptr;
}

function passArrayF64ToWasm0(arg, malloc) {
    const ptr = malloc(arg.length * 8, 8) >>> 0;
    getFloat64ArrayMemory0().set(arg, ptr / 8);
    WASM_VECTOR_LEN = arg.length;
    return ptr;
}

function passStringToWasm0(arg, malloc, realloc) {
    if (realloc === undefined) {
        const buf = cachedTextEncoder.encode(arg);
        const ptr = malloc(buf.length, 1) >>> 0;
        getUint8ArrayMemory0().subarray(ptr, ptr + buf.length).set(buf);
        WASM_VECTOR_LEN = buf.length;
        return ptr;
    }

    let len = arg.length;
    let ptr = malloc(len, 1) >>> 0;

    const mem = getUint8ArrayMemory0();

    let offset = 0;

    for (; offset < len; offset++) {
        const code = arg.charCodeAt(offset);
        if (code > 0x7F) break;
        mem[ptr + offset] = code;
    }
    if (offset !== len) {
        if (offset !== 0) {
            arg = arg.slice(offset);
        }
        ptr = realloc(ptr, len, len = offset + arg.length * 3, 1) >>> 0;
        const view = getUint8ArrayMemory0().subarray(ptr + offset, ptr + len);
        const ret = cachedTextEncoder.encodeInto(arg, view);

        offset += ret.written;
        ptr = realloc(ptr, len, offset, 1) >>> 0;
    }

    WASM_VECTOR_LEN = offset;
    return ptr;
}

function takeFromExternrefTable0(idx) {
    const value = wasm.__wbindgen_externrefs.get(idx);
    wasm.__externref_table_dealloc(idx);
    return value;
}

let cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
cachedTextDecoder.decode();
const MAX_SAFARI_DECODE_BYTES = 2146435072;
let numBytesDecoded = 0;
function decodeText(ptr, len) {
    numBytesDecoded += len;
    if (numBytesDecoded >= MAX_SAFARI_DECODE_BYTES) {
        cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
        cachedTextDecoder.decode();
        numBytesDecoded = len;
    }
    return cachedTextDecoder.decode(getUint8ArrayMemory0().subarray(ptr, ptr + len));
}

const cachedTextEncoder = new TextEncoder();

if (!('encodeInto' in cachedTextEncoder)) {
    cachedTextEncoder.encodeInto = function (arg, view) {
        const buf = cachedTextEncoder.encode(arg);
        view.set(buf);
        return {
            read: arg.length,
            written: buf.length
        };
    };
}

let WASM_VECTOR_LEN = 0;

let wasmModule, wasmInstance, wasm;
function __wbg_finalize_init(instance, module) {
    wasmInstance = instance;
    wasm = instance.exports;
    wasmModule = module;
    cachedDataViewMemory0 = null;
    cachedFloat32ArrayMemory0 = null;
    cachedFloat64ArrayMemory0 = null;
    cachedUint8ArrayMemory0 = null;
    wasm.__wbindgen_start();
    return wasm;
}

async function __wbg_load(module, imports) {
    if (typeof Response === 'function' && module instanceof Response) {
        if (typeof WebAssembly.instantiateStreaming === 'function') {
            try {
                return await WebAssembly.instantiateStreaming(module, imports);
            } catch (e) {
                const validResponse = module.ok && expectedResponseType(module.type);

                if (validResponse && module.headers.get('Content-Type') !== 'application/wasm') {
                    console.warn("`WebAssembly.instantiateStreaming` failed because your server does not serve Wasm with `application/wasm` MIME type. Falling back to `WebAssembly.instantiate` which is slower. Original error:\n", e);

                } else { throw e; }
            }
        }

        const bytes = await module.arrayBuffer();
        return await WebAssembly.instantiate(bytes, imports);
    } else {
        const instance = await WebAssembly.instantiate(module, imports);

        if (instance instanceof WebAssembly.Instance) {
            return { instance, module };
        } else {
            return instance;
        }
    }

    function expectedResponseType(type) {
        switch (type) {
            case 'basic': case 'cors': case 'default': return true;
        }
        return false;
    }
}

function initSync(module) {
    if (wasm !== undefined) return wasm;


    if (module !== undefined) {
        if (Object.getPrototypeOf(module) === Object.prototype) {
            ({module} = module)
        } else {
            console.warn('using deprecated parameters for `initSync()`; pass a single object instead')
        }
    }

    const imports = __wbg_get_imports();
    if (!(module instanceof WebAssembly.Module)) {
        module = new WebAssembly.Module(module);
    }
    const instance = new WebAssembly.Instance(module, imports);
    return __wbg_finalize_init(instance, module);
}

async function __wbg_init(module_or_path) {
    if (wasm !== undefined) return wasm;


    if (module_or_path !== undefined) {
        if (Object.getPrototypeOf(module_or_path) === Object.prototype) {
            ({module_or_path} = module_or_path)
        } else {
            console.warn('using deprecated parameters for the initialization function; pass a single object instead')
        }
    }

    if (module_or_path === undefined) {
        module_or_path = new URL('ptiles_client_bg.wasm', import.meta.url);
    }
    const imports = __wbg_get_imports();

    if (typeof module_or_path === 'string' || (typeof Request === 'function' && module_or_path instanceof Request) || (typeof URL === 'function' && module_or_path instanceof URL)) {
        module_or_path = fetch(module_or_path);
    }

    const { instance, module } = await __wbg_load(await module_or_path, imports);

    return __wbg_finalize_init(instance, module);
}

export { initSync, __wbg_init as default };
