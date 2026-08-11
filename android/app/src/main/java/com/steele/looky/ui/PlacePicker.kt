package com.steele.looky.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Add
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.DragHandle
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.zIndex
import com.steele.looky.model.GeoPoint
import kotlin.math.roundToInt

/**
 * Pieces both Drive and Trail draw, with no behaviour of their own.
 *
 * The two modes are separate paths -- what to search, what to route, what to
 * record -- but a row is a row and a drag is a drag. These take their contents
 * as arguments and decide nothing.
 */

/** A place on a route. The last one in the list is the destination. */
internal data class Stop(val label: String, val point: GeoPoint)

internal fun List<Stop>.move(from: Int, to: Int): List<Stop> {
    if (from == to || from !in indices || to !in indices) return this
    val out = toMutableList()
    out.add(to, out.removeAt(from))
    return out
}

/** What a search box currently has to say. */
internal sealed interface PickerState {
    data object Searching : PickerState
    data class Found(val hits: List<PlaceHit>) : PickerState

    /** The query ran and matched nothing. Not an error, and not a blank box. */
    data class NoMatches(val query: String) : PickerState

    /** No layer covers here, which is the one thing downloading fixes. */
    data object NoMaps : PickerState
    data class Failed(val message: String) : PickerState
}

/**
 * One searchable place, whatever layer it came out of.
 *
 * `bearingDeg` is the straight-line direction to it, and `onRoute` marks a hit
 * that is barely a detour from the route already planned -- the two things
 * that decide whether a place is worth stopping at, which a distance alone
 * does not say.
 */
internal data class PlaceHit(
    val name: String,
    val point: GeoPoint,
    val distanceM: Double,
    val bearingDeg: Double = 0.0,
    val onRoute: Boolean = false,
)

/** Bearing from one point to another, degrees clockwise from north. */
internal fun bearingDeg(from: GeoPoint, to: GeoPoint): Double {
    val dLon = Math.toRadians(to.lon - from.lon)
    val fromLat = Math.toRadians(from.lat)
    val toLat = Math.toRadians(to.lat)
    val y = kotlin.math.sin(dLon) * kotlin.math.cos(toLat)
    val x = kotlin.math.cos(fromLat) * kotlin.math.sin(toLat) -
        kotlin.math.sin(fromLat) * kotlin.math.cos(toLat) * kotlin.math.cos(dLon)
    return (Math.toDegrees(kotlin.math.atan2(y, x)) + 360.0) % 360.0
}

/** The eight-point compass name for a bearing: N, NE, E, and so on. */
internal fun compassPoint(bearing: Double): String {
    val points = listOf("N", "NE", "E", "SE", "S", "SW", "W", "NW")
    val index = Math.round(((bearing % 360.0) + 360.0) % 360.0 / 45.0).toInt() % 8
    return points[index]
}

/** True when a point is within [metres] of any leg of a planned route. */
internal fun nearRoute(route: List<GeoPoint>, point: GeoPoint, metres: Double): Boolean =
    route.any { GpxReader.distanceM(it, point) <= metres }

/** Hits scroll in their own box, so the stop chain below always shows. */
internal val RESULTS_MAX_HEIGHT = 170.dp
internal val STOPS_MAX_HEIGHT = 160.dp
internal val STOP_ROW_HEIGHT = 52.dp

/** How near a planned route a hit must be to count as "on your route". */
internal const val ON_ROUTE_M = 500.0

@Composable
internal fun PlaceRow(hit: PlaceHit, imperial: Boolean, onAdd: () -> Unit) {
    Row(Modifier.fillMaxWidth().clickable(onClick = onAdd), verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f).padding(vertical = 6.dp)) {
            Text(hit.name, style = MaterialTheme.typography.titleSmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(
                listOfNotNull(
                    "${formatDistance(hit.distanceM, imperial)} ${compassPoint(hit.bearingDeg)}",
                    if (hit.onRoute) "on your route" else null,
                ).joinToString(" · "),
                style = MaterialTheme.typography.bodySmall,
                color = if (hit.onRoute) Forest else ForestSoft,
            )
        }
        IconButton(onClick = onAdd) { Icon(Icons.Rounded.Add, "Add stop") }
    }
}

/** The picker's message when it has hits to show or a reason it has none. */
@Composable
internal fun PickerMessage(state: PickerState, onFixMaps: () -> Unit) {
    when (state) {
        is PickerState.Searching -> Text("Searching…", style = MaterialTheme.typography.bodySmall, color = ForestSoft)
        is PickerState.NoMatches -> Text(
            "Nothing matched \"${state.query}\"",
            style = MaterialTheme.typography.bodySmall,
            color = ForestSoft,
        )
        is PickerState.NoMaps -> Text(
            "No maps downloaded for this area. Tap to fix.",
            Modifier.clickable(onClick = onFixMaps),
            style = MaterialTheme.typography.bodySmall,
            color = Clay,
        )
        is PickerState.Failed -> Text(
            "Search failed: ${state.message}",
            style = MaterialTheme.typography.bodySmall,
            color = Clay,
        )
        is PickerState.Found -> Unit
    }
}

/**
 * The stop chain, reordered by dragging a row's handle.
 *
 * ponytail: rows are a fixed height so the drop index is offset/height. If the
 * rows ever wrap to two lines, measure them instead.
 */
@Composable
internal fun StopList(stops: List<Stop>, onMove: (Int, Int) -> Unit, onRemove: (Int) -> Unit) {
    val rowPx = with(LocalDensity.current) { STOP_ROW_HEIGHT.toPx() }
    var dragging by remember { mutableStateOf<Int?>(null) }
    var dragOffset by remember { mutableStateOf(0f) }
    Column(Modifier.fillMaxWidth()) {
        stops.forEachIndexed { index, stop ->
            val held = dragging == index
            Row(
                Modifier
                    .fillMaxWidth()
                    .height(STOP_ROW_HEIGHT)
                    .zIndex(if (held) 1f else 0f)
                    .offset { IntOffset(0, if (held) dragOffset.roundToInt() else 0) },
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Icon(
                    Icons.Rounded.DragHandle,
                    "Reorder stop",
                    tint = ForestSoft,
                    modifier = Modifier.pointerInput(index, stops.size) {
                        detectDragGestures(
                            onDragStart = { dragging = index; dragOffset = 0f },
                            onDrag = { change, amount -> change.consume(); dragOffset += amount.y },
                            onDragEnd = {
                                onMove(index, (index + (dragOffset / rowPx).roundToInt()).coerceIn(0, stops.lastIndex))
                                dragging = null; dragOffset = 0f
                            },
                            onDragCancel = { dragging = null; dragOffset = 0f },
                        )
                    },
                )
                Spacer(Modifier.width(10.dp))
                Column(Modifier.weight(1f)) {
                    Text(stop.label, style = MaterialTheme.typography.titleSmall, maxLines = 1, overflow = TextOverflow.Ellipsis, color = Forest)
                    Text(
                        if (index == stops.lastIndex) "Destination" else "Stop ${index + 1}",
                        style = MaterialTheme.typography.labelSmall,
                        color = ForestSoft,
                    )
                }
                IconButton(onClick = { onRemove(index) }) { Icon(Icons.Rounded.Close, "Remove stop") }
            }
        }
    }
}

internal fun formatDistance(meters: Double, imperial: Boolean): String = if (imperial) {
    val feet = meters * 3.28084
    if (feet < 1_000) "${feet.roundToInt()} ft" else "%.1f mi".format(feet / 5_280)
} else {
    if (meters < 1_000) "${meters.roundToInt()} m" else "%.1f km".format(meters / 1_000)
}
