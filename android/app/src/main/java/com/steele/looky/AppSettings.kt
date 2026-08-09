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

    var avoidHighways: Boolean
        get() = prefs.getBoolean("avoid_highways", false)
        set(value) = prefs.edit().putBoolean("avoid_highways", value).apply()

    var avoidIntersections: Boolean
        get() = prefs.getBoolean("avoid_intersections", false)
        set(value) = prefs.edit().putBoolean("avoid_intersections", value).apply()

    var gpsIntervalSeconds: Int
        get() = prefs.getInt("gps_interval_seconds", 7).coerceIn(3, 60)
        set(value) = prefs.edit().putInt("gps_interval_seconds", value.coerceIn(3, 60)).apply()

    var accelerometerRateHz: Int
        get() = prefs.getInt("accelerometer_rate_hz", 50).coerceIn(10, 100)
        set(value) = prefs.edit().putInt("accelerometer_rate_hz", value.coerceIn(10, 100)).apply()
}
