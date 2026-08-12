package com.steele.looky.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.platform.LocalContext
import com.steele.looky.model.GeoPoint
import com.steele.looky.model.MapFeature
import com.steele.looky.offline.PtilesRepository
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.File
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import kotlin.math.atan2
import kotlin.math.roundToInt
import kotlin.math.cos
import kotlin.math.sin
import kotlin.math.sqrt

/**
 * One stretch of a day travelled the same way.
 *
 * `file` is filled in by [GpxReader.readSegments] so a row in a merged list
 * still knows which day file to open.
 */
data class TraceSegment(
    val movement: String,
    val points: List<GeoPoint>,
    val firstFix: Instant?,
    val lastFix: Instant?,
    val distanceM: Double,
    val file: File? = null,
)

/** A parsed GPX day file: the track and the movement labels it contains. */
data class RecordedTrace(
    val points: List<GeoPoint>,
    val movements: List<String>,
    val distanceM: Double,
    /** Fixes recorded per movement label, in file order. Drives the bar. */
    val breakdown: List<Pair<String, Int>> = emptyList(),
    val firstFix: Instant? = null,
    val lastFix: Instant? = null,
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
    private val TIME = Regex("""<time>([^<]+)</time>""")
    private const val EARTH_RADIUS_M = 6_371_000.0

    fun parse(text: String): RecordedTrace {
        val parts = segments(text)
        val points = parts.flatMap { it.points }
        return RecordedTrace(
            points = points,
            movements = parts.map { it.movement }.distinct(),
            distanceM = pathLengthM(points),
            breakdown = parts.map { it.movement to it.points.size },
            firstFix = parts.firstNotNullOfOrNull { it.firstFix },
            lastFix = parts.lastOrNull { it.lastFix != null }?.lastFix,
        )
    }

    /**
     * One entry per `<trk>`, which is one run of a single movement type.
     *
     * The recorder opens a new track every time the classifier changes its
     * mind, so these are exactly the stretches of a day: drove for twenty
     * minutes, walked for five, sat still for an hour.
     */
    fun segments(text: String): List<TraceSegment> {
        val tracks = TRK_NAME.findAll(text).toList()
        return tracks.mapIndexedNotNull { index, match ->
            val movement = match.groupValues[1]
            if (movement.isBlank()) return@mapIndexedNotNull null
            val from = match.range.last + 1
            val to = tracks.getOrNull(index + 1)?.range?.first ?: text.length
            val body = text.substring(from, to)
            val points = TRKPT.findAll(body).mapNotNull { point ->
                val lat = point.groupValues[1].toDoubleOrNull()
                val lon = point.groupValues[2].toDoubleOrNull()
                if (lat == null || lon == null) null else GeoPoint(lat, lon)
            }.toList()
            if (points.isEmpty()) return@mapIndexedNotNull null
            val times = TIME.findAll(body).mapNotNull { runCatching { Instant.parse(it.groupValues[1]) }.getOrNull() }.toList()
            TraceSegment(
                movement = movement,
                points = points,
                firstFix = times.firstOrNull(),
                lastFix = times.lastOrNull(),
                distanceM = pathLengthM(points),
            )
        }
    }

    fun readSegments(file: File): List<TraceSegment> =
        runCatching { segments(file.readText()).map { it.copy(file = file) } }.getOrDefault(emptyList())

    /** Fixes per movement label, largest share first. */
    fun totals(breakdown: List<Pair<String, Int>>): List<Pair<String, Int>> = breakdown
        .groupingBy { it.first }
        .fold(0) { sum, entry -> sum + entry.second }
        .toList()
        .sortedByDescending { it.second }

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

/**
 * How long a stretch lasted, when it carries both ends.
 *
 * The place lookup uses it to reject a stretch too short to be a visit; a file
 * still being written has a first fix and no last one yet.
 */
fun segmentSeconds(segment: TraceSegment): Long? {
    val from = segment.firstFix ?: return null
    val to = segment.lastFix ?: return null
    return java.time.Duration.between(from, to).seconds
}

/**
 * Where a stretch was, if it stayed anywhere. See [PtilesRepository.placeLabel]
 * for what counts as staying.
 */
suspend fun PtilesRepository.placeLabel(segment: TraceSegment): String? =
    placeLabel(segment.points, segmentSeconds(segment))

@Composable
fun RecordingDetailScreen(file: File) {
    val context = LocalContext.current
    val repo = remember { PtilesRepository(context) }
    var trace by remember(file) { mutableStateOf<RecordedTrace?>(null) }
    var features by remember(file) { mutableStateOf(emptyList<MapFeature>()) }
    var stops by remember(file) { mutableStateOf(emptyList<Pair<TraceSegment, String>>()) }
    LaunchedEffect(file) {
        trace = withContext(Dispatchers.IO) { GpxReader.read(file) }
    }
    // Named stops, after the track: reading the file and then the buildings and
    // business layers for each stretch is far slower than the map, and the day
    // is legible without them.
    LaunchedEffect(file) {
        val segments = withContext(Dispatchers.IO) { GpxReader.readSegments(file) }
        stops = segments.mapNotNull { segment ->
            repo.placeLabel(segment)?.let { segment to it }
        }
    }
    // The roads the day was travelled on, decoded around the middle of it.
    val around = trace?.points?.getOrNull((trace?.points?.size ?: 0) / 2)
    LaunchedEffect(around?.lat, around?.lon) {
        val center = around ?: return@LaunchedEffect
        features = withContext(Dispatchers.IO) {
            repo.featuresAround(center.lat, center.lon, trails = true, places = true)
                .filter { it.kind != "building" }
        }
    }
    val imperial = remember { com.steele.looky.AppSettings(context).imperialUnits }
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
    val totals = GpxReader.totals(loaded.breakdown)
    // Map first, then the breakdown: the shape of the day answers "where was
    // I" before any number does.
    Column(Modifier.fillMaxSize()) {
        OfflineMap(
            center = center,
            features = features,
            current = null,
            destination = null,
            route = emptyList(),
            trace = loaded.points,
            modifier = Modifier.weight(1f),
        )
        HorizontalDivider()
        Column(Modifier.fillMaxWidth().padding(16.dp)) {
            Text(file.nameWithoutExtension, style = MaterialTheme.typography.headlineSmall)
            formatSpan(loaded.firstFix, loaded.lastFix)?.let {
                Text(it, style = MaterialTheme.typography.bodyMedium, color = ForestSoft)
            }
            Row(Modifier.fillMaxWidth().padding(top = 8.dp), horizontalArrangement = Arrangement.SpaceBetween) {
                Stat(formatDistance(loaded.distanceM, imperial), "DISTANCE")
                Stat(loaded.points.size.toString(), "POINTS")
                Stat("${file.length() / 1024} KB", "SIZE")
            }
            MovementBar(loaded.breakdown, Modifier.padding(top = 12.dp))
            Row(
                Modifier.fillMaxWidth().padding(top = 10.dp).horizontalScroll(rememberScrollState()),
                horizontalArrangement = Arrangement.spacedBy(14.dp),
            ) {
                totals.forEach { (movement, fixes) ->
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Box(Modifier.size(10.dp).background(movementColor(movement), CircleShape))
                        Spacer(Modifier.width(6.dp))
                        Text(
                            "$movement · ${percent(fixes, loaded.points.size)}",
                            style = MaterialTheme.typography.bodySmall,
                            color = Forest,
                        )
                    }
                }
            }
            // Three at most: the map keeps the rest of the height, and a day's
            // worth of "near Kroger" is a list, not a summary.
            stops.take(3).forEach { (segment, place) ->
                Text(
                    listOfNotNull(formatSpan(segment.firstFix, segment.lastFix), "near $place")
                        .joinToString(" · "),
                    style = MaterialTheme.typography.bodySmall,
                    color = ForestSoft,
                    modifier = Modifier.padding(top = 6.dp),
                )
            }
        }
    }
}

/**
 * How a track was travelled, as one bar.
 *
 * Widths are fix counts, not time: the recorder writes one point per GPS
 * interval, so counts are proportional to time at a constant polling rate and
 * do not need timestamps parsed to be useful.
 */
@Composable
fun MovementBar(breakdown: List<Pair<String, Int>>, modifier: Modifier = Modifier) {
    val total = breakdown.sumOf { it.second }
    if (total == 0) return
    Row(modifier.fillMaxWidth().height(10.dp).clip(RoundedCornerShape(5.dp))) {
        breakdown.forEach { (movement, fixes) ->
            Box(Modifier.weight(fixes.toFloat()).fillMaxHeight().background(movementColor(movement)))
        }
    }
}

private fun percent(part: Int, whole: Int): String =
    if (whole <= 0) "0%" else "${(part * 100.0 / whole).roundToInt()}%"

/** `08:14 - 17:02` in local time, or null when the file carries no times. */
fun formatSpan(from: Instant?, to: Instant?): String? {
    if (from == null) return null
    val zone = ZoneId.systemDefault()
    val clock = DateTimeFormatter.ofPattern("HH:mm").withZone(zone)
    val start = clock.format(from)
    val end = to?.let(clock::format)
    return if (end == null || end == start) start else "$start - $end"
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
