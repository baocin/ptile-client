package com.steele.looky.ui

import android.content.Intent
import android.net.Uri
import android.os.PowerManager
import android.provider.Settings
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.steele.looky.AppSettings
import com.steele.looky.model.LiveTraceState
import com.steele.looky.model.MotionStaleness
import com.steele.looky.model.motionStaleness
import com.steele.looky.model.speedWindows
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import kotlin.math.roundToInt

private val CLOCK = DateTimeFormatter.ofPattern("HH:mm:ss").withZone(ZoneId.systemDefault())

/**
 * Why the movement badge says what it says.
 *
 * The badge is one word derived from two inputs that can each quietly die --
 * a location subscription Doze killed, a sensor the OS throttled -- and when
 * they do the word stops changing rather than turning into an error. This is
 * where that becomes visible: the raw readings, how far back the averages
 * still have data, and how long each input has been quiet.
 */
/**
 * Measured rate against the configured one.
 *
 * They diverge for reasons the user can act on -- a busy screen, power
 * management, a hardware rate the platform rounded to -- so the gap is the
 * diagnostic, and either number alone hides it.
 */
internal fun rateLine(measured: Double?, configured: Int): String = when {
    measured == null -> "$configured Hz asked, nothing measured yet"
    kotlin.math.abs(measured - configured) < 1.0 -> "%.0f Hz".format(measured)
    else -> "%.1f Hz arriving, $configured Hz asked".format(measured)
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun MotionSheet(live: LiveTraceState, settings: AppSettings, nowMs: Long, onDismiss: () -> Unit) {
    val context = LocalContext.current
    val imperial = settings.imperialUnits
    val stale = motionStaleness(
        nowMs,
        live.lastFixAtMs,
        live.lastAccelAtMs,
        settings.gpsIntervalSeconds,
        settings.accelerometerRateHz,
    )
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = false),
        containerColor = Color.White,
    ) {
        Column(
            Modifier.fillMaxWidth().verticalScroll(rememberScrollState())
                .padding(start = 20.dp, end = 20.dp, bottom = 32.dp),
            verticalArrangement = Arrangement.spacedBy(2.dp),
        ) {
            Text("Motion", style = MaterialTheme.typography.headlineSmall, color = Forest)
            Text(
                "${live.movement} · ${(live.confidence * 100).roundToInt()}% confidence",
                style = MaterialTheme.typography.bodyMedium,
                color = ForestSoft,
            )
            Spacer(Modifier.height(14.dp))
            DiagnosticRow("Speed", live.location?.let {
                if (it.hasSpeed()) formatSpeed(it.speed.toDouble(), imperial) else null
            } ?: "—")
            DiagnosticRow("Heading", live.location?.let {
                if (it.hasBearing()) "${it.bearing.roundToInt()}° ${compassPoint(it.bearing.toDouble())}" else null
            } ?: "—")
            DiagnosticRow("Accuracy", live.location?.let {
                if (it.hasAccuracy()) "±${it.accuracy.roundToInt()} m" else null
            } ?: "—")
            DiagnosticRow("GPS fix", inputAge(live.lastFixAtMs, stale.gpsAgeMs))
            DiagnosticRow("Accelerometer", inputAge(live.lastAccelAtMs, stale.accelAgeMs))
            DiagnosticRow("Accelerometer rate", rateLine(live.accelHz, settings.accelerometerRateHz))

            if (stale.any) {
                Spacer(Modifier.height(14.dp))
                Text("Stale input", style = MaterialTheme.typography.titleMedium, color = Clay)
                if (stale.gpsStale) {
                    Text(
                        staleLine("GPS", stale.gpsAgeMs, stale.gpsLimitMs, "${settings.gpsIntervalSeconds}s polling"),
                        style = MaterialTheme.typography.bodyMedium,
                        color = ForestSoft,
                    )
                }
                if (stale.accelStale) {
                    Text(
                        // The configured rate is what was asked for, not what
                        // arrived: the delay is a hint the platform rounds, and
                        // a saturated looper makes the driver drop samples
                        // outright. Printing the setting here explained a stall
                        // with the number that was not happening.
                        staleLine(
                            "Accelerometer",
                            stale.accelAgeMs,
                            stale.accelLimitMs,
                            rateLine(live.accelHz, settings.accelerometerRateHz),
                        ),
                        style = MaterialTheme.typography.bodyMedium,
                        color = ForestSoft,
                    )
                }
                PowerAdvice(stale)
            }

            Spacer(Modifier.height(18.dp))
            Text("ROLLING SPEED", style = MaterialTheme.typography.labelLarge, color = ForestSoft)
            HorizontalDivider(Modifier.padding(vertical = 6.dp))
            speedWindows(live.fixes, nowMs).forEach { window ->
                Row(Modifier.fillMaxWidth().padding(vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text(
                        "${window.seconds}s",
                        Modifier.width(70.dp),
                        style = MaterialTheme.typography.labelLarge,
                        color = ForestSoft,
                    )
                    Text(
                        window.meanMps?.let { formatSpeed(it, imperial) } ?: "—",
                        Modifier.width(100.dp),
                        style = MaterialTheme.typography.bodyLarge,
                        color = Forest,
                    )
                    // The verdict beside the speed, because either alone is
                    // ambiguous: 1.4 m/s is a brisk walk or a car in a car
                    // park, and the two disagreeing is how a starved
                    // classifier shows itself.
                    Row(Modifier.width(112.dp), verticalAlignment = Alignment.CenterVertically) {
                        window.movement?.let { movement ->
                            Box(Modifier.size(8.dp).background(movementColor(movement), CircleShape))
                            Spacer(Modifier.width(6.dp))
                            Text(
                                movement,
                                style = MaterialTheme.typography.labelMedium,
                                color = Forest,
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                            )
                        }
                    }
                    Text(
                        if (window.samples == 1) "1 fix" else "${window.samples} fixes",
                        style = MaterialTheme.typography.labelMedium,
                        // Two fixes averaged is a guess wearing a number's
                        // clothes, so the count is never optional.
                        color = if (window.samples < 3) Clay else ForestSoft,
                    )
                }
            }
        }
    }
}

/**
 * Whether the OS is the reason an input went quiet.
 *
 * Both checks are advisory: power saving and battery optimisation are the
 * usual causes of a throttled sensor or a suspended location subscription,
 * but neither proves it, so this suggests rather than concludes.
 */
@Composable
private fun PowerAdvice(stale: MotionStaleness) {
    val context = LocalContext.current
    val power = context.getSystemService(PowerManager::class.java)
    val saving = power?.isPowerSaveMode == true
    val optimised = power?.isIgnoringBatteryOptimizations(context.packageName) == false
    if (!saving && !optimised) {
        Text(
            "Battery settings look fine, so this is more likely a GPS signal or provider problem.",
            style = MaterialTheme.typography.bodyMedium,
            color = ForestSoft,
            modifier = Modifier.padding(top = 8.dp),
        )
        return
    }
    Text(
        listOfNotNull(
            if (saving) "Battery saver is on" else null,
            if (optimised) "Looky is battery-optimised" else null,
        ).joinToString(" and ") + ", which throttles background sensors and location.",
        style = MaterialTheme.typography.bodyMedium,
        color = ForestSoft,
        modifier = Modifier.padding(top = 8.dp),
    )
    Row(Modifier.padding(top = 10.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        if (optimised) {
            FilledTonalButton(onClick = {
                context.startActivity(
                    Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS)
                        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                )
            }) { Text("Battery optimisation") }
        }
        FilledTonalButton(onClick = {
            context.startActivity(
                Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.fromParts("package", context.packageName, null))
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            )
        }) { Text("App settings") }
    }
}

@Composable
private fun DiagnosticRow(label: String, value: String) {
    Row(Modifier.fillMaxWidth().padding(vertical = 8.dp)) {
        Text(label, Modifier.width(150.dp), style = MaterialTheme.typography.labelLarge, color = ForestSoft)
        Text(value, style = MaterialTheme.typography.bodyLarge, color = Forest)
    }
}

private fun inputAge(atMs: Long, ageMs: Long): String =
    if (atMs <= 0L) "never" else "${CLOCK.format(Instant.ofEpochMilli(atMs))} · ${formatAge(ageMs)} ago"

internal fun staleLine(name: String, ageMs: Long, limitMs: Long, configured: String): String =
    if (ageMs < 0L) "$name has produced nothing since recording started ($configured)."
    else "$name last reported ${formatAge(ageMs)} ago, past the ${formatAge(limitMs)} limit for $configured."

internal fun formatAge(ms: Long): String = when {
    ms < 1_000 -> "${ms}ms"
    ms < 60_000 -> "%.1fs".format(ms / 1_000.0)
    ms < 3_600_000 -> "${ms / 60_000}m ${(ms % 60_000) / 1_000}s"
    else -> "${ms / 3_600_000}h ${(ms % 3_600_000) / 60_000}m"
}

internal fun formatSpeed(mps: Double, imperial: Boolean): String =
    if (imperial) "%.1f mph".format(mps * 2.23694) else "%.1f km/h".format(mps * 3.6)
