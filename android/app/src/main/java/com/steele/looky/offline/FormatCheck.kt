package com.steele.looky.offline

import android.content.Context
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONObject
import java.net.HttpURLConnection
import java.net.URL

/**
 * Is this build of Looky too old for the snapshot it is about to download?
 *
 * Layer formats move independently: buildings went v9 -> v10 when records
 * gained flag-guarded alternative names, highways v2 -> v3 when its records
 * were finally written in the layout the decoder implements. An app that
 * downloads a newer format than its bundled core can parse does not crash --
 * it draws an empty map, or plausible nonsense, and the user has no way to
 * know which.
 *
 * So the APK carries `assets/client_formats.json`, written by the ptiles repo's
 * scripts/write_client_manifest.py from the build this app was verified
 * against, and compares it with the snapshot's own manifest.json before
 * downloading anything. The web demo carries its own copy of the same file for
 * the same reason: the two ship on different days against different snapshots,
 * so one shared list would be wrong for whichever shipped second.
 */
object FormatCheck {

    data class Report(
        /** Layer -> "vSnapshot > vClient", for layers this build cannot decode. */
        val unsupported: Map<String, String>,
        /** Layer -> filename stem the snapshot actually publishes, e.g. buildings_v10. */
        val stems: Map<String, String>,
        /** Snapshot date the manifest describes, when it names one. */
        val snapshotDate: String?,
        /** Null when no manifest could be read: then nothing is known, not "all fine". */
        val checked: Boolean,
    ) {
        val isUsable: Boolean get() = unsupported.isEmpty()

        fun summary(): String = when {
            !checked -> "Could not read the snapshot manifest; using built-in layer names."
            unsupported.isEmpty() -> "Snapshot formats match this build."
            else -> "Update Looky: " + unsupported.entries.joinToString(", ") {
                "${it.key} ${it.value}"
            }
        }
    }

    private const val MANIFEST_TIMEOUT_MS = 15_000

    fun clientFormats(context: Context): JSONObject? = runCatching {
        context.assets.open("client_formats.json").use {
            JSONObject(it.readBytes().decodeToString())
        }
    }.getOrNull()

    /** The version of each layer this build was validated against. */
    fun clientVersions(context: Context): Map<String, Int> {
        val root = clientFormats(context) ?: return emptyMap()
        val formats = root.optJSONObject("formats") ?: return emptyMap()
        val out = mutableMapOf<String, Int>()
        for (layer in formats.keys()) {
            val entry = formats.optJSONObject(layer) ?: continue
            if (!entry.isNull("version")) out[layer] = entry.getInt("version")
        }
        return out
    }

    suspend fun check(context: Context, base: String, date: String): Report =
        withContext(Dispatchers.IO) {
            val client = clientVersions(context)
            val manifest = fetchManifest("$base$date/manifest.json")
                ?: return@withContext Report(emptyMap(), emptyMap(), null, checked = false)

            val layers = manifest.optJSONObject("layers")
                ?: return@withContext Report(emptyMap(), emptyMap(), null, checked = false)

            val unsupported = mutableMapOf<String, String>()
            val stems = mutableMapOf<String, String>()
            for (layer in layers.keys()) {
                val entry = layers.optJSONObject(layer) ?: continue
                val pattern = entry.optString("pattern", "")
                if (pattern.isNotEmpty()) {
                    stems[layer] = pattern.removePrefix("{scope}.").removeSuffix(".ptiles")
                }
                if (entry.isNull("version")) continue  // unversioned, e.g. camera
                val snapshotVersion = entry.getInt("version")
                val known = client[layer] ?: continue  // a layer this build never reads
                if (snapshotVersion > known) {
                    unsupported[layer] = "v$snapshotVersion > v$known"
                }
            }
            Report(unsupported, stems, manifest.optString("built", null), checked = true)
        }

    private fun fetchManifest(url: String): JSONObject? = runCatching {
        val connection = URL(url).openConnection() as HttpURLConnection
        connection.connectTimeout = MANIFEST_TIMEOUT_MS
        connection.readTimeout = MANIFEST_TIMEOUT_MS
        connection.requestMethod = "GET"
        connection.connect()
        if (connection.responseCode !in 200..299) return null
        connection.inputStream.use { JSONObject(it.readBytes().decodeToString()) }
    }.getOrNull()
}
