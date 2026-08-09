package com.steele.looky.offline

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File
import java.io.RandomAccessFile
import uniffi.ptiles_ffi.CameraInfo
import uniffi.ptiles_ffi.LatLon

class PtilesRepositoryTest {
    @get:Rule
    val temporary = TemporaryFolder()

    private fun file(name: String, length: Long = 1L, modified: Long = 1L): File =
        temporary.newFile(name).also {
            RandomAccessFile(it, "rw").use { output -> output.setLength(length) }
            check(it.setLastModified(modified))
        }

    @Test
    fun versionedDownloadedLayerWinsOverLegacySliceAndOtherKinds() {
        val legacy = file("TN.roads.ptiles", length = 40_000L, modified = 30L)
        val v2 = file("TN.roads_v2.ptiles", length = 2L, modified = 10L)
        val v1 = file("TN.roads_v1.ptiles", length = 3L, modified = 20L)
        val parks = file("TN.parks_v9.ptiles", length = 4L, modified = 40L)

        val candidates = PtilesRepository.layerCandidates(
            arrayOf(legacy, v1, parks, v2),
            "roads",
        )

        assertEquals(listOf(v2, v1, legacy), candidates)
        assertFalse(candidates.contains(parks))
    }

    @Test
    fun everyInstalledStateRemainsACandidateForTheCoverageCheck() {
        val alaska = file("AK.roads_v2.ptiles")
        val montana = file("MT.roads_v2.ptiles")
        val tennessee = file("TN.roads_v2.ptiles")

        val candidates = PtilesRepository.layerCandidates(
            arrayOf(montana, tennessee, alaska),
            "roads",
        )

        assertEquals(3, candidates.size)
        assertTrue(candidates.containsAll(listOf(alaska, montana, tennessee)))
        assertEquals(
            tennessee,
            PtilesRepository.layerCandidates(
                arrayOf(montana, tennessee, alaska),
                "roads",
                preferredState = "TN",
            ).first(),
        )
    }

    @Test
    fun oldBundledConformanceRoadSliceIsNotAMapPack() {
        val slice = file(
            "TN.roads.ptiles",
            PackManager.BUNDLED_CONFORMANCE_LENGTHS.getValue("TN.roads.ptiles"),
        )

        assertTrue(PackManager.isBundledConformanceSlice(slice))
        assertTrue(PtilesRepository.layerCandidates(arrayOf(slice), "roads").isEmpty())
    }

    @Test
    fun downloadedCameraBecomesAMapMarkerWithUsefulLabel() {
        val feature = PtilesRepository.cameraMapFeature(
            CameraInfo(
                osmId = 9L,
                location = LatLon(36.16, -86.78),
                deviceType = "camera",
                placement = "public",
                cameraType = "fixed",
                direction = null,
                angle = null,
                operator = "Metro",
                name = null,
                refTag = null,
            )
        )

        assertEquals("camera:fixed", feature.kind)
        assertEquals("Metro", feature.name)
        assertEquals(36.16, feature.points.single().lat, 0.0)
    }
}
