package com.steele.looky.model

import android.location.Location
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow

enum class LookyMode { DRIVE, TRAIL }

data class GeoPoint(val lat: Double, val lon: Double)

data class MapFeature(
    val points: List<GeoPoint>,
    val kind: String,
    val name: String? = null,
)

data class LiveTraceState(
    val running: Boolean = false,
    val mode: LookyMode = LookyMode.DRIVE,
    /** Which log is being written: drive, trail, or background. */
    val session: String = "background",
    val movement: String = "Unknown",
    val confidence: Double = 0.0,
    val location: Location? = null,
    val pointsToday: Int = 0,
    val distanceM: Double = 0.0,
    val traceFile: String? = null,
    val recentPoints: List<GeoPoint> = emptyList(),
    val error: String? = null,
    /** Wall clock of the last GPS fix and accelerometer sample; 0 means none yet. */
    val lastFixAtMs: Long = 0L,
    val lastAccelAtMs: Long = 0L,
    /**
     * Accelerometer samples per second actually delivered; null until measured.
     *
     * The configured rate is only a request, so the two are shown separately
     * rather than the setting being presented as fact.
     */
    val accelHz: Double? = null,
    val fixes: List<MotionFix> = emptyList(),
) {
    /** No fix yet this session -- every position-derived number is a placeholder. */
    val awaitingFix: Boolean get() = location == null
}

/**
 * Where the map looks while there is no fix.
 *
 * Jackson, Tennessee: the pack the first build shipped with. It is a real
 * place and nothing to do with the user, so anything drawn against it has to
 * say so rather than let a distance read as a distance from here.
 */
val FALLBACK_ANCHOR = GeoPoint(35.73377, -88.03220)

/** A GPS fix reduced to the fields the diagnostics panel averages. */
data class MotionFix(
    val atMs: Long,
    val speedMps: Double?,
    val headingDeg: Double?,
    val accuracyM: Double?,
    /** What the classifier called this fix, so a window can say how it was travelled. */
    val movement: String? = null,
)

/**
 * Averaging windows in seconds.
 *
 * Fibonacci because the interesting resolutions are dense near now -- where a
 * classification flips -- and coarse further back, where only the trend
 * matters. The last one is the retention bound for [appendFix].
 */
val SPEED_WINDOWS_S = listOf(1, 2, 3, 5, 8, 13, 21, 34, 55, 89, 144, 233, 377, 610)

/**
 * Hard ceiling on retained fixes.
 *
 * Age alone bounds the buffer at any sane polling rate; this is the guard for
 * a provider that decides to deliver a burst.
 */
const val MOTION_HISTORY_CAP = 1_024

/** Append a fix, dropping anything older than the widest window. */
fun appendFix(history: List<MotionFix>, fix: MotionFix): List<MotionFix> {
    val oldest = fix.atMs - SPEED_WINDOWS_S.last() * 1_000L
    return (history + fix).filter { it.atMs >= oldest }.takeLast(MOTION_HISTORY_CAP)
}

/** [samples] is shown alongside [meanMps] so a two-fix average is not read as settled. */
/**
 * [movement] is what the classifier called most of the window.
 *
 * A speed without its verdict is ambiguous -- 1.4 m/s is a brisk walk or a car
 * in a car park -- and the two columns disagreeing is itself the diagnostic
 * when the classifier is being starved of samples.
 */
data class SpeedWindow(
    val seconds: Int,
    val samples: Int,
    val meanMps: Double?,
    val movement: String? = null,
)

fun speedWindows(history: List<MotionFix>, nowMs: Long): List<SpeedWindow> =
    SPEED_WINDOWS_S.map { seconds ->
        val inWindow = history.filter { it.atMs >= nowMs - seconds * 1_000L }
        val speeds = inWindow.mapNotNull { it.speedMps }
        SpeedWindow(
            seconds = seconds,
            samples = speeds.size,
            meanMps = if (speeds.isEmpty()) null else speeds.average(),
            // The commonest verdict, not the latest: one stray classification
            // in a minute of driving should not relabel the minute.
            movement = inWindow.mapNotNull { it.movement }
                .groupingBy { it }
                .eachCount()
                .maxByOrNull { it.value }
                ?.key,
        )
    }

/** Ages of -1 mean the input has produced nothing at all this session. */
data class MotionStaleness(
    val gpsAgeMs: Long,
    val gpsLimitMs: Long,
    val gpsStale: Boolean,
    val accelAgeMs: Long,
    val accelLimitMs: Long,
    val accelStale: Boolean,
) {
    val any: Boolean get() = gpsStale || accelStale
}

/**
 * Whether either motion input has gone quiet.
 *
 * GPS gets three polling intervals: one skipped fix is normal, three is the
 * provider having stopped. The accelerometer gets a hundred sample periods
 * with a floor, because sensor batching legitimately delivers in bursts and a
 * strict per-period deadline would flag every healthy phone.
 */
fun motionStaleness(
    nowMs: Long,
    lastFixAtMs: Long,
    lastAccelAtMs: Long,
    gpsIntervalSeconds: Int,
    accelRateHz: Int,
): MotionStaleness {
    val gpsLimit = gpsIntervalSeconds.coerceAtLeast(1) * 3_000L
    val accelLimit = (100_000L / accelRateHz.coerceAtLeast(1)).coerceAtLeast(1_500L)
    // The caller's clock ticks once a second while the bus updates faster, so
    // a reading can legitimately be newer than "now". Negative is reserved for
    // an input that has never reported.
    val gpsAge = if (lastFixAtMs <= 0L) -1L else (nowMs - lastFixAtMs).coerceAtLeast(0L)
    val accelAge = if (lastAccelAtMs <= 0L) -1L else (nowMs - lastAccelAtMs).coerceAtLeast(0L)
    return MotionStaleness(
        gpsAgeMs = gpsAge,
        gpsLimitMs = gpsLimit,
        gpsStale = gpsAge < 0L || gpsAge > gpsLimit,
        accelAgeMs = accelAge,
        accelLimitMs = accelLimit,
        accelStale = accelAge < 0L || accelAge > accelLimit,
    )
}

object TraceBus {
    private val mutable = MutableStateFlow(LiveTraceState())
    val state = mutable.asStateFlow()

    fun update(transform: (LiveTraceState) -> LiveTraceState) {
        mutable.value = transform(mutable.value)
    }
}
