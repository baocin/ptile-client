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
    val movement: String = "Unknown",
    val confidence: Double = 0.0,
    val location: Location? = null,
    val pointsToday: Int = 0,
    val distanceM: Double = 0.0,
    val traceFile: String? = null,
    val recentPoints: List<GeoPoint> = emptyList(),
    val error: String? = null,
)

object TraceBus {
    private val mutable = MutableStateFlow(LiveTraceState())
    val state = mutable.asStateFlow()

    fun update(transform: (LiveTraceState) -> LiveTraceState) {
        mutable.value = transform(mutable.value)
    }
}
