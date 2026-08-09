package com.steele.looky.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.detectTransformGestures
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import com.steele.looky.model.GeoPoint
import com.steele.looky.model.MapFeature
import kotlin.math.cos

private val MapPaper = Color(0xFFF1F0E8)
private val Road = Color(0xFFB4B6AE)
private val MajorRoad = Color(0xFF777D74)
private val Trail = Color(0xFF487563)
private val Water = Color(0xFF78AFC2)
private val Park = Color(0xFF91AE83)
private val Building = Color(0xFFB68E73)
private val Route = Color(0xFF173F35)
private val Track = Color(0xFFD67246)

@Composable
fun OfflineMap(
    center: GeoPoint,
    features: List<MapFeature>,
    current: GeoPoint?,
    destination: GeoPoint?,
    route: List<GeoPoint>,
    trace: List<GeoPoint>,
    modifier: Modifier = Modifier,
    onLongPress: (GeoPoint) -> Unit = {},
) {
    var scale by remember(center) { mutableFloatStateOf(1f) }
    var pan by remember(center) { mutableStateOf(Offset.Zero) }

    Canvas(
        modifier
            .fillMaxSize()
            .semantics { contentDescription = "Offline PTiles map. Long press to set destination." }
            .pointerInput(center, scale, pan) {
                detectTapGestures(onLongPress = { tap ->
                    val spanLat = 0.075 / scale
                    val spanLon = spanLat / cos(Math.toRadians(center.lat)).coerceAtLeast(0.25)
                    val x = tap.x - size.width / 2f - pan.x
                    val y = tap.y - size.height / 2f - pan.y
                    onLongPress(
                        GeoPoint(
                            center.lat - y / size.height * spanLat,
                            center.lon + x / size.width * spanLon,
                        )
                    )
                })
            }
            .pointerInput(center) {
                detectTransformGestures { _, panChange, zoomChange, _ ->
                    scale = (scale * zoomChange).coerceIn(0.6f, 18f)
                    pan += panChange
                }
            }
    ) {
        drawRect(MapPaper)
        val spanLat = 0.075 / scale
        val spanLon = spanLat / cos(Math.toRadians(center.lat)).coerceAtLeast(0.25)
        fun project(point: GeoPoint): Offset = Offset(
            size.width / 2f + ((point.lon - center.lon) / spanLon * size.width).toFloat() + pan.x,
            size.height / 2f - ((point.lat - center.lat) / spanLat * size.height).toFloat() + pan.y,
        )
        fun line(points: List<GeoPoint>, color: Color, width: Float, cap: StrokeCap = StrokeCap.Round) {
            if (points.size < 2) return
            val path = Path()
            val first = project(points.first())
            path.moveTo(first.x, first.y)
            points.drop(1).forEach { point -> project(point).also { path.lineTo(it.x, it.y) } }
            drawPath(path, color, style = Stroke(width = width, cap = cap))
        }

        for (i in 1..4) {
            val alpha = 0.12f
            drawLine(Color(0xFF6B756D).copy(alpha = alpha), Offset(size.width * i / 5f, 0f), Offset(size.width * i / 5f, size.height), 1f)
            drawLine(Color(0xFF6B756D).copy(alpha = alpha), Offset(0f, size.height * i / 5f), Offset(size.width, size.height * i / 5f), 1f)
        }
        features.forEach { feature ->
            if (feature.points.size == 1 && feature.kind == "building") {
                drawCircle(Building, 3.5f, project(feature.points.first()))
                return@forEach
            }
            val isTrail = feature.kind.startsWith("trail") || feature.kind in setOf("path", "footway", "track", "steps")
            val major = feature.kind in setOf("motorway", "trunk", "primary", "secondary")
            val color = when {
                feature.kind == "water" -> Water
                feature.kind == "park" -> Park
                isTrail -> Trail
                major -> MajorRoad
                else -> Road
            }
            line(feature.points, color, if (isTrail) 4f else if (major) 5f else 2.5f)
        }
        line(trace, Track, 7f)
        line(route, Route, 10f)
        line(route, Lime, 4f)

        destination?.let {
            val p = project(it)
            drawCircle(Route, 13f, p)
            drawCircle(Lime, 6f, p)
        }
        current?.let {
            val p = project(it)
            drawCircle(Color.White, 13f, p)
            drawCircle(Color(0xFF2477D4), 8f, p)
        }
    }
}
