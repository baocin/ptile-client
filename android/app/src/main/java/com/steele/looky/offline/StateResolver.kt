package com.steele.looky.offline

import kotlin.math.pow

/** Lightweight offline state lookup used before US.admin.ptiles is installed. */
object StateResolver {
    private data class Bounds(
        val minLat: Double,
        val minLon: Double,
        val maxLat: Double,
        val maxLon: Double,
    ) {
        val centerLat = (minLat + maxLat) / 2.0
        val centerLon = (minLon + maxLon) / 2.0
        fun contains(lat: Double, lon: Double) =
            lat in minLat..maxLat && lon in minLon..maxLon
    }

    // The same deliberately rough offline boxes used by web-demo. Once the
    // national admin layer is installed PtilesRepository uses that exact
    // lookup first; these boxes make the very first state download possible.
    private val bounds = mapOf(
        "AL" to Bounds(30.1, -88.5, 35.1, -84.8), "AK" to Bounds(51.0, -180.0, 72.0, -129.0),
        "AZ" to Bounds(31.3, -114.9, 37.1, -109.0), "AR" to Bounds(33.0, -94.7, 36.6, -89.6),
        "CA" to Bounds(32.5, -124.5, 42.1, -114.1), "CO" to Bounds(36.9, -109.1, 41.1, -102.0),
        "CT" to Bounds(40.9, -73.8, 42.1, -71.7), "DC" to Bounds(38.7, -77.2, 39.0, -76.9),
        "DE" to Bounds(38.4, -75.8, 39.9, -75.0), "FL" to Bounds(24.4, -87.7, 31.1, -79.9),
        "GA" to Bounds(30.3, -85.7, 35.1, -80.7), "HI" to Bounds(18.8, -160.3, 22.3, -154.7),
        "ID" to Bounds(41.9, -117.3, 49.1, -111.0), "IL" to Bounds(36.9, -91.6, 42.6, -87.4),
        "IN" to Bounds(37.7, -88.1, 41.8, -84.7), "IA" to Bounds(40.3, -96.7, 43.6, -90.1),
        "KS" to Bounds(36.9, -102.1, 40.1, -94.5), "KY" to Bounds(36.4, -89.6, 39.2, -81.9),
        "LA" to Bounds(28.8, -94.1, 33.1, -88.7), "ME" to Bounds(43.0, -71.2, 47.5, -66.8),
        "MD" to Bounds(37.8, -79.5, 39.8, -75.0), "MA" to Bounds(41.2, -73.6, 42.9, -69.8),
        "MI" to Bounds(41.6, -90.5, 48.4, -82.1), "MN" to Bounds(43.4, -97.3, 49.4, -89.4),
        "MS" to Bounds(30.0, -91.7, 35.1, -88.0), "MO" to Bounds(35.9, -95.8, 40.7, -89.0),
        "MT" to Bounds(44.3, -116.1, 49.1, -104.0), "NE" to Bounds(39.9, -104.1, 43.1, -95.3),
        "NV" to Bounds(35.0, -120.1, 42.1, -114.0), "NH" to Bounds(42.6, -72.6, 45.4, -70.5),
        "NJ" to Bounds(38.8, -75.6, 41.4, -73.8), "NM" to Bounds(31.3, -109.1, 37.1, -103.0),
        "NY" to Bounds(40.4, -79.8, 45.1, -71.7), "NC" to Bounds(33.7, -84.4, 36.7, -75.3),
        "ND" to Bounds(45.9, -104.1, 49.1, -96.5), "OH" to Bounds(38.3, -84.9, 42.0, -80.4),
        "OK" to Bounds(33.5, -103.1, 37.1, -94.3), "OR" to Bounds(41.9, -124.7, 46.3, -116.4),
        "PA" to Bounds(39.6, -80.6, 42.3, -74.6), "RI" to Bounds(41.1, -71.9, 42.1, -71.1),
        "SC" to Bounds(32.0, -83.4, 35.3, -78.4), "SD" to Bounds(42.4, -104.1, 46.0, -96.4),
        "TN" to Bounds(34.9, -90.4, 36.7, -81.6), "TX" to Bounds(25.7, -106.7, 36.6, -93.4),
        "UT" to Bounds(36.9, -114.1, 42.1, -109.0), "VT" to Bounds(42.7, -73.5, 45.1, -71.4),
        "VA" to Bounds(36.5, -83.7, 39.5, -75.1), "WA" to Bounds(45.5, -124.9, 49.1, -116.9),
        "WV" to Bounds(37.1, -82.7, 40.7, -77.6), "WI" to Bounds(42.4, -92.9, 47.1, -86.2),
        "WY" to Bounds(40.9, -111.1, 45.1, -104.0),
    )

    val names = mapOf(
        "AL" to "Alabama", "AK" to "Alaska", "AZ" to "Arizona", "AR" to "Arkansas",
        "CA" to "California", "CO" to "Colorado", "CT" to "Connecticut", "DC" to "District of Columbia",
        "DE" to "Delaware", "FL" to "Florida", "GA" to "Georgia", "HI" to "Hawaii",
        "ID" to "Idaho", "IL" to "Illinois", "IN" to "Indiana", "IA" to "Iowa",
        "KS" to "Kansas", "KY" to "Kentucky", "LA" to "Louisiana", "ME" to "Maine",
        "MD" to "Maryland", "MA" to "Massachusetts", "MI" to "Michigan", "MN" to "Minnesota",
        "MS" to "Mississippi", "MO" to "Missouri", "MT" to "Montana", "NE" to "Nebraska",
        "NV" to "Nevada", "NH" to "New Hampshire", "NJ" to "New Jersey", "NM" to "New Mexico",
        "NY" to "New York", "NC" to "North Carolina", "ND" to "North Dakota", "OH" to "Ohio",
        "OK" to "Oklahoma", "OR" to "Oregon", "PA" to "Pennsylvania", "RI" to "Rhode Island",
        "SC" to "South Carolina", "SD" to "South Dakota", "TN" to "Tennessee", "TX" to "Texas",
        "UT" to "Utah", "VT" to "Vermont", "VA" to "Virginia", "WA" to "Washington",
        "WV" to "West Virginia", "WI" to "Wisconsin", "WY" to "Wyoming",
    )
    private val codesByName = names.entries.associate { (code, name) -> name.lowercase() to code }

    fun codeForName(name: String): String? = codesByName[name.trim().lowercase()]
        ?: if (name.length == 2) name.uppercase().takeIf(bounds::containsKey) else null

    fun name(code: String?): String? = code?.let(names::get)

    fun stateAt(lat: Double, lon: Double, preferred: String? = null): String? {
        if (!lat.isFinite() || !lon.isFinite()) return null
        if (lat in 51.0..72.0 && lon >= 170.0) return "AK"
        val hits = bounds.filterValues { it.contains(lat, lon) }
        if (preferred in hits) return preferred
        return hits.minByOrNull { (_, box) ->
            (lat - box.centerLat).pow(2) + (lon - box.centerLon).pow(2)
        }?.key
    }
}
