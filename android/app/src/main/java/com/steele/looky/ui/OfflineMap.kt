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
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.DrawScope
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
private val Camera = Color(0xFFB72F3E)
private val Rail = Color(0xFF6D4C7D)
private val Business = Color(0xFFD67246)

/**
 * The map's screen/geography conversion, kept out of the composable so the
 * forward and inverse transforms are one definition and can be unit tested.
 *
 * `pan` is a screen-space offset applied after projection, so panning moves the
 * picture without moving `center`. That separation is what lets the caller
 * reload PTiles data for wherever the user scrolled to while the projection
 * anchor stays put.
 */
internal object MapProjection {
    const val BASE_SPAN_LAT = 0.075

    fun spanLat(scale: Float): Double = BASE_SPAN_LAT / scale

    fun spanLon(centerLat: Double, scale: Float): Double =
        spanLat(scale) / cos(Math.toRadians(centerLat)).coerceAtLeast(0.25)

    fun project(
        point: GeoPoint,
        center: GeoPoint,
        pan: Offset,
        width: Float,
        height: Float,
        scale: Float,
    ): Offset = Offset(
        width / 2f + ((point.lon - center.lon) / spanLon(center.lat, scale) * width).toFloat() + pan.x,
        height / 2f - ((point.lat - center.lat) / spanLat(scale) * height).toFloat() + pan.y,
    )

    fun unproject(
        offset: Offset,
        center: GeoPoint,
        pan: Offset,
        width: Float,
        height: Float,
        scale: Float,
    ): GeoPoint {
        val x = offset.x - width / 2f - pan.x
        val y = offset.y - height / 2f - pan.y
        return GeoPoint(
            center.lat - y / height * spanLat(scale),
            center.lon + x / width * spanLon(center.lat, scale),
        )
    }

    /** Where the middle of the visible canvas currently sits on the ground. */
    fun viewportCenter(
        center: GeoPoint,
        pan: Offset,
        width: Float,
        height: Float,
        scale: Float,
    ): GeoPoint = unproject(Offset(width / 2f, height / 2f), center, pan, width, height, scale)
}

/**
 * What the map draws at a given zoom.
 *
 * Every polyline is its own `drawPath`, so a zoomed-out viewport over a dense
 * city was strokes in the thousands -- enough to push frames past the input
 * timeout, which is felt as a map that ignores you rather than one that is
 * slow. Detail arrives as the view tightens and the stroke count falls.
 */
internal object MapDetail {
    /**
     * Below this scale only through roads are worth a stroke.
     *
     * The map opens at scale 1.0, so a threshold above that culled minor roads
     * in the *default* view: 251 decoded roads rendered as three lines and a
     * lot of paper. Culling now starts only once the user zooms out past where
     * they began.
     */
    const val ARTERIAL_ONLY_BELOW = 0.9f

    /** Points (businesses, cameras) start drawing here -- also the opening view. */
    const val POINTS_ABOVE = 0.9f

    private val MAJOR = setOf("motorway", "trunk", "primary", "secondary", "motorway_link", "trunk_link")
    private val THROUGH = MAJOR + setOf("tertiary", "water", "park", "rail")

    /** Points worth drawing before the rest: they are destinations. */
    private val ALWAYS_POINTS = setOf("trailhead", "trail_end")

    fun draws(kind: String, isPoint: Boolean, scale: Float): Boolean = when {
        isPoint && kind in ALWAYS_POINTS -> true
        isPoint -> scale >= POINTS_ABOVE
        scale >= ARTERIAL_ONLY_BELOW -> true
        else -> THROUGH.any { kind == it || kind.startsWith("$it:") } || kind.startsWith("rail")
    }

    fun isMajor(kind: String): Boolean = kind in MAJOR
}

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
    onTap: (GeoPoint) -> Unit = {},
    onViewportChange: (GeoPoint) -> Unit = {},
    recenterKey: Int = 0,
) {
    // Keyed on recenterKey, not center: center changes with every GPS fix, and
    // resetting the pan on each one yanked the map back mid-gesture.
    var scale by remember(recenterKey) { mutableFloatStateOf(1f) }
    var pan by remember(recenterKey) { mutableStateOf(Offset.Zero) }

    Canvas(
        modifier
            .fillMaxSize()
            // Canvas does not clip: the trace and roads drew straight over the
            // panel below a weighted map, which read as the map leaking.
            .clipToBounds()
            .semantics { contentDescription = "Offline PTiles map. Long press to add a stop to the route." }
            .pointerInput(center, scale, pan) {
                fun ground(tap: Offset) = MapProjection.unproject(
                    tap, center, pan, size.width.toFloat(), size.height.toFloat(), scale,
                )
                detectTapGestures(
                    onLongPress = { onLongPress(ground(it)) },
                    onTap = { onTap(ground(it)) },
                )
            }
            .pointerInput(center, recenterKey) {
                detectTransformGestures { _, panChange, zoomChange, _ ->
                    scale = (scale * zoomChange).coerceIn(0.6f, 18f)
                    pan += panChange
                    onViewportChange(
                        MapProjection.viewportCenter(
                            center, pan, size.width.toFloat(), size.height.toFloat(), scale,
                        )
                    )
                }
            }
    ) {
        drawRect(MapPaper)
        fun project(point: GeoPoint): Offset =
            MapProjection.project(point, center, pan, size.width, size.height, scale)
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
            if (!MapDetail.draws(feature.kind, feature.points.size == 1, scale)) return@forEach
            if (feature.points.size == 1) {
                val point = project(feature.points.first())
                when {
                    feature.kind == "building" -> drawCircle(Building, 3.5f, point)
                    feature.kind.startsWith("camera") -> {
                        drawCircle(Color.White, 9f, point)
                        drawCircle(Camera, 7f, point)
                        drawCircle(MapPaper, 2.5f, point)
                        drawLine(Camera, point, point + Offset(8f, -6f), 3f, StrokeCap.Round)
                    }
                    feature.kind == "station" -> {
                        drawCircle(Color.White, 7f, point)
                        drawCircle(Rail, 5f, point)
                    }
                    feature.kind.startsWith("business") -> {
                        // A pin, not a dot: a business is somewhere you go.
                        drawCircle(Color.White, 7f, point)
                        drawCircle(Business, 5f, point)
                        drawCircle(Color.White, 1.8f, point)
                    }
                    feature.kind == "trailhead" -> {
                        drawCircle(Color.White, 9f, point)
                        drawCircle(Trail, 7f, point)
                        // A little flag, so a trailhead reads apart from a stop.
                        drawLine(Trail, point + Offset(0f, -7f), point + Offset(0f, -18f), 3f, StrokeCap.Round)
                        drawLine(Trail, point + Offset(0f, -18f), point + Offset(9f, -14f), 3f, StrokeCap.Round)
                        drawLine(Trail, point + Offset(9f, -14f), point + Offset(0f, -11f), 3f, StrokeCap.Round)
                    }
                    feature.kind == "trail_end" -> {
                        drawCircle(Color.White, 5f, point)
                        drawCircle(Trail, 3.5f, point)
                    }
                    else -> drawCircle(Road, 3.5f, point)
                }
                return@forEach
            }
            val isTrail = feature.kind.startsWith("trail") || feature.kind in setOf("path", "footway", "track", "steps")
            val major = feature.kind in setOf("motorway", "trunk", "primary", "secondary")
            val color = when {
                feature.kind == "water" -> Water
                feature.kind == "park" -> Park
                feature.kind.startsWith("rail") -> Rail
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
        drawCompass()
    }
}

/**
 * A north marker in the top-right corner.
 *
 * The projection is north-up and never rotates, so this is a fixed needle
 * rather than a live compass -- it answers "which way is north on this
 * picture", which is the question a paper map answers too.
 */
private fun DrawScope.drawCompass() {
    val cx = size.width - 46f
    val cy = 46f
    val center = Offset(cx, cy)
    drawCircle(Color.White.copy(alpha = .85f), 26f, center)
    drawCircle(Route.copy(alpha = .35f), 26f, center, style = Stroke(width = 1.5f))
    // North half in red, south half in ink, the way every compass rose reads.
    val north = Path().apply {
        moveTo(cx, cy - 17f)
        lineTo(cx - 7f, cy + 5f)
        lineTo(cx + 7f, cy + 5f)
        close()
    }
    drawPath(north, Camera)
    val south = Path().apply {
        moveTo(cx, cy + 15f)
        lineTo(cx - 6f, cy + 5f)
        lineTo(cx + 6f, cy + 5f)
        close()
    }
    drawPath(south, Route)
}
