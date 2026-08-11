package com.steele.looky.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.time.Instant

class MapDetailAndTraceTest {
    @Test fun theOpeningViewDrawsEverything() {
        // The map opens at 1.0; anything culled here is culled by default.
        assertTrue(MapDetail.draws("residential", isPoint = false, scale = 1.0f))
        assertTrue(MapDetail.draws("service", isPoint = false, scale = 1.0f))
        assertTrue(MapDetail.draws("business:5", isPoint = true, scale = 1.0f))
    }

    @Test fun zoomedOutKeepsThroughRoadsAndDropsTheRest() {
        val far = MapDetail.ARTERIAL_ONLY_BELOW - 0.1f

        assertTrue(MapDetail.draws("motorway", isPoint = false, scale = far))
        assertTrue(MapDetail.draws("water", isPoint = false, scale = far))
        assertFalse(MapDetail.draws("residential", isPoint = false, scale = far))
        assertFalse(MapDetail.draws("service", isPoint = false, scale = far))
    }

    @Test fun zoomedInDrawsEveryLine() {
        val near = MapDetail.ARTERIAL_ONLY_BELOW + 0.1f

        assertTrue(MapDetail.draws("residential", isPoint = false, scale = near))
        assertTrue(MapDetail.draws("service", isPoint = false, scale = near))
    }

    @Test fun pointsWaitForACloserZoomThanLines() {
        assertFalse(MapDetail.draws("business:5", isPoint = true, scale = MapDetail.POINTS_ABOVE - 0.1f))
        assertTrue(MapDetail.draws("business:5", isPoint = true, scale = MapDetail.POINTS_ABOVE))
    }

    private val gpx = """
        <trk><name>Driving</name><trkseg>
        <trkpt lat="35.0" lon="-88.0"><time>2026-08-09T08:00:00Z</time></trkpt>
        <trkpt lat="35.1" lon="-88.0"><time>2026-08-09T08:05:00Z</time></trkpt>
        <trkpt lat="35.2" lon="-88.0"><time>2026-08-09T08:10:00Z</time></trkpt>
        </trkseg></trk>
        <trk><name>Walking</name><trkseg>
        <trkpt lat="35.3" lon="-88.0"><time>2026-08-09T08:20:00Z</time></trkpt>
        </trkseg></trk>
    """.trimIndent()

    @Test fun aTraceBreaksDownIntoFixesPerMovement() {
        val trace = GpxReader.parse(gpx)

        assertEquals(listOf("Driving" to 3, "Walking" to 1), trace.breakdown)
        assertEquals(4, trace.points.size)
    }

    @Test fun totalsMergeRepeatedLabelsAndRankThem() {
        val totals = GpxReader.totals(listOf("Walking" to 2, "Driving" to 5, "Walking" to 4))

        assertEquals(listOf("Walking" to 6, "Driving" to 5), totals)
    }

    @Test fun theFirstAndLastFixBoundTheDay() {
        val trace = GpxReader.parse(gpx)

        assertEquals(Instant.parse("2026-08-09T08:00:00Z"), trace.firstFix)
        assertEquals(Instant.parse("2026-08-09T08:20:00Z"), trace.lastFix)
    }

    @Test fun aFileWithNoTimesHasNoSpanToShow() {
        assertNull(formatSpan(null, null))
    }

    @Test fun eachTrackBecomesOneSegmentOfTheDay() {
        val segments = GpxReader.segments(gpx)

        assertEquals(listOf("Driving", "Walking"), segments.map { it.movement })
        assertEquals(3, segments[0].points.size)
        assertEquals(Instant.parse("2026-08-09T08:00:00Z"), segments[0].firstFix)
        assertEquals(Instant.parse("2026-08-09T08:10:00Z"), segments[0].lastFix)
    }

    @Test fun aSegmentCarriesTheDistanceItsOwnPointsCover() {
        val segments = GpxReader.segments(gpx)

        assertTrue(segments[0].distanceM > 20_000)
        assertEquals(0.0, segments[1].distanceM, 0.0)
    }

    @Test fun anEmptyTrackIsNotASegment() {
        val text = "<trk><name>Stationary</name><trkseg></trkseg></trk>"

        assertTrue(GpxReader.segments(text).isEmpty())
    }
}
