package com.steele.looky.offline

import android.content.Context
import com.steele.looky.model.GeoPoint
import com.steele.looky.model.MapFeature
import uniffi.ptiles_ffi.PtilesLayer
import uniffi.ptiles_ffi.PtilesStack
import uniffi.ptiles_ffi.AdminLayer
import uniffi.ptiles_ffi.BusinessInfo
import uniffi.ptiles_ffi.CameraInfo
import uniffi.ptiles_ffi.OfflineRouteMode
import uniffi.ptiles_ffi.RoadContext
import uniffi.ptiles_ffi.TrailInfo
import uniffi.ptiles_ffi.LatLon
import uniffi.ptiles_ffi.Navigator
import java.io.File
import java.util.concurrent.ConcurrentHashMap
import kotlin.math.cos

data class NearbyContext(
    val roadName: String?,
    val roadClass: String?,
    val roadDistanceM: Double?,
)

class PtilesRepository(context: Context) {
    private val appContext = context.applicationContext
    private val manager = PackManager(context)
    private data class CachedLayer(
        val length: Long,
        val modified: Long,
        val layer: PtilesLayer,
    )

    // Cache by path, not logical layer. More than one state can be installed,
    // and caching the first file under "roads" made every other state query
    // use that state's graph.
    private val layers = ConcurrentHashMap<String, CachedLayer>()
    @Volatile private var adminLayer: AdminLayer? = null
    @Volatile private var stateHint: String? = null

    data class RouteResult(
        val points: List<GeoPoint>,
        val distanceM: Double,
        val durationS: Double,
        val decodedSegments: Int,
    )

    data class BusinessResult(val name: String, val point: GeoPoint, val score: Int)

    private fun layer(suffix: String, lat: Double, lon: Double): PtilesLayer? {
        val state = currentStateCode(lat, lon)
        return layerCandidates(manager.packsDir.listFiles().orEmpty(), suffix, state)
            .asSequence()
            .mapNotNull(::openCached)
            .firstOrNull { it.covers(lat, lon) }
    }

    private fun openCached(file: File): PtilesLayer? {
        val path = file.absolutePath
        layers[path]?.takeIf {
            it.length == file.length() && it.modified == file.lastModified()
        }?.let { return it.layer }
        return runCatching { PtilesLayer.open(path) }.getOrNull()?.also {
            layers[path] = CachedLayer(file.length(), file.lastModified(), it)
        }
    }

    fun nearbyRoadContext(lat: Double, lon: Double): Pair<RoadContext?, NearbyContext> {
        val road = runCatching { layer("roads", lat, lon)?.nearestRoad(lat, lon) }.getOrNull()
        return if (road == null) {
            null to NearbyContext(null, null, null)
        } else {
            RoadContext(road.roadClass, road.distanceM, null) to
                NearbyContext(road.name, road.roadClass, road.distanceM)
        }
    }

    /** Snap a route endpoint to the installed offline network before routing. */
    fun snapForRoute(point: GeoPoint, trail: Boolean): GeoPoint? = runCatching {
        if (trail) {
            layer("trails", point.lat, point.lon)?.nearestTrail(point.lat, point.lon, 1u)?.let { GeoPoint(it.snapped.lat, it.snapped.lon) }
                ?: layer("roads", point.lat, point.lon)?.nearestRoad(point.lat, point.lon)?.let { GeoPoint(it.snappedLat, it.snappedLon) }
                ?: nearestVertex(layer("trails", point.lat, point.lon)?.trails(point.lat, point.lon, 1u)?.flatMap { it.geometry }, point)
        } else {
            layer("roads", point.lat, point.lon)?.nearestRoad(point.lat, point.lon)?.let { GeoPoint(it.snappedLat, it.snappedLon) }
                ?: nearestVertex(layer("roads", point.lat, point.lon)?.roads(point.lat, point.lon, 1u)?.flatMap { it.geometry }, point)
        }
    }.getOrNull()

    private fun nearestVertex(points: List<LatLon>?, target: GeoPoint): GeoPoint? = points
        ?.minByOrNull { (it.lat - target.lat) * (it.lat - target.lat) + (it.lon - target.lon) * (it.lon - target.lon) }
        ?.let { GeoPoint(it.lat, it.lon) }

    /**
     * Every renderable feature around a coordinate.
     *
     * A single ring-1 query covers about 4 km, well short of the roughly 8 km
     * the default viewport shows, so the map ran out of data mid-screen and
     * immediately when panned. The FFI caps `ring` at 1 (`validate_ring` in
     * `ffi/src/lib.rs`), and passing 2 does not widen the query -- it throws,
     * which the `runCatching` blocks below turn into an empty map. So coverage
     * is widened by querying ring 1 at several sample centres and merging.
     *
     * ponytail: `spread` sample centres cost `2 * spread + 1` times the decode
     * work. If first paint drags, drop the default to 0 before reaching for a
     * tile cache.
     */
    fun featuresAround(
        lat: Double,
        lon: Double,
        trails: Boolean,
        developer: Boolean = false,
        places: Boolean = false,
        spread: Int = DEFAULT_SPREAD,
    ): List<MapFeature> {
        val out = mutableListOf<MapFeature>()
        val centers = sampleCenters(lat, lon, spread)
        centers.forEach { c ->
            runCatching {
                layer("roads", c.lat, c.lon)?.roads(c.lat, c.lon, RING)?.forEach { road ->
                    out += MapFeature(
                        road.geometry.map { GeoPoint(it.lat, it.lon) },
                        road.roadClass,
                        road.name,
                    )
                }
            }
            runCatching {
                layer("water", c.lat, c.lon)?.water(c.lat, c.lon, RING)?.forEach { water ->
                    if (water.geometry.size < 2) return@forEach
                    // geom_type 0 is a polygon -- a lake, a reservoir, a wide
                    // river's bank -- and wants filling, not outlining.
                    val kind = if (water.geomType == 0.toUByte()) "water_area" else "water"
                    out += MapFeature(water.geometry.map { GeoPoint(it.lat, it.lon) }, kind, water.name)
                }
            }
            runCatching {
                layer("parks", c.lat, c.lon)?.parks(c.lat, c.lon, RING)?.forEach { park ->
                    if (park.geometry.size > 1) out += MapFeature(park.geometry.map { GeoPoint(it.lat, it.lon) }, "park", park.name)
                }
            }
            runCatching {
                layer("trails", c.lat, c.lon)?.trails(c.lat, c.lon, RING)?.forEach { trail ->
                    val points = trail.geometry.map { GeoPoint(it.lat, it.lon) }
                    if (points.isEmpty()) return@forEach
                    if (trail.isTrailhead) {
                        out += MapFeature(listOf(points.first()), "trailhead", trail.name)
                        return@forEach
                    }
                    out += MapFeature(points, "trail:${trail.trailType}", trail.name)
                    // Where a path starts and stops is the thing a walker
                    // squints for; the line alone does not say.
                    out += MapFeature(listOf(points.first()), "trail_end", trail.name)
                    if (points.size > 1) out += MapFeature(listOf(points.last()), "trail_end", trail.name)
                }
            }
            if (places || developer) runCatching {
                layer("business", c.lat, c.lon)?.businessesNear(c.lat, c.lon, RING, 1_500.0)?.forEach { business ->
                    if (isFlightNode(business.name)) return@forEach
                    out += MapFeature(
                        listOf(GeoPoint(business.location.lat, business.location.lon)),
                        "business:${business.categoryIdx}",
                        business.name,
                    )
                }
            }
            // Camera is a national layer, so the normal state-aware selection
            // falls through to US.camera.ptiles. It is rendered in both Drive
            // and Trail whenever installed, not hidden behind developer mode.
            runCatching {
                layer("camera", c.lat, c.lon)?.cameras(c.lat, c.lon, RING)?.forEach { camera ->
                    out += cameraMapFeature(camera)
                }
            }
            if (developer) {
                runCatching {
                    layer("rail", c.lat, c.lon)?.rail(c.lat, c.lon, RING)?.forEach { rail ->
                        if (rail.geometry.isNotEmpty()) {
                            out += MapFeature(
                                rail.geometry.map { GeoPoint(it.lat, it.lon) },
                                if (rail.geomType == 1.toUByte()) "station" else "rail:${rail.railType}",
                                rail.name,
                            )
                        }
                    }
                }
            }
        }
        // Buildings are fetched by explicit point list rather than by ring, so
        // one widened grid covers every sample centre at once. The footprint
        // ring comes back with them now; the centroid is the fallback for
        // records that carry no polygon.
        runCatching {
            val buildingLayer = layer("buildings", lat, lon)
            if (buildingLayer != null) {
                val span = buildingSampleSpan(spread)
                val sample = (-span..span).flatMap { y -> (-span..span).map { x -> LatLon(lat + y * 0.003, lon + x * 0.004) } }
                buildingLayer.buildingsAt(sample).filterNotNull().forEach { building ->
                    val outline = building.geometry.map { GeoPoint(it.lat, it.lon) }
                    out += if (outline.size > 2) {
                        MapFeature(outline, "building_area", building.name)
                    } else {
                        MapFeature(listOf(GeoPoint(building.centroid.lat, building.centroid.lon)), "building", building.name)
                    }
                }
            }
        }
        return capFeatures(dedupeFeatures(out))
    }

    /**
     * Business search: what is near you that resembles what you typed.
     *
     * Two passes, because neither alone answers the question a driver asks.
     * The spatial layer gives everything within a few kilometres regardless of
     * spelling, which is where a hit almost always is. The state name index
     * (`{STATE}.business_name_index.ptiles`, bucketed by first letter, not by
     * geography -- so deliberately not routed through [layer]) reaches the rest
     * of the state for the times the answer is genuinely further out.
     *
     * Both are then scored on name similarity and pulled toward you by
     * distance, so a misspelling down the road beats a perfect match across
     * the state -- which is what the name index alone kept returning.
     */
    fun searchBusinesses(
        query: String,
        origin: GeoPoint? = null,
        limit: Int = SEARCH_LIMIT,
    ): List<BusinessResult>? {
        if (query.isBlank() || limit <= 0) return emptyList()
        val indexes = nameIndexFiles()
        val nearLayer = origin?.let { layer("business", it.lat, it.lon) }
        // Null, not empty: "nothing installed to search" and "nothing matched"
        // are different answers, and only one of them is the user's to fix.
        if (indexes.isEmpty() && nearLayer == null) return null

        val nearby = if (origin == null || nearLayer == null) {
            emptyList()
        } else {
            nearLayer.businessesNear(origin.lat, origin.lon, RING, SEARCH_NEARBY_RADIUS_M)
                .map { BusinessResult(it.name, GeoPoint(it.location.lat, it.location.lon), score = 0) }
        }
        val perIndex = (limit * SEARCH_OVERFETCH).coerceAtMost(SEARCH_MAX_FETCH)
        val statewide = indexes.flatMap { file ->
            runCatching {
                openCached(file)
                    ?.searchBusiness(query, perIndex.toUInt())
                    ?.map { BusinessResult(it.name, GeoPoint(it.location.lat, it.location.lon), it.score.toInt()) }
                    .orEmpty()
            }.getOrDefault(emptyList())
        }
        return rankByNameAndDistance(query, nearby + statewide, origin, limit)
    }

    /**
     * What is around you, nearest first -- the list an empty search box shows.
     *
     * Straight off the spatial business layer, so it needs no query and no
     * name index; the same flight-node junk is filtered out of it.
     */
    fun businessesNearby(
        origin: GeoPoint,
        radiusM: Double = NEARBY_RADIUS_M,
        limit: Int = SEARCH_LIMIT,
    ): List<BusinessResult>? {
        // Null when no business layer covers here -- a decode failure is a
        // different thing again, and is left to throw.
        val layer = layer("business", origin.lat, origin.lon) ?: return null
        val hits = layer.businessesNear(origin.lat, origin.lon, RING, radiusM)
            .map { BusinessResult(it.name, GeoPoint(it.location.lat, it.location.lon), score = 0) }
        return mergeBusinessHits(hits, limit, origin)
    }

    /**
     * Named trails and trailheads around a point, nearest first.
     *
     * Trail mode's answer to the business search: the trails layer is
     * geographic, so this is the same query with or without a name typed --
     * `query` only narrows what came back. Unnamed ways are dropped; a list of
     * "path" repeated forty times is not a destination picker.
     *
     * The point offered is the vertex nearest the origin, which for a long
     * trail is the end you would actually walk in from.
     */
    fun trailsNearby(
        origin: GeoPoint,
        query: String = "",
        limit: Int = SEARCH_LIMIT,
    ): List<BusinessResult>? {
        val needle = query.trim().lowercase()
        val centers = sampleCenters(origin.lat, origin.lon, DEFAULT_SPREAD)
        val layers = centers.mapNotNull { c -> layer("trails", c.lat, c.lon)?.let { c to it } }
        if (layers.isEmpty()) return null
        val hits = layers.flatMap { (c, layer) ->
            layer.trails(c.lat, c.lon, RING)
        }.mapNotNull { trail ->
            val name = trail.name?.takeIf { it.isNotBlank() } ?: return@mapNotNull null
            if (needle.isNotEmpty() && !name.lowercase().contains(needle)) return@mapNotNull null
            val nearest = nearestVertex(trail.geometry, origin) ?: return@mapNotNull null
            // Trailheads first among equals: they are where you park.
            BusinessResult(name, nearest, score = if (trail.isTrailhead) 1 else 0)
        }
        // One row per trail, not one per stretch of it: the same path decodes
        // as several segments, and "Chirt Pit Road" twice is not two choices.
        val nearestPerTrail = hits
            .sortedBy { flatDistance2(origin, it.point) }
            .distinctBy { it.name.lowercase() }
        return mergeBusinessHits(nearestPerTrail, limit, origin)
    }

    /**
     * The trail nearest a tap, with the attributes the layer carries.
     *
     * Distance is to the nearest vertex, which for a long path is the bit of
     * it you actually pointed at.
     */
    fun trailAt(point: GeoPoint, radiusM: Double = BUSINESS_TAP_RADIUS_M): TrailInfo? = runCatching {
        layer("trails", point.lat, point.lon)
            ?.trails(point.lat, point.lon, RING)
            ?.mapNotNull { trail ->
                val nearest = trail.geometry.minByOrNull { v ->
                    flatDistance2(point, GeoPoint(v.lat, v.lon))
                } ?: return@mapNotNull null
                trail to flatDistance2(point, GeoPoint(nearest.lat, nearest.lon))
            }
            ?.filter { (_, d2) -> d2 <= radiusM * radiusM }
            ?.minByOrNull { (_, d2) -> d2 }
            ?.first
    }.getOrNull()

    /**
     * The business record nearest to a tap, with its extended attributes.
     *
     * The map draws businesses from [featuresAround], which keeps only name and
     * category. Phone, website, status, and provenance live on [BusinessInfo],
     * so the detail card re-reads the layer for the one record that was tapped.
     */
    fun businessAt(point: GeoPoint, radiusM: Double = BUSINESS_TAP_RADIUS_M): BusinessInfo? = runCatching {
        layer("business", point.lat, point.lon)
            ?.businessesNear(point.lat, point.lon, RING, radiusM)
            // The FFI already bounded the set by radius; ranking only needs a
            // monotone distance, so a flat-earth squared metre suffices.
            ?.minByOrNull { hit ->
                val dLat = (hit.location.lat - point.lat) * 111_320.0
                val dLon = (hit.location.lon - point.lon) * 111_320.0 * cos(Math.toRadians(point.lat))
                dLat * dLat + dLon * dLon
            }
    }.getOrNull()

    /**
     * A [Navigator] over a computed route, with its turns named where roads
     * could be decoded.
     *
     * Naming reads the roads around points sampled along the path. Every
     * sample is a decode, so they are spaced [NAME_SAMPLE_STEP] apart and
     * capped: a turn on an unnamed lane is normal, and the queue is still
     * correct without a name.
     */
    fun navigatorFor(path: List<GeoPoint>): Navigator? {
        if (path.size < 2) return null
        val step = (path.size / NAME_SAMPLE_MAX).coerceAtLeast(NAME_SAMPLE_STEP)
        val roads = path.filterIndexed { index, _ -> index % step == 0 }
            .flatMap { point ->
                runCatching { layer("roads", point.lat, point.lon)?.roads(point.lat, point.lon, RING).orEmpty() }
                    .getOrDefault(emptyList())
            }
            .distinctBy { it.osmId }
        return runCatching {
            Navigator(path.map { LatLon(it.lat, it.lon) }, roads, TURN_NAME_RADIUS_M)
        }.getOrNull()
    }

    private fun nameIndexFiles(): List<File> = manager.packsDir.listFiles()
        .orEmpty()
        .filter { it.isFile && it.name.endsWith(".business_name_index.ptiles", ignoreCase = true) }
        .sortedBy { it.name }

    /**
     * One leg, split in half as many times as the corridor cap demands.
     *
     * The FFI bounds a route's corridor at 512 H3 res-7 cells, and fails two
     * ways once a trip outgrows it: "bad bounding box" when the box itself is
     * over the cap, and "Disconnected" when it fits but the road joining the
     * ends arcs outside it. Both are facts about the corridor, not about the
     * trip being impossible, and halving fixes both -- each half's box hugs
     * the line more tightly, so the missing link falls inside it.
     *
     * Measured on the Tennessee roads pack: Savannah to Camden is
     * Disconnected whole, and routes as 70.9 km + 53.1 km split in half.
     *
     * ponytail: the split point is the geometric midpoint snapped to the
     * network, not a real waypoint. It can put a joint somewhere a router
     * would not have chosen. Good enough until someone drives one and says
     * otherwise.
     */
    fun offlineRoute(
        start: GeoPoint,
        end: GeoPoint,
        trail: Boolean,
        avoidHighways: Boolean,
        avoidIntersections: Boolean,
        splitsLeft: Int = MAX_ROUTE_SPLITS,
    ): RouteResult = try {
        routeLeg(start, end, trail, avoidHighways, avoidIntersections)
    } catch (e: Exception) {
        if (splitsLeft <= 0 || !isSplittableFailure(e)) throw e
        val midpoint = GeoPoint((start.lat + end.lat) / 2, (start.lon + end.lon) / 2)
        val split = snapForRoute(midpoint, trail) ?: midpoint
        joinLegs(
            listOf(
                offlineRoute(start, split, trail, avoidHighways, avoidIntersections, splitsLeft - 1),
                offlineRoute(split, end, trail, avoidHighways, avoidIntersections, splitsLeft - 1),
            )
        )
    }

    private fun routeLeg(
        start: GeoPoint,
        end: GeoPoint,
        trail: Boolean,
        avoidHighways: Boolean,
        avoidIntersections: Boolean,
    ): RouteResult {
        val stack = PtilesStack.withLayers(
            roads = layer("roads", start.lat, start.lon),
            buildings = layer("buildings", start.lat, start.lon),
            business = layer("business", start.lat, start.lon),
            trails = layer("trails", start.lat, start.lon),
            parks = layer("parks", start.lat, start.lon),
            water = layer("water", start.lat, start.lon),
            camera = layer("camera", start.lat, start.lon),
            addresses = null,
        )
        val route = stack.offlineRoute(
            start.lat, start.lon, end.lat, end.lon,
            if (trail) OfflineRouteMode.TRAIL else OfflineRouteMode.DRIVING,
            avoidHighways,
            avoidIntersections,
        )
        check(route.path.isNotEmpty()) { "No roads or trails in the downloaded maps here -- download this area in Offline maps" }
        return RouteResult(
            route.path.map { GeoPoint(it.lat, it.lon) },
            route.distanceM,
            route.durationS,
            route.decodedSegments.toInt(),
        )
    }

    /**
     * A multi-stop route, built by chaining single-leg [offlineRoute] calls.
     *
     * `PtilesStack.offline_route` takes exactly one start and one end, so a
     * waypoint route is N independent graph builds and costs roughly N times a
     * single route. `onLegDone(completed, total)` reports that progress so the
     * caller can show a real percentage instead of an indeterminate spinner.
     */
    fun offlineRouteVia(
        start: GeoPoint,
        waypoints: List<GeoPoint>,
        end: GeoPoint,
        trail: Boolean,
        avoidHighways: Boolean,
        avoidIntersections: Boolean,
        onLegDone: (Int, Int) -> Unit = { _, _ -> },
    ): RouteResult {
        val legs = routeLegs(start, waypoints, end)
        val results = legs.mapIndexed { index, (from, to) ->
            val snappedFrom = snapForRoute(from, trail) ?: from
            val snappedTo = snapForRoute(to, trail) ?: to
            offlineRoute(snappedFrom, snappedTo, trail, avoidHighways, avoidIntersections)
                .also { onLegDone(index + 1, legs.size) }
        }
        return joinLegs(results)
    }

    fun installedLayers(): List<File> = manager.packs().flatMap { it.layers }

    /** True when a real installed roads pack covers the current coordinate. */
    fun mapsReadyAt(lat: Double, lon: Double): Boolean = layer("roads", lat, lon) != null

    /**
     * Which state a coordinate is in.
     *
     * `US.admin.ptiles` when it is installed, then the boundaries baked into
     * the APK, and only then [StateResolver]'s bounding boxes -- which overlap
     * at every border and were picking the wrong state on the strength of a
     * box centre.
     */
    fun currentStateCode(lat: Double, lon: Double): String? {
        val admin = adminLayer ?: manager.packsDir.listFiles().orEmpty()
            .firstOrNull { it.isFile && it.name == "US.admin.ptiles" }
            ?.let { runCatching { AdminLayer.open(it.absolutePath) }.getOrNull() }
            ?.also { adminLayer = it }
        val resolved = admin
            ?.let { runCatching { it.adminAt(lat, lon)?.state }.getOrNull() }
            ?.let(StateResolver::codeForName)
            ?: StateBoundaries.stateAt(appContext, lat, lon)
            ?: StateResolver.stateAt(lat, lon, stateHint)
        if (resolved != null) stateHint = resolved
        return resolved
    }

    companion object {
        private val VERSIONED_STEM = Regex("^(.+)_v(\\d+)$")

        /**
         * The only ring the FFI accepts. `validate_ring` in `ffi/src/lib.rs`
         * rejects anything above 1, so widening coverage means more query
         * centres, not a bigger ring.
         */
        internal val RING: UByte = 1u

        /** Extra sample centres in each direction. See [featuresAround]. */
        internal const val DEFAULT_SPREAD = 1

        /**
         * Spacing between sample centres, roughly one res-7 cell.
         *
         * A res-7 cell is about 1.4 km across; these are the degree equivalents
         * at mid-latitudes, close enough for query placement -- overlap is
         * deduped, and only a gap would show, as blank paper.
         */
        internal const val SAMPLE_STEP_LAT = 0.030
        internal const val SAMPLE_STEP_LON = 0.037

        internal const val SEARCH_LIMIT = 20

        /** Hits pulled per index before the distance sort trims them. */
        internal const val SEARCH_OVERFETCH = 5
        internal const val SEARCH_MAX_FETCH = 200

        /**
         * How many times a leg may be halved before the corridor cap is
         * accepted as real. Eight legs covers a cross-state drive; past that
         * the pack almost certainly does not hold the road anyway.
         */
        internal const val MAX_ROUTE_SPLITS = 3

        /**
         * True for the two corridor failures splitting fixes, false for a
         * missing layer, an unsnappable endpoint, or an empty graph -- none of
         * which a smaller corridor helps.
         *
         * Widening the corridor instead was tried and measured: on a
         * disconnected route the box is already at the cell cap, so there is
         * usually no room to widen, and where there was room the extra
         * segments did not bridge the gap.
         */
        internal fun isSplittableFailure(error: Throwable): Boolean {
            val message = generateSequence(error, Throwable::cause)
                .mapNotNull { it.message }
                .joinToString(" ")
                .lowercase()
            val overBudget = "bounding box" in message && ("too large" in message || "cells" in message)
            return overBudget || "disconnected" in message
        }

        /** How far a typed search sweeps the spatial layer before the index. */
        internal const val SEARCH_NEARBY_RADIUS_M = 8_000.0

        /** Below this, a name is not a match however lenient the scoring. */
        internal const val MIN_NAME_SIMILARITY = 0.55

        /** Distance at which the pull toward the user stops growing. */
        internal const val DISTANCE_FALLOFF_KM = 40.0

        /** Similarity points the full falloff distance costs a hit. */
        internal const val DISTANCE_WEIGHT = 0.45

        /** How far the default nearby list reaches. */
        internal const val NEARBY_RADIUS_M = 2_000.0

        /** Turn naming: how far a road may be from a manoeuvre to name it. */
        internal const val TURN_NAME_RADIUS_M = 30.0

        /** Path vertices between road decodes when naming turns, and a cap. */
        internal const val NAME_SAMPLE_STEP = 8
        internal const val NAME_SAMPLE_MAX = 40

        /**
         * Ceiling on features handed to the renderer in one viewport.
         *
         * Every polyline is its own `drawPath` stroke. Five ring-1 queries over
         * central Nashville return enough of them to block the render thread
         * past the 5 s input timeout -- an ANR whose main-thread stack sits in
         * `syncAndDrawFrame`, with no app code on it. Dense cities are the
         * normal case, not the edge one.
         *
         * ponytail: a flat cap with a length-based priority. The real fix is
         * per-zoom road-class filtering, worth building when someone complains
         * about a missing side street rather than before.
         */
        internal const val MAX_DRAWN_FEATURES = 1_500

        /**
         * Draw priority when a viewport exceeds [MAX_DRAWN_FEATURES].
         *
         * Ranked by kind, not by vertex count: park and water polygons carry
         * far more points than a street does, so a purely length-based cap
         * kept the scenery and deleted the road network -- exactly backwards
         * for a map you navigate by.
         */
        internal fun featureRank(kind: String): Int = when {
            kind in setOf("motorway", "trunk", "primary", "secondary") -> 0
            // Every other road, ahead of trails. Ranking trails higher meant a
            // town's footways -- one per street, and there is a footway beside
            // most streets -- filled the cap and evicted the street grid they
            // run alongside, leaving highways floating on blank paper.
            kind in setOf("tertiary", "residential", "unclassified", "service", "living_street", "road") -> 1
            kind.startsWith("trail") || kind in setOf("path", "footway", "track", "steps") -> 3
            kind == "water_area" || kind == "water" -> 2
            kind == "park" -> 4
            kind == "building_area" || kind == "building" ||
                kind.startsWith("camera") || kind.startsWith("business") -> 5
            else -> 1 // anything else the roads layer classified
        }

        /**
         * Keep the most navigationally useful features when over the cap.
         *
         * Kind first, then vertex count inside a kind, so the arterial grid
         * survives and the least legible detail goes first.
         */
        internal fun capFeatures(
            features: List<MapFeature>,
            max: Int = MAX_DRAWN_FEATURES,
        ): List<MapFeature> =
            if (features.size <= max) features
            else features
                .sortedWith(compareBy<MapFeature> { featureRank(it.kind) }.thenByDescending { it.points.size })
                .take(max)

        /** How close a tap must land to count as hitting a business pin. */
        internal const val BUSINESS_TAP_RADIUS_M = 60.0

        /** Half-width of the building sample grid for a given spread. */
        internal fun buildingSampleSpan(spread: Int): Int = (spread + 2).coerceIn(2, 6)

        /**
         * Query centres for a viewport: the coordinate itself plus `spread`
         * steps out along each axis.
         *
         * A plus shape rather than a full grid -- the diagonals are largely
         * reached by the ring around each arm, and a 3x3 grid would cost 9
         * decodes for coverage 5 mostly provides.
         */
        internal fun sampleCenters(lat: Double, lon: Double, spread: Int): List<GeoPoint> {
            if (spread <= 0) return listOf(GeoPoint(lat, lon))
            val out = mutableListOf(GeoPoint(lat, lon))
            for (step in 1..spread) {
                out += GeoPoint(lat + step * SAMPLE_STEP_LAT, lon)
                out += GeoPoint(lat - step * SAMPLE_STEP_LAT, lon)
                out += GeoPoint(lat, lon + step * SAMPLE_STEP_LON)
                out += GeoPoint(lat, lon - step * SAMPLE_STEP_LON)
            }
            return out
        }

        /**
         * Drop features returned by more than one sample centre.
         *
         * Neighbouring ring queries share cells, so the same road comes back
         * several times. Identity is the geometry itself: this API exposes no
         * stable id, and two features with identical kind, name, length and
         * endpoints would render identically anyway.
         */
        internal fun dedupeFeatures(features: List<MapFeature>): List<MapFeature> =
            features.distinctBy {
                listOf(it.kind, it.name, it.points.size, it.points.firstOrNull(), it.points.lastOrNull())
            }

        /**
         * Rank and de-duplicate hits gathered from several state indexes.
         *
         * Score is the FFI's match quality (2 exact, 1 prefix, 0 substring),
         * so it sorts descending and name breaks ties. Two states can hold the
         * same chain at the same spot only when their bounding boxes overlap at
         * a border, so identity is name plus coordinate rounded to 5 decimal
         * places -- about a metre, well under the spacing of two real stores.
         */
        /**
         * Flight numbers that ride along in the business layer.
         *
         * OSM aeroway import leaves nodes named `AA 1234`, `DL2201`, `UA 45`:
         * a gate's departure, not a place. They are never what a search for a
         * business meant, and they crowd out real hits near an airport.
         */
        private val FLIGHT_NODE = Regex("""^[A-Z]{2,3}\s?\d{1,4}[A-Z]?$""")

        internal fun isFlightNode(name: String): Boolean = FLIGHT_NODE.matches(name.trim())

        /**
         * How much a name looks like what was typed, 0 (not at all) to 1.
         *
         * Exact beats prefix beats substring beats a word that merely starts
         * the same; below that it is edit distance, so "wafle huse" still
         * finds Waffle House. Anything under [MIN_NAME_SIMILARITY] is not a
         * match at all -- fuzzy without a floor returns the whole layer.
         */
        internal fun nameSimilarity(query: String, name: String): Double {
            val q = query.trim().lowercase()
            val n = name.trim().lowercase()
            if (q.isEmpty() || n.isEmpty()) return 0.0
            if (q == n) return 1.0
            if (n.startsWith(q)) return 0.92
            if (n.contains(q)) return 0.85
            val words = n.split(' ', '-', '/', ',').filter { it.isNotEmpty() }
            if (words.any { it.startsWith(q) }) return 0.8
            // Typos: the whole string, then the single best word, since
            // "huse" against "waffle house" is mostly a distance to `waffle`.
            val whole = editRatio(q, n)
            val best = words.maxOfOrNull { editRatio(q, it) } ?: 0.0
            return maxOf(whole, best * 0.95)
        }

        /** 1 minus normalised Levenshtein distance. */
        internal fun editRatio(a: String, b: String): Double {
            if (a.isEmpty() || b.isEmpty()) return 0.0
            val longer = maxOf(a.length, b.length)
            return 1.0 - editDistance(a, b).toDouble() / longer
        }

        private fun editDistance(a: String, b: String): Int {
            var previous = IntArray(b.length + 1) { it }
            var current = IntArray(b.length + 1)
            for (i in 1..a.length) {
                current[0] = i
                for (j in 1..b.length) {
                    val substitution = previous[j - 1] + if (a[i - 1] == b[j - 1]) 0 else 1
                    current[j] = minOf(current[j - 1] + 1, previous[j] + 1, substitution)
                }
                val swap = previous
                previous = current
                current = swap
            }
            return previous[b.length]
        }

        /**
         * Rank by resemblance, pulled toward the user by distance.
         *
         * A perfect match forty kilometres away scores below a close-enough
         * one on this street: [DISTANCE_WEIGHT] is how much of a similarity
         * point the full [DISTANCE_FALLOFF_KM] is worth.
         */
        internal fun rankByNameAndDistance(
            query: String,
            hits: List<BusinessResult>,
            origin: GeoPoint?,
            limit: Int,
        ): List<BusinessResult> = hits
            .asSequence()
            .filterNot { isFlightNode(it.name) }
            .map { it to nameSimilarity(query, it.name) }
            .filter { (_, similarity) -> similarity >= MIN_NAME_SIMILARITY }
            .map { (hit, similarity) ->
                val km = origin?.let { kotlin.math.sqrt(flatDistance2(it, hit.point)) / 1_000.0 } ?: 0.0
                val penalty = (km.coerceAtMost(DISTANCE_FALLOFF_KM) / DISTANCE_FALLOFF_KM) * DISTANCE_WEIGHT
                Triple(hit, similarity - penalty, km)
            }
            .sortedWith(compareByDescending<Triple<BusinessResult, Double, Double>> { it.second }.thenBy { it.third })
            .map { it.first }
            .distinctBy { Triple(it.name, round5(it.point.lat), round5(it.point.lon)) }
            .take(limit)
            .toList()

        internal fun mergeBusinessHits(
            hits: List<BusinessResult>,
            limit: Int,
            origin: GeoPoint? = null,
        ): List<BusinessResult> {
            // Nearest first when we know where the user is -- the web demo
            // reaches the same order by walking cells outward from the origin,
            // but the name index answers in one block, so the sort is the whole
            // job. Match quality only breaks ties between equidistant hits.
            val order = if (origin == null) {
                compareByDescending<BusinessResult> { it.score }.thenBy { it.name }
            } else {
                compareBy<BusinessResult> { flatDistance2(origin, it.point) }
                    .thenByDescending { it.score }
                    .thenBy { it.name }
            }
            return hits
                .filterNot { isFlightNode(it.name) }
                .sortedWith(order)
                .distinctBy { Triple(it.name, round5(it.point.lat), round5(it.point.lon)) }
                .take(limit)
        }

        /** Squared metres, flat-earth. Ranking never needs the real thing. */
        internal fun flatDistance2(from: GeoPoint, to: GeoPoint): Double {
            val dLat = (to.lat - from.lat) * 111_320.0
            val dLon = (to.lon - from.lon) * 111_320.0 * cos(Math.toRadians(from.lat))
            return dLat * dLat + dLon * dLon
        }

        private fun round5(value: Double): Long = Math.round(value * 100_000.0)

        /**
         * Consecutive (from, to) pairs for a start, ordered waypoints, and end.
         *
         * No waypoints yields the single original leg, so the waypoint path and
         * the plain path are the same code.
         */
        internal fun routeLegs(
            start: GeoPoint,
            waypoints: List<GeoPoint>,
            end: GeoPoint,
        ): List<Pair<GeoPoint, GeoPoint>> {
            val stops = listOf(start) + waypoints + end
            return stops.zipWithNext()
        }

        /**
         * Concatenate leg results into one route.
         *
         * Each leg starts where the previous ended, so the shared joint point
         * is dropped rather than drawn twice.
         */
        internal fun joinLegs(legs: List<RouteResult>): RouteResult {
            require(legs.isNotEmpty()) { "a route needs at least one leg" }
            val points = mutableListOf<GeoPoint>()
            legs.forEach { leg ->
                if (points.isEmpty()) points += leg.points
                else points += leg.points.drop(1)
            }
            return RouteResult(
                points,
                legs.sumOf { it.distanceM },
                legs.sumOf { it.durationS },
                legs.sumOf { it.decodedSegments },
            )
        }

        internal fun layerCandidates(
            files: Array<out File>,
            suffix: String,
            preferredState: String? = null,
        ): List<File> = files
            .asSequence()
            .filter { it.isFile && it.extension.equals("ptiles", ignoreCase = true) }
            .filterNot(PackManager::isBundledConformanceSlice)
            .mapNotNull { file ->
                val layerStem = file.nameWithoutExtension.substringAfter('.', "")
                val match = VERSIONED_STEM.matchEntire(layerStem)
                val logical = match?.groupValues?.get(1) ?: layerStem
                val version = match?.groupValues?.get(2)?.toIntOrNull() ?: -1
                if (logical == suffix) Triple(file, version, file.lastModified()) else null
            }
            // Prefer the newest format-named candidate within a state. Across
            // states the coverage check above, rather than directory order,
            // decides which file answers the coordinate.
            .sortedWith(
                compareByDescending<Triple<File, Int, Long>> { it.second }
                    .thenByDescending { it.third }
                    .thenByDescending { it.first.length() }
                    .thenBy { it.first.name },
            )
            .map { it.first }
            .sortedByDescending { it.name.substringBefore('.') == preferredState }
            .toList()

        internal fun cameraMapFeature(camera: CameraInfo) = MapFeature(
            listOf(GeoPoint(camera.location.lat, camera.location.lon)),
            "camera:${camera.cameraType}",
            camera.name ?: camera.operator ?: camera.deviceType,
        )
    }
}
