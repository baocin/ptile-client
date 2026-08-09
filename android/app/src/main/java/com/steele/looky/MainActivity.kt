package com.steele.looky

import android.Manifest
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.core.content.ContextCompat
import com.steele.looky.location.TraceService
import com.steele.looky.offline.MapDownloadProgress
import com.steele.looky.offline.MapPackDownloader
import com.steele.looky.ui.LookyApp
import com.steele.looky.ui.LookyTheme
import com.steele.looky.ui.Onboarding
import kotlinx.coroutines.launch
import androidx.lifecycle.lifecycleScope

class MainActivity : ComponentActivity() {
    private lateinit var settings: AppSettings
    private var permissionsGranted by mutableStateOf(false)
    private var mapDownload by mutableStateOf<MapDownloadProgress?>(null)
    private var mapDownloadRunning by mutableStateOf(false)
    private var mapDownloadError by mutableStateOf<String?>(null)

    private val backgroundPermission = registerForActivityResult(
        ActivityResultContracts.RequestPermission()
    ) { updatePermissionState() }

    private val permissions = registerForActivityResult(
        ActivityResultContracts.RequestMultiplePermissions()
    ) {
        updatePermissionState()
        if (permissionsGranted && Build.VERSION.SDK_INT >= 29 &&
            ContextCompat.checkSelfPermission(this, Manifest.permission.ACCESS_BACKGROUND_LOCATION) != PackageManager.PERMISSION_GRANTED
        ) {
            backgroundPermission.launch(Manifest.permission.ACCESS_BACKGROUND_LOCATION)
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        settings = AppSettings(this)
        updatePermissionState()
        setContent {
            LookyTheme {
                var onboarded by remember { mutableStateOf(settings.onboardingComplete) }
                if (!onboarded) {
                    Onboarding(
                        permissionsGranted = permissionsGranted,
                        onRequestPermissions = ::requestLookyPermissions,
                        onExploreOffline = {
                            settings.continuousRecording = false
                            settings.onboardingComplete = true
                            onboarded = true
                        },
                        mapDownload = mapDownload,
                        mapDownloadRunning = mapDownloadRunning,
                        mapDownloadError = mapDownloadError,
                        onDownloadMaps = ::downloadMaps,
                        onComplete = {
                            settings.onboardingComplete = true
                            onboarded = true
                            if (permissionsGranted && settings.continuousRecording) {
                                TraceService.start(this, settings.activeMode)
                            }
                        },
                    )
                } else {
                    LaunchedEffect(permissionsGranted, settings.continuousRecording) {
                        if (permissionsGranted && settings.continuousRecording) {
                            TraceService.start(this@MainActivity, settings.activeMode)
                        }
                    }
                    LookyApp(settings, ::requestLookyPermissions)
                }
            }
        }
    }

    private fun downloadMaps() {
        if (mapDownloadRunning) return
        mapDownloadRunning = true
        mapDownloadError = null
        lifecycleScope.launch {
            MapPackDownloader.downloadCurrentState(this@MainActivity) { progress ->
                runOnUiThread { mapDownload = progress }
            }
                .onFailure { mapDownloadError = it.message ?: "Map download failed" }
                .onSuccess {
                    settings.onboardingComplete = true
                    settings.continuousRecording = permissionsGranted
                }
            mapDownloadRunning = false
            if (settings.onboardingComplete) {
                if (permissionsGranted && settings.continuousRecording) TraceService.start(this@MainActivity, settings.activeMode)
                // Compose observes onboardingComplete only through this local state.
                setContent {
                    LookyTheme { LookyApp(settings, ::requestLookyPermissions) }
                }
            }
        }
    }

    private fun requestLookyPermissions() {
        val wanted = buildList {
            add(Manifest.permission.ACCESS_FINE_LOCATION)
            add(Manifest.permission.ACCESS_COARSE_LOCATION)
            if (Build.VERSION.SDK_INT >= 29) add(Manifest.permission.ACTIVITY_RECOGNITION)
            if (Build.VERSION.SDK_INT >= 33) add(Manifest.permission.POST_NOTIFICATIONS)
        }
        permissions.launch(wanted.toTypedArray())
    }

    private fun updatePermissionState() {
        permissionsGranted = ContextCompat.checkSelfPermission(
            this, Manifest.permission.ACCESS_FINE_LOCATION
        ) == PackageManager.PERMISSION_GRANTED
    }
}
