package com.steele.looky.location

import android.Manifest
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.hardware.SensorManager
import android.location.Location
import android.os.IBinder
import androidx.core.app.ActivityCompat
import androidx.core.app.NotificationCompat
import androidx.core.content.ContextCompat
import com.google.android.gms.location.FusedLocationProviderClient
import com.google.android.gms.location.LocationCallback
import com.google.android.gms.location.LocationRequest
import com.google.android.gms.location.LocationResult
import com.google.android.gms.location.LocationServices
import com.google.android.gms.location.Priority
import com.steele.looky.AppSettings
import com.steele.looky.MainActivity
import com.steele.looky.R
import com.steele.looky.model.LookyMode
import com.steele.looky.model.GeoPoint
import com.steele.looky.model.TraceBus
import com.steele.looky.offline.PtilesRepository
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch

class TraceService : Service() {
    companion object {
        const val ACTION_START = "com.steele.looky.START"
        const val ACTION_STOP = "com.steele.looky.STOP"
        const val ACTION_DRIVE = "com.steele.looky.DRIVE"
        const val ACTION_TRAIL = "com.steele.looky.TRAIL"
        const val ACTION_APPLY_SETTINGS = "com.steele.looky.APPLY_SETTINGS"
        private const val CHANNEL = "looky-trace"
        private const val NOTIFICATION = 4102

        fun start(context: Context, mode: LookyMode) {
            val action = if (mode == LookyMode.DRIVE) ACTION_DRIVE else ACTION_TRAIL
            ContextCompat.startForegroundService(context, Intent(context, TraceService::class.java).setAction(action))
        }

        fun stop(context: Context) {
            context.startService(Intent(context, TraceService::class.java).setAction(ACTION_STOP))
        }

        fun applySettings(context: Context) {
            context.startService(Intent(context, TraceService::class.java).setAction(ACTION_APPLY_SETTINGS))
        }
    }

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO.limitedParallelism(1))
    private lateinit var fused: FusedLocationProviderClient
    private lateinit var motion: MotionEngine
    private lateinit var recorder: TraceRecorder
    private lateinit var ptiles: PtilesRepository
    private var mode = LookyMode.DRIVE
    private var last: Location? = null
    private var distanceM = 0.0
    private var started = false

    private val callback = object : LocationCallback() {
        override fun onLocationResult(result: LocationResult) {
            result.locations.forEach { fix -> scope.launch { record(fix) } }
        }
    }

    override fun onCreate() {
        super.onCreate()
        fused = LocationServices.getFusedLocationProviderClient(this)
        ptiles = PtilesRepository(this)
        motion = MotionEngine(getSystemService(SensorManager::class.java), ptiles)
        recorder = TraceRecorder(this)
        createChannel()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val settings = AppSettings(this)
        when (intent?.action) {
            ACTION_APPLY_SETTINGS -> {
                if (started) {
                    motion.setAccelerometerRate(settings.accelerometerRateHz)
                    requestLocationUpdates(settings.gpsIntervalSeconds)
                }
                return START_STICKY
            }
            ACTION_STOP -> {
                settings.continuousRecording = false
                stopSelf()
                return START_NOT_STICKY
            }
            ACTION_TRAIL -> mode = LookyMode.TRAIL
            ACTION_DRIVE -> mode = LookyMode.DRIVE
            null -> mode = settings.activeMode
        }
        settings.apply {
            continuousRecording = true
            activeMode = mode
        }
        if (!started) {
            if (!begin()) return START_NOT_STICKY
        } else {
            motion.setMode(mode)
        }
        TraceBus.update { it.copy(running = true, mode = mode, error = null) }
        startForeground(NOTIFICATION, notification("Finding GPS…"))
        return START_STICKY
    }

    private fun begin(): Boolean {
        if (!hasLocationPermission()) {
            TraceBus.update { it.copy(running = false, error = "Location permission is required") }
            stopSelf()
            return false
        }
        started = true
        motion.start(mode, AppSettings(this).accelerometerRateHz)
        requestLocationUpdates(AppSettings(this).gpsIntervalSeconds)
        return true
    }

    private fun requestLocationUpdates(intervalSeconds: Int) {
        if (!hasLocationPermission()) return
        fused.removeLocationUpdates(callback)
        val intervalMs = intervalSeconds.coerceIn(3, 60) * 1_000L
        val request = LocationRequest.Builder(Priority.PRIORITY_HIGH_ACCURACY, intervalMs)
            .setMinUpdateIntervalMillis((intervalMs / 2).coerceAtLeast(1_000L))
            .setMinUpdateDistanceMeters(4f)
            .setWaitForAccurateLocation(false)
            .build()
        try {
            fused.requestLocationUpdates(request, callback, mainLooper)
        } catch (e: SecurityException) {
            started = false
            TraceBus.update { it.copy(running = false, error = e.message) }
            stopSelf()
            return
        }
    }

    private fun record(fix: Location) {
        val result = motion.classify(fix)
        val nearby = ptiles.nearbyRoadContext(fix.latitude, fix.longitude).second
        last?.let { previous ->
            val jump = previous.distanceTo(fix).toDouble()
            if (jump < 2_000.0) distanceM += jump
        }
        last = Location(fix)
        val appended = recorder.append(fix, result.movement, result.accel, nearby)
        TraceBus.update {
            val recent = (it.recentPoints + GeoPoint(fix.latitude, fix.longitude)).takeLast(2_000)
            it.copy(
                running = true,
                mode = mode,
                movement = result.movement,
                confidence = result.confidence,
                location = Location(fix),
                pointsToday = appended.pointsToday,
                distanceM = distanceM,
                traceFile = appended.file.absolutePath,
                recentPoints = recent,
                error = null,
            )
        }
        getSystemService(NotificationManager::class.java)
            .notify(NOTIFICATION, notification("${result.movement} · ${appended.pointsToday} points"))
    }

    private fun notification(status: String) = NotificationCompat.Builder(this, CHANNEL)
        .setSmallIcon(R.drawable.ic_looky)
        .setContentTitle("Looky · ${if (mode == LookyMode.DRIVE) "Drive" else "Trail"}")
        .setContentText(status)
        .setOngoing(true)
        .setOnlyAlertOnce(true)
        .setContentIntent(
            PendingIntent.getActivity(
                this, 0, Intent(this, MainActivity::class.java),
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
        )
        .addAction(0, "Drive", serviceIntent(ACTION_DRIVE, 1))
        .addAction(0, "Trail", serviceIntent(ACTION_TRAIL, 2))
        .build()

    private fun serviceIntent(action: String, code: Int) = PendingIntent.getService(
        this, code, Intent(this, TraceService::class.java).setAction(action),
        PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
    )

    private fun createChannel() {
        val manager = getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL, "Background traces", NotificationManager.IMPORTANCE_LOW).apply {
                description = "Persistent GPS and motion recording"
                setShowBadge(false)
            }
        )
    }

    private fun hasLocationPermission(): Boolean =
        ActivityCompat.checkSelfPermission(this, Manifest.permission.ACCESS_FINE_LOCATION) == PackageManager.PERMISSION_GRANTED ||
            ActivityCompat.checkSelfPermission(this, Manifest.permission.ACCESS_COARSE_LOCATION) == PackageManager.PERMISSION_GRANTED

    override fun onDestroy() {
        if (started) fused.removeLocationUpdates(callback)
        if (::motion.isInitialized) motion.stop()
        if (::recorder.isInitialized) recorder.close()
        scope.cancel()
        TraceBus.update { it.copy(running = false) }
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null
}
