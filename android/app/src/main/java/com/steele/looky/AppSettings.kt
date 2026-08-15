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
