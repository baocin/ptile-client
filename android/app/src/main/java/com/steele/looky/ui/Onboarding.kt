package com.steele.looky.ui

import androidx.compose.foundation.background
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
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.ArrowBack
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.steele.looky.offline.MapDownloadProgress

private data class Intro(val eyebrow: String, val title: String, val body: String)

@Composable
fun Onboarding(
    permissionsGranted: Boolean,
    onRequestPermissions: () -> Unit,
    onExploreOffline: () -> Unit,
    mapDownload: MapDownloadProgress? = null,
    mapDownloadRunning: Boolean = false,
    mapDownloadError: String? = null,
    onDownloadMaps: () -> Unit = {},
    onComplete: () -> Unit,
) {
    val pages = listOf(
        Intro("MEET LOOKY", "The map that keeps looking out for you.", "Drive across town or walk beyond reception. Looky keeps maps, routes, and your complete trace on your phone."),
        Intro("OFFLINE BY DESIGN", "Installed means available.", "PTiles packs are decoded locally. Browsing, nearby context, and bounded routes do not depend on a map server once a pack is installed."),
        Intro("ONE DAY, ONE RECORD", "Your route survives the background.", "Looky writes a valid GPX day file after every GPS fix and separates walking, running, driving, and stationary segments automatically."),
        Intro("READY WHEN YOU ARE", "Allow precise location.", "Looky needs location while in use and in the background. Motion recognition improves classification; notifications keep recording visible and controllable."),
    )
    var page by remember { mutableIntStateOf(0) }
    val item = pages[page]
    Surface(Modifier.fillMaxSize(), color = Paper) {
        Column(Modifier.fillMaxSize().padding(horizontal = 24.dp, vertical = 20.dp)) {
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                if (page > 0) IconButton(onClick = { page-- }) { Icon(Icons.AutoMirrored.Rounded.ArrowBack, "Back") }
                else Spacer(Modifier.size(48.dp))
                Row(Modifier.weight(1f), horizontalArrangement = Arrangement.Center) {
                    pages.indices.forEach { i ->
                        Box(
                            Modifier.padding(horizontal = 4.dp).size(if (i == page) 24.dp else 8.dp, 8.dp)
                                .background(if (i == page) Forest else Color(0xFFD1D3CA), CircleShape)
                                .semantics { contentDescription = "Step ${i + 1} of ${pages.size}" }
                        )
                    }
                }
                Spacer(Modifier.size(48.dp))
            }
            Spacer(Modifier.weight(0.8f))
            Box(
                Modifier.size(88.dp).background(if (page == 2) Clay else Lime, RoundedCornerShape(28.dp)),
                contentAlignment = Alignment.Center,
            ) {
                Text(if (page == 0) "L" else listOf("", "↓", "●", "✓")[page], style = MaterialTheme.typography.displaySmall, color = Forest)
            }
            Spacer(Modifier.height(30.dp))
            Text(item.eyebrow, style = MaterialTheme.typography.labelLarge, color = ForestSoft)
            Spacer(Modifier.height(10.dp))
            Text(item.title, style = MaterialTheme.typography.displaySmall, color = Ink)
            Spacer(Modifier.height(18.dp))
            Text(item.body, style = MaterialTheme.typography.bodyLarge, color = Color(0xFF58615C))
            Spacer(Modifier.weight(1f))
            if (page == pages.lastIndex && mapDownloadRunning) {
                Text("Downloading Tennessee offline maps…", color = Forest, fontWeight = FontWeight.Bold)
                mapDownload?.let { Text("${it.completed}/${it.total} · ${it.layer.replace('_', ' ')}", color = ForestSoft) }
                Spacer(Modifier.height(12.dp))
                Button(onClick = {}, enabled = false, modifier = Modifier.fillMaxWidth().height(56.dp)) { Text("Preparing offline maps…") }
            } else if (page == pages.lastIndex && permissionsGranted && mapDownload == null) {
                Text("Tennessee and Montana roads, highways, trails, parks, rail, places, water, buildings, cameras, and lookup layers are ready to cache.", color = ForestSoft)
                mapDownloadError?.let { Text(it, color = Color(0xFF9B3D2B)) }
                Spacer(Modifier.height(10.dp))
                Button(
                    onClick = onDownloadMaps,
                    modifier = Modifier.fillMaxWidth().height(56.dp),
                    shape = RoundedCornerShape(18.dp),
                    colors = ButtonDefaults.buttonColors(containerColor = Lime, contentColor = Forest),
                ) { Text("Download TN + Montana maps", fontWeight = FontWeight.Bold) }
                Spacer(Modifier.height(10.dp))
                Button(onClick = onExploreOffline, modifier = Modifier.fillMaxWidth().height(48.dp), colors = ButtonDefaults.textButtonColors(contentColor = Forest)) { Text("Skip for now") }
            } else if (page == pages.lastIndex && !permissionsGranted) {
                Button(
                    onClick = onRequestPermissions,
                    modifier = Modifier.fillMaxWidth().height(56.dp),
                    shape = RoundedCornerShape(18.dp),
                    colors = ButtonDefaults.buttonColors(containerColor = Lime, contentColor = Forest),
                ) { Text("Allow location & continue", fontWeight = FontWeight.Bold) }
                Spacer(Modifier.height(10.dp))
                Button(
                    onClick = onExploreOffline,
                    modifier = Modifier.fillMaxWidth().height(48.dp),
                    colors = ButtonDefaults.textButtonColors(contentColor = Forest),
                ) { Text("Explore without recording") }
            } else if (page != pages.lastIndex) {
                Button(
                    onClick = { if (page < pages.lastIndex) page++ else onComplete() },
                    modifier = Modifier.fillMaxWidth().height(56.dp),
                    shape = RoundedCornerShape(18.dp),
                    colors = ButtonDefaults.buttonColors(containerColor = Forest, contentColor = Color.White),
                ) { Text("Continue") }
            }
        }
    }
}
