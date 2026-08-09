package com.steele.looky.offline

import android.content.Context
import com.steele.looky.model.GeoPoint
import com.steele.looky.model.MapFeature
import uniffi.ptiles_ffi.PtilesLayer
import uniffi.ptiles_ffi.PtilesStack
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
    private val layers = ConcurrentHashMap<String, PtilesLayer>()

    data class RouteResult(
        val points: List<GeoPoint>,
        val distanceM: Double,
        val durationS: Double,
        val decodedSegments: Int,
    )

    private fun layer(suffix: String): PtilesLayer? {
        layers[suffix]?.let { return it }
        val file = manager.packsDir.listFiles()?.firstOrNull {
            val stem = it.name.removeSuffix(".ptiles").substringAfter('.', "")
            stem == suffix || stem.substringBeforeLast("_v", stem) == suffix
        } ?: return null
        return runCatching { PtilesLayer.open(file.absolutePath) }.getOrNull()?.also { layers[suffix] = it }
    }

    fun nearbyRoadContext(lat: Double, lon: Double): Pair<RoadContext?, NearbyContext> {
        val road = runCatching { layer("roads")?.nearestRoad(lat, lon) }.getOrNull()
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
            layer("trails")?.nearestTrail(point.lat, point.lon, 1u)?.let { GeoPoint(it.snapped.lat, it.snapped.lon) }
                ?: layer("roads")?.nearestRoad(point.lat, point.lon)?.let { GeoPoint(it.snappedLat, it.snappedLon) }
                ?: nearestVertex(layer("trails")?.trails(point.lat, point.lon, 1u)?.flatMap { it.geometry }, point)
        } else {
            layer("roads")?.nearestRoad(point.lat, point.lon)?.let { GeoPoint(it.snappedLat, it.snappedLon) }
                ?: nearestVertex(layer("roads")?.roads(point.lat, point.lon, 1u)?.flatMap { it.geometry }, point)
        }
    }.getOrNull()

    private fun nearestVertex(points: List<LatLon>?, target: GeoPoint): GeoPoint? = points
        ?.minByOrNull { (it.lat - target.lat) * (it.lat - target.lat) + (it.lon - target.lon) * (it.lon - target.lon) }
        ?.let { GeoPoint(it.lat, it.lon) }

    fun featuresAround(lat: Double, lon: Double, trails: Boolean): List<MapFeature> {
        val out = mutableListOf<MapFeature>()
        runCatching {
            layer("roads")?.roads(lat, lon, 1u)?.forEach { road ->
                out += MapFeature(
                    road.geometry.map { GeoPoint(it.lat, it.lon) },
                    road.roadClass,
                    road.name,
                )
            }
        }
        runCatching {
            layer("water")?.water(lat, lon, 1u)?.forEach { water ->
                if (water.geometry.size > 1) out += MapFeature(water.geometry.map { GeoPoint(it.lat, it.lon) }, "water", water.name)
            }
        }
        runCatching {
            layer("parks")?.parks(lat, lon, 1u)?.forEach { park ->
                if (park.geometry.size > 1) out += MapFeature(park.geometry.map { GeoPoint(it.lat, it.lon) }, "park", park.name)
            }
        }
        // Buildings are point centroids in the PTiles API. Sample the visible
        // viewport so the drive map still communicates the built environment.
        runCatching {
            val buildingLayer = layer("buildings")
            if (buildingLayer != null) {
                val sample = (-2..2).flatMap { y -> (-2..2).map { x -> LatLon(lat + y * 0.003, lon + x * 0.004) } }
                buildingLayer.buildingsAt(sample).filterNotNull().forEach { building ->
                    out += MapFeature(listOf(GeoPoint(building.centroid.lat, building.centroid.lon)), "building", building.name)
                }
            }
        }
        if (trails) runCatching {
            layer("trails")?.trails(lat, lon, 1u)?.filter { !it.isTrailhead }?.forEach { trail ->
                out += MapFeature(
                    trail.geometry.map { GeoPoint(it.lat, it.lon) },
                    "trail:${trail.trailType}",
                    trail.name,
                )
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
            roads = layer("roads"),
            buildings = layer("buildings"),
            business = layer("business"),
            trails = layer("trails"),
            parks = layer("parks"),
            water = layer("water"),
            camera = layer("camera"),
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
}
