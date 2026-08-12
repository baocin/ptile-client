package com.steele.looky.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.detectTransformGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.MyLocation
import androidx.compose.material.icons.rounded.ZoomOutMap
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.nativeCanvas
import androidx.compose.ui.graphics.toArgb
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

/**
 * Route colours.
 *
 * The old line was ink casing plus a thin lime core, which on cream paper
 * crossed by grey roads read as one more road. A saturated blue is the one hue
 * nothing else on this map uses at full strength, and it keeps the casing so it
 * still separates from whatever it runs over. [RouteWalk] is the same line on
 * foot, [RouteUnclassified] is a stretch no installed layer could place.
 */
internal val RouteDrive = Color(0xFF2B5BE0)
internal val RouteWalk = Color(0xFFE23C8A)
internal val RouteUnclassified = Color(0xFF6F7770)
private val Camera = Color(0xFFB72F3E)
private val Rail = Color(0xFF6D4C7D)
private val Business = Color(0xFFD67246)
private val WaterEdge = Color(0xFF5E93A6)
private val AdminLine = Color(0x998A7FA6)
private val AdminStateLine = Color(0xBB6F5F94)

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

    /**
     * Longitude span across the canvas width, at the same metres per pixel as
     * latitude across its height.
     *
     * Without the aspect term the map was stretched: 0.075 degrees of latitude
     * over 1742 px is 4.8 m/px, while the same figure of longitude over
     * 1080 px is 7.7 m/px, so everything was 1.6x too wide. A degree of
     * longitude is also shorter than a degree of latitude away from the
     * equator, which is what the cosine corrects.
     */
    fun spanLon(centerLat: Double, scale: Float, width: Float, height: Float): Double {
        val aspect = if (height > 0f) (width / height).toDouble() else 1.0
        return spanLat(scale) * aspect / cos(Math.toRadians(centerLat)).coerceAtLeast(0.25)
    }

    fun project(
        point: GeoPoint,
        center: GeoPoint,
        pan: Offset,
        width: Float,
        height: Float,
        scale: Float,
    ): Offset = Offset(
        width / 2f + ((point.lon - center.lon) / spanLon(center.lat, scale, width, height) * width).toFloat() + pan.x,
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
            center.lon + x / width * spanLon(center.lat, scale, width, height),
        )
    }

    /**
     * The zoom range a gesture or a fit may land on.
     *
     * The floor used to be 0.6, about 14 km of latitude, which is roughly as
     * far out as the decoded viewport has anything to say. That is fine for a
     * pinch and useless for "show all": a cross-county route framed at 0.6 has
     * both ends off screen. 0.08 is about 100 km, which the bounded router
     * cannot exceed by much.
     */
    const val MIN_SCALE = 0.08f
    const val MAX_SCALE = 18f

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
 * The zoom and pan that put a set of points on screen at once.
 *
 * Both spans are inversely proportional to scale, so the scale that just fits
 * an extent is the extent's share of the span at scale 1 -- which is asked of
 * [MapProjection] rather than re-derived here, and the pan comes straight out
 * of its forward transform.
 */
internal object MapFit {
    /** Room for the compass, the panel edge, and the marker rings themselves. */
    const val MARGIN_PX = 90f

    fun solve(
        points: List<GeoPoint>,
        center: GeoPoint,
        width: Float,
        height: Float,
        marginPx: Float = MARGIN_PX,
    ): Pair<Float, Offset>? {
        if (points.isEmpty() || width <= 0f || height <= 0f) return null
        val minLat = points.minOf { it.lat }
        val maxLat = points.maxOf { it.lat }
        val minLon = points.minOf { it.lon }
        val maxLon = points.maxOf { it.lon }
        val usableWidth = (width - 2 * marginPx).coerceAtLeast(1f)
        val usableHeight = (height - 2 * marginPx).coerceAtLeast(1f)
        // A single point has no extent to fit, so only the centring applies and
        // the caller's zoom is left where it is.
        val byLat = fitScale(MapProjection.spanLat(1f), maxLat - minLat, usableHeight / height)
        val byLon = fitScale(
            MapProjection.spanLon(center.lat, 1f, width, height), maxLon - minLon, usableWidth / width,
        )
        val scale = minOf(byLat, byLon).coerceIn(MapProjection.MIN_SCALE, MapProjection.MAX_SCALE)
        val middle = GeoPoint((minLat + maxLat) / 2, (minLon + maxLon) / 2)
        val at = MapProjection.project(middle, center, Offset.Zero, width, height, scale)
        return scale to Offset(width / 2f - at.x, height / 2f - at.y)
    }

    private fun fitScale(spanAtOne: Double, extent: Double, usableFraction: Float): Float =
        if (extent <= 0.0) MapProjection.MAX_SCALE else (spanAtOne * usableFraction / extent).toFloat()
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

    /**
     * How wide a net to cast for features at this zoom.
     *
     * The viewport grew by a factor of twelve when the zoom floor dropped to
     * frame a whole route, and the fetch did not: five sample centres cover
     * about 8 km, so everything beyond that was blank paper. Zoomed in the
     * opposite is true -- the old five centres decoded twenty-two cells to
     * draw three.
     */
    fun fetchSpread(scale: Float): Int = when {
        scale >= 2.0f -> 0
        scale >= ARTERIAL_ONLY_BELOW -> 1
        else -> 2
    }

    /**
     * True where pavement and parking aisles are not drawn anyway.
     *
     * They are 82.5% of the segments a city cell decodes, so skipping them is
     * what makes the wide fetch above affordable.
     */
    fun skipsMinorRoads(scale: Float): Boolean = scale < ARTERIAL_ONLY_BELOW

    /** Coarse-zoom jurisdiction lines: state lines survive further out. */
    const val COUNTY_LINES_BELOW = 1.0f
    const val STATE_LINES_BELOW = 1.6f

    /** Points worth drawing before the rest: they are destinations. */
    private val ALWAYS_POINTS = setOf("trailhead", "trail_end")

    /** Buildings are the densest thing on the map; they arrive last. */
    const val BUILDINGS_ABOVE = 2.2f

    /**
     * Sidewalks and footways wait for a close zoom.
     *
     * In a town the trails layer traces every street with a footway, so at
     * arm's length the whole grid came up green and the roads underneath were
     * invisible. Named paths and tracks still draw; pavement does not.
     */
    const val FOOTWAYS_ABOVE = 3.0f
    private val MINOR_TRAILS = setOf("trail:footway", "trail:steps", "trail:sidewalk", "footway", "steps", "sidewalk")

    /** Road names first, then business names once there is room to read them. */
    const val ROAD_LABELS_ABOVE = 1.8f
    const val BUSINESS_LABELS_ABOVE = 3.2f

    /**
     * Painting order: ground first, then what sits on it.
     *
     * Water and parks are areas, roads and trails are the network over them,
     * buildings and pins are detail on top. Drawn in feature order the map was
     * a lottery -- a lake could land on a highway.
     */
    fun layer(kind: String): Int = when {
        kind == "admin_county" || kind == "admin_state" -> 0
        kind == "water_area" || kind == "park" -> 0
        kind == "building_area" -> 1
        kind == "water" -> 2
        kind.startsWith("rail") -> 3
        kind.startsWith("trail") -> 4
        isMajor(kind) -> 6
        kind == "building" || kind.startsWith("business") || kind.startsWith("camera") ||
            kind == "station" || kind == "trailhead" || kind == "trail_end" -> 7
        else -> 5
    }

    fun draws(kind: String, isPoint: Boolean, scale: Float): Boolean = when {
        // County lines are the coarse-zoom answer to "where am I", and clutter
        // once the streets are back.
        // Jurisdiction lines answer "where am I" when the streets are gone,
        // and clutter once they are back.
        kind == "admin_county" -> scale < COUNTY_LINES_BELOW
        kind == "admin_state" -> scale < STATE_LINES_BELOW
        isPoint && kind in ALWAYS_POINTS -> true
        isPoint -> scale >= POINTS_ABOVE
        // A town's footprints are thousands of little rings: worth it close
        // up, ruinous at arm's length.
        kind == "building_area" -> scale >= BUILDINGS_ABOVE
        kind in MINOR_TRAILS -> scale >= FOOTWAYS_ABOVE
        scale >= ARTERIAL_ONLY_BELOW -> true
        else -> THROUGH.any { kind == it || kind.startsWith("$it:") } || kind.startsWith("rail") ||
            kind == "water_area"
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
    onViewportChange: (GeoPoint, Float) -> Unit = { _, _ -> },
    recenterKey: Int = 0,
    /** Drawn over [route] instead of one flat line when the caller can split it. */
    routeParts: List<Pair<List<GeoPoint>, Color>> = emptyList(),
    fitPoints: List<GeoPoint> = emptyList(),
    fitKey: Int = 0,
) {
    // Keyed on recenterKey, not center: center changes with every GPS fix, and
    // resetting the pan on each one yanked the map back mid-gesture.
    var scale by remember(recenterKey) { mutableFloatStateOf(1f) }
    var pan by remember(recenterKey) { mutableStateOf(Offset.Zero) }
    // The fit needs the canvas size, which only the draw scope knows, so it is
    // captured on layout and the fit waits for it.
    var canvas by remember { mutableStateOf(Size.Zero) }

    LaunchedEffect(fitKey, canvas) {
        if (fitKey == 0 || canvas == Size.Zero) return@LaunchedEffect
        val (fitScale, fitPan) = MapFit.solve(fitPoints, center, canvas.width, canvas.height) ?: return@LaunchedEffect
        scale = fitScale
        pan = fitPan
        // A fit usually lands away from `center`, and without this the map
        // shows the route over paper: nothing reloads the PTiles data there.
        onViewportChange(MapProjection.viewportCenter(center, pan, canvas.width, canvas.height, scale), scale)
    }

    Canvas(
        modifier
            .fillMaxSize()
            .onSizeChanged { canvas = Size(it.width.toFloat(), it.height.toFloat()) }
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
                    scale = (scale * zoomChange).coerceIn(MapProjection.MIN_SCALE, MapProjection.MAX_SCALE)
                    pan += panChange
                    onViewportChange(
                        MapProjection.viewportCenter(
                            center, pan, size.width.toFloat(), size.height.toFloat(), scale,
                        ),
                        scale,
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
        fun area(points: List<GeoPoint>, fill: Color, edge: Color) {
            if (points.size < 3) return
            val path = Path()
            val first = project(points.first())
            path.moveTo(first.x, first.y)
            points.drop(1).forEach { point -> project(point).also { path.lineTo(it.x, it.y) } }
            path.close()
            drawPath(path, fill)
            drawPath(path, edge, style = Stroke(width = 1.2f))
        }

        val visible = features.filter { MapDetail.draws(it.kind, it.points.size == 1, scale) }
        visible.sortedBy { MapDetail.layer(it.kind) }.forEach { feature ->
            when (feature.kind) {
                // Lakes, reservoirs, and wide river banks are areas, and read
                // as water only when they are filled.
                "water_area" -> { area(feature.points, Water, WaterEdge); return@forEach }
                "building_area" -> { area(feature.points, Building.copy(alpha = .45f), Building); return@forEach }
            }
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
            if (feature.kind == "admin_county" || feature.kind == "admin_state") {
                val state = feature.kind == "admin_state"
                line(feature.points, if (state) AdminStateLine else AdminLine, if (state) 3.5f else 1.8f)
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
            line(feature.points, color, if (isTrail) 2f else if (major) 5.5f else 3f)
        }
        // Road names, where there is room to read one.
        if (scale >= MapDetail.ROAD_LABELS_ABOVE) {
            drawLabels(visible.filter { it.points.size > 1 && MapDetail.isMajor(it.kind) }, ::project, Route, 30f)
        }
        if (scale >= MapDetail.BUSINESS_LABELS_ABOVE) {
            drawLabels(visible.filter { it.kind.startsWith("business") }, ::project, Business, 26f)
        }
        line(trace, Track, 7f)
        // Casing under every part, so a split route still reads as one line.
        line(route, Route, 16f)
        if (routeParts.isEmpty()) line(route, RouteDrive, 9f)
        else routeParts.forEach { (points, color) -> line(points, color, 9f) }

        destination?.let {
            val p = project(it)
            drawCircle(Route, 14f, p)
            drawCircle(Lime, 7f, p)
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
 * The two map buttons, shared by Drive and Trail because a control that moves
 * between the modes is a control users stop trusting.
 *
 * Recenter only appears once the map has been moved off the live position;
 * Show all only once there is more than one thing to frame.
 */
@Composable
internal fun BoxScope.MapControls(canFit: Boolean, panned: Boolean, onFit: () -> Unit, onRecenter: () -> Unit) {
    if (!canFit && !panned) return
    Row(
        Modifier.align(Alignment.BottomEnd).padding(end = 16.dp, bottom = 96.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        if (canFit) {
            FilledTonalButton(onClick = onFit, shape = RoundedCornerShape(16.dp)) {
                Icon(Icons.Rounded.ZoomOutMap, null)
                Spacer(Modifier.width(8.dp))
                Text("Show all")
            }
        }
        if (panned) {
            FilledTonalButton(onClick = onRecenter, shape = RoundedCornerShape(16.dp)) {
                Icon(Icons.Rounded.MyLocation, null)
                Spacer(Modifier.width(8.dp))
                Text("Recenter")
            }
        }
    }
}

/**
 * Names on the map, one per feature name, never overlapping.
 *
 * A label is only worth drawing if it can be read: each one claims a box, and
 * a name whose box collides with a label already placed is dropped rather than
 * smeared over it. Nothing is drawn off-canvas, which is most of the corridor
 * a decode returns.
 */
private fun DrawScope.drawLabels(
    features: List<MapFeature>,
    project: (GeoPoint) -> Offset,
    color: Color,
    sizePx: Float,
) {
    val paint = android.graphics.Paint().apply {
        isAntiAlias = true
        textSize = sizePx
        this.color = color.toArgb()
        typeface = android.graphics.Typeface.DEFAULT_BOLD
        textAlign = android.graphics.Paint.Align.CENTER
    }
    val halo = android.graphics.Paint(paint).apply {
        style = android.graphics.Paint.Style.STROKE
        strokeWidth = sizePx / 6f
        this.color = android.graphics.Color.WHITE
    }
    val taken = mutableListOf<android.graphics.RectF>()
    val placed = HashSet<String>()
    drawContext.canvas.nativeCanvas.let { canvas ->
        features.forEach { feature ->
            val name = feature.name?.takeIf { it.isNotBlank() } ?: return@forEach
            if (!placed.add(name)) return@forEach
            val at = project(feature.points[feature.points.size / 2])
            if (at.x < 0f || at.y < 0f || at.x > size.width || at.y > size.height) return@forEach
            val halfWidth = paint.measureText(name) / 2f
            val box = android.graphics.RectF(at.x - halfWidth, at.y - sizePx, at.x + halfWidth, at.y + sizePx / 2f)
            if (taken.any { android.graphics.RectF.intersects(it, box) }) return@forEach
            taken += box
            canvas.drawText(name, at.x, at.y, halo)
            canvas.drawText(name, at.x, at.y, paint)
        }
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
