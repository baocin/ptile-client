package com.steele.looky.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.time.Instant

/** What the export sheet will write for a given stretch and trim window. */
class ExportSelectionTest {
    private val gpx = """
        <trk><name>Driving</name><trkseg>
        <trkpt lat="35.00" lon="-88.0"><time>2026-08-13T08:00:00Z</time></trkpt>
        <trkpt lat="35.05" lon="-88.0"><time>2026-08-13T08:10:00Z</time></trkpt>
        <trkpt lat="35.10" lon="-88.0"><time>2026-08-13T08:20:00Z</time></trkpt>
        </trkseg></trk>
        <trk><name>Walking</name><trkseg>
        <trkpt lat="35.15" lon="-88.0"><time>2026-08-13T08:30:00Z</time></trkpt>
        <trkpt lat="35.16" lon="-88.0"><time>2026-08-13T08:40:00Z</time></trkpt>
        </trkseg></trk>
    """.trimIndent()

    private val segments = GpxReader.segments(gpx)

    @Test fun anUntouchedWindowIsTheWholeRecording() {
        val selection = GpxExport.select(segments, stretch = null, range = 0f..1f)

        assertEquals(segments, selection)
    }

    /** Picking a stretch must keep all of it -- both ends included. */
    @Test fun oneStretchExportsExactlyItself() {
        val selection = GpxExport.select(segments, stretch = 1, range = 0f..1f)

        assertEquals(1, selection.size)
        assertEquals("Walking", selection.single().movement)
        assertEquals(2, selection.single().points.size)
    }

    @Test fun theTrimNarrowsWithinTheChosenStretch() {
        // The drive alone spans 08:00-08:20; the back half is its last two fixes.
        val selection = GpxExport.select(segments, stretch = 0, range = 0.5f..1f)

        assertEquals(1, selection.size)
        assertEquals(Instant.parse("2026-08-13T08:10:00Z"), selection.single().firstFix)
        assertEquals(2, selection.single().points.size)
    }

    @Test fun theTrimOverTheWholeRecordingCanDropAStretchEntirely() {
        // 0..0.4 of 08:00-08:40 ends at 08:16, before the walk starts.
        val selection = GpxExport.select(segments, stretch = null, range = 0f..0.4f)

        assertEquals(listOf("Driving"), selection.map { it.movement })
        assertEquals(2, selection.single().points.size)
    }

    @Test fun aStretchIndexThatIsGoneSelectsNothingRatherThanEverything() {
        assertTrue(GpxExport.select(segments, stretch = 9, range = 0f..1f).isEmpty())
    }

    /** A recording rolled up from several files is still one recording. */
    @Test fun stretchesFromSeveralFilesRollIntoOneTrace() {
        val trace = GpxReader.traceOf(segments)

        assertEquals(5, trace.points.size)
        assertEquals(listOf("Driving" to 3, "Walking" to 2), trace.breakdown)
        assertEquals(Instant.parse("2026-08-13T08:00:00Z"), trace.firstFix)
        assertEquals(Instant.parse("2026-08-13T08:40:00Z"), trace.lastFix)
        assertTrue(trace.distanceM > 0.0)
    }
}
