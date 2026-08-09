package com.steele.looky.offline

import android.content.Context
import com.steele.looky.model.GeoPoint
import com.steele.looky.model.MapFeature
import uniffi.ptiles_ffi.PtilesLayer
import uniffi.ptiles_ffi.PtilesStack
import uniffi.ptiles_ffi.AdminLayer
import uniffi.ptiles_ffi.CameraInfo
import uniffi.ptiles_ffi.OfflineRouteMode
import uniffi.ptiles_ffi.RoadContext
import uniffi.ptiles_ffi.LatLon
import java.io.File
import java.util.concurrent.ConcurrentHashMap

data class NearbyContext(
    val roadName: String?,
    val roadClass: String?,
    val roadDistanceM: Double?,
)

class PtilesRepository(context: Context) {
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
     * Every renderable feature within `ring` H3 rings of the coordinate.
     *
     * `ring` is the H3 k-ring radius handed to the PTiles queries: 1 is 7
     * res-7 cells, 2 is 19. Ring 1 left visible blank paper at the default
     * 0.075-degree viewport and immediately when panned, which is why the
     * default is 2.
     *
     * ponytail: ring 2 is ~2.7x the decode work of ring 1. If first paint
     * drags on a real device, turn this default down rather than adding a
     * tile cache.
     */
    fun featuresAround(
        lat: Double,
        lon: Double,
        trails: Boolean,
        developer: Boolean = false,
        ring: UByte = DEFAULT_RING,
    ): List<MapFeature> {
        val out = mutableListOf<MapFeature>()
        runCatching {
            layer("roads", lat, lon)?.roads(lat, lon, ring)?.forEach { road ->
                out += MapFeature(
                    road.geometry.map { GeoPoint(it.lat, it.lon) },
                    road.roadClass,
                    road.name,
                )
            }
        }
        runCatching {
            layer("water", lat, lon)?.water(lat, lon, ring)?.forEach { water ->
                if (water.geometry.size > 1) out += MapFeature(water.geometry.map { GeoPoint(it.lat, it.lon) }, "water", water.name)
            }
        }
        runCatching {
            layer("parks", lat, lon)?.parks(lat, lon, ring)?.forEach { park ->
                if (park.geometry.size > 1) out += MapFeature(park.geometry.map { GeoPoint(it.lat, it.lon) }, "park", park.name)
            }
        }
        // Buildings are point centroids in the PTiles API. Sample the visible
        // viewport so the drive map still communicates the built environment.
        // The grid widens with the ring so a panned view is not ringed by
        // roads with no buildings between them.
        runCatching {
            val buildingLayer = layer("buildings", lat, lon)
            if (buildingLayer != null) {
                val span = buildingSampleSpan(ring)
                val sample = (-span..span).flatMap { y -> (-span..span).map { x -> LatLon(lat + y * 0.003, lon + x * 0.004) } }
                buildingLayer.buildingsAt(sample).filterNotNull().forEach { building ->
                    out += MapFeature(listOf(GeoPoint(building.centroid.lat, building.centroid.lon)), "building", building.name)
                }
            }
        }
        if (trails) runCatching {
            layer("trails", lat, lon)?.trails(lat, lon, ring)?.filter { !it.isTrailhead }?.forEach { trail ->
                out += MapFeature(
                    trail.geometry.map { GeoPoint(it.lat, it.lon) },
                    "trail:${trail.trailType}",
                    trail.name,
                )
            }
        }
        // Camera is a national layer, so the normal state-aware selection
        // falls through to US.camera.ptiles. It is rendered in both Drive and
        // Trail whenever installed, not hidden behind developer mode.
        runCatching {
            layer("camera", lat, lon)?.cameras(lat, lon, ring)?.forEach { camera ->
                out += cameraMapFeature(camera)
            }
        }
        if (developer) {
            runCatching {
                layer("rail", lat, lon)?.rail(lat, lon, ring)?.forEach { rail ->
                    if (rail.geometry.isNotEmpty()) {
                        out += MapFeature(
                            rail.geometry.map { GeoPoint(it.lat, it.lon) },
                            if (rail.geomType == 1.toUByte()) "station" else "rail:${rail.railType}",
                            rail.name,
                        )
                    }
                }
            }
            runCatching {
                layer("business", lat, lon)?.businessesNear(lat, lon, ring, 1_500.0)?.forEach { business ->
                    out += MapFeature(
                        listOf(GeoPoint(business.location.lat, business.location.lon)),
                        "business:${business.categoryIdx}",
                        business.name,
                    )
                }
            }
        }
        return out
    }

    /**
     * Business-name search across every installed state's name index.
     *
     * Deliberately not routed through [layer]: that filters candidates on
     * `covers(lat, lon)`, and `{STATE}.business_name_index.ptiles` is keyed by
     * the first letter of the business name, not by geography. Its header bbox
     * is not a coverage claim to filter on.
     */
    fun searchBusinesses(query: String, limit: Int = SEARCH_LIMIT): List<BusinessResult> {
        if (query.isBlank() || limit <= 0) return emptyList()
        val hits = nameIndexFiles().flatMap { file ->
            runCatching {
                openCached(file)
                    ?.searchBusiness(query, limit.toUInt())
                    ?.map { BusinessResult(it.name, GeoPoint(it.location.lat, it.location.lon), it.score.toInt()) }
                    .orEmpty()
            }.getOrDefault(emptyList())
        }
        return mergeBusinessHits(hits, limit)
    }

    private fun nameIndexFiles(): List<File> = manager.packsDir.listFiles()
        .orEmpty()
        .filter { it.isFile && it.name.endsWith(".business_name_index.ptiles", ignoreCase = true) }
        .sortedBy { it.name }

    fun offlineRoute(
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
        check(route.path.isNotEmpty()) { "Offline route graph is empty: install the local roads/trails PTiles layer for this area" }
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

    /** Exact admin lookup when installed; bbox fallback for the first download. */
    fun currentStateCode(lat: Double, lon: Double): String? {
        val admin = adminLayer ?: manager.packsDir.listFiles().orEmpty()
            .firstOrNull { it.isFile && it.name == "US.admin.ptiles" }
            ?.let { runCatching { AdminLayer.open(it.absolutePath) }.getOrNull() }
            ?.also { adminLayer = it }
        val resolved = admin
            ?.let { runCatching { it.adminAt(lat, lon)?.state }.getOrNull() }
            ?.let(StateResolver::codeForName)
            ?: StateResolver.stateAt(lat, lon, stateHint)
        if (resolved != null) stateHint = resolved
        return resolved
    }

    companion object {
        private val VERSIONED_STEM = Regex("^(.+)_v(\\d+)$")

        /** H3 k-ring radius for map queries. See [featuresAround]. */
        internal val DEFAULT_RING: UByte = 2u
        internal const val SEARCH_LIMIT = 20

        /** Half-width of the building sample grid for an H3 ring radius. */
        internal fun buildingSampleSpan(ring: UByte): Int = (ring.toInt() + 1).coerceIn(2, 5)

        /**
         * Rank and de-duplicate hits gathered from several state indexes.
         *
         * Score is the FFI's match quality (2 exact, 1 prefix, 0 substring),
         * so it sorts descending and name breaks ties. Two states can hold the
         * same chain at the same spot only when their bounding boxes overlap at
         * a border, so identity is name plus coordinate rounded to 5 decimal
         * places -- about a metre, well under the spacing of two real stores.
         */
        internal fun mergeBusinessHits(hits: List<BusinessResult>, limit: Int): List<BusinessResult> = hits
            .sortedWith(compareByDescending<BusinessResult> { it.score }.thenBy { it.name })
            .distinctBy { Triple(it.name, round5(it.point.lat), round5(it.point.lon)) }
            .take(limit)

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
