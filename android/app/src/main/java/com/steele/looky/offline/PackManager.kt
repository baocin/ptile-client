package com.steele.looky.offline

import android.content.Context
import java.io.File

data class OfflinePack(val region: String, val layers: List<File>, val bytes: Long)

class PackManager(private val context: Context) {
    val packsDir: File = File(context.filesDir, "ptiles").apply { mkdirs() }

    fun installBundledDemoIfNeeded() {
        val names = runCatching { context.assets.list("ptiles")?.toList().orEmpty() }.getOrDefault(emptyList())
        names.forEach { name ->
            val out = File(packsDir, name)
            if (!out.exists()) {
                context.assets.open("ptiles/$name").use { input ->
                    out.outputStream().use(input::copyTo)
                }
            }
        }
    }

    fun importLayer(displayName: String, bytes: ByteArray): File {
        require(displayName.endsWith(".ptiles", ignoreCase = true)) { "Choose a .ptiles file" }
        val safe = displayName.substringAfterLast('/').replace(Regex("[^A-Za-z0-9_.-]"), "_")
        return File(packsDir, safe).also { file ->
            val pending = File(packsDir, ".$safe.pending")
            pending.writeBytes(bytes)
            if (!pending.renameTo(file)) {
                file.writeBytes(bytes)
                pending.delete()
            }
        }
    }

    fun packs(): List<OfflinePack> = packsDir.listFiles()
        .orEmpty()
        .filter { it.isFile && it.extension == "ptiles" }
        .groupBy { it.name.substringBefore('.') }
        .map { (region, files) -> OfflinePack(region, files.sortedBy(File::getName), files.sumOf(File::length)) }
        .sortedBy { it.region }
}
