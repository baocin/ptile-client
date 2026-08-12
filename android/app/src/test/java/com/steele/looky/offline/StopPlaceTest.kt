package com.steele.looky.offline

import com.steele.looky.model.GeoPoint
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** What counts as having been somewhere, and what is merely passing it. */
class StopPlaceTest {
    private val here = GeoPoint(35.0, -88.0)

    /** `count` fixes scattered up to `metres` from [here]. */
    private fun cluster(count: Int, metres: Double) = List(count) { i ->
        GeoPoint(here.lat + (if (i % 2 == 0) metres else -metres) / 111_320.0, here.lon)
    }

    @Test fun aParkedPhoneWanderingAFewMetresIsAStop() {
        val centre = PtilesRepository.stopCentroid(cluster(8, 15.0), durationS = 900)

        assertNotNull(centre)
        assertEquals(here.lat, centre!!.lat, 1e-6)
    }

    @Test fun twoFixesAreNotEvidenceOfAnything() {
        assertNull(PtilesRepository.stopCentroid(cluster(2, 2.0), durationS = 900))
    }

    @Test fun aDriveThroughIsNotAVisit() {
        // Two minutes of queueing covers more ground than a parked phone drifts.
        assertNull(PtilesRepository.stopCentroid(cluster(20, 200.0), durationS = 900))
    }

    @Test fun aRedLightIsTooBriefToName() {
        assertNull(PtilesRepository.stopCentroid(cluster(20, 5.0), durationS = 40))
    }

    @Test fun aFileWithNoTimestampsYetStillNamesATightCluster() {
        assertNotNull(PtilesRepository.stopCentroid(cluster(20, 5.0), durationS = null))
    }

    @Test fun aLoopThatReturnsHomeIsAJourneyNotAStop() {
        // Ends where it began, so an end-to-end spread would call it a stop.
        val loop = listOf(
            here,
            GeoPoint(35.01, -88.0),
            GeoPoint(35.01, -87.99),
            GeoPoint(35.0, -87.99),
            here,
        )

        assertNull(PtilesRepository.stopCentroid(loop, durationS = 3_600))
    }

    @Test fun aFootprintContainsWhatSitsInsideIt() {
        val ring = listOf(
            GeoPoint(35.0000, -88.0000),
            GeoPoint(35.0005, -88.0000),
            GeoPoint(35.0005, -87.9995),
            GeoPoint(35.0000, -87.9995),
            GeoPoint(35.0000, -88.0000),
        )

        assertTrue(PtilesRepository.containsPoint(ring, GeoPoint(35.0002, -87.9998)))
        assertFalse(PtilesRepository.containsPoint(ring, GeoPoint(35.0012, -87.9998)))
    }

    @Test fun aCentroidWithNoFootprintIsNotInsideOne() {
        assertFalse(PtilesRepository.containsPoint(emptyList(), here))
        assertFalse(PtilesRepository.containsPoint(listOf(here, here), here))
    }
}
