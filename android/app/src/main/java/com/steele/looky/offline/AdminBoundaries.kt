package com.steele.looky.offline

import android.content.Context
import com.steele.looky.model.GeoPoint
import com.steele.looky.model.MapFeature

/**
 * County lines, drawn when the map is too far out to show streets.
 *
 * `US.admin.ptiles` answers "which county is this point in" and carries no
 * geometry at all, so it cannot draw a boundary. These are the US Census
 * cartographic county boundaries (1:20m, public domain), thinned hard --
 * they exist to say "you are leaving Madison County" at a glance, not to
 * survey a line.
 */
object AdminBoundaries {
    private const val ASSET = "us_county_bounds.txt"

    /** Only worth drawing once streets have dropped out of the picture. */
    const val COUNTY_LINES_BELOW = 0.9f

    private class Ring(val lat: IntArray, val lon: IntArray) {
        val minLat = lat.min()
        val maxLat = lat.max()
        val minLon = lon.min()
        val maxLon = lon.max()
    }

    @Volatile private var rings: List<Ring>? = null

    /**
     * County lines overlapping a bounding box, as map features.
     *
     * A bbox filter on precomputed extents, so a viewport costs a scan of
     * 3,300 integers rather than any decoding.
     */
    fun linesWithin(
        context: Context,
        minLat: Double,
        minLon: Double,
        maxLat: Double,
        maxLon: Double,
    ): List<MapFeature> {
        val loLat = Math.round(minLat * 1000).toInt()
        val hiLat = Math.round(maxLat * 1000).toInt()
        val loLon = Math.round(minLon * 1000).toInt()
        val hiLon = Math.round(maxLon * 1000).toInt()
        return load(context)
            .filter { it.minLat <= hiLat && it.maxLat >= loLat && it.minLon <= hiLon && it.maxLon >= loLon }
            .map { ring ->
                MapFeature(
                    ring.lat.indices.map { GeoPoint(ring.lat[it] / 1000.0, ring.lon[it] / 1000.0) },
                    "admin_county",
                    null,
                )
            }
    }

    private fun load(context: Context): List<Ring> {
        rings?.let { return it }
        synchronized(this) {
            rings?.let { return it }
            val parsed = runCatching {
                context.assets.open(ASSET).bufferedReader().useLines { lines ->
                    lines.mapNotNull(::parseRing).toList()
                }
            }.getOrDefault(emptyList())
            rings = parsed
            return parsed
        }
    }

    private fun parseRing(line: String): Ring? {
        val points = line.split(' ')
        if (points.size < 3) return null
        val lat = IntArray(points.size)
        val lon = IntArray(points.size)
        points.forEachIndexed { index, point ->
            val comma = point.indexOf(',')
            if (comma <= 0) return null
            lat[index] = point.substring(0, comma).toIntOrNull() ?: return null
            lon[index] = point.substring(comma + 1).toIntOrNull() ?: return null
        }
        return Ring(lat, lon)
    }
}
