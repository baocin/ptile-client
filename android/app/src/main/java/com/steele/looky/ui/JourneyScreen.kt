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
import androidx.compose.material.icons.rounded.DirectionsCar
import androidx.compose.material.icons.rounded.Search
import androidx.compose.material.icons.rounded.Terrain
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.FilterChip
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
import uniffi.ptiles_ffi.NavStateInfo
import uniffi.ptiles_ffi.Navigator
import uniffi.ptiles_ffi.TurnInfo
import kotlin.math.cos
import kotlin.math.roundToInt

/**
 * Says the map is not showing where you are.
 *
 * Without a fix every position-derived number on this screen -- the map
 * centre, the distance and bearing on each search hit, the route's first leg
 * -- is measured from [FALLBACK_ANCHOR], a place in Tennessee. Those numbers
 * used to render identically to real ones. This is not a spinner: the wait is
 * genuine and open-ended, and naming what is standing in for the answer is
 * more use than an animation.
 */
@Composable
internal fun AwaitingFixNotice(running: Boolean) {
    Surface(color = Color(0xFFFFF3D6), shape = RoundedCornerShape(14.dp), modifier = Modifier.fillMaxWidth()) {
        Row(Modifier.padding(12.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp, color = Forest)
            Text(
                if (running) "Waiting for the first GPS fix. Distances are measured from the map's default area, not from you."
                else "No GPS fix yet. Start recording to place yourself; until then distances are from the map's default area.",
                style = MaterialTheme.typography.bodySmall,
                color = Color(0xFF6B4E10),
            )
        }
    }
}

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
 * One journey screen: search, chain stops, route, and go.
 *
 * Drive and Trail used to be two screens on the argument that every shared
 * parameter was a place where changing one mode broke the other. They are now
 * one screen and a sort, because a walk that starts with a drive to the
 * trailhead was two screens' work and the tabs made the choice look bigger
 * than it is. What the sort still decides is real and unchanged: what is
 * searched, which routing profile runs, what is shown while under way, and
 * which recording session starts -- day files and the stop-journey control in
 * the top bar both key off that session, so it stays drive-or-trail.
 *
 * Drive is the default. Trail is the departure from it.
 */
@Composable
internal fun JourneyScreen(settings: AppSettings, onRequestPermissions: () -> Unit, onOpenMaps: () -> Unit) {
    val context = LocalContext.current
    val live by TraceBus.state.collectAsState()
    val repo = remember { PtilesRepository(context) }
    val scope = rememberCoroutineScope()
    val current = live.location?.let { GeoPoint(it.latitude, it.longitude) }
    val anchor = current ?: FALLBACK_ANCHOR
    // The last journey type is where the screen opens, so a walker who closed
    // the app on a trail does not land on Drive. Drive is what an install with
    // no history gets, and what AppSettings defaults to.
    // Drive is the default every time, not the last thing used: most journeys
    // are drives, and a walk last Tuesday is no reason to open in trail today.
    // A trail session actually recording is the one thing that outranks it,
    // since the screen must agree with the day file being written.
    var trail by remember {
        mutableStateOf(TraceBus.state.value.session == TraceRecorder.SESSION_TRAIL)
    }
    var stops by remember { mutableStateOf(emptyList<Stop>()) }
    var query by remember { mutableStateOf("") }
    // Raw hits are held, not the rendered rows: the rows depend on where you
    // are, and that changes far more often than what is nearby.
    var hits by remember { mutableStateOf<List<PtilesRepository.BusinessResult>?>(null) }
    var status by remember { mutableStateOf<PickerState?>(PickerState.Searching) }
    var searchedAt by remember { mutableStateOf(anchor) }
    var searchedFor by remember { mutableStateOf("") }
    var searchedTrail by remember { mutableStateOf(trail) }
    // Where the radius ladder starts. Zero is "near first, widen until there is
    // enough"; tapping "Search farther" pushes the floor out a rung, and past
    // the last rung the ceiling is gone and the installed packs answer whole.
    var fromRung by remember { mutableIntStateOf(0) }
    var searchedRung by remember { mutableIntStateOf(0) }
    var settledRung by remember { mutableIntStateOf(0) }
    var reach by remember { mutableStateOf<Double?>(null) }
    var beyond by remember { mutableIntStateOf(0) }
    var features by remember { mutableStateOf(emptyList<MapFeature>()) }
    var route by remember { mutableStateOf<PtilesRepository.RouteResult?>(null) }
    var routeError by remember { mutableStateOf<String?>(null) }
    var routeRunning by remember { mutableStateOf(false) }
    var routeProgress by remember { mutableStateOf(0f) }
    var navigator by remember { mutableStateOf<Navigator?>(null) }
    var turns by remember { mutableStateOf(emptyList<TurnInfo>()) }
    var navState by remember { mutableStateOf<NavStateInfo?>(null) }
    var routeParts by remember { mutableStateOf(emptyList<Pair<RouteModes.Surface, List<GeoPoint>>>()) }
    var dataCenter by remember { mutableStateOf(anchor) }
    var panned by remember { mutableStateOf(false) }
    // The zoom the map is showing, so the fetch can match it: one net width
    // cannot serve a street corner and a hundred kilometres of it.
    var mapScale by remember { mutableStateOf(1f) }
    var panelOpen by remember { mutableStateOf(true) }
    var recenterKey by remember { mutableIntStateOf(0) }
    var fitKey by remember { mutableIntStateOf(0) }
    // A journey is the session, not the mode. Background recording also runs
    // with a mode, and testing that instead hid this whole panel whenever the
    // always-on log was going -- which is always.
    val journeying = live.running && live.session != TraceRecorder.SESSION_BACKGROUND
    val imperial = settings.imperialUnits

    // The sort follows a running journey rather than the other way round. One
    // session records at a time and its day file is already committed to drive
    // or trail; a toggle that disagreed with the recorder is what the two tabs
    // used to be gated against.
    LaunchedEffect(journeying, live.mode) {
        if (journeying) trail = live.mode == LookyMode.TRAIL
    }

    // A drive route is not a walking route. Switching the sort throws away
    // everything computed under the old profile rather than redrawing a road
    // route in trail colours.
    LaunchedEffect(trail) {
        if (!journeying) {
            route = null
            routeError = null
            navigator = null
            navState = null
            turns = emptyList()
        }
    }

    LaunchedEffect(anchor.lat, anchor.lon) { if (!panned) dataCenter = anchor }

    // A new query starts near again. Having asked to search farther for one
    // thing is no reason to open the next search a hundred miles out.
    LaunchedEffect(query, trail) { fromRung = 0 }

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

    // Ending the journey ends everything that belonged to it: the turn card,
    // the route line, and the lime summary card, which used to sit there after
    // it was over describing a route nobody was on.
    LaunchedEffect(journeying) {
        if (!journeying) {
            navigator = null
            navState = null
            turns = emptyList()
            route = null
            routeError = null
        }
    }

    LaunchedEffect(navigator, live.location) {
        val fix = live.location ?: return@LaunchedEffect
        val nav = navigator ?: return@LaunchedEffect
        navState = withContext(Dispatchers.Default) {
            runCatching {
                nav.update(fix.latitude, fix.longitude, if (fix.hasAccuracy()) fix.accuracy.toDouble() else 0.0)
            }.getOrNull()
        }
    }

    // Empty box means "what is near me". Every outcome is named: hits, nothing
    // matched, no maps here, or a failure -- an empty list used to stand for
    // all four.
    val plannedPath = route?.points.orEmpty()
    // The layers are queried on the query and the sort, not on the fix.
    // Re-running this on every GPS update rebuilt the list roughly once a
    // second, which reset the scroll and swapped the row under a finger
    // mid-tap. Only a move far enough to change what is nearby is worth asking
    // again for.
    val movedSinceSearch = GpxReader.distanceM(searchedAt, anchor) > SEARCH_REANCHOR_M
    LaunchedEffect(query, trail, movedSinceSearch, fromRung) {
        if (!movedSinceSearch && hits != null && query == searchedFor && trail == searchedTrail &&
            fromRung == searchedRung
        ) {
            return@LaunchedEffect
        }
        // Blank the panel only when there is nothing on it: replacing a list of
        // real hits with "Searching..." and back is worse than a stale list.
        if (hits.isNullOrEmpty()) status = PickerState.Searching
        if (query.isNotBlank()) delay(SEARCH_DEBOUNCE_MS)
        val sort = trail
        val rung = fromRung
        val found = withContext(Dispatchers.IO) {
            runCatching { repo.journeyResults(anchor, query, trailSort = sort, fromRung = rung) }
        }
        searchedAt = anchor
        searchedFor = query
        searchedTrail = sort
        searchedRung = rung
        found.fold(
            onSuccess = { result ->
                hits = result?.hits.orEmpty()
                reach = result?.reachM
                beyond = result?.beyondReach ?: 0
                // The rung this settled on, not the one asked for: widening
                // stops as soon as a rung has enough, so "farther" has to mean
                // the one after that or it lands on the same answer again.
                settledRung = result?.rungIndex ?: rung
                status = when {
                    result == null -> PickerState.NoMaps
                    result.hits.isEmpty() && query.isBlank() ->
                        PickerState.NoMatches(if (sort) "any trail or park nearby" else "anything nearby")
                    result.hits.isEmpty() ->
                        PickerState.NoMatches(query, result.reachM, result.beyondReach)
                    else -> null
                }
            },
            onFailure = {
                hits = emptyList()
                status = PickerState.Failed(it.message ?: "the offline layers could not be read")
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
                note = it.note,
            )
        },
        // Only a typed query widens, so only a typed query has a reach worth
        // stating; a blank box is a browse of what is around you.
        reachM = reach.takeIf { query.isNotBlank() },
        more = if (query.isNotBlank()) beyond else 0,
    )

    // Off the main thread: the match is every route vertex against every
    // decoded road and trail vertex in the viewport. Only trail sort draws the
    // split, so drive does not pay for it.
    LaunchedEffect(plannedPath, features, trail) {
        routeParts = if (!trail) emptyList()
        else withContext(Dispatchers.Default) { RouteModes.classify(plannedPath, features) }
    }

    // What "show all" has to frame: the planned line, every stop on it, and
    // where the traveller is now.
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
                    Row(
                        Modifier.fillMaxWidth().clickable { panelOpen = true },
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        // The sort locks while a journey records: the day file
                        // is already one or the other.
                        FilterChip(
                            selected = !trail,
                            enabled = !journeying,
                            onClick = { trail = false },
                            leadingIcon = { Icon(Icons.Rounded.DirectionsCar, null, Modifier.size(18.dp)) },
                            label = { Text("Drive") },
                        )
                        FilterChip(
                            selected = trail,
                            enabled = !journeying,
                            onClick = { trail = true },
                            leadingIcon = { Icon(Icons.Rounded.Terrain, null, Modifier.size(18.dp)) },
                            label = { Text("Trail") },
                        )
                        if (!panelOpen) {
                            Text(
                                "tap the map to search",
                                style = MaterialTheme.typography.labelMedium,
                                color = ForestSoft,
                            )
                        }
                    }
                    // Outside the search panel: the panel hides once you are
                    // under way, and that is precisely when a dead fix matters.
                    if (live.awaitingFix) AwaitingFixNotice(live.running)
                    if (!journeying && panelOpen) {
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
                            placeholder = { Text(if (trail) "Trail or Park Name" else "Business Name") },
                        )
                        PickerMessage(
                            picker,
                            onOpenMaps,
                            imperial = imperial,
                            onSearchFarther = { fromRung = settledRung + 1 },
                        )
                        // Stops first, then a divider, then hits: what you have
                        // already chosen outranks what you are still browsing,
                        // and sharing one scroll box hid it entirely.
                        val rows = (picker as? PickerState.Found)?.hits.orEmpty()
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
                        if (rows.isNotEmpty()) {
                            LazyColumn(
                                Modifier.fillMaxWidth().heightIn(max = RESULTS_MAX_HEIGHT),
                            ) {
                                items(rows, key = { "${it.name}@${it.point.lat},${it.point.lon}" }) { hit ->
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
                                val onTrail = trail
                                TraceService.start(context, if (onTrail) LookyMode.TRAIL else LookyMode.DRIVE)
                                val chain = stops
                                val end = chain.lastOrNull()?.point ?: return@Button
                                routeRunning = true; routeError = null; routeProgress = 0f
                                scope.launch {
                                    runCatching {
                                        withContext(Dispatchers.Default) {
                                            val found = repo.offlineRouteVia(
                                                anchor, chain.dropLast(1).map { it.point }, end, trail = onTrail,
                                                settings.avoidHighways, settings.avoidIntersections,
                                            ) { done, total -> routeProgress = done.toFloat() / total }
                                            // Turn-by-turn is a driving idea. A
                                            // footpath's turns are the path, and
                                            // a walker reading a phone for them
                                            // is the failure case.
                                            found to if (onTrail) null else repo.navigatorFor(found.points)
                                        }
                                    }.onSuccess { (found, nav) ->
                                        route = found
                                        navigator = nav
                                        turns = nav?.let { runCatching { it.turns() }.getOrDefault(emptyList()) }.orEmpty()
                                    }.onFailure {
                                        routeError = it.message
                                            ?: "No connected ${if (onTrail) "path" else "road"} between these stops in the downloaded maps"
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
                                Text(
                                    when {
                                        live.awaitingFix -> "Waiting for GPS…"
                                        trail -> "Start trail"
                                        else -> "Start drive"
                                    }
                                )
                            }
                        }
                    }
                }
            }
            if (navigator != null) {
                TurnCard(navState, turns, imperial)
            } else route?.let {
                Card(colors = CardDefaults.cardColors(containerColor = Lime), shape = RoundedCornerShape(18.dp)) {
                    Row(Modifier.fillMaxWidth().padding(14.dp), horizontalArrangement = Arrangement.SpaceBetween) {
                        Text(formatDistance(it.distanceM, imperial), fontWeight = FontWeight.Black, color = Forest)
                        Text(
                            "${(it.durationS / 60).roundToInt().coerceAtLeast(1)} min${if (trail) " walk" else ""}",
                            fontWeight = FontWeight.Bold,
                            color = Forest,
                        )
                    }
                }
                if (trail) SurfaceLegend(routeParts)
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
