package com.steele.looky.offline

import android.content.Context
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.net.HttpURLConnection
import java.net.URL

data class MapDownloadProgress(val completed: Int, val total: Int, val layer: String, val bytes: Long)

object MapPackDownloader {
    const val CURRENT_DATE = "2026-08-07"
    private const val BASE = "https://maps.mydatatimeline.com/maps/"
    // Navigation-first pack: roads plus highway overlay and the surrounding natural/transit layers.
    val STATE_LAYERS = listOf(
        "address_v2", "buildings_v9", "business_v4", "business_name_index",
        "business_categories", "ev_v1", "highways_v2", "parks_v1", "places_v1",
        "rail_v1", "roads_v2", "trails_v1", "water_v1",
    )
    val CORE_LAYERS = STATE_LAYERS
    val US_STATES = "AL AK AZ AR CA CO CT DE FL GA HI ID IL IN IA KS KY LA ME MD MA MI MN MS MO MT NE NV NH NJ NM NY NC ND OH OK OR PA RI SC SD TN TX UT VT VA WA WV WI WY DC".split(' ')
    val US_LAYERS = listOf("admin", "camera", "signals")

    suspend fun downloadCurrentState(
        context: Context,
        state: String,
        onProgress: (MapDownloadProgress) -> Unit,
    ): Result<Int> {
        require(state in US_STATES) { "Unknown US state: $state" }
        return downloadStates(context, listOf(state), onProgress, includeUsLayers = true)
    }

    suspend fun downloadStates(context: Context, states: List<String>, onProgress: (MapDownloadProgress) -> Unit, includeUsLayers: Boolean = false): Result<Int> = withContext(Dispatchers.IO) {
        val dir = PackManager(context).packsDir
        var completed = 0
        val jobs = states.flatMap { state -> STATE_LAYERS.map { state to it } } + if (includeUsLayers) US_LAYERS.map { "US" to it } else emptyList()
        runCatching {
            jobs.forEach { (state, layer) ->
                val extension = if (layer == "business_categories") "json" else "ptiles"
                val name = "$state.$layer.$extension"
                val target = java.io.File(dir, name)
                val connection = URL("$BASE$CURRENT_DATE/$name").openConnection() as HttpURLConnection
                connection.connectTimeout = 15_000
                connection.readTimeout = 120_000
                connection.requestMethod = "GET"
                connection.connect()
                if (connection.responseCode !in 200..299) error("${connection.responseCode} for $layer")
                val pending = java.io.File(dir, ".$name.pending")
                pending.outputStream().use { output ->
                    connection.inputStream.use { input ->
                        val buffer = ByteArray(64 * 1024)
                        var total = 0L
                        while (true) {
                            val n = input.read(buffer)
                            if (n < 0) break
                            output.write(buffer, 0, n)
                            total += n
                            onProgress(MapDownloadProgress(completed, jobs.size, "$state $layer", total))
                        }
                    }
                }
                connection.disconnect()
                if (target.exists()) target.delete()
                if (!pending.renameTo(target)) error("could not install $name")
                completed++
                onProgress(MapDownloadProgress(completed, jobs.size, "$state $layer", target.length()))
            }
            completed
        }
    }
}
