package com.steele.looky.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.activity.compose.BackHandler
import androidx.compose.ui.unit.dp
import com.steele.looky.AppSettings
import com.steele.looky.location.TraceRecorder
import com.steele.looky.model.TraceBus
import com.steele.looky.offline.PtilesRepository
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.File
import java.time.Instant
import java.time.LocalDate
import java.time.ZoneId
import java.time.format.DateTimeFormatter

/**
 * A day's recording, however many files it was written across.
 *
 * The recorder opens a separate file per session -- drive, trail, and the
 * always-on background log -- so "Tuesday" is a set of files, not one. The
 * archive treats the day as the unit because that is what someone looks for
 * a week later; which session wrote which stretch is a storage detail.
 */
data class RecordingDay(val date: LocalDate, val segments: List<TraceSegment>) {
    val files: List<File> get() = segments.mapNotNull { it.file }.distinct()
    val distanceM: Double get() = segments.sumOf { it.distanceM }
    val firstFix: Instant? get() = segments.mapNotNull { it.firstFix }.minOrNull()
    val lastFix: Instant? get() = segments.mapNotNull { it.lastFix }.maxOrNull()

    /** Fixes per movement label, largest share first. Drives the bar. */
    val breakdown: List<Pair<String, Int>>
        get() = GpxReader.totals(segments.map { it.movement to it.points.size })
}

/** Today in full, everything older collapsed to one row per day. */
data class Recordings(val today: List<TraceSegment>, val archive: List<RecordingDay>)

/**
 * Which local day a stretch belongs to.
 *
 * The file name is the recorder's own answer and is preferred; a segment
 * carried in memory falls back to its first fix. Null for a stretch that has
 * neither -- it cannot be dated and it cannot be opened.
 */
internal fun dayOf(segment: TraceSegment, zone: ZoneId): LocalDate? =
    segment.file?.let(TraceRecorder::dateOf)
        ?: segment.firstFix?.atZone(zone)?.toLocalDate()

/**
 * Split a flat list of stretches into today and an archive of whole days.
 *
 * Today keeps its stretches because that is the day being lived in and each
 * one is a separate answer to "what have I been doing". Everything older is one
 * row: a list of every stretch of every day is hundreds of rows of "Stationary
 * for forty minutes", which is noise wearing detail's clothes.
 */
fun recordingsOf(segments: List<TraceSegment>, today: LocalDate, zone: ZoneId): Recordings {
    val dated = segments.mapNotNull { segment -> dayOf(segment, zone)?.let { it to segment } }
    val byDay = dated.groupBy({ it.first }, { it.second })
    return Recordings(
        today = byDay[today].orEmpty().sortedByDescending { it.lastFix ?: Instant.EPOCH },
        archive = byDay.filterKeys { it < today }
            .toSortedMap(reverseOrder())
            .map { (date, parts) ->
                RecordingDay(date, parts.sortedBy { it.firstFix ?: Instant.EPOCH })
            },
    )
}

/** `Yesterday`, or `Tue 11 Aug` once that stops being useful. */
fun dayLabel(date: LocalDate, today: LocalDate): String = when (date) {
    today -> "Today"
    today.minusDays(1) -> "Yesterday"
    else -> date.format(DateTimeFormatter.ofPattern("EEE d MMM"))
}

/**
 * Whether a list may replace its contents under the reader right now.
 *
 * The recordings list re-reads the file being written on every GPS fix, so
 * without this the list rebuilds roughly once a second. Rebuilding is fine at
 * the top of an untouched list and unforgivable anywhere else: someone reading
 * last Tuesday should not be thrown back to today because a fix arrived.
 * Updates that arrive while unsafe are held, not dropped.
 */
internal fun refreshIsSafe(firstVisibleIndex: Int, firstVisibleOffset: Int, query: String = ""): Boolean =
    firstVisibleIndex == 0 && firstVisibleOffset == 0 && query.isBlank()

/**
 * Fold a re-read of one day file back into the list, newest stretch first.
 *
 * Only the re-read file's stretches are replaced: the rest of the history did
 * not change, and re-parsing it at 1 Hz is not a live view, it is a stall.
 */
internal fun mergeReread(existing: List<TraceSegment>, reread: List<TraceSegment>, file: File): List<TraceSegment> =
    (existing.filterNot { it.file?.name == file.name } + reread)
        .sortedByDescending { it.lastFix ?: Instant.EPOCH }

@Composable
internal fun RecordingsScreen() {
    val context = LocalContext.current
    val live by TraceBus.state.collectAsState()
    var open by remember { mutableStateOf<RecordingDay?>(null) }
    open?.let { day ->
        BackHandler { open = null }
        RecordingDetailScreen(day.files, dayLabel(day.date, LocalDate.now()))
        return
    }
    var segments by remember { mutableStateOf(emptyList<TraceSegment>()) }
    // Distinct from "no recordings": the first read is a directory listing plus
    // a parse of every day file, and the empty state used to flash first.
    var loading by remember { mutableStateOf(true) }
    LaunchedEffect(Unit) {
        segments = withContext(Dispatchers.IO) {
            File(context.filesDir, "traces").listFiles().orEmpty()
                .filter { it.extension == "gpx" }
                .flatMap(GpxReader::readSegments)
                .sortedByDescending { it.lastFix ?: Instant.EPOCH }
        }
        loading = false
    }
    val listState = rememberLazyListState()
    val safe = refreshIsSafe(listState.firstVisibleItemIndex, listState.firstVisibleItemScrollOffset)
    // The open stretch grows while you are moving, so re-read the file being
    // written every time the recorder reports a new fix -- but only apply it
    // when the list is where it started. Otherwise it waits.
    var pending by remember { mutableStateOf<List<TraceSegment>?>(null) }
    LaunchedEffect(live.pointsToday, live.traceFile) {
        val active = live.traceFile?.let(::File)?.takeIf { it.exists() } ?: return@LaunchedEffect
        val reread = withContext(Dispatchers.IO) { GpxReader.readSegments(active) }
        val merged = mergeReread(pending ?: segments, reread, active)
        if (safe) segments = merged else pending = merged
    }
    LaunchedEffect(safe) {
        if (safe) pending?.let { segments = it; pending = null }
    }

    if (loading) {
        LoadingState("Reading day files…")
        return
    }
    if (segments.isEmpty()) {
        EmptyState("Nothing recorded yet", "Start Drive or Trail and the first accurate fix opens a segment.")
        return
    }
    val imperial = remember { AppSettings(context).imperialUnits }
    val placeRepo = remember { PtilesRepository(context) }
    val today = LocalDate.now()
    val split = remember(segments, today) { recordingsOf(segments, today, ZoneId.systemDefault()) }
    LazyColumn(
        Modifier.fillMaxSize().padding(16.dp),
        state = listState,
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        if (split.today.isNotEmpty()) {
            item(key = "today") { SectionHeading("TODAY") }
        }
        items(split.today, key = { "${it.file?.name}:${it.firstFix}:${it.movement}" }) { segment ->
            val recording = live.running && segment.file?.absolutePath == live.traceFile &&
                segment === split.today.first()
            // Where a stretch was spent, when the fixes are evidence of a stop
            // rather than a pause at a light. Keyed on the segment's own shape
            // so a growing live segment re-asks as it settles.
            var place by remember(segment.firstFix, segment.points.size) { mutableStateOf<String?>(null) }
            LaunchedEffect(segment.firstFix, segment.points.size) {
                place = placeRepo.placeLabel(segment)
            }
            SegmentRow(segment, place, recording, imperial) {
                segment.file?.let { file ->
                    open = RecordingDay(today, split.today.filter { it.file?.name == file.name })
                }
            }
        }
        if (split.archive.isNotEmpty()) {
            item(key = "archive") { SectionHeading("ARCHIVE") }
        }
        items(split.archive, key = { it.date.toString() }) { day ->
            DayRow(day, today, imperial) { open = day }
        }
    }
}

@Composable
private fun SectionHeading(text: String) {
    Text(
        text,
        Modifier.padding(top = 6.dp),
        style = MaterialTheme.typography.labelLarge,
        color = ForestSoft,
    )
}

@Composable
private fun SegmentRow(
    segment: TraceSegment,
    place: String?,
    recording: Boolean,
    imperial: Boolean,
    onOpen: () -> Unit,
) {
    Card(
        Modifier.clickable(onClick = onOpen),
        colors = CardDefaults.cardColors(containerColor = Color.White),
        shape = RoundedCornerShape(18.dp),
    ) {
        Row(Modifier.fillMaxWidth().padding(14.dp), verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.size(12.dp).background(movementColor(segment.movement), CircleShape))
            Spacer(Modifier.width(12.dp))
            Column(Modifier.weight(1f)) {
                Text(
                    // "Unknown" is the classifier's starting state, not a kind
                    // of travel; naming it that in a list of journeys reads as
                    // a bug.
                    movementLabel(segment.movement, recording),
                    style = MaterialTheme.typography.titleMedium,
                    color = Forest,
                )
                Text(
                    listOfNotNull(
                        formatSpan(segment.firstFix, segment.lastFix),
                        place?.let { "near $it" },
                        formatDistance(segment.distanceM, imperial),
                        if (segment.points.size == 1) "1 fix" else "${segment.points.size} fixes",
                    ).joinToString(" · "),
                    style = MaterialTheme.typography.bodySmall,
                    color = Color(0xFF69716C),
                )
            }
        }
    }
}

/**
 * One archived day.
 *
 * When it ran, how far it went, and how it was travelled -- the three things
 * that tell you whether this is the day you are looking for. Fix counts and
 * file names are storage, and neither has ever identified a day.
 */
@Composable
private fun DayRow(day: RecordingDay, today: LocalDate, imperial: Boolean, onOpen: () -> Unit) {
    Card(
        Modifier.clickable(onClick = onOpen),
        colors = CardDefaults.cardColors(containerColor = Color.White),
        shape = RoundedCornerShape(18.dp),
    ) {
        Column(Modifier.fillMaxWidth().padding(14.dp)) {
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                Text(
                    dayLabel(day.date, today),
                    Modifier.weight(1f),
                    style = MaterialTheme.typography.titleMedium,
                    color = Forest,
                )
                Text(
                    formatDistance(day.distanceM, imperial),
                    style = MaterialTheme.typography.titleSmall,
                    color = Forest,
                )
            }
            Text(
                listOfNotNull(
                    formatSpan(day.firstFix, day.lastFix),
                    day.breakdown.firstOrNull()?.let {
                        "mostly ${movementLabel(it.first, recording = false).lowercase()}"
                    },
                ).joinToString(" · "),
                style = MaterialTheme.typography.bodySmall,
                color = Color(0xFF69716C),
            )
            MovementBar(day.breakdown, Modifier.padding(top = 10.dp))
        }
    }
}

internal fun movementLabel(movement: String, recording: Boolean): String {
    val name = if (movement.equals("Unknown", ignoreCase = true)) "Unclassified" else movement
    return if (recording) "$name · now" else name
}

/** A screen that is reading files, said plainly rather than left blank. */
@Composable
internal fun LoadingState(message: String) {
    Column(
        Modifier.fillMaxSize().padding(32.dp),
        verticalArrangement = Arrangement.Center,
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        androidx.compose.material3.CircularProgressIndicator(color = Forest)
        Spacer(Modifier.height(16.dp))
        Text(message, color = ForestSoft)
    }
}
