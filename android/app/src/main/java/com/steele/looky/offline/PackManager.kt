package com.steele.looky.offline

import android.content.Context
import java.io.File

data class OfflinePack(val region: String, val layers: List<File>, val bytes: Long)

class PackManager(private val context: Context) {
    val packsDir: File = File(context.filesDir, "ptiles").apply { mkdirs() }

    /**
     * Remove the tiny conformance slices shipped by the first debug build.
     *
     * They retain a statewide header but only a short prefix of real cells,
     * so they look like downloaded state maps while returning empty blocks in
     * almost every location. Exact filename + byte length keeps this migration
     * from touching a user-imported layer that merely has a legacy filename.
     */
    fun removeBundledConformanceSlices() {
        BUNDLED_CONFORMANCE_LENGTHS.forEach { (name, length) ->
            File(packsDir, name).takeIf { it.isFile && it.length() == length }?.delete()
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

    companion object {
        internal val BUNDLED_CONFORMANCE_LENGTHS = mapOf(
            "TN.buildings_v8.ptiles" to 39_958L,
            "TN.business.ptiles" to 8_460L,
            "TN.parks.ptiles" to 7_900L,
            "TN.roads.ptiles" to 29_897L,
            "TN.water.ptiles" to 25_898L,
        )

        internal fun isBundledConformanceSlice(file: File): Boolean =
            BUNDLED_CONFORMANCE_LENGTHS[file.name] == file.length()
    }
}
