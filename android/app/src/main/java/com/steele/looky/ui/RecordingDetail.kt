package com.steele.looky.ui

import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.AssistChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.steele.looky.model.GeoPoint
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.File
import kotlin.math.atan2
import kotlin.math.cos
import kotlin.math.sin
import kotlin.math.sqrt

/** A parsed GPX day file: the track and the movement labels it contains. */
data class RecordedTrace(
    val points: List<GeoPoint>,
    val movements: List<String>,
    val distanceM: Double,
)

/**
 * Read a day file written by `TraceRecorder`.
 *
 * A regex rather than an XML parser: this reads exactly one writer's output,
 * whose element shape is fixed by `TraceRecorder.trackPoint`, and a partially
 * written tail (the recorder appends live) should yield the points it can read
 * rather than throwing on a truncated document.
 */
object GpxReader {
    private val TRKPT = Regex("""<trkpt lat="([-0-9.]+)" lon="([-0-9.]+)"""")
    private val TRK_NAME = Regex("""<trk><name>([^<]*)</name>""")
    private const val EARTH_RADIUS_M = 6_371_000.0

    fun parse(text: String): RecordedTrace {
        val points = TRKPT.findAll(text).mapNotNull { match ->
            val lat = match.groupValues[1].toDoubleOrNull()
            val lon = match.groupValues[2].toDoubleOrNull()
            if (lat == null || lon == null) null else GeoPoint(lat, lon)
        }.toList()
        val movements = TRK_NAME.findAll(text)
            .map { it.groupValues[1] }
            .filter { it.isNotBlank() }
            .distinct()
            .toList()
        return RecordedTrace(points, movements, pathLengthM(points))
    }

    fun read(file: File): RecordedTrace =
        runCatching { parse(file.readText()) }.getOrDefault(RecordedTrace(emptyList(), emptyList(), 0.0))

    /** Haversine sum over consecutive fixes. */
    fun pathLengthM(points: List<GeoPoint>): Double = points.zipWithNext()
        .sumOf { (from, to) -> distanceM(from, to) }

    fun distanceM(from: GeoPoint, to: GeoPoint): Double {
        val dLat = Math.toRadians(to.lat - from.lat)
        val dLon = Math.toRadians(to.lon - from.lon)
        val a = sin(dLat / 2) * sin(dLat / 2) +
            cos(Math.toRadians(from.lat)) * cos(Math.toRadians(to.lat)) * sin(dLon / 2) * sin(dLon / 2)
        return EARTH_RADIUS_M * 2 * atan2(sqrt(a), sqrt(1 - a))
    }
}

@Composable
fun RecordingDetailScreen(file: File) {
    var trace by remember(file) { mutableStateOf<RecordedTrace?>(null) }
    LaunchedEffect(file) {
        trace = withContext(Dispatchers.IO) { GpxReader.read(file) }
    }
    val loaded = trace
    if (loaded == null) {
        Text("Reading ${file.name}…", Modifier.padding(16.dp), color = ForestSoft)
        return
    }
    if (loaded.points.isEmpty()) {
        EmptyRecording(file)
        return
    }
    val center = loaded.points[loaded.points.size / 2]
    Column(Modifier.fillMaxSize()) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 10.dp)) {
            Text(file.nameWithoutExtension, style = MaterialTheme.typography.headlineSmall)
            Row(Modifier.fillMaxWidth().padding(top = 6.dp), horizontalArrangement = Arrangement.SpaceBetween) {
                Stat(formatTraceDistance(loaded.distanceM), "DISTANCE")
                Stat(loaded.points.size.toString(), "POINTS")
                Stat("${file.length() / 1024} KB", "SIZE")
            }
            if (loaded.movements.isNotEmpty()) {
                Row(
                    Modifier.fillMaxWidth().padding(top = 10.dp).horizontalScroll(rememberScrollState()),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    loaded.movements.forEach { AssistChip(onClick = {}, label = { Text(it) }) }
                }
            }
        }
        HorizontalDivider()
        OfflineMap(
            center = center,
            features = emptyList(),
            current = null,
            destination = null,
            route = emptyList(),
            trace = loaded.points,
            modifier = Modifier.weight(1f),
        )
    }
}

@Composable
private fun Stat(value: String, label: String) {
    Column {
        Text(value, fontWeight = FontWeight.Black, color = Forest)
        Text(label, style = MaterialTheme.typography.labelSmall, color = ForestSoft)
    }
}

@Composable
private fun EmptyRecording(file: File) {
    Column(Modifier.fillMaxSize().padding(24.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(file.nameWithoutExtension, style = MaterialTheme.typography.headlineSmall)
        Text("No track points in this file yet.", color = Color(0xFF69716C))
    }
}

private fun formatTraceDistance(meters: Double): String =
    if (meters < 1_000) "${meters.toInt()} m" else "%.1f km".format(meters / 1_000)
