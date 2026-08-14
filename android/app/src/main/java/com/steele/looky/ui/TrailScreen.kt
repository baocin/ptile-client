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
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.Search
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.steele.looky.AppSettings
import com.steele.looky.location.TraceRecorder
import com.steele.looky.location.TraceService
import com.steele.looky.model.FALLBACK_ANCHOR
import com.steele.looky.model.GeoPoint
import com.steele.looky.model.LookyMode
import com.steele.looky.model.MapFeature
import com.steele.looky.model.TraceBus
import com.steele.looky.offline.PtilesRepository
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlin.math.cos
import kotlin.math.roundToInt

/**
 * Which surface each stretch of a trail route runs on.
 *
 * The router hands back one path and no per-vertex mode, so the split is not
 * read off the route -- it is measured against the installed layers: a vertex
 * is walking when the nearest mapped line to it is a trail, driving when it is
 * a road, and unmapped when neither is within [MATCH_M]. That last case is real
 * and gets its own colour rather than being folded into either side; the
 * decoded features only cover the viewport, so a route running off the loaded
 * area is honestly unclassified there.
 *
 * ponytail: nearest is measured to feature *vertices*, not to the segments
 * between them, and costs route x feature vertices. OSM geometry is dense
 * enough for the 40 m threshold; move to point-segment distance and a grid
 * index if a sparse layer starts mislabelling.
 */
internal object RouteModes {
    enum class Surface { ROAD, TRAIL, UNKNOWN }

    /** How near a mapped line a vertex must fall before that line names it. */
    const val MATCH_M = 40.0

    private const val M_PER_DEG = 111_320.0
    private val NOT_A_WAY = setOf("water", "water_area", "park", "admin_county", "building_area")

    fun surfaceOf(feature: MapFeature): Surface? = when {
        feature.points.size < 2 -> null
        feature.kind.startsWith("trail") -> Surface.TRAIL
        feature.kind in NOT_A_WAY || feature.kind.startsWith("rail") -> null
        else -> Surface.ROAD
    }

    fun classify(
        route: List<GeoPoint>,
        features: List<MapFeature>,
        matchM: Double = MATCH_M,
    ): List<Pair<Surface, List<GeoPoint>>> {
        if (route.size < 2) return emptyList()
        val roads = features.filter { surfaceOf(it) == Surface.ROAD }.flatMap { it.points }
        val trails = features.filter { surfaceOf(it) == Surface.TRAIL }.flatMap { it.points }
        if (roads.isEmpty() && trails.isEmpty()) return emptyList()
        // Degrees of longitude shrink away from the equator; one cosine for the
        // whole route is plenty over the tens of metres being compared.
        val kx = cos(Math.toRadians(route.first().lat))
        val limit = (matchM / M_PER_DEG).let { it * it }
        val labels = route.map { point ->
            val road = nearestSq(point, roads, kx)
            val trail = nearestSq(point, trails, kx)
            when {
                minOf(road, trail) > limit -> Surface.UNKNOWN
                trail < road -> Surface.TRAIL
                else -> Surface.ROAD
            }
        }
        return runs(route, labels)
    }

    private fun nearestSq(point: GeoPoint, candidates: List<GeoPoint>, kx: Double): Double {
        var best = Double.MAX_VALUE
        candidates.forEach {
            val dy = it.lat - point.lat
            val dx = (it.lon - point.lon) * kx
            val d = dy * dy + dx * dx
            if (d < best) best = d
        }
        return best
    }

    /** Consecutive vertices of one surface, sharing the joint so no gap shows. */
    private fun runs(route: List<GeoPoint>, labels: List<Surface>): List<Pair<Surface, List<GeoPoint>>> {
        val out = mutableListOf<Pair<Surface, List<GeoPoint>>>()
        var start = 0
        for (i in 1..route.lastIndex) {
            if (labels[i] != labels[start]) {
                out += labels[start] to route.subList(start, i + 1)
                start = i
            }
        }
        out += labels[start] to route.subList(start, route.size)
        return out.filter { it.second.size > 1 }
    }
}

internal fun surfaceColor(surface: RouteModes.Surface): Color = when (surface) {
    RouteModes.Surface.ROAD -> RouteDrive
    RouteModes.Surface.TRAIL -> RouteWalk
    RouteModes.Surface.UNKNOWN -> RouteUnclassified
}

internal fun surfaceLabel(surface: RouteModes.Surface): String = when (surface) {
    RouteModes.Surface.ROAD -> "Drive"
    RouteModes.Surface.TRAIL -> "Walk"
    RouteModes.Surface.UNKNOWN -> "Unmapped"
}

/** Names the colours on the line, in the order they are walked. */
@Composable
private fun SurfaceLegend(parts: List<Pair<RouteModes.Surface, List<GeoPoint>>>) {
    val present = parts.map { it.first }.distinct()
    if (present.isEmpty()) return
    Card(shape = RoundedCornerShape(14.dp), colors = CardDefaults.cardColors(containerColor = Color.White.copy(alpha = .94f))) {
        Row(Modifier.padding(horizontal = 12.dp, vertical = 8.dp), horizontalArrangement = Arrangement.spacedBy(14.dp)) {
            present.forEach { surface ->
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    Box(Modifier.size(width = 18.dp, height = 5.dp).background(surfaceColor(surface), RoundedCornerShape(3.dp)))
                    Text(surfaceLabel(surface), style = MaterialTheme.typography.labelMedium, color = Forest)
                }
            }
        }
    }
}

/**
 * Trail: find a named trail or trailhead, walk it, keep the breadcrumb.
 *
 * Its own path, sharing nothing with Drive but the map canvas and the row
 * widgets. There is no turn-by-turn here on purpose: a footpath's turns are
 * the path, and a walker looking at a phone for them is the failure case.
 * The trails layer is the only thing searched.
 */
@Composable
internal fun TrailScreen(settings: AppSettings, onRequestPermissions: () -> Unit, onOpenMaps: () -> Unit) {
    val context = LocalContext.current
    val live by TraceBus.state.collectAsState()
    val repo = remember { PtilesRepository(context) }
    val scope = rememberCoroutineScope()
    val current = live.location?.let { GeoPoint(it.latitude, it.longitude) }
    val anchor = current ?: FALLBACK_ANCHOR
    var stops by remember { mutableStateOf(emptyList<Stop>()) }
    var query by remember { mutableStateOf("") }
    // Raw hits are held, not the rendered rows: the rows depend on where you
    // are, and that changes far more often than what is nearby.
    var hits by remember { mutableStateOf<List<PtilesRepository.BusinessResult>?>(null) }
    var status by remember { mutableStateOf<PickerState?>(PickerState.Searching) }
    var searchedAt by remember { mutableStateOf(anchor) }
    var searchedFor by remember { mutableStateOf("") }
    var features by remember { mutableStateOf(emptyList<MapFeature>()) }
    var route by remember { mutableStateOf<PtilesRepository.RouteResult?>(null) }
    var routeError by remember { mutableStateOf<String?>(null) }
    var routeRunning by remember { mutableStateOf(false) }
    var routeProgress by remember { mutableStateOf(0f) }
    var dataCenter by remember { mutableStateOf(anchor) }
    var panned by remember { mutableStateOf(false) }
    // The zoom the map is showing, so the fetch can match it: one net width
    // cannot serve a street corner and a hundred kilometres of it.
    var mapScale by remember { mutableStateOf(1f) }
    var panelOpen by remember { mutableStateOf(true) }
    var recenterKey by remember { mutableIntStateOf(0) }
    var fitKey by remember { mutableIntStateOf(0) }
    var routeParts by remember { mutableStateOf(emptyList<Pair<RouteModes.Surface, List<GeoPoint>>>()) }
    // The trail session, not the trail mode: background recording runs with a
    // mode too, and testing that hid this panel whenever the always-on log was
    // going.
    val walking = live.running && live.session == TraceRecorder.SESSION_TRAIL
    val imperial = settings.imperialUnits

    // A finished walk leaves no route card behind, same as a finished drive.
    LaunchedEffect(walking) {
        if (!walking) {
            route = null
            routeError = null
        }
    }

    LaunchedEffect(anchor.lat, anchor.lon) { if (!panned) dataCenter = anchor }

    val fetchSpread = MapDetail.fetchSpread(mapScale)
    val skipMinorRoads = MapDetail.skipsMinorRoads(mapScale)
    LaunchedEffect(dataCenter.lat, dataCenter.lon, fetchSpread, skipMinorRoads) {
        delay(VIEWPORT_DEBOUNCE_MS)
        // Two passes. The wide fetch is what makes panning land on ground that
        // is already decoded, but it is seconds of work on a cold cache, and
        // for those seconds the screen was blank paper. The narrow pass draws
        // what is under the user almost immediately, and the wide one replaces
        // it -- reusing the narrow pass's cells, which the per-centre cache
        // still holds.
        if (fetchSpread > NEAR_SPREAD) {
            features = withContext(Dispatchers.IO) {
                repo.featuresAround(
                    dataCenter.lat,
                    dataCenter.lon,
                    trails = true,
                    places = true,
                    spread = NEAR_SPREAD,
                    skipMinorRoads = skipMinorRoads,
                ).filter { it.kind != "building" }
            }
        }
        features = withContext(Dispatchers.IO) {
            repo.featuresAround(
                dataCenter.lat,
                dataCenter.lon,
                trails = true,
                places = true,
                spread = fetchSpread,
                skipMinorRoads = skipMinorRoads,
            ).filter { it.kind != "building" }
        }
    }

    // Trails are geographic, so the same query runs typed or not; typing only
    // narrows it. Every outcome is named rather than collapsing to an empty
    // list.
    val plannedPath = route?.points.orEmpty()
    // Queried on the query, not on the fix: re-running per GPS update reset the
    // scroll and swapped the row under a finger mid-tap. Only a real move asks
    // the layers again.
    val movedSinceSearch = GpxReader.distanceM(searchedAt, anchor) > SEARCH_REANCHOR_M
    LaunchedEffect(query, movedSinceSearch) {
        if (!movedSinceSearch && hits != null && query == searchedFor) return@LaunchedEffect
        // Blank the panel only when there is nothing on it.
        if (hits.isNullOrEmpty()) status = PickerState.Searching
        if (query.isNotBlank()) delay(SEARCH_DEBOUNCE_MS)
        val found = withContext(Dispatchers.IO) { runCatching { repo.trailsNearby(anchor, query) } }
        searchedAt = anchor
        searchedFor = query
        found.fold(
            onSuccess = { result ->
                hits = result.orEmpty()
                status = when {
                    result == null -> PickerState.NoMaps
                    result.isEmpty() && query.isBlank() -> PickerState.NoMatches("any named trail nearby")
                    result.isEmpty() -> PickerState.NoMatches(query)
                    else -> null
                }
            },
            onFailure = {
                hits = emptyList()
                status = PickerState.Failed(it.message ?: "the trails layer could not be read")
            },
        )
    }
    // Distance, bearing and "on your route" are pure functions of a hit and
    // where you are now, so they follow the fix without another decode.
    val picker: PickerState = status ?: PickerState.Found(
        hits.orEmpty().map {
            PlaceHit(
                name = it.name,
                point = it.point,
                distanceM = GpxReader.distanceM(anchor, it.point),
                bearingDeg = bearingDeg(anchor, it.point),
                onRoute = nearRoute(plannedPath, it.point, ON_ROUTE_M),
            )
        }
    )

    // Off the main thread: the match is every route vertex against every
    // decoded road and trail vertex in the viewport.
    LaunchedEffect(plannedPath, features) {
        routeParts = withContext(Dispatchers.Default) { RouteModes.classify(plannedPath, features) }
    }

    // What "show all" has to frame: the planned line, every stop on it, and
    // where the walker is now.
    val fitPoints = plannedPath + stops.map { it.point } + listOfNotNull(current)

    Box(Modifier.fillMaxSize()) {
        OfflineMap(
            center = anchor,
            features = features,
            current = current,
            destination = stops.lastOrNull()?.point,
            route = route?.points.orEmpty(),
            trace = live.recentPoints,
            // A tap on the map means "let me look at the map".
            onTap = { panelOpen = !panelOpen },
            onViewportChange = { viewport, scale ->
                mapScale = scale
                // Reload sooner while panning: waiting for a 400 m move meant
                // the map ran off the edge of its data before fetching more.
                if (GpxReader.distanceM(dataCenter, viewport) > VIEWPORT_RELOAD_M) {
                    panned = true
                    dataCenter = viewport
                }
            },
            recenterKey = recenterKey,
            routeParts = routeParts.map { (surface, points) -> points to surfaceColor(surface) },
            fitPoints = fitPoints,
            fitKey = fitKey,
        )
        Column(Modifier.fillMaxWidth().padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Card(shape = RoundedCornerShape(22.dp), colors = CardDefaults.cardColors(containerColor = Color.White.copy(alpha = .96f))) {
                Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    Text(
                        if (panelOpen) "TRAIL MODE" else "TRAIL MODE · tap the map to search",
                        Modifier.clickable { panelOpen = true },
                        style = MaterialTheme.typography.labelLarge,
                        color = ForestSoft,
                    )
                    // Outside the search panel: the panel hides once you are
                    // under way, and that is precisely when a dead fix matters.
                    if (live.awaitingFix) AwaitingFixNotice(live.running)
                    if (!walking && panelOpen) {
                        OutlinedTextField(
                            value = query,
                            onValueChange = { query = it },
                            modifier = Modifier.fillMaxWidth(),
                            singleLine = true,
                            leadingIcon = { Icon(Icons.Rounded.Search, null) },
                            trailingIcon = {
                                if (query.isNotEmpty()) IconButton(onClick = { query = "" }) {
                                    Icon(Icons.Rounded.Close, "Clear search")
                                }
                            },
                            label = { Text("Search") },
                            placeholder = { Text("Trail Name") },
                        )
                        PickerMessage(picker, onOpenMaps)
                        // Stops first, then a divider, then hits: what you have
                        // already chosen outranks what you are still browsing,
                        // and sharing one scroll box hid it entirely.
                        val hits = (picker as? PickerState.Found)?.hits.orEmpty()
                        if (stops.isNotEmpty()) {
                            Column(
                                Modifier.fillMaxWidth().heightIn(max = STOPS_MAX_HEIGHT).verticalScroll(rememberScrollState()),
                            ) {
                                StopList(
                                    stops = stops,
                                    onMove = { from, to -> stops = stops.move(from, to); route = null },
                                    onRemove = { index -> stops = stops.filterIndexed { at, _ -> at != index }; route = null },
                                )
                            }
                            HorizontalDivider()
                        }
                        if (hits.isNotEmpty()) {
                            LazyColumn(
                                Modifier.fillMaxWidth().heightIn(max = RESULTS_MAX_HEIGHT),
                            ) {
                                items(hits, key = { "${it.name}@${it.point.lat},${it.point.lon}" }) { hit ->
                                    PlaceRow(hit, imperial) {
                                        stops = stops + Stop(hit.name, hit.point)
                                        route = null
                                        routeError = null
                                    }
                                }
                            }
                        }
                        Button(
                            // Without a fix the route would start from
                            // FALLBACK_ANCHOR, which is a different state to
                            // most users. Better to wait than to plan from
                            // somewhere nobody is.
                            enabled = !routeRunning && !live.awaitingFix,
                            onClick = {
                                if (!hasLocationPermission(context)) {
                                    onRequestPermissions()
                                    return@Button
                                }
                                TraceService.start(context, LookyMode.TRAIL)
                                val chain = stops
                                val end = chain.lastOrNull()?.point ?: return@Button
                                routeRunning = true; routeError = null; routeProgress = 0f
                                scope.launch {
                                    runCatching {
                                        withContext(Dispatchers.Default) {
                                            repo.offlineRouteVia(
                                                anchor, chain.dropLast(1).map { it.point }, end, trail = true,
                                                settings.avoidHighways, settings.avoidIntersections,
                                            ) { done, total -> routeProgress = done.toFloat() / total }
                                        }
                                    }.onSuccess { route = it }.onFailure {
                                        routeError = it.message ?: "No connected path between these stops in the downloaded maps"
                                    }
                                    routeRunning = false
                                }
                            },
                            modifier = Modifier.fillMaxWidth().height(50.dp),
                            shape = RoundedCornerShape(16.dp),
                            colors = ButtonDefaults.buttonColors(containerColor = Forest),
                        ) {
                            if (routeRunning) {
                                CircularProgressIndicator(
                                    progress = { routeProgress },
                                    modifier = Modifier.size(20.dp),
                                    strokeWidth = 2.dp,
                                    color = Color.White,
                                )
                                Spacer(Modifier.width(10.dp))
                                Text("${(routeProgress * 100).roundToInt()}%")
                            } else {
                                Text(if (live.awaitingFix) "Waiting for GPS…" else "Start trail")
                            }
                        }
                    }
                }
            }
            route?.let {
                Card(colors = CardDefaults.cardColors(containerColor = Lime), shape = RoundedCornerShape(18.dp)) {
                    Row(Modifier.fillMaxWidth().padding(14.dp), horizontalArrangement = Arrangement.SpaceBetween) {
                        Text(formatDistance(it.distanceM, imperial), fontWeight = FontWeight.Black, color = Forest)
                        Text("${(it.durationS / 60).roundToInt().coerceAtLeast(1)} min walk", fontWeight = FontWeight.Bold, color = Forest)
                    }
                }
                SurfaceLegend(routeParts)
            }
            routeError?.let {
                Surface(color = Color(0xFFFFE4DA), shape = RoundedCornerShape(14.dp)) {
                    Text(it, Modifier.padding(12.dp), color = Color(0xFF7A2B16))
                }
            }
        }
        MapControls(
            canFit = fitPoints.size > 1,
            panned = panned,
            onFit = { fitKey++ },
            onRecenter = { panned = false; dataCenter = anchor; recenterKey++ },
        )
        LiveMetrics(imperial, Modifier.align(Alignment.BottomCenter))
    }
}
