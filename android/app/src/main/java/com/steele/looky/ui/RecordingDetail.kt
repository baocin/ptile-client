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
import androidx.compose.material3.FilledTonalButton
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
    /**
     * When each fix in [points] was recorded, same length and order.
     *
     * Null for a fix the writer left untimed. Parallel lists rather than a
     * list of pairs because every consumer wants the geometry alone.
     */
    val times: List<Instant?> = emptyList(),
    /**
     * Each fix's `<extensions>` payload, verbatim, same length and order.
     *
     * Kept as raw XML rather than parsed into fields: the recorder writes
     * speed, accuracy, three accelerometer summaries and a cadence today and
     * may write more tomorrow, and an exporter that models them would silently
     * drop whatever it had not been taught. Null where a fix carried none.
     */
    val sensors: List<String?> = emptyList(),
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
    private val EXTENSIONS = Regex("""<extensions>(.*?)</extensions>""", RegexOption.DOT_MATCHES_ALL)
    private const val EARTH_RADIUS_M = 6_371_000.0

    fun parse(text: String): RecordedTrace = traceOf(segments(text))

    /**
     * Roll stretches up into one recording.
     *
     * Taken from segments rather than from the text so a day spread over a
     * drive file, a trail file and the background log is one recording, and so
     * a screen that already holds the segments does not read the disk twice to
     * get the totals.
     *
     * Distance is the whole path, gaps between stretches included: the recorder
     * closes one track and opens the next on the same fix interval, so the step
     * across a boundary is travel, not a seam.
     */
    fun traceOf(parts: List<TraceSegment>): RecordedTrace {
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
            // Each fix is read with its own timestamp rather than collecting
            // the two separately: trimming a recording to a window needs to
            // know which point a time belongs to, and a `<trkpt>` with no
            // `<time>` would otherwise shift every later pairing by one.
            val fixes = body.split("<trkpt").drop(1).mapNotNull { chunk ->
                val point = TRKPT.find("<trkpt$chunk") ?: return@mapNotNull null
                val lat = point.groupValues[1].toDoubleOrNull() ?: return@mapNotNull null
                val lon = point.groupValues[2].toDoubleOrNull() ?: return@mapNotNull null
                val at = TIME.find(chunk)?.let { runCatching { Instant.parse(it.groupValues[1]) }.getOrNull() }
                val sensors = EXTENSIONS.find(chunk)?.groupValues?.get(1)?.takeIf { it.isNotBlank() }
                Triple(GeoPoint(lat, lon), at, sensors)
            }
            if (fixes.isEmpty()) return@mapIndexedNotNull null
            val points = fixes.map { it.first }
            val times = fixes.map { it.second }
            val sensors = fixes.map { it.third }
            TraceSegment(
                movement = movement,
                points = points,
                times = times,
                sensors = sensors,
                firstFix = times.firstOrNull { it != null },
                lastFix = times.lastOrNull { it != null },
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

/**
 * One recording, read from the files a day was written across.
 *
 * `title` is what the day is called on the way in -- "Yesterday", a date -- so
 * the screen does not have to re-derive it from a filename that may be one of
 * several.
 */
@Composable
fun RecordingDetailScreen(files: List<File>, title: String) {
    val context = LocalContext.current
    val repo = remember { PtilesRepository(context) }
    val key = remember(files) { files.joinToString { it.name } }
    var stops by remember(key) { mutableStateOf(emptyList<Pair<TraceSegment, String>>()) }
    var segments by remember(key) { mutableStateOf<List<TraceSegment>?>(null) }
    var exporting by remember(key) { mutableStateOf(false) }
    LaunchedEffect(key) {
        segments = withContext(Dispatchers.IO) {
            files.flatMap(GpxReader::readSegments).sortedBy { it.firstFix ?: Instant.EPOCH }
        }
    }
    val parts = segments.orEmpty()
    val trace = remember(parts) { GpxReader.traceOf(parts) }
    // Named stops, after the track: reading the file and then the buildings and
    // business layers for each stretch is far slower than the map, and the day
    // is legible without them.
    LaunchedEffect(parts) {
        stops = parts.mapNotNull { segment ->
            repo.placeLabel(segment)?.let { segment to it }
        }
    }
    val imperial = remember { com.steele.looky.AppSettings(context).imperialUnits }
    if (segments == null) {
        LoadingState("Reading $title…")
        return
    }
    if (trace.points.isEmpty()) {
        EmptyRecording(title)
        return
    }
    val center = trace.points[trace.points.size / 2]
    val totals = GpxReader.totals(trace.breakdown)
    // Map first, then the breakdown: the shape of the day answers "where was
    // I" before any number does. The map shows the whole recording; what a
    // trim would select is the export sheet's business.
    Column(Modifier.fillMaxSize()) {
        // Framed on the whole recording, and the data follows the frame: a
        // day-long drive is far wider than one fetch around its midpoint.
        MapCanvas(
            repo = repo,
            center = center,
            trace = trace.points,
            modifier = Modifier.weight(1f),
            fitPoints = trace.points,
            fitKey = trace.points.size,
        )
        HorizontalDivider()
        Column(Modifier.fillMaxWidth().padding(16.dp)) {
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text(title, style = MaterialTheme.typography.headlineSmall)
                    formatSpan(trace.firstFix, trace.lastFix)?.let {
                        Text(it, style = MaterialTheme.typography.bodyMedium, color = ForestSoft)
                    }
                }
                FilledTonalButton(onClick = { exporting = true }) { Text("Trim & export") }
            }
            Row(Modifier.fillMaxWidth().padding(top = 8.dp), horizontalArrangement = Arrangement.SpaceBetween) {
                Stat(formatDistance(trace.distanceM, imperial), "DISTANCE")
                Stat(trace.points.size.toString(), "POINTS")
                Stat("${files.sumOf { it.length() } / 1024} KB", "SIZE")
            }
            MovementBar(trace.breakdown, Modifier.padding(top = 12.dp))
            Row(
                Modifier.fillMaxWidth().padding(top = 10.dp).horizontalScroll(rememberScrollState()),
                horizontalArrangement = Arrangement.spacedBy(14.dp),
            ) {
                totals.forEach { (movement, fixes) ->
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Box(Modifier.size(10.dp).background(movementColor(movement), CircleShape))
                        Spacer(Modifier.width(6.dp))
                        Text(
                            "$movement · ${percent(fixes, trace.points.size)}",
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
    if (exporting) {
        ExportSheet(
            name = files.firstOrNull()?.name ?: "$title.gpx",
            segments = parts,
            center = center,
        ) { exporting = false }
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
private fun EmptyRecording(title: String) {
    Column(Modifier.fillMaxSize().padding(24.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(title, style = MaterialTheme.typography.headlineSmall)
        Text("No track points in this recording yet.", color = Color(0xFF69716C))
    }
}

/** Where a slider position falls on a recording's own span. */
internal fun atFraction(from: Instant, to: Instant, fraction: Float): Instant =
    from.plusMillis(((to.toEpochMilli() - from.toEpochMilli()) * fraction.toDouble()).toLong())
