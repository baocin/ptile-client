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
import androidx.compose.foundation.lazy.itemsIndexed
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
import uniffi.ptiles_ffi.TrailInfo
import uniffi.ptiles_ffi.NavStateInfo
import uniffi.ptiles_ffi.Navigator
import uniffi.ptiles_ffi.TurnInfo
import androidx.activity.compose.BackHandler
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File
import java.time.Instant
import kotlin.math.roundToInt

private enum class Screen { DRIVE, TRAIL, MORE, RECORDINGS, PACKS, SETTINGS, DEVELOPER }

/** Settle time before a panned viewport triggers a PTiles decode. */
internal const val VIEWPORT_DEBOUNCE_MS = 220L

/** Settle time before a search query hits the name indexes. */
internal const val SEARCH_DEBOUNCE_MS = 300L

/** How far the viewport must move before reloading features, in metres. */
internal const val VIEWPORT_RELOAD_M = 220.0

/** Height cap on the hits-and-stops list before it scrolls inside the card. */


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
                    // The corner says the one thing that matters right now:
                    // stop the journey you are on, or go get the maps you are
                    // missing. "Ready" was neither -- it only ever confirmed
                    // that nothing needed doing.
                    val onAJourney = live.running && live.session != TraceRecorder.SESSION_BACKGROUND
                    when {
                        onAJourney -> Surface(
                            color = Clay,
                            shape = RoundedCornerShape(100.dp),
                            modifier = Modifier.clickable { TraceService.endSession(context) },
                        ) {
                            Text(
                                if (live.mode == LookyMode.TRAIL) "Stop trail" else "Stop drive",
                                Modifier.padding(horizontal = 14.dp, vertical = 7.dp),
                                style = MaterialTheme.typography.labelLarge,
                                color = Color.White,
                            )
                        }
                        !mapsReady -> Surface(
                            color = Clay.copy(alpha = .55f),
                            shape = RoundedCornerShape(100.dp),
                            modifier = Modifier.clickable { screen = Screen.PACKS },
                        ) {
                            Text(
                                "Downloads Needed",
                                Modifier.padding(horizontal = 11.dp, vertical = 6.dp),
                                style = MaterialTheme.typography.labelLarge,
                                color = Forest,
                            )
                        }
                        // The classifier's verdict, once it has one. "Unknown"
                        // is its starting state, and printing that is worse
                        // than printing nothing.
                        live.running && live.movement != "Unknown" -> Surface(
                            color = Lime,
                            shape = RoundedCornerShape(100.dp),
                        ) {
                            Text(
                                live.movement,
                                Modifier.padding(horizontal = 12.dp, vertical = 6.dp),
                                style = MaterialTheme.typography.labelLarge,
                                color = Forest,
                            )
                        }
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
                Screen.DRIVE -> DriveScreen(settings, onRequestPermissions) { screen = Screen.PACKS }
                Screen.TRAIL -> TrailScreen(settings, onRequestPermissions) { screen = Screen.PACKS }
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
/**
 * The next manoeuvre, its distance, and what is left of the route.
 *
 * Everything shown is decided natively by `Navigator`; this only draws it.
 * A null state before the first fix snaps is normal, not an error.
 */
@Composable
internal fun TurnCard(state: NavStateInfo?, turns: List<TurnInfo>, imperial: Boolean) {
    val turn = state?.nextTurn?.toInt()?.let(turns::getOrNull)
    Card(
        colors = CardDefaults.cardColors(containerColor = if (state?.offRoute == true) Clay else Forest),
        shape = RoundedCornerShape(18.dp),
    ) {
        Column(Modifier.fillMaxWidth().padding(16.dp)) {
            Text(
                when {
                    state == null -> "Finding you on the route…"
                    state.offRoute -> "Off route"
                    turn == null -> "Continue to the destination"
                    else -> maneuverText(turn)
                },
                style = MaterialTheme.typography.headlineSmall,
                color = Color.White,
                fontWeight = FontWeight.Black,
            )
            if (state != null) {
                Spacer(Modifier.height(6.dp))
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                    Text(
                        if (turn == null) "" else "in ${formatDistance(state.distanceToTurnM, imperial)}",
                        color = Lime,
                        fontWeight = FontWeight.Bold,
                    )
                    Text("${formatDistance(state.remainingM, imperial)} left", color = Lime)
                }
            }
        }
    }
}

internal fun maneuverText(turn: TurnInfo): String {
    val verb = when (turn.maneuver) {
        "depart" -> "Head out"
        "arrive" -> "Arrive"
        "left" -> "Turn left"
        "right" -> "Turn right"
        "slight_left" -> "Bear left"
        "slight_right" -> "Bear right"
        "sharp_left" -> "Sharp left"
        "sharp_right" -> "Sharp right"
        "u_turn" -> "Make a U-turn"
        else -> "Continue"
    }
    // An unnamed service road is common; inventing a name helps nobody.
    val onto = turn.roadName ?: turn.roadRef ?: return verb
    return if (turn.maneuver == "arrive") verb else "$verb onto $onto"
}

@Composable
internal fun Metric(value: String, label: String) {
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

internal fun hasLocationPermission(context: android.content.Context): Boolean =
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
    val live by TraceBus.state.collectAsState()
    val traces = File(context.filesDir, "traces").listFiles().orEmpty().filter { it.extension == "gpx" }
    var open by remember { mutableStateOf<File?>(null) }
    open?.let { file ->
        BackHandler { open = null }
        RecordingDetailScreen(file)
        return
    }
    // One list of stretches, newest first, across every day file: drove for
    // twenty minutes, walked for five, sat still for an hour. Which file a
    // stretch came from is a storage detail, not a heading.
    var segments by remember { mutableStateOf(emptyList<TraceSegment>()) }
    val fileNames = traces.map { it.name }.sorted().joinToString()
    LaunchedEffect(fileNames) {
        segments = withContext(Dispatchers.IO) {
            traces.flatMap(GpxReader::readSegments).sortedByDescending { it.lastFix ?: Instant.EPOCH }
        }
    }
    // The open segment grows while you are moving, so re-read the file being
    // written every time the recorder reports a new fix. Only that one file:
    // re-parsing the whole history at 1 Hz is not a live view, it is a stall.
    LaunchedEffect(live.pointsToday, live.traceFile) {
        val active = live.traceFile?.let(::File)?.takeIf { it.exists() } ?: return@LaunchedEffect
        val reread = withContext(Dispatchers.IO) { GpxReader.readSegments(active) }
        segments = (segments.filterNot { it.file?.name == active.name } + reread)
            .sortedByDescending { it.lastFix ?: Instant.EPOCH }
    }
    if (segments.isEmpty()) {
        EmptyState("Nothing recorded yet", "Start Drive or Trail and the first accurate fix opens a segment.")
        return
    }
    val imperial = remember { AppSettings(context).imperialUnits }
    LazyColumn(Modifier.fillMaxSize().padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        itemsIndexed(segments) { index, segment ->
            val recording = index == 0 && live.running && segment.file?.absolutePath == live.traceFile
            Card(
                Modifier.clickable { segment.file?.let { open = it } },
                colors = CardDefaults.cardColors(containerColor = Color.White),
                shape = RoundedCornerShape(18.dp),
            ) {
                Row(Modifier.fillMaxWidth().padding(14.dp), verticalAlignment = Alignment.CenterVertically) {
                    Box(Modifier.size(12.dp).background(movementColor(segment.movement), CircleShape))
                    Spacer(Modifier.width(12.dp))
                    Column(Modifier.weight(1f)) {
                        Text(
                            // "Unknown" is the classifier's starting state, not
                            // a kind of travel; naming it that in a list of
                            // journeys reads as a bug.
                            movementLabel(segment.movement, recording),
                            style = MaterialTheme.typography.titleMedium,
                            color = Forest,
                        )
                        Text(
                            listOfNotNull(
                                formatSpan(segment.firstFix, segment.lastFix),
                                formatDistance(segment.distanceM, imperial),
                                if (segment.points.size == 1) "1 fix" else "${segment.points.size} fixes",
                            ).joinToString(" · "),
                            style = MaterialTheme.typography.bodySmall,
                            color = Color(0xFF69716C),
                        )
                    }
                    segment.file?.let { file ->
                        Text(
                            TraceRecorder.dateOf(file)?.toString().orEmpty(),
                            style = MaterialTheme.typography.labelSmall,
                            color = ForestSoft,
                        )
                    }
                }
            }
        }
    }
}

private fun movementLabel(movement: String, recording: Boolean): String {
    val name = if (movement.equals("Unknown", ignoreCase = true)) "Unclassified" else movement
    return if (recording) "$name · now" else name
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
                    message = if (states.size == 1) "${states.first()} maps installed" else "All US PTiles layers installed"
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
                            message = "${com.steele.looky.offline.StateResolver.name(state)} maps installed"
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
        // What you already have, first: the installed packs are the ones you
        // come here to check on or update.
        val regions = MapPackDownloader.US_STATES.sortedWith(
            compareByDescending<String> { installedByRegion.containsKey(it) }.thenBy { it }
        )
        items(regions, key = { it }) { region ->
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
    var selectedTrail by remember { mutableStateOf<TrailInfo?>(null) }
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
                    // Businesses first, trails second: a tap in town is nearly
                    // always a place, and a tap on a ridge nearly always a path.
                    val hit = withContext(Dispatchers.IO) {
                        repo.businessAt(tap)?.let { it as Any } ?: repo.trailAt(tap)
                    }
                    selected = hit as? BusinessInfo
                    selectedTrail = hit as? TrailInfo
                }
            },
        )
    }
    selected?.let { business ->
        BusinessSheet(business) { selected = null }
    }
    selectedTrail?.let { trail ->
        TrailSheet(trail) { selectedTrail = null }
    }
}

/** Tapped-trail detail: what the trails layer knows about this path. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun TrailSheet(trail: TrailInfo, onDismiss: () -> Unit) {
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = false),
        containerColor = Color.White,
    ) {
        Column(
            Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(start = 20.dp, end = 20.dp, bottom = 32.dp),
            verticalArrangement = Arrangement.spacedBy(2.dp),
        ) {
            Text(trail.name ?: "Unnamed trail", style = MaterialTheme.typography.headlineSmall, color = Forest)
            trail.geometry.firstOrNull()?.let {
                Text("%.5f, %.5f".format(it.lat, it.lon), style = MaterialTheme.typography.bodyMedium, color = ForestSoft)
            }
            Spacer(Modifier.height(14.dp))
            DetailRow("Type", trail.trailType)
            DetailRow("Surface", trail.surface)
            DetailRow("SAC scale", trail.sacScale)
            DetailRow("Developed", if (trail.developed) "yes" else "no")
            DetailRow("Trailhead", if (trail.isTrailhead) "yes" else "no")
            DetailRow("Points", trail.geometry.size.toString())
            DetailRow("OSM id", trail.osmId.toString())
        }
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

/** The GPS/distance/points strip both mode screens pin to the bottom. */
@Composable
internal fun LiveMetrics(imperial: Boolean, modifier: Modifier = Modifier) {
    val live by TraceBus.state.collectAsState()
    Card(
        modifier = modifier.padding(16.dp),
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

private fun formatBytes(bytes: Long): String = when {
    bytes >= 1_000_000_000L -> "%.1f GB".format(bytes / 1_000_000_000.0)
    bytes >= 1_000_000L -> "%.1f MB".format(bytes / 1_000_000.0)
    else -> "%.0f KB".format(bytes / 1_000.0)
}
