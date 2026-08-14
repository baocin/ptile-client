package com.steele.looky.location

import android.hardware.Sensor
import android.hardware.SensorEvent
import android.hardware.SensorEventListener
import android.hardware.SensorManager
import android.location.Location
import android.os.Handler
import android.os.HandlerThread
import android.os.Process
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
import kotlin.math.roundToInt

data class MotionResult(val movement: String, val confidence: Double, val accel: AccelStats)

/**
 * Delivered sample rate over a window, or null before the window closes.
 *
 * The registered rate is a hint the OS is free to ignore, and the phone that
 * prompted this was delivering a fraction of the 50 Hz it asked for. Only a
 * count of what actually arrived can say so, which is why this is measured
 * rather than reported from the setting.
 */
internal fun measuredHz(samples: Int, spanMs: Long): Double? =
    if (spanMs < MotionEngine.RATE_WINDOW_MS) null else samples * 1_000.0 / spanMs

class MotionEngine(
    private val sensorManager: SensorManager,
    private val ptiles: PtilesRepository,
) : SensorEventListener {
    companion object {
        /** Long enough that one late batch does not read as a collapsed rate. */
        internal const val RATE_WINDOW_MS = 2_000L
    }

    @Volatile
    private var session = AdaptiveMotionSession(defaultAdaptiveMotionConfig(), defaultSamplingCapabilities())
    private val lock = Any()
    private val x = ArrayList<Float>(256)
    private val y = ArrayList<Float>(256)
    private val z = ArrayList<Float>(256)

    private var running = false
    private var accelerometerRateHz = 50

    /**
     * Sensor delivery gets its own looper.
     *
     * The three-argument `registerListener` dispatches on the main looper, so
     * every accelerometer event queued behind whatever Compose was doing --
     * and on a real phone that is the vector map redrawing. The event queue
     * between the sensor HAL and the looper is small and fixed: when it is not
     * drained in time the driver drops samples, so a busy UI thread does not
     * delay the samples, it destroys them. This thread does nothing but append
     * three floats to a list.
     */
    private var sensorThread: HandlerThread? = null

    /** Wall clock of the newest sample, for the diagnostics staleness check. */
    @Volatile
    var lastSampleAtMs = 0L
        private set

    /** Samples per second actually delivered, or null until the first window closes. */
    @Volatile
    var deliveredRateHz: Double? = null
        private set

    private var windowSamples = 0
    private var windowStartedAtMs = 0L

    fun start(mode: LookyMode, rateHz: Int = accelerometerRateHz) {
        accelerometerRateHz = rateHz.coerceIn(10, 100)
        session.setIntent(
            if (mode == LookyMode.DRIVE) SamplingIntent.NAVIGATION else SamplingIntent.TRACKING,
            SystemClock.elapsedRealtime().toULong(),
        )
        register()
        running = true
    }

    fun setAccelerometerRate(rateHz: Int) {
        accelerometerRateHz = rateHz.coerceIn(10, 100)
        if (running) {
            sensorManager.unregisterListener(this)
            register()
        }
    }

    private fun register() {
        val sensor = sensorManager.getDefaultSensor(Sensor.TYPE_ACCELEROMETER) ?: return
        val handler = Handler(
            (sensorThread ?: HandlerThread("looky-accel", Process.THREAD_PRIORITY_FOREGROUND)
                .also { it.start(); sensorThread = it }).looper
        )
        synchronized(lock) {
            deliveredRateHz = null
            windowSamples = 0
            windowStartedAtMs = 0L
        }
        // The delay is in microseconds and is a hint, not a contract: the OS
        // rounds it to a rate the hardware supports and may return less under
        // power management. maxReportLatencyUs stays 0 -- batching would trade
        // the 1 Hz classifier's freshness for battery it does not need while a
        // wake lock is already held.
        sensorManager.registerListener(this, sensor, 1_000_000 / accelerometerRateHz, 0, handler)
    }

    fun setMode(mode: LookyMode) {
        session.setIntent(
            if (mode == LookyMode.DRIVE) SamplingIntent.NAVIGATION else SamplingIntent.TRACKING,
            SystemClock.elapsedRealtime().toULong(),
        )
    }

    /**
     * Start classification over for a new session.
     *
     * Clearing the accelerometer window is not enough: the PTiles session
     * debounces its own verdict, so "Driving" survived the end of the drive
     * and sat in the badge until real movement changed it. The session is
     * replaced, not just cleared, because the debounce is its state.
     */
    fun reset(mode: LookyMode) {
        synchronized(lock) {
            x.clear()
            y.clear()
            z.clear()
        }
        val fresh = AdaptiveMotionSession(defaultAdaptiveMotionConfig(), defaultSamplingCapabilities())
        fresh.setIntent(intentFor(mode), SystemClock.elapsedRealtime().toULong())
        val previous = session
        session = fresh
        runCatching { previous.close() }
    }

    private fun intentFor(mode: LookyMode) =
        if (mode == LookyMode.DRIVE) SamplingIntent.NAVIGATION else SamplingIntent.TRACKING

    fun stop() {
        running = false
        sensorManager.unregisterListener(this)
        sensorThread?.quitSafely()
        sensorThread = null
        deliveredRateHz = null
        session.close()
    }

    fun classify(location: Location): MotionResult {
        val stats = synchronized(lock) {
            val result = if (x.size >= 3) {
                // The measured rate, not the requested one. The native side
                // divides by this to get both the window duration and the
                // cadence (frequency = rate / best autocorrelation lag), so
                // declaring 50 Hz over samples that arrived at 10 stretches a
                // walk's cadence out of the 0.5-4 Hz band it is looked for in
                // and the gait disappears.
                val rate = (deliveredRateHz ?: accelerometerRateHz.toDouble())
                    .roundToInt().coerceIn(1, 1_000)
                accelStatsFromSamples(x.toList(), y.toList(), z.toList(), rate.toUInt())
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
        val now = System.currentTimeMillis()
        lastSampleAtMs = now
        synchronized(lock) {
            if (windowStartedAtMs == 0L) windowStartedAtMs = now
            windowSamples++
            measuredHz(windowSamples, now - windowStartedAtMs)?.let {
                deliveredRateHz = it
                windowSamples = 0
                windowStartedAtMs = now
            }
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
