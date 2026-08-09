package com.steele.looky.ui

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
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
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.ArrowBack
import androidx.compose.material.icons.rounded.DirectionsCar
import androidx.compose.material.icons.rounded.Folder
import androidx.compose.material.icons.rounded.Map
import androidx.compose.material.icons.rounded.MoreHoriz
import androidx.compose.material.icons.rounded.Route
import androidx.compose.material.icons.rounded.Settings
import androidx.compose.material.icons.rounded.Terrain
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import com.steele.looky.AppSettings
import com.steele.looky.location.TraceService
import com.steele.looky.model.GeoPoint
import com.steele.looky.model.LookyMode
import com.steele.looky.model.TraceBus
import com.steele.looky.offline.PackManager
import com.steele.looky.offline.MapDownloadProgress
import com.steele.looky.offline.MapPackDownloader
import com.steele.looky.offline.PtilesRepository
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File
import kotlin.math.roundToInt

private enum class Screen { DRIVE, TRAIL, MORE, RECORDINGS, PACKS, SETTINGS, DEVELOPER }

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun LookyApp(settings: AppSettings, onRequestPermissions: () -> Unit) {
    val context = LocalContext.current
    var screen by remember {
        mutableStateOf(if (settings.activeMode == LookyMode.TRAIL) Screen.TRAIL else Screen.DRIVE)
    }
    val root = screen in setOf(Screen.DRIVE, Screen.TRAIL, Screen.MORE)
    Scaffold(
        topBar = {
            TopAppBar(
                title = {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Box(Modifier.size(32.dp).background(Lime, RoundedCornerShape(10.dp)), contentAlignment = Alignment.Center) {
                            Text("L", color = Forest, fontWeight = FontWeight.Black)
                        }
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
                    Surface(color = Lime.copy(alpha = .45f), shape = RoundedCornerShape(100.dp)) {
                        Text("OFFLINE", Modifier.padding(horizontal = 11.dp, vertical = 6.dp), style = MaterialTheme.typography.labelLarge, color = Forest)
                    }
                    Spacer(Modifier.width(12.dp))
                },
                colors = TopAppBarDefaults.topAppBarColors(containerColor = Paper),
            )
        },
        bottomBar = {
            if (root) NavigationBar(containerColor = Color.White) {
                NavigationBarItem(
                    selected = screen == Screen.DRIVE,
                    onClick = {
                        screen = Screen.DRIVE
                        if (settings.continuousRecording) TraceService.start(context, LookyMode.DRIVE)
                    },
                    icon = { Icon(Icons.Rounded.DirectionsCar, null) }, label = { Text("Drive") },
                )
                NavigationBarItem(
                    selected = screen == Screen.TRAIL,
                    onClick = {
                        screen = Screen.TRAIL
                        if (settings.continuousRecording) TraceService.start(context, LookyMode.TRAIL)
                    },
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
                Screen.DRIVE -> ModeMap(false, settings, onRequestPermissions)
                Screen.TRAIL -> ModeMap(true, settings, onRequestPermissions)
                Screen.MORE -> MoreScreen(settings) { screen = it }
                Screen.RECORDINGS -> RecordingsScreen()
                Screen.PACKS -> PacksScreen()
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

@Composable
private fun ModeMap(trail: Boolean, settings: AppSettings, onRequestPermissions: () -> Unit) {
    val context = LocalContext.current
    val live by TraceBus.state.collectAsState()
    val repo = remember { PtilesRepository(context) }
    val scope = rememberCoroutineScope()
    val current = live.location?.let { GeoPoint(it.latitude, it.longitude) }
    val center = current ?: GeoPoint(35.73377, -88.03220)
    var destination by remember(trail) { mutableStateOf<GeoPoint?>(null) }
    var coordinate by remember(trail) { mutableStateOf("") }
    var features by remember { mutableStateOf(emptyList<com.steele.looky.model.MapFeature>()) }
    var route by remember { mutableStateOf<PtilesRepository.RouteResult?>(null) }
    var routeError by remember { mutableStateOf<String?>(null) }
    var routing by remember { mutableStateOf(false) }

    LaunchedEffect(center.lat, center.lon, trail) {
        features = withContext(Dispatchers.IO) { repo.featuresAround(center.lat, center.lon, trail) }
    }

    Box(Modifier.fillMaxSize()) {
        OfflineMap(
            center = center,
            features = features,
            current = current,
            destination = destination,
            route = route?.points.orEmpty(),
            trace = live.recentPoints,
            onLongPress = {
                destination = it
                coordinate = "%.5f, %.5f".format(it.lat, it.lon)
                route = null
                routeError = null
            },
        )
        Column(Modifier.fillMaxWidth().padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Card(shape = RoundedCornerShape(22.dp), colors = CardDefaults.cardColors(containerColor = Color.White.copy(alpha = .96f))) {
                Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Column(Modifier.weight(1f)) {
                            Text(if (trail) "TRAIL MODE" else "DRIVE MODE", style = MaterialTheme.typography.labelLarge, color = ForestSoft)
                            Text(
                                if (live.running && live.mode == if (trail) LookyMode.TRAIL else LookyMode.DRIVE) "Recording in background" else "Ready offline",
                                style = MaterialTheme.typography.titleLarge,
                            )
                        }
                        if (live.running && repo.installedLayers().isNotEmpty()) {
                            Surface(color = Lime, shape = CircleShape) {
                                val label = if (live.movement == "Unknown") "Starting…" else live.movement
                                Text(label, Modifier.padding(horizontal = 12.dp, vertical = 8.dp), style = MaterialTheme.typography.labelLarge)
                            }
                        }
                    }
                    OutlinedTextField(
                        value = coordinate,
                        onValueChange = {
                            coordinate = it
                            parseCoordinate(it)?.let { point -> destination = point }
                        },
                        modifier = Modifier.fillMaxWidth(),
                        singleLine = true,
                        label = { Text(if (trail) "Trailhead or destination coordinates" else "Destination coordinates") },
                        placeholder = { Text("35.7338, -88.0322") },
                    )
                    Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                        Button(
                            onClick = {
                                if (hasLocationPermission(context)) {
                                    TraceService.start(context, if (trail) LookyMode.TRAIL else LookyMode.DRIVE)
                                } else {
                                    onRequestPermissions()
                                }
                                destination?.let { end ->
                                    routing = true; routeError = null
                                    scope.launch {
                                        runCatching {
                                            withContext(Dispatchers.Default) {
                                                val snappedStart = repo.snapForRoute(center, trail) ?: center
                                                val snappedEnd = repo.snapForRoute(end, trail) ?: end
                                                repo.offlineRoute(snappedStart, snappedEnd, trail, settings.avoidHighways, settings.avoidIntersections)
                                            }
                                        }.onSuccess { route = it }.onFailure { routeError = it.message ?: "No connected route in this pack" }
                                        routing = false
                                    }
                                }
                            },
                            modifier = Modifier.weight(1f).height(50.dp),
                            shape = RoundedCornerShape(16.dp),
                            colors = ButtonDefaults.buttonColors(containerColor = Forest),
                        ) {
                            if (routing) CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp, color = Color.White)
                            else Text(if (destination == null) "Start ${if (trail) "trail" else "drive"}" else "Route offline")
                        }
                        FilledTonalButton(
                            onClick = {
                                destination = GeoPoint(center.lat + 0.006, center.lon + 0.006)
                                coordinate = "%.5f, %.5f".format(center.lat + 0.006, center.lon + 0.006)
                            },
                            modifier = Modifier.height(50.dp),
                            shape = RoundedCornerShape(16.dp),
                        ) { Text("Drop pin") }
                    }
                    Text("Long-press the map to place a destination.", style = MaterialTheme.typography.bodySmall, color = Color(0xFF69716C))
                }
            }
            route?.let {
                Card(colors = CardDefaults.cardColors(containerColor = Lime), shape = RoundedCornerShape(18.dp)) {
                    Row(Modifier.fillMaxWidth().padding(14.dp), horizontalArrangement = Arrangement.SpaceBetween) {
                        Text(formatDistance(it.distanceM), fontWeight = FontWeight.Black, color = Forest)
                        Text("${(it.durationS / 60).roundToInt().coerceAtLeast(1)} min", fontWeight = FontWeight.Bold, color = Forest)
                        Text("${it.decodedSegments} local segments", color = ForestSoft)
                    }
                }
            }
            routeError?.let {
                Surface(color = Color(0xFFFFE4DA), shape = RoundedCornerShape(14.dp)) {
                    Text(it, Modifier.padding(12.dp), color = Color(0xFF7A2B16))
                }
            }
        }
        Card(
            modifier = Modifier.align(Alignment.BottomCenter).padding(16.dp),
            shape = RoundedCornerShape(18.dp),
            colors = CardDefaults.cardColors(containerColor = Color(0xEE173F35)),
        ) {
            Row(Modifier.fillMaxWidth().padding(14.dp), horizontalArrangement = Arrangement.SpaceAround) {
                Metric(if (live.location?.hasAccuracy() == true) "±${live.location?.accuracy?.roundToInt()} m" else "—", "GPS")
                Metric(formatDistance(live.distanceM), "TODAY")
                Metric(live.pointsToday.toString(), "POINTS")
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
                if (it) TraceService.start(context, settings.activeMode) else TraceService.stop(context)
            }
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
        item {
            Card(colors = CardDefaults.cardColors(containerColor = Lime.copy(alpha = .45f))) {
                Text("Offline is the normal state. Looky never uploads traces and never requests an online route.", Modifier.padding(16.dp), color = Forest)
            }
        }
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
    if (traces.isEmpty()) {
        EmptyState("No day files yet", "Start Drive or Trail and the first accurate fix will create one.")
        return
    }
    LazyColumn(Modifier.fillMaxSize().padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        items(traces, key = File::getName) { file ->
            Card(colors = CardDefaults.cardColors(containerColor = Color.White), shape = RoundedCornerShape(18.dp)) {
                Row(Modifier.fillMaxWidth().padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f)) {
                        Text(file.nameWithoutExtension, style = MaterialTheme.typography.titleMedium)
                        Text("${file.length() / 1024} KB · GPX 1.1", color = Color(0xFF69716C))
                    }
                    Text("Saved", color = Forest, fontWeight = FontWeight.Bold)
                }
            }
        }
    }
}

@Composable
private fun PacksScreen() {
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
                .onSuccess { message = if (states.size == 1) "${states.first()} offline maps installed" else "All US PTiles layers installed" }
                .onFailure { message = it.message ?: "Download failed" }
            packs = manager.packs(); downloading = false
        }
    }
    LazyColumn(Modifier.fillMaxSize().padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        item {
            Button(enabled = !downloading, onClick = {
                downloading = true
                scope.launch {
                    MapPackDownloader.downloadCurrentState(context) { progress = it }
                        .onSuccess { message = "Tennessee and Montana offline maps installed" }
                        .onFailure { message = it.message ?: "Download failed" }
                    packs = manager.packs(); downloading = false
                }
            }, modifier = Modifier.fillMaxWidth(), shape = RoundedCornerShape(16.dp)) {
                Text(if (downloading) "Downloading ${progress?.completed ?: 0}/${progress?.total ?: 0}…" else "Download TN + Montana maps")
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
            Card(colors = CardDefaults.cardColors(containerColor = Color.White), shape = RoundedCornerShape(18.dp)) {
                Column(Modifier.padding(16.dp)) {
                    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                        Column(Modifier.weight(1f)) {
                            Text(region, style = MaterialTheme.typography.titleLarge)
                            Text(if (pack == null) "Not downloaded" else "${pack.layers.size} layers · ${formatBytes(pack.bytes)}", color = Color(0xFF69716C))
                        }
                        FilledTonalButton(enabled = !downloading, onClick = { download(listOf(region)) }) { Text(if (pack == null) "Download" else "Update") }
                    }
                    if (pack != null) {
                        Text(if (expanded) "Hide layer filenames" else "Show layer filenames", Modifier.clickable { expanded = !expanded }.padding(top = 10.dp), color = ForestSoft, style = MaterialTheme.typography.labelMedium)
                        if (expanded) pack.layers.forEach { Text("${it.name} · ${formatBytes(it.length())}", style = MaterialTheme.typography.bodySmall, color = Color(0xFF69716C)) }
                    }
                }
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
    LaunchedEffect(center) { features = withContext(Dispatchers.IO) { repo.featuresAround(center.lat, center.lon, true) } }
    Column(Modifier.fillMaxSize()) {
        Row(Modifier.fillMaxWidth().padding(12.dp), horizontalArrangement = Arrangement.SpaceBetween) {
            Text("${features.size} visible features", fontWeight = FontWeight.Bold)
            Text("${repo.installedLayers().size} installed layers", color = ForestSoft)
        }
        HorizontalDivider()
        OfflineMap(center, features, center, null, emptyList(), live.recentPoints, Modifier.weight(1f))
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

private fun parseCoordinate(value: String): GeoPoint? {
    val pieces = value.split(',').map { it.trim().toDoubleOrNull() }
    if (pieces.size != 2 || pieces.any { it == null }) return null
    val lat = pieces[0]!!; val lon = pieces[1]!!
    return if (lat in -90.0..90.0 && lon in -180.0..180.0) GeoPoint(lat, lon) else null
}

private fun formatDistance(meters: Double): String = when {
    meters < 1_000 -> "${meters.roundToInt()} m"
    else -> "%.1f km".format(meters / 1_000)
}

private fun formatBytes(bytes: Long): String = when {
    bytes >= 1_000_000_000L -> "%.1f GB".format(bytes / 1_000_000_000.0)
    bytes >= 1_000_000L -> "%.1f MB".format(bytes / 1_000_000.0)
    else -> "%.0f KB".format(bytes / 1_000.0)
}
