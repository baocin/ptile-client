package com.steele.looky.ui

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
import uniffi.ptiles_ffi.NavStateInfo
import uniffi.ptiles_ffi.Navigator
import uniffi.ptiles_ffi.TurnInfo
import kotlin.math.roundToInt

/**
 * Says the map is not showing where you are.
 *
 * Without a fix every position-derived number on these screens -- the map
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
 * Drive: search businesses, chain stops, route, and follow the turns.
 *
 * Deliberately its own path rather than a flag on a shared screen. Driving and
 * walking want different destinations, different networks, and different
 * things on screen while under way; every shared parameter here was a place
 * where changing one mode broke the other.
 */
@Composable
internal fun DriveScreen(settings: AppSettings, onRequestPermissions: () -> Unit, onOpenMaps: () -> Unit) {
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
    var navigator by remember { mutableStateOf<Navigator?>(null) }
    var turns by remember { mutableStateOf(emptyList<TurnInfo>()) }
    var navState by remember { mutableStateOf<NavStateInfo?>(null) }
    var dataCenter by remember { mutableStateOf(anchor) }
    var panned by remember { mutableStateOf(false) }
    // The zoom the map is showing, so the fetch can match it: one net width
    // cannot serve a street corner and a hundred kilometres of it.
    var mapScale by remember { mutableStateOf(1f) }
    var panelOpen by remember { mutableStateOf(true) }
    var recenterKey by remember { mutableIntStateOf(0) }
    var fitKey by remember { mutableIntStateOf(0) }
    // A drive is the session, not the mode. Background recording also runs
    // with mode DRIVE, and testing that instead hid this whole panel whenever
    // the always-on log was going -- which is always.
    val driving = live.running && live.session == TraceRecorder.SESSION_DRIVE
    val imperial = settings.imperialUnits

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

    // Ending the drive ends everything that belonged to it: the turn card, the
    // route line, and the lime summary card, which used to sit there after the
    // drive was over describing a route nobody was on.
    LaunchedEffect(driving) {
        if (!driving) {
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
    // The layers are queried on the query, not on the fix. Re-running this on
    // every GPS update rebuilt the list roughly once a second, which reset the
    // scroll and swapped the row under a finger mid-tap. Only a move far enough
    // to change what is nearby is worth asking again for.
    val movedSinceSearch = GpxReader.distanceM(searchedAt, anchor) > SEARCH_REANCHOR_M
    LaunchedEffect(query, movedSinceSearch) {
        if (!movedSinceSearch && hits != null && query == searchedFor) return@LaunchedEffect
        // Blank the panel only when there is nothing on it: replacing a list of
        // real hits with "Searching..." and back is worse than a stale list.
        if (hits.isNullOrEmpty()) status = PickerState.Searching
        if (query.isNotBlank()) delay(SEARCH_DEBOUNCE_MS)
        val found = withContext(Dispatchers.IO) {
            runCatching {
                if (query.isBlank()) repo.businessesNearby(anchor) else repo.searchBusinesses(query, anchor)
            }
        }
        searchedAt = anchor
        searchedFor = query
        found.fold(
            onSuccess = { result ->
                hits = result.orEmpty()
                status = when {
                    result == null -> PickerState.NoMaps
                    result.isEmpty() && query.isBlank() -> PickerState.NoMatches("anything nearby")
                    result.isEmpty() -> PickerState.NoMatches(query)
                    else -> null
                }
            },
            onFailure = {
                hits = emptyList()
                status = PickerState.Failed(it.message ?: "the business layer could not be read")
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

    // What "show all" has to frame: the planned line, every stop on it, and
    // where the driver is now.
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
            fitPoints = fitPoints,
            fitKey = fitKey,
        )
        Column(Modifier.fillMaxWidth().padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Card(shape = RoundedCornerShape(22.dp), colors = CardDefaults.cardColors(containerColor = Color.White.copy(alpha = .96f))) {
                Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    Text(
                        if (panelOpen) "DRIVE MODE" else "DRIVE MODE · tap the map to search",
                        Modifier.clickable { panelOpen = true },
                        style = MaterialTheme.typography.labelLarge,
                        color = ForestSoft,
                    )
                    // Outside the search panel: the panel hides once you are
                    // under way, and that is precisely when a dead fix matters.
                    if (live.awaitingFix) AwaitingFixNotice(live.running)
                    if (!driving && panelOpen) {
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
                            placeholder = { Text("Business Name") },
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
                                TraceService.start(context, LookyMode.DRIVE)
                                val chain = stops
                                val end = chain.lastOrNull()?.point ?: return@Button
                                routeRunning = true; routeError = null; routeProgress = 0f
                                scope.launch {
                                    runCatching {
                                        withContext(Dispatchers.Default) {
                                            val found = repo.offlineRouteVia(
                                                anchor, chain.dropLast(1).map { it.point }, end, trail = false,
                                                settings.avoidHighways, settings.avoidIntersections,
                                            ) { done, total -> routeProgress = done.toFloat() / total }
                                            found to repo.navigatorFor(found.points)
                                        }
                                    }.onSuccess { (found, nav) ->
                                        route = found
                                        navigator = nav
                                        turns = nav?.let { runCatching { it.turns() }.getOrDefault(emptyList()) }.orEmpty()
                                    }.onFailure {
                                        routeError = it.message ?: "No connected road between these stops in the downloaded maps"
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
                                Text(if (live.awaitingFix) "Waiting for GPS…" else "Start drive")
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
                        Text("${(it.durationS / 60).roundToInt().coerceAtLeast(1)} min", fontWeight = FontWeight.Bold, color = Forest)
                    }
                }
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
