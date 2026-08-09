package com.steele.looky.ui

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
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
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.ArrowBack
import androidx.compose.material.icons.rounded.Add
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.Delete
import androidx.compose.material.icons.rounded.DirectionsCar
import androidx.compose.material.icons.rounded.Folder
import androidx.compose.material.icons.rounded.Map
import androidx.compose.material.icons.rounded.MoreHoriz
import androidx.compose.material.icons.rounded.MyLocation
import androidx.compose.material.icons.rounded.Route
import androidx.compose.material.icons.rounded.Search
import androidx.compose.material.icons.rounded.Settings
import androidx.compose.material.icons.rounded.Terrain
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
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
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.foundation.Image
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.material.icons.rounded.DragHandle
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.zIndex
import androidx.compose.foundation.layout.offset
import androidx.core.content.ContextCompat
import com.steele.looky.AppSettings
import com.steele.looky.R
import com.steele.looky.location.TraceRecorder
import com.steele.looky.location.TraceService
import com.steele.looky.model.GeoPoint
import com.steele.looky.model.LookyMode
import com.steele.looky.model.TraceBus
import com.steele.looky.offline.PackManager
import com.steele.looky.offline.MapDownloadProgress
import com.steele.looky.offline.MapPackDownloader
import com.steele.looky.offline.PtilesRepository
import uniffi.ptiles_ffi.BusinessInfo
import androidx.activity.compose.BackHandler
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File
import kotlin.math.roundToInt

private enum class Screen { DRIVE, TRAIL, MORE, RECORDINGS, PACKS, SETTINGS, DEVELOPER }

/** Settle time before a panned viewport triggers a PTiles decode. */
private const val VIEWPORT_DEBOUNCE_MS = 400L

/** Settle time before a search query hits the name indexes. */
private const val SEARCH_DEBOUNCE_MS = 300L

/** How far the viewport must move before reloading features, in metres. */
private const val VIEWPORT_RELOAD_M = 400.0

/** Height cap on the hits-and-stops list before it scrolls inside the card. */
private val LIST_MAX_HEIGHT = 200.dp

private val STOP_ROW_HEIGHT = 52.dp

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun LookyApp(
    settings: AppSettings,
    onRequestPermissions: () -> Unit,
    initialStateCode: String? = null,
) {
    val context = LocalContext.current
    val live by TraceBus.state.collectAsState()
    val coverageRepository = remember { PtilesRepository(context) }
    var mapsRevision by remember { mutableIntStateOf(0) }
    var mapsReady by remember { mutableStateOf(false) }
    var currentStateCode by remember { mutableStateOf<String?>(null) }
    var screen by remember {
        mutableStateOf(if (settings.activeMode == LookyMode.TRAIL) Screen.TRAIL else Screen.DRIVE)
    }
    val currentLat = live.location?.latitude
    val currentLon = live.location?.longitude
    LaunchedEffect(initialStateCode) {
        if (currentLat == null && initialStateCode != null) currentStateCode = initialStateCode
    }
    LaunchedEffect(currentLat, currentLon, mapsRevision) {
        if (currentLat == null || currentLon == null) {
            mapsReady = false
        } else {
            val coverage = withContext(Dispatchers.IO) {
                coverageRepository.currentStateCode(currentLat, currentLon) to
                    coverageRepository.mapsReadyAt(currentLat, currentLon)
            }
            currentStateCode = coverage.first
            mapsReady = coverage.second
        }
    }
    val root = screen in setOf(Screen.DRIVE, Screen.TRAIL, Screen.MORE)
    // Back walks up the app instead of leaving it. Only Drive, the home
    // screen, hands the gesture back to the system.
    BackHandler(screen != Screen.DRIVE) {
        screen = if (root) Screen.DRIVE else Screen.MORE
    }
    Scaffold(
        topBar = {
            TopAppBar(
                title = {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Image(painterResource(R.drawable.ic_looky), "Looky", Modifier.size(36.dp))
                        Spacer(Modifier.width(10.dp))
                        Text(if (root) "Looky" else screen.title(), fontWeight = FontWeight.Black)
                    }
                },
                navigationIcon = {
                    if (!root) IconButton(onClick = { screen = Screen.MORE }) {
                        Icon(Icons.AutoMirrored.Rounded.ArrowBack, "Back")
                    }
                },
                actions = {
                    Surface(
                        color = if (mapsReady) Lime.copy(alpha = .45f) else Clay.copy(alpha = .55f),
                        shape = RoundedCornerShape(100.dp),
                        // The badge is the only place the app admits a pack is
                        // missing, so it is also the fastest way to go fix it.
                        modifier = Modifier.clickable { screen = Screen.PACKS },
                    ) {
                        Text(
                            if (mapsReady) "Ready" else "Downloads Needed",
                            Modifier.padding(horizontal = 11.dp, vertical = 6.dp),
                            style = MaterialTheme.typography.labelLarge,
                            color = Forest,
                        )
                    }
                    Spacer(Modifier.width(12.dp))
                },
                colors = TopAppBarDefaults.topAppBarColors(containerColor = Paper),
            )
        },
        bottomBar = {
            if (root) NavigationBar(containerColor = Color.White) {
                // Tabs navigate only. Starting the service here re-labelled a
                // running Drive session as Trail the moment the Trail tab was
                // opened, so Trail read as in-progress when the user was just
                // looking. Recording starts from the Start button or the
                // Settings toggle -- the two places it is actually asked for.
                // One session records at a time, so the other mode's tab greys
                // out while a drive or trail is running. Background recording
                // is nobody's session and blocks neither tab.
                NavigationBarItem(
                    selected = screen == Screen.DRIVE,
                    enabled = live.session != TraceRecorder.SESSION_TRAIL || !live.running,
                    onClick = { screen = Screen.DRIVE },
                    icon = { Icon(Icons.Rounded.DirectionsCar, null) }, label = { Text("Drive") },
                )
                NavigationBarItem(
                    selected = screen == Screen.TRAIL,
                    enabled = live.session != TraceRecorder.SESSION_DRIVE || !live.running,
                    onClick = { screen = Screen.TRAIL },
                    icon = { Icon(Icons.Rounded.Terrain, null) }, label = { Text("Trail") },
                )
                NavigationBarItem(
                    selected = screen == Screen.MORE,
                    onClick = { screen = Screen.MORE },
                    icon = { Icon(Icons.Rounded.MoreHoriz, null) }, label = { Text("More") },
                )
            }
        },
        containerColor = Paper,
    ) { padding ->
        Box(Modifier.fillMaxSize().padding(padding)) {
            when (screen) {
                Screen.DRIVE -> DriveScreen(settings, onRequestPermissions)
                Screen.TRAIL -> TrailScreen(settings, onRequestPermissions)
                Screen.MORE -> MoreScreen(settings) { screen = it }
                Screen.RECORDINGS -> RecordingsScreen()
                Screen.PACKS -> PacksScreen(currentStateCode) { mapsRevision++ }
                Screen.SETTINGS -> SettingsScreen(settings, onRequestPermissions)
                Screen.DEVELOPER -> DeveloperMapScreen()
            }
        }
    }
}

private fun Screen.title() = when (this) {
    Screen.RECORDINGS -> "Recordings"
    Screen.PACKS -> "Offline maps"
    Screen.SETTINGS -> "Settings"
    Screen.DEVELOPER -> "Developer map"
    else -> "Looky"
}

/** Drive: search, destination chain, offline route, and its own recording. */
@Composable
private fun DriveScreen(settings: AppSettings, onRequestPermissions: () -> Unit) =
    ModeMap(trail = false, routing = true, settings = settings, onRequestPermissions = onRequestPermissions)

/**
 * Trail: a log and a breadcrumb, nothing else.
 *
 * A walk has no destination to pick, so the whole search-and-route panel is
 * left out rather than shown and ignored.
 */
@Composable
private fun TrailScreen(settings: AppSettings, onRequestPermissions: () -> Unit) =
    ModeMap(trail = true, routing = false, settings = settings, onRequestPermissions = onRequestPermissions)

@Composable
private fun ModeMap(trail: Boolean, routing: Boolean, settings: AppSettings, onRequestPermissions: () -> Unit) {
    val context = LocalContext.current
    val live by TraceBus.state.collectAsState()
    val repo = remember { PtilesRepository(context) }
    val scope = rememberCoroutineScope()
    val current = live.location?.let { GeoPoint(it.latitude, it.longitude) }
    val anchor = current ?: GeoPoint(35.73377, -88.03220)
    // One ordered list, no separate destination field: the last stop is the
    // destination by definition, which is also what a one-stop route means.
    var stops by remember(trail) { mutableStateOf(emptyList<Stop>()) }
    var query by remember(trail) { mutableStateOf("") }
    var results by remember(trail) { mutableStateOf(emptyList<PtilesRepository.BusinessResult>()) }
    var features by remember { mutableStateOf(emptyList<com.steele.looky.model.MapFeature>()) }
    var route by remember { mutableStateOf<PtilesRepository.RouteResult?>(null) }
    var routeError by remember { mutableStateOf<String?>(null) }
    var routeRunning by remember { mutableStateOf(false) }
    var routeProgress by remember { mutableStateOf(0f) }
    // dataCenter is where PTiles data is loaded from; anchor is where the map
    // is projected from. Panning moves the first without disturbing the second,
    // which is what stops a reload from yanking the view back.
    var dataCenter by remember(trail) { mutableStateOf(anchor) }
    var panned by remember(trail) { mutableStateOf(false) }
    var recenterKey by remember(trail) { mutableIntStateOf(0) }
    val requestedMode = if (trail) LookyMode.TRAIL else LookyMode.DRIVE
    val active = live.running && live.mode == requestedMode
    val imperial = settings.imperialUnits
    val hasLayers = remember(live.running) { repo.installedLayers().isNotEmpty() }

    // Follow the GPS fix until the user pans away from it.
    LaunchedEffect(anchor.lat, anchor.lon) {
        if (!panned) dataCenter = anchor
    }

    LaunchedEffect(dataCenter.lat, dataCenter.lon, trail) {
        delay(VIEWPORT_DEBOUNCE_MS)
        features = withContext(Dispatchers.IO) { repo.featuresAround(dataCenter.lat, dataCenter.lon, trail) }
    }

    LaunchedEffect(query) {
        if (query.isBlank()) {
            results = emptyList()
        } else {
            delay(SEARCH_DEBOUNCE_MS)
            results = withContext(Dispatchers.IO) { repo.searchBusinesses(query) }
        }
    }

    Box(Modifier.fillMaxSize()) {
        OfflineMap(
            center = anchor,
            features = features,
            current = current,
            destination = stops.lastOrNull()?.point,
            route = route?.points.orEmpty(),
            trace = live.recentPoints,
            onViewportChange = { viewport ->
                // Ignore sub-tile drags so a nudge does not trigger a decode.
                if (GpxReader.distanceM(dataCenter, viewport) > VIEWPORT_RELOAD_M) {
                    panned = true
                    dataCenter = viewport
                }
            },
            recenterKey = recenterKey,
        )
        Column(Modifier.fillMaxWidth().padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Card(shape = RoundedCornerShape(22.dp), colors = CardDefaults.cardColors(containerColor = Color.White.copy(alpha = .96f))) {
                Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Column(Modifier.weight(1f)) {
                            Text(if (trail) "TRAIL MODE" else "DRIVE MODE", style = MaterialTheme.typography.labelLarge, color = ForestSoft)
                        }
                        // No badge until there is a real classification: the
                        // bus starts at "Unknown", and printing that is worse
                        // than printing nothing.
                        if (live.running && hasLayers && live.movement != "Unknown") {
                            Surface(color = Lime, shape = CircleShape) {
                                Text(
                                    live.movement,
                                    Modifier.padding(horizontal = 12.dp, vertical = 8.dp),
                                    style = MaterialTheme.typography.labelLarge,
                                )
                            }
                        }
                    }
                    if (routing) {
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
                        // Both lists are height-capped and scroll inside the
                        // card. Unbounded, a dozen stops grew the panel over
                        // the whole map and pushed Route offline off-screen.
                        if (results.isNotEmpty() || stops.isNotEmpty()) {
                            Column(
                                Modifier.fillMaxWidth().heightIn(max = LIST_MAX_HEIGHT).verticalScroll(rememberScrollState()),
                            ) {
                                results.forEach { hit ->
                                    BusinessRow(
                                        hit = hit,
                                        distanceM = GpxReader.distanceM(anchor, hit.point),
                                        imperial = imperial,
                                        onAdd = {
                                            stops = stops + Stop(hit.name, hit.point)
                                            route = null
                                            routeError = null
                                            query = ""
                                        },
                                    )
                                }
                                StopList(
                                    stops = stops,
                                    onMove = { from, to -> stops = stops.move(from, to); route = null },
                                    onRemove = { index -> stops = stops.filterIndexed { at, _ -> at != index }; route = null },
                                )
                            }
                        }
                        Button(
                            enabled = stops.isNotEmpty() && !routeRunning,
                            onClick = {
                                val chain = stops
                                val end = chain.lastOrNull()?.point ?: return@Button
                                routeRunning = true; routeError = null; routeProgress = 0f
                                scope.launch {
                                    runCatching {
                                        withContext(Dispatchers.Default) {
                                            repo.offlineRouteVia(
                                                anchor, chain.dropLast(1).map { it.point }, end, trail,
                                                settings.avoidHighways, settings.avoidIntersections,
                                            ) { done, total -> routeProgress = done.toFloat() / total }
                                        }
                                    }.onSuccess { route = it }.onFailure { routeError = it.message ?: "No connected route in this pack" }
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
                                Text("Route offline")
                            }
                        }
                    }
                    // Recording is its own action. Routing used to start it as a
                    // side effect, which let the Drive tab relabel a running
                    // Trail session. Starting here takes over from whatever was
                    // running; the old session's day file is already closed.
                    Button(
                        onClick = {
                            when {
                                active -> TraceService.stop(context)
                                hasLocationPermission(context) -> TraceService.start(context, requestedMode)
                                else -> onRequestPermissions()
                            }
                        },
                        modifier = Modifier.fillMaxWidth().height(50.dp),
                        shape = RoundedCornerShape(16.dp),
                        colors = ButtonDefaults.buttonColors(containerColor = if (active) Clay else Forest),
                    ) {
                        Text(if (active) "Stop ${if (trail) "trail" else "drive"}" else "Start ${if (trail) "trail" else "drive"}")
                    }
                }
            }
            route?.let {
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
        if (panned) {
            FilledTonalButton(
                onClick = {
                    panned = false
                    dataCenter = anchor
                    recenterKey++
                },
                modifier = Modifier.align(Alignment.BottomEnd).padding(end = 16.dp, bottom = 96.dp),
                shape = RoundedCornerShape(16.dp),
            ) {
                Icon(Icons.Rounded.MyLocation, null)
                Spacer(Modifier.width(8.dp))
                Text("Recenter")
            }
        }
        Card(
            modifier = Modifier.align(Alignment.BottomCenter).padding(16.dp),
            shape = RoundedCornerShape(18.dp),
            colors = CardDefaults.cardColors(containerColor = Color(0xEE173F35)),
        ) {
            Row(Modifier.fillMaxWidth().padding(14.dp), horizontalArrangement = Arrangement.SpaceAround) {
                Metric(if (live.location?.hasAccuracy() == true) "±${live.location?.accuracy?.roundToInt()} m" else "—", "GPS")
                Metric(formatDistance(live.distanceM, imperial), "TODAY")
                Metric(live.pointsToday.toString(), "POINTS")
            }
        }
    }
}

/** A place on the route. The last one in the list is the destination. */
internal data class Stop(val label: String, val point: GeoPoint)

internal fun List<Stop>.move(from: Int, to: Int): List<Stop> {
    if (from == to || from !in indices || to !in indices) return this
    val out = toMutableList()
    out.add(to, out.removeAt(from))
    return out
}

@Composable
private fun BusinessRow(
    hit: PtilesRepository.BusinessResult,
    distanceM: Double,
    imperial: Boolean,
    onAdd: () -> Unit,
) {
    Row(Modifier.fillMaxWidth().clickable(onClick = onAdd), verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f).padding(vertical = 6.dp)) {
            Text(hit.name, style = MaterialTheme.typography.titleSmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(formatDistance(distanceM, imperial), style = MaterialTheme.typography.bodySmall, color = ForestSoft)
        }
        IconButton(onClick = onAdd) { Icon(Icons.Rounded.Add, "Add stop") }
    }
}

/**
 * The stop chain, reordered by dragging a row's handle.
 *
 * ponytail: rows are a fixed height so the drop index is offset/height. If the
 * rows ever wrap to two lines, measure them instead.
 */
@Composable
private fun StopList(stops: List<Stop>, onMove: (Int, Int) -> Unit, onRemove: (Int) -> Unit) {
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

@Composable
private fun Metric(value: String, label: String) {
    Column(horizontalAlignment = Alignment.CenterHorizontally) {
        Text(value, color = Color.White, fontWeight = FontWeight.Black)
        Text(label, color = Lime, style = MaterialTheme.typography.labelSmall)
    }
}

@Composable
private fun MoreScreen(settings: AppSettings, open: (Screen) -> Unit) {
    LazyColumn(Modifier.fillMaxSize().padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        item {
            Text("Everything stays on this phone.", style = MaterialTheme.typography.headlineMedium)
            Text("Maps, routes, and day files remain useful in airplane mode.", color = Color(0xFF626A65))
        }
        item { MenuCard(Icons.Rounded.Route, "Recordings", "Durable GPX day files", { open(Screen.RECORDINGS) }) }
        item { MenuCard(Icons.Rounded.Folder, "Offline maps", "Download and inspect PTiles packs", { open(Screen.PACKS) }) }
        item { MenuCard(Icons.Rounded.Settings, "Settings", "Recording, routes, and developer tools", { open(Screen.SETTINGS) }) }
        if (settings.developerMapEnabled) {
            item { MenuCard(Icons.Rounded.Map, "Developer map", "Experimental PTiles layers and diagnostics", { open(Screen.DEVELOPER) }) }
        }
    }
}

@Composable
private fun MenuCard(icon: ImageVector, title: String, subtitle: String, onClick: () -> Unit) {
    Card(
        Modifier.fillMaxWidth().clickable(onClick = onClick),
        shape = RoundedCornerShape(20.dp),
        colors = CardDefaults.cardColors(containerColor = Color.White),
    ) {
        Row(Modifier.padding(18.dp), verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.size(46.dp).background(Lime, RoundedCornerShape(14.dp)), contentAlignment = Alignment.Center) {
                Icon(icon, null, tint = Forest)
            }
            Spacer(Modifier.width(14.dp))
            Column {
                Text(title, style = MaterialTheme.typography.titleMedium)
                Text(subtitle, color = Color(0xFF69716C), maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        }
    }
}

@Composable
private fun SettingsScreen(settings: AppSettings, onRequestPermissions: () -> Unit) {
    val context = LocalContext.current
    var recording by remember { mutableStateOf(settings.continuousRecording) }
    var developer by remember { mutableStateOf(settings.developerMapEnabled) }
    var imperial by remember { mutableStateOf(settings.imperialUnits) }
    var highways by remember { mutableStateOf(settings.avoidHighways) }
    var intersections by remember { mutableStateOf(settings.avoidIntersections) }
    var gpsSeconds by remember { mutableStateOf(settings.gpsIntervalSeconds) }
    var accelHz by remember { mutableStateOf(settings.accelerometerRateHz) }
    LazyColumn(Modifier.fillMaxSize().padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        item { SettingsToggle("Continuous movement traces", "Foreground service writes a GPX day file after every fix.", recording) {
            if (it && !hasLocationPermission(context)) {
                recording = false
                settings.continuousRecording = false
                onRequestPermissions()
            } else {
                recording = it
                settings.continuousRecording = it
                if (it) TraceService.startBackground(context) else TraceService.stop(context)
            }
        } }
        item { SettingsToggle("Imperial units", "Feet and miles instead of metres and kilometres.", imperial) {
            imperial = it; settings.imperialUnits = it
        } }
        item { SettingsToggle("Developer map", "Show experimental layers and diagnostics. On by default during development.", developer) {
            developer = it; settings.developerMapEnabled = it
        } }
        item { Text("RECORDING RATE", style = MaterialTheme.typography.labelLarge, color = ForestSoft, modifier = Modifier.padding(top = 8.dp)) }
        item { RateSetting("GPS polling", "How often Looky requests a location fix.", "${gpsSeconds}s", listOf(3, 5, 7, 10, 15, 30), gpsSeconds) {
            gpsSeconds = it; settings.gpsIntervalSeconds = it; TraceService.applySettings(context)
        } }
        item { RateSetting("Accelerometer polling", "Samples used by PTiles motion classification.", "${accelHz} Hz", listOf(10, 25, 50, 100), accelHz) {
            accelHz = it; settings.accelerometerRateHz = it; TraceService.applySettings(context)
        } }
        item { Text("ROUTING", style = MaterialTheme.typography.labelLarge, color = ForestSoft, modifier = Modifier.padding(top = 8.dp)) }
        item { SettingsToggle("Avoid highways", "Prefer local roads when a connected alternative exists.", highways) {
            highways = it; settings.avoidHighways = it
        } }
        item { SettingsToggle("Avoid intersections", "Prefer routes with fewer junctions.", intersections) {
            intersections = it; settings.avoidIntersections = it
        } }
    }
}

@Composable
private fun RateSetting(title: String, subtitle: String, valueLabel: String, values: List<Int>, value: Int, onChange: (Int) -> Unit) {
    Card(colors = CardDefaults.cardColors(containerColor = Color.White), shape = RoundedCornerShape(18.dp)) {
        Row(Modifier.fillMaxWidth().padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) { Text(title, style = MaterialTheme.typography.titleMedium); Text(subtitle, color = Color(0xFF69716C)) }
            FilledTonalButton(onClick = { onChange(values[(values.indexOf(value).coerceAtLeast(0) + 1) % values.size]) }) { Text(valueLabel) }
        }
    }
}

private fun hasLocationPermission(context: android.content.Context): Boolean =
    ContextCompat.checkSelfPermission(context, Manifest.permission.ACCESS_FINE_LOCATION) == PackageManager.PERMISSION_GRANTED ||
        ContextCompat.checkSelfPermission(context, Manifest.permission.ACCESS_COARSE_LOCATION) == PackageManager.PERMISSION_GRANTED

@Composable
private fun SettingsToggle(title: String, subtitle: String, checked: Boolean, onChange: (Boolean) -> Unit) {
    Card(colors = CardDefaults.cardColors(containerColor = Color.White), shape = RoundedCornerShape(18.dp)) {
        Row(Modifier.fillMaxWidth().padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text(title, style = MaterialTheme.typography.titleMedium)
                Text(subtitle, style = MaterialTheme.typography.bodySmall, color = Color(0xFF69716C))
            }
            Switch(checked, onChange)
        }
    }
}

@Composable
private fun RecordingsScreen() {
    val context = LocalContext.current
    val traces = remember { File(context.filesDir, "traces").listFiles().orEmpty().filter { it.extension == "gpx" }.sortedDescending() }
    var open by remember { mutableStateOf<File?>(null) }
    open?.let { file ->
        BackHandler { open = null }
        RecordingDetailScreen(file)
        return
    }
    if (traces.isEmpty()) {
        EmptyState("No day files yet", "Start Drive or Trail and the first accurate fix will create one.")
        return
    }
    // Three logs, three sections: a drive, a walk, and the always-on background
    // recording are different journeys and were never worth reading merged.
    val sections = listOf(
        "Drives" to TraceRecorder.SESSION_DRIVE,
        "Trails" to TraceRecorder.SESSION_TRAIL,
        "Background" to TraceRecorder.SESSION_BACKGROUND,
    ).mapNotNull { (title, session) ->
        traces.filter { TraceRecorder.sessionOf(it) == session }.takeIf { it.isNotEmpty() }?.let { title to it }
    }
    LazyColumn(Modifier.fillMaxSize().padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        sections.forEach { (title, files) ->
        item(key = title) {
            Text(title.uppercase(), style = MaterialTheme.typography.labelLarge, color = ForestSoft)
        }
        items(files, key = File::getName) { file ->
            Card(
                Modifier.clickable { open = file },
                colors = CardDefaults.cardColors(containerColor = Color.White),
                shape = RoundedCornerShape(18.dp),
            ) {
                Row(Modifier.fillMaxWidth().padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f)) {
                        Text(
                            TraceRecorder.dateOf(file)?.toString() ?: file.nameWithoutExtension,
                            style = MaterialTheme.typography.titleMedium,
                        )
                        Text("${file.length() / 1024} KB · GPX 1.1", color = Color(0xFF69716C))
                    }
                    Text("Open", color = Forest, fontWeight = FontWeight.Bold)
                }
            }
        }
        }
    }
}

@Composable
private fun PacksScreen(currentStateCode: String?, onPacksChanged: () -> Unit) {
    val context = LocalContext.current
    val manager = remember { PackManager(context) }
    var packs by remember { mutableStateOf(manager.packs()) }
    var message by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()
    var progress by remember { mutableStateOf<MapDownloadProgress?>(null) }
    var downloading by remember { mutableStateOf(false) }
    val installedByRegion = packs.associateBy { it.region }
    fun download(states: List<String>, includeUsLayers: Boolean = false) {
        if (downloading) return
        downloading = true
        scope.launch {
            MapPackDownloader.downloadStates(context, states, { progress = it }, includeUsLayers)
                .onSuccess {
                    message = if (states.size == 1) "${states.first()} offline maps installed" else "All US PTiles layers installed"
                    onPacksChanged()
                }
                .onFailure { message = it.message ?: "Download failed" }
            packs = manager.packs(); downloading = false
        }
    }
    LazyColumn(Modifier.fillMaxSize().padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        item {
            Button(enabled = !downloading && currentStateCode != null, onClick = {
                val state = currentStateCode ?: return@Button
                downloading = true
                scope.launch {
                    MapPackDownloader.downloadCurrentState(context, state) { progress = it }
                        .onSuccess {
                            message = "${com.steele.looky.offline.StateResolver.name(state)} offline maps installed"
                            onPacksChanged()
                        }
                        .onFailure { message = it.message ?: "Download failed" }
                    packs = manager.packs(); downloading = false
                }
            }, modifier = Modifier.fillMaxWidth(), shape = RoundedCornerShape(16.dp)) {
                Text(
                    if (downloading) "Downloading ${progress?.completed ?: 0}/${progress?.total ?: 0}…"
                    else "Download ${com.steele.looky.offline.StateResolver.name(currentStateCode) ?: "your state"}",
                )
            }
        }
        item {
            Button(enabled = !downloading, onClick = { download(MapPackDownloader.US_STATES, includeUsLayers = true) }, modifier = Modifier.fillMaxWidth().height(56.dp), shape = RoundedCornerShape(16.dp), colors = ButtonDefaults.buttonColors(containerColor = Forest, contentColor = Color.White)) {
                Text("Download all US PTiles layers")
            }
        }
        progress?.let { item { Text("${it.completed}/${it.total} · ${it.layer}", color = ForestSoft) } }
        message?.let { item { Text(it, color = Forest) } }
        item { Text("STATES", style = MaterialTheme.typography.labelLarge, color = ForestSoft, modifier = Modifier.padding(top = 8.dp)) }
        items(MapPackDownloader.US_STATES, key = { it }) { region ->
            val pack = installedByRegion[region]
            var expanded by remember { mutableStateOf(false) }
            var confirmDelete by remember { mutableStateOf(false) }
            val active = progress?.takeIf { downloading && it.region == region }
            Card(colors = CardDefaults.cardColors(containerColor = Color.White), shape = RoundedCornerShape(18.dp)) {
                Column(Modifier.padding(16.dp)) {
                    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                        Column(Modifier.weight(1f)) {
                            Text(region, style = MaterialTheme.typography.titleLarge)
                            Text(
                                when {
                                    active != null -> "${active.layer} · ${formatBytes(active.bytes)}"
                                    pack == null -> "Not downloaded"
                                    else -> "${pack.layers.size} layers · ${formatBytes(pack.bytes)}"
                                },
                                color = Color(0xFF69716C),
                            )
                        }
                        if (pack != null && !downloading) {
                            IconButton(onClick = { confirmDelete = true }) {
                                Icon(Icons.Rounded.Delete, "Delete $region maps", tint = Clay)
                            }
                        }
                        FilledTonalButton(enabled = !downloading, onClick = { download(listOf(region)) }) { Text(if (pack == null) "Download" else "Update") }
                    }
                    if (active != null) {
                        LinearProgressIndicator(
                            progress = { active.completed.toFloat() / active.total.coerceAtLeast(1) },
                            modifier = Modifier.fillMaxWidth().padding(top = 10.dp),
                        )
                        Text(
                            "${active.completed}/${active.total} layers",
                            style = MaterialTheme.typography.labelMedium,
                            color = ForestSoft,
                            modifier = Modifier.padding(top = 4.dp),
                        )
                    }
                    if (pack != null) {
                        Text(if (expanded) "Hide layer filenames" else "Show layer filenames", Modifier.clickable { expanded = !expanded }.padding(top = 10.dp), color = ForestSoft, style = MaterialTheme.typography.labelMedium)
                        if (expanded) pack.layers.forEach { Text("${it.name} · ${formatBytes(it.length())}", style = MaterialTheme.typography.bodySmall, color = Color(0xFF69716C)) }
                    }
                }
            }
            if (confirmDelete && pack != null) {
                AlertDialog(
                    onDismissRequest = { confirmDelete = false },
                    title = { Text("Delete $region maps?") },
                    text = { Text("Removes ${pack.layers.size} layers and frees ${formatBytes(pack.bytes)}. Offline routing and search stop working in $region until you download again.") },
                    confirmButton = {
                        TextButton(onClick = {
                            confirmDelete = false
                            val freed = manager.delete(region)
                            packs = manager.packs()
                            message = "$region maps deleted · ${formatBytes(freed)} freed"
                            onPacksChanged()
                        }) { Text("Delete", color = Clay) }
                    },
                    dismissButton = { TextButton(onClick = { confirmDelete = false }) { Text("Keep") } },
                )
            }
        }
    }
}

@Composable
private fun DeveloperMapScreen() {
    val context = LocalContext.current
    val live by TraceBus.state.collectAsState()
    val repo = remember { PtilesRepository(context) }
    val center = live.location?.let { GeoPoint(it.latitude, it.longitude) } ?: GeoPoint(35.73377, -88.03220)
    var features by remember { mutableStateOf(emptyList<com.steele.looky.model.MapFeature>()) }
    var stateCode by remember { mutableStateOf<String?>(null) }
    var nearestRoad by remember { mutableStateOf<String?>(null) }
    var selected by remember { mutableStateOf<BusinessInfo?>(null) }
    val scope = rememberCoroutineScope()
    val groups = listOf("Roads", "Trails", "Water", "Parks", "Buildings", "Rail", "Cameras", "Businesses")
    var enabledGroups by remember { mutableStateOf(groups.toSet()) }
    LaunchedEffect(center) {
        val snapshot = withContext(Dispatchers.IO) {
            Triple(
                repo.featuresAround(center.lat, center.lon, true, developer = true),
                repo.currentStateCode(center.lat, center.lon),
                repo.nearbyRoadContext(center.lat, center.lon).second.roadName,
            )
        }
        features = snapshot.first
        stateCode = snapshot.second
        nearestRoad = snapshot.third
    }
    fun group(feature: com.steele.looky.model.MapFeature): String = when {
        feature.kind.startsWith("trail") || feature.kind in setOf("path", "footway", "track", "steps") -> "Trails"
        feature.kind == "water" -> "Water"
        feature.kind == "park" -> "Parks"
        feature.kind == "building" -> "Buildings"
        feature.kind.startsWith("rail") || feature.kind == "station" -> "Rail"
        feature.kind.startsWith("camera") -> "Cameras"
        feature.kind.startsWith("business") -> "Businesses"
        else -> "Roads"
    }
    val visible = features.filter { group(it) in enabledGroups }
    val installed = repo.installedLayers()
    Column(Modifier.fillMaxSize()) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 8.dp)) {
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                Text("${visible.size}/${features.size} features", fontWeight = FontWeight.Bold)
                Text("${installed.size} PTiles layers · ${stateCode ?: "outside coverage"}", color = ForestSoft)
            }
            Text(
                "%.5f, %.5f · %s".format(center.lat, center.lon, nearestRoad ?: "no nearby named road"),
                style = MaterialTheme.typography.bodySmall,
                color = Color(0xFF69716C),
            )
            Row(
                Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                groups.forEach { name ->
                    val count = features.count { group(it) == name }
                    FilterChip(
                        selected = name in enabledGroups,
                        onClick = {
                            enabledGroups = if (name in enabledGroups) enabledGroups - name else enabledGroups + name
                        },
                        label = { Text("$name $count") },
                    )
                }
            }
            Text(
                installed.joinToString(" · ") { it.name }.ifEmpty { "No PTiles downloaded" },
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
                style = MaterialTheme.typography.labelSmall,
                color = ForestSoft,
            )
        }
        HorizontalDivider()
        OfflineMap(
            center, visible, center, null, emptyList(), live.recentPoints, Modifier.weight(1f),
            onTap = { tap ->
                scope.launch {
                    selected = withContext(Dispatchers.IO) { repo.businessAt(tap) }
                }
            },
        )
    }
    selected?.let { business ->
        BusinessSheet(business) { selected = null }
    }
}

/**
 * Tapped-business detail, dragged up for the full record.
 *
 * The map only carries name and category, so everything below the first two
 * rows comes from the layer's extended-attributes trailer and is missing on
 * older packs -- absent fields are dropped rather than printed empty.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun BusinessSheet(business: BusinessInfo, onDismiss: () -> Unit) {
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = false),
        containerColor = Color.White,
    ) {
        Column(
            Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(start = 20.dp, end = 20.dp, bottom = 32.dp),
            verticalArrangement = Arrangement.spacedBy(2.dp),
        ) {
            Text(business.name, style = MaterialTheme.typography.headlineSmall, color = Forest)
            Text(
                "%.5f, %.5f".format(business.location.lat, business.location.lon),
                style = MaterialTheme.typography.bodyMedium,
                color = ForestSoft,
            )
            Spacer(Modifier.height(14.dp))
            DetailRow("Category index", business.categoryIdx.toString())
            DetailRow("Operating status", business.operatingStatus)
            business.phone?.let { DetailRow("Phone", it) }
            business.website?.let { DetailRow("Website", it) }
            DetailRow("OSM id", business.osmId.toString())
            business.sourceType?.let {
                DetailRow("Source", when (it.toInt()) { 1 -> "Overture"; 2 -> "Foursquare"; else -> "Unknown ($it)" })
            }
            business.sourceId?.let { DetailRow("Source id", it) }
            business.confidence?.let { DetailRow("Confidence", "$it/100") }
        }
    }
}

@Composable
private fun DetailRow(label: String, value: String) {
    Row(Modifier.fillMaxWidth().padding(vertical = 8.dp)) {
        Text(label, Modifier.width(150.dp), style = MaterialTheme.typography.labelLarge, color = ForestSoft)
        Text(value, style = MaterialTheme.typography.bodyLarge, color = Forest)
    }
}

@Composable
private fun EmptyState(title: String, subtitle: String) {
    Column(Modifier.fillMaxSize().padding(32.dp), verticalArrangement = Arrangement.Center, horizontalAlignment = Alignment.CenterHorizontally) {
        Box(Modifier.size(72.dp).background(Lime, RoundedCornerShape(24.dp)), contentAlignment = Alignment.Center) {
            Icon(Icons.Rounded.Route, null, tint = Forest)
        }
        Spacer(Modifier.height(20.dp))
        Text(title, style = MaterialTheme.typography.headlineMedium)
        Text(subtitle, color = Color(0xFF69716C))
    }
}

private fun formatDistance(meters: Double, imperial: Boolean): String = if (imperial) {
    val feet = meters * 3.28084
    if (feet < 1_000) "${feet.roundToInt()} ft" else "%.1f mi".format(feet / 5_280)
} else {
    if (meters < 1_000) "${meters.roundToInt()} m" else "%.1f km".format(meters / 1_000)
}

private fun formatBytes(bytes: Long): String = when {
    bytes >= 1_000_000_000L -> "%.1f GB".format(bytes / 1_000_000_000.0)
    bytes >= 1_000_000L -> "%.1f MB".format(bytes / 1_000_000.0)
    else -> "%.0f KB".format(bytes / 1_000.0)
}
