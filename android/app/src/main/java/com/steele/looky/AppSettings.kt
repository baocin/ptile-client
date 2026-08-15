package com.steele.looky

import android.content.Context
import com.steele.looky.model.LookyMode

class AppSettings(context: Context) {
    private val prefs = context.getSharedPreferences("looky.settings", Context.MODE_PRIVATE)

    var onboardingComplete: Boolean
        get() = prefs.getBoolean("onboarding_complete", false)
        set(value) = prefs.edit().putBoolean("onboarding_complete", value).apply()

    var developerMapEnabled: Boolean
        get() = prefs.getBoolean("developer_map", true)
        set(value) = prefs.edit().putBoolean("developer_map", value).apply()

    var continuousRecording: Boolean
        get() = prefs.getBoolean("continuous_recording", true)
        set(value) = prefs.edit().putBoolean("continuous_recording", value).apply()

    var activeMode: LookyMode
        get() = runCatching {
            LookyMode.valueOf(prefs.getString("active_mode", LookyMode.DRIVE.name)!!)
        }.getOrDefault(LookyMode.DRIVE)
        set(value) = prefs.edit().putString("active_mode", value.name).apply()

    /** Feet and miles. Default on: this ships to a US-only pack set. */
    var imperialUnits: Boolean
        get() = prefs.getBoolean("imperial_units", true)
        set(value) = prefs.edit().putBoolean("imperial_units", value).apply()

    /**
     * The last coordinate a fix reported, across launches.
     *
     * "Downloads Needed" was decided from the recording service's live fix, so
     * a fresh launch with every pack installed still accused the user of having
     * no maps until the first fix landed. Where they were last is a far better
     * guess than a hardcoded coordinate in Tennessee.
     */
    var lastFix: Pair<Double, Double>?
        get() {
            val lat = prefs.getFloat("last_fix_lat", Float.NaN)
            val lon = prefs.getFloat("last_fix_lon", Float.NaN)
            return if (lat.isNaN() || lon.isNaN()) null else lat.toDouble() to lon.toDouble()
        }
        set(value) {
            if (value == null) return
            prefs.edit()
                .putFloat("last_fix_lat", value.first.toFloat())
                .putFloat("last_fix_lon", value.second.toFloat())
                .apply()
        }

    /** Cheap enough to call per fix: a float pair through `apply()`. */
    fun rememberLastFix(lat: Double, lon: Double) {
        lastFix = lat to lon
    }

    /**
     * The destination chain of a journey in progress, across launches.
     *
     * Navigation outlives the screen showing it: the recorder keeps writing
     * from a foreground service whether or not the activity exists, so an app
     * killed in a pocket and reopened at a junction must come back to the same
     * route rather than to an empty search box. Only the stops are kept -- the
     * route, the turn list and the navigator are all derived, and recomputing
     * them from the current position is more correct than restoring a path
     * that was planned from where the phone used to be.
     *
     * Stored as `lat,lon,label` per line. A label may contain a comma, so it
     * is last and the split is bounded.
     */
    var activeJourney: List<Pair<Pair<Double, Double>, String>>
        get() = prefs.getString("active_journey", "").orEmpty()
            .lineSequence()
            .mapNotNull { line ->
                val parts = line.split(",", limit = 3)
                if (parts.size < 3) return@mapNotNull null
                val lat = parts[0].toDoubleOrNull() ?: return@mapNotNull null
                val lon = parts[1].toDoubleOrNull() ?: return@mapNotNull null
                (lat to lon) to parts[2]
            }
            .toList()
        set(value) = prefs.edit()
            .putString(
                "active_journey",
                value.joinToString("\n") { (at, label) ->
                    // A newline in a name would split one stop into two, and
                    // the label is free text from the layer.
                    "${at.first},${at.second},${label.replace('\n', ' ')}"
                },
            )
            .apply()

    var avoidHighways: Boolean
        get() = prefs.getBoolean("avoid_highways", false)
        set(value) = prefs.edit().putBoolean("avoid_highways", value).apply()

    var avoidIntersections: Boolean
        get() = prefs.getBoolean("avoid_intersections", false)
        set(value) = prefs.edit().putBoolean("avoid_intersections", value).apply()

    var gpsIntervalSeconds: Int
        get() = prefs.getInt("gps_interval_seconds", 3).coerceIn(3, 60)
        set(value) = prefs.edit().putInt("gps_interval_seconds", value.coerceIn(3, 60)).apply()

    var accelerometerRateHz: Int
        get() = prefs.getInt("accelerometer_rate_hz", 50).coerceIn(10, 100)
        set(value) = prefs.edit().putInt("accelerometer_rate_hz", value.coerceIn(10, 100)).apply()
}
