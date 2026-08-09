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

    fun featuresAround(
        lat: Double,
        lon: Double,
        trails: Boolean,
        developer: Boolean = false,
    ): List<MapFeature> {
        val out = mutableListOf<MapFeature>()
        runCatching {
            layer("roads", lat, lon)?.roads(lat, lon, 1u)?.forEach { road ->
                out += MapFeature(
                    road.geometry.map { GeoPoint(it.lat, it.lon) },
                    road.roadClass,
                    road.name,
                )
            }
        }
        runCatching {
            layer("water", lat, lon)?.water(lat, lon, 1u)?.forEach { water ->
                if (water.geometry.size > 1) out += MapFeature(water.geometry.map { GeoPoint(it.lat, it.lon) }, "water", water.name)
            }
        }
        runCatching {
            layer("parks", lat, lon)?.parks(lat, lon, 1u)?.forEach { park ->
                if (park.geometry.size > 1) out += MapFeature(park.geometry.map { GeoPoint(it.lat, it.lon) }, "park", park.name)
            }
        }
        // Buildings are point centroids in the PTiles API. Sample the visible
        // viewport so the drive map still communicates the built environment.
        runCatching {
            val buildingLayer = layer("buildings", lat, lon)
            if (buildingLayer != null) {
                val sample = (-2..2).flatMap { y -> (-2..2).map { x -> LatLon(lat + y * 0.003, lon + x * 0.004) } }
                buildingLayer.buildingsAt(sample).filterNotNull().forEach { building ->
                    out += MapFeature(listOf(GeoPoint(building.centroid.lat, building.centroid.lon)), "building", building.name)
                }
            }
        }
        if (trails) runCatching {
            layer("trails", lat, lon)?.trails(lat, lon, 1u)?.filter { !it.isTrailhead }?.forEach { trail ->
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
            layer("camera", lat, lon)?.cameras(lat, lon, 1u)?.forEach { camera ->
                out += cameraMapFeature(camera)
            }
        }
        if (developer) {
            runCatching {
                layer("rail", lat, lon)?.rail(lat, lon, 1u)?.forEach { rail ->
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
                layer("business", lat, lon)?.businessesNear(lat, lon, 1u, 1_500.0)?.forEach { business ->
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
