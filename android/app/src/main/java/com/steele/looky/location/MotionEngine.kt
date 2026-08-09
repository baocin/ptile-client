package com.steele.looky.location

import android.hardware.Sensor
import android.hardware.SensorEvent
import android.hardware.SensorEventListener
import android.hardware.SensorManager
import android.location.Location
import android.os.SystemClock
import com.steele.looky.model.LookyMode
import com.steele.looky.offline.PtilesRepository
import uniffi.ptiles_ffi.AccelStats
import uniffi.ptiles_ffi.AdaptiveMotionSession
import uniffi.ptiles_ffi.LocationSample
import uniffi.ptiles_ffi.MotionObservation
import uniffi.ptiles_ffi.SamplingIntent
import uniffi.ptiles_ffi.accelStatsFromSamples
import uniffi.ptiles_ffi.defaultAdaptiveMotionConfig
import uniffi.ptiles_ffi.defaultSamplingCapabilities

data class MotionResult(val movement: String, val confidence: Double, val accel: AccelStats)

class MotionEngine(
    private val sensorManager: SensorManager,
    private val ptiles: PtilesRepository,
) : SensorEventListener {
    private val session = AdaptiveMotionSession(defaultAdaptiveMotionConfig(), defaultSamplingCapabilities())
    private val lock = Any()
    private val x = ArrayList<Float>(256)
    private val y = ArrayList<Float>(256)
    private val z = ArrayList<Float>(256)

    private var running = false
    private var accelerometerRateHz = 50

    fun start(mode: LookyMode, rateHz: Int = accelerometerRateHz) {
        accelerometerRateHz = rateHz.coerceIn(10, 100)
        session.setIntent(
            if (mode == LookyMode.DRIVE) SamplingIntent.NAVIGATION else SamplingIntent.TRACKING,
            SystemClock.elapsedRealtime().toULong(),
        )
        sensorManager.getDefaultSensor(Sensor.TYPE_ACCELEROMETER)?.let {
            sensorManager.registerListener(this, it, 1_000_000 / accelerometerRateHz)
        }
        running = true
    }

    fun setAccelerometerRate(rateHz: Int) {
        accelerometerRateHz = rateHz.coerceIn(10, 100)
        if (running) {
            sensorManager.unregisterListener(this)
            sensorManager.getDefaultSensor(Sensor.TYPE_ACCELEROMETER)?.let {
                sensorManager.registerListener(this, it, 1_000_000 / accelerometerRateHz)
            }
        }
    }

    fun setMode(mode: LookyMode) {
        session.setIntent(
            if (mode == LookyMode.DRIVE) SamplingIntent.NAVIGATION else SamplingIntent.TRACKING,
            SystemClock.elapsedRealtime().toULong(),
        )
    }

    fun stop() {
        running = false
        sensorManager.unregisterListener(this)
        session.close()
    }

    fun classify(location: Location): MotionResult {
        val stats = synchronized(lock) {
            val result = if (x.size >= 3) {
                accelStatsFromSamples(
                    x.toList(),
                    y.toList(),
                    z.toList(),
                    accelerometerRateHz.toUInt(),
                )
            } else {
                AccelStats(0.0, null, 0.0, 0u, null)
            }
            // Keep the most recent second rather than clearing outright. The
            // classifier now runs every second, and a full clear left the next
            // window starved of samples -- under three of them the stats above
            // collapse to zeroes, which reads as stationary.
            trimToWindow()
            result
        }
        val road = ptiles.nearbyRoadContext(location.latitude, location.longitude).first
        val update = session.observe(
            MotionObservation(
                tMs = SystemClock.elapsedRealtime().toULong(),
                location = LocationSample(
                    lat = location.latitude,
                    lon = location.longitude,
                    horizontalAccuracyM = if (location.hasAccuracy()) location.accuracy.toDouble() else null,
                    speedMps = if (location.hasSpeed()) location.speed.toDouble() else null,
                    bearingDegrees = if (location.hasBearing()) location.bearing.toDouble() else null,
                ),
                accelerometer = stats,
                road = road,
                trafficControl = null,
            )
        )
        return MotionResult(
            update.movement.name.lowercase().replaceFirstChar(Char::uppercase),
            update.vote.confidence,
            stats,
        )
    }

    /** Drop all but the newest second of samples. Caller holds [lock]. */
    private fun trimToWindow() {
        val keep = accelerometerRateHz.coerceIn(10, 100)
        while (x.size > keep) {
            x.removeAt(0); y.removeAt(0); z.removeAt(0)
        }
    }

    override fun onSensorChanged(event: SensorEvent) {
        if (event.sensor.type != Sensor.TYPE_ACCELEROMETER) return
        synchronized(lock) {
            if (x.size >= 300) {
                x.removeAt(0); y.removeAt(0); z.removeAt(0)
            }
            x += event.values[0]
            y += event.values[1]
            z += event.values[2]
        }
    }

    override fun onAccuracyChanged(sensor: Sensor?, accuracy: Int) = Unit
}
