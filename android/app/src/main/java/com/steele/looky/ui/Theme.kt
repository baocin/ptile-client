package com.steele.looky.ui

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.sp

val Forest = Color(0xFF173F35)
val ForestSoft = Color(0xFF315F51)
val Lime = Color(0xFFE7F36A)
val Paper = Color(0xFFF7F7F2)
val Ink = Color(0xFF17201D)
val Clay = Color(0xFFD67246)

// One movement palette for the whole app: the recordings list, the trace
// detail, and anything else that breaks a track down by how it was travelled.
val Driving = Color(0xFF2477D4)
val Walking = Color(0xFF3E9C6D)
val Running = Color(0xFFD67246)
val Stationary = Color(0xFF9AA29C)

/** Colour for a `TraceRecorder` movement label; grey for anything unknown. */
fun movementColor(movement: String): Color = when (movement.lowercase()) {
    "driving" -> Driving
    "walking" -> Walking
    "running" -> Running
    "stationary" -> Stationary
    else -> Color(0xFFC7CCC8)
}

private val LightColors = lightColorScheme(
    primary = Forest,
    onPrimary = Color.White,
    primaryContainer = Lime,
    onPrimaryContainer = Forest,
    secondary = Clay,
    background = Paper,
    onBackground = Ink,
    surface = Color.White,
    onSurface = Ink,
    surfaceVariant = Color(0xFFEAEAE2),
    outline = Color(0xFF737A74),
)

private val DarkColors = darkColorScheme(primary = Lime, secondary = Clay)

private val LookyTypography = Typography(
    displaySmall = TextStyle(fontFamily = FontFamily.SansSerif, fontWeight = FontWeight.Black, fontSize = 36.sp, lineHeight = 39.sp),
    headlineMedium = TextStyle(fontFamily = FontFamily.SansSerif, fontWeight = FontWeight.Bold, fontSize = 25.sp),
    titleLarge = TextStyle(fontFamily = FontFamily.SansSerif, fontWeight = FontWeight.Bold, fontSize = 20.sp),
    titleMedium = TextStyle(fontFamily = FontFamily.SansSerif, fontWeight = FontWeight.SemiBold, fontSize = 16.sp),
    bodyLarge = TextStyle(fontFamily = FontFamily.SansSerif, fontSize = 16.sp, lineHeight = 23.sp),
    labelLarge = TextStyle(fontFamily = FontFamily.SansSerif, fontWeight = FontWeight.Bold, fontSize = 14.sp),
)

@Composable
fun LookyTheme(content: @Composable () -> Unit) {
    MaterialTheme(colorScheme = LightColors, typography = LookyTypography, content = content)
}
