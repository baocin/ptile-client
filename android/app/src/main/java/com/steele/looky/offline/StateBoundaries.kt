package com.steele.looky.offline

import android.content.Context

/**
 * Exact-enough state lookup from boundaries shipped inside the APK.
 *
 * [StateResolver]'s bounding boxes overlap badly -- a point in Chattanooga
 * sits inside the boxes for Tennessee, Georgia, and Alabama at once, and the
 * nearest-centre tie-break picks whichever box happens to be centred closer,
 * which is not a border. These are real polygons (US Census cartographic
 * boundaries, 1:20m, public domain), rounded to thousandths of a degree --
 * about 100 m, far finer than any border question this app asks.
 *
 * `US.admin.ptiles` is still preferred when installed; this is what makes the
 * answer right before any pack is downloaded, and everywhere the national
 * layer is not.
 *
 * ponytail: ray casting over 185 rings, no spatial index. The bbox prefilter
 * leaves one or two rings to scan; add an index only if this ever shows up in
 * a profile.
 */
object StateBoundaries {
    private const val ASSET = "us_state_bounds.txt"

    /** Coordinates are stored as thousandths of a degree, as ints. */
    private class Ring(val code: String, val lat: IntArray, val lon: IntArray) {
        val minLat = lat.min()
        val maxLat = lat.max()
        val minLon = lon.min()
        val maxLon = lon.max()

        fun mayContain(latE3: Int, lonE3: Int) =
            latE3 in minLat..maxLat && lonE3 in minLon..maxLon

        /** Ray casting: count crossings of a ray east from the point. */
        fun contains(latE3: Int, lonE3: Int): Boolean {
            var inside = false
            var j = lat.size - 1
            for (i in lat.indices) {
                val yi = lat[i]
                val yj = lat[j]
                if ((yi > latE3) != (yj > latE3)) {
                    val slope = (lonE3 - lon[i]).toLong() * (yj - yi) - (lon[j] - lon[i]).toLong() * (latE3 - yi)
                    if ((slope < 0) != (yj < yi)) inside = !inside
                }
                j = i
            }
            return inside
        }
    }

    @Volatile private var rings: List<Ring>? = null

    fun stateAt(context: Context, lat: Double, lon: Double): String? {
        if (!lat.isFinite() || !lon.isFinite()) return null
        val latE3 = Math.round(lat * 1000).toInt()
        val lonE3 = Math.round(lon * 1000).toInt()
        return load(context)
            .firstOrNull { it.mayContain(latE3, lonE3) && it.contains(latE3, lonE3) }
            ?.code
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

    /** `CODE lat,lon lat,lon ...`, every coordinate in thousandths. */
    private fun parseRing(line: String): Ring? {
        val space = line.indexOf(' ')
        if (space <= 0) return null
        val code = line.substring(0, space)
        val points = line.substring(space + 1).split(' ')
        if (points.size < 4) return null
        val lat = IntArray(points.size)
        val lon = IntArray(points.size)
        points.forEachIndexed { index, point ->
            val comma = point.indexOf(',')
            if (comma <= 0) return null
            lat[index] = point.substring(0, comma).toIntOrNull() ?: return null
            lon[index] = point.substring(comma + 1).toIntOrNull() ?: return null
        }
        return Ring(code, lat, lon)
    }
}
