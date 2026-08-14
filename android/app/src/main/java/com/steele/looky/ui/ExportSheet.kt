package com.steele.looky.ui

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Checkbox
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.RangeSlider
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import com.steele.looky.AppSettings
import com.steele.looky.model.GeoPoint
import com.steele.looky.model.MapFeature
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** Enough map to judge a trim by, without taking the sheet over. */
private val SHEET_MAP_HEIGHT = 240.dp

/**
 * Choose part of a recording and write it out as GPX.
 *
 * Everything about an export lives here rather than on the detail screen: the
 * screen is for reading a day, and a trim slider parked under it made every
 * visit look like the start of a file operation. The sheet carries its own map
 * because a window is only judgeable against the shape it selects -- it redraws
 * on every drag, while the screen behind keeps showing the whole recording.
 *
 * The export goes through the system file picker, which is what makes the copy
 * the user's own: outside app storage, and the only thing an uninstall cannot
 * take.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun ExportSheet(
    name: String,
    segments: List<TraceSegment>,
    features: List<MapFeature>,
    center: GeoPoint,
    onDismiss: () -> Unit,
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val imperial = remember { AppSettings(context).imperialUnits }
    var range by remember { mutableStateOf(0f..1f) }
    var stretch by remember { mutableStateOf<Int?>(null) }
    var withSensors by remember { mutableStateOf(false) }
    // Only worth offering when the recording actually carries any: files
    // written before the recorder logged sensors have none.
    val hasSensors = remember(segments) { segments.any { part -> part.sensors.any { it != null } } }

    val selection = remember(segments, stretch, range) { GpxExport.select(segments, stretch, range) }
    val summary = remember(selection) { summarise(selection) }
    val selected = remember(selection) { selection.flatMap { it.points } }
    val whole = remember(segments) { segments.flatMap { it.points } }
    val trimmable = remember(segments, stretch) {
        GpxExport.timeline(stretch?.let { listOfNotNull(segments.getOrNull(it)) } ?: segments).size > 1
    }
    val untouched = stretch == null && range.start <= 0f && range.endInclusive >= 1f

    val save = rememberLauncherForActivityResult(
        ActivityResultContracts.CreateDocument("application/gpx+xml")
    ) { uri ->
        val target = uri ?: return@rememberLauncherForActivityResult
        scope.launch {
            withContext(Dispatchers.IO) {
                context.contentResolver.openOutputStream(target)?.use { out ->
                    out.write(GpxExport.write(selection, includeSensors = withSensors).toByteArray())
                }
            }
            onDismiss()
        }
    }

    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        containerColor = Color.White,
    ) {
        Column(
            Modifier.fillMaxWidth().padding(start = 20.dp, end = 20.dp, bottom = 32.dp),
            verticalArrangement = Arrangement.spacedBy(2.dp),
        ) {
            Text("Export GPX", style = MaterialTheme.typography.headlineSmall, color = Forest)
            Text(name, style = MaterialTheme.typography.bodyMedium, color = ForestSoft)
            Spacer(Modifier.height(12.dp))
            OfflineMap(
                center = center,
                features = features,
                current = null,
                destination = null,
                route = emptyList(),
                trace = selected,
                // The whole recording stays behind the selection: a window can
                // only be judged against what it is leaving out.
                dimmedTrace = whole,
                modifier = Modifier.fillMaxWidth().height(SHEET_MAP_HEIGHT),
                // Framed once, on the whole recording. Refitting per drag would
                // rescale the map under the thumb and make every window look
                // the same size.
                fitPoints = whole,
                fitKey = 1,
            )
            Spacer(Modifier.height(14.dp))
            if (segments.size > 1) {
                Text("WHAT TO EXPORT", style = MaterialTheme.typography.labelLarge, color = ForestSoft)
                Row(
                    Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(vertical = 6.dp),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    FilterChip(
                        selected = stretch == null,
                        onClick = { stretch = null; range = 0f..1f },
                        label = { Text("Whole recording") },
                    )
                    segments.forEachIndexed { index, part ->
                        FilterChip(
                            selected = stretch == index,
                            onClick = { stretch = index; range = 0f..1f },
                            label = {
                                Text(
                                    listOfNotNull(
                                        movementLabel(part.movement, recording = false),
                                        formatSpan(part.firstFix, part.lastFix),
                                    ).joinToString(" · ")
                                )
                            },
                        )
                    }
                }
            }
            if (trimmable) {
                Text("TRIM", style = MaterialTheme.typography.labelLarge, color = ForestSoft)
                // Inset from the edges: at full width the end thumbs sit inside
                // the system back-gesture strip, so dragging one leaves the
                // screen instead of trimming.
                RangeSlider(
                    value = range,
                    onValueChange = { range = it },
                    modifier = Modifier.padding(horizontal = 24.dp),
                )
            }
            Text(
                "${formatSpan(summary.from, summary.to) ?: "whole recording"} · " +
                    "${summary.points} fixes · ${formatDistance(summary.distanceM, imperial)}",
                style = MaterialTheme.typography.bodySmall,
                color = ForestSoft,
            )
            if (hasSensors) {
                Row(
                    Modifier.fillMaxWidth().padding(top = 6.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Checkbox(checked = withSensors, onCheckedChange = { withSensors = it })
                    Column(Modifier.padding(start = 4.dp)) {
                        Text("Include sensor data", style = MaterialTheme.typography.bodyMedium, color = Forest)
                        Text(
                            "Speed, accuracy, accelerometer and cadence. Roughly triples the file.",
                            style = MaterialTheme.typography.bodySmall,
                            color = ForestSoft,
                        )
                    }
                }
            }
            Button(
                onClick = { save.launch(GpxExport.fileName(name, summary.from, summary.to, untouched)) },
                enabled = summary.points > 0,
                modifier = Modifier.fillMaxWidth().padding(top = 10.dp).height(48.dp),
                shape = RoundedCornerShape(16.dp),
                colors = ButtonDefaults.buttonColors(containerColor = Forest),
            ) {
                Text(if (untouched) "Export whole recording" else "Export selection")
            }
        }
    }
}
