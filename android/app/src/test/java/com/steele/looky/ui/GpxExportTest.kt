package com.steele.looky.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.time.Instant

class GpxExportTest {
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

    /** `08:05` or `08:00:30`, both on the day the fixtures were recorded. */
    private fun at(clock: String): Instant {
        val withSeconds = if (clock.count { it == ':' } == 2) clock else "$clock:00"
        return Instant.parse("2026-08-13T${withSeconds}Z")
    }

    @Test fun everyFixKeepsItsOwnTimestamp() {
        val segments = GpxReader.segments(gpx)

        assertEquals(3, segments[0].times.size)
        assertEquals(at("08:10"), segments[0].times[1])
    }

    /** An untimed fix must not shift every later pairing by one. */
    @Test fun aFixWithNoTimeLeavesTheOthersAligned() {
        val patchy = """
            <trk><name>Driving</name><trkseg>
            <trkpt lat="35.0" lon="-88.0"></trkpt>
            <trkpt lat="35.1" lon="-88.0"><time>2026-08-13T09:00:00Z</time></trkpt>
            </trkseg></trk>
        """.trimIndent()

        val segment = GpxReader.segments(patchy).single()

        assertEquals(2, segment.times.size)
        assertNull(segment.times[0])
        assertEquals(at("09:00"), segment.times[1])
        assertEquals(at("09:00"), segment.firstFix)
    }

    @Test fun trimmingKeepsOnlyTheFixesInsideTheWindow() {
        val kept = GpxExport.trim(GpxReader.segments(gpx), at("08:05"), at("08:32"))

        assertEquals(listOf("Driving", "Walking"), kept.map { it.movement })
        assertEquals(2, kept[0].points.size)
        assertEquals(1, kept[1].points.size)
        assertEquals(at("08:10"), kept[0].firstFix)
    }

    @Test fun aSegmentEntirelyOutsideTheWindowIsDropped() {
        val kept = GpxExport.trim(GpxReader.segments(gpx), at("08:25"), at("08:45"))

        assertEquals(listOf("Walking"), kept.map { it.movement })
    }

    @Test fun aBackwardsWindowIsReadTheWayItWasMeant() {
        val forwards = GpxExport.trim(GpxReader.segments(gpx), at("08:05"), at("08:25"))
        val backwards = GpxExport.trim(GpxReader.segments(gpx), at("08:25"), at("08:05"))

        assertEquals(forwards.map { it.points }, backwards.map { it.points })
    }

    @Test fun trimmingRecomputesDistanceRatherThanCarryingTheOldOne() {
        val whole = GpxReader.segments(gpx).first()
        val part = GpxExport.trim(listOf(whole), at("08:00"), at("08:10")).single()

        assertTrue(part.distanceM < whole.distanceM)
        assertEquals(GpxReader.pathLengthM(part.points), part.distanceM, 0.001)
    }

    /** The export has to survive the reader it will be opened with. */
    @Test fun whatIsWrittenReadsBackAsWhatWasSelected() {
        val selection = GpxExport.trim(GpxReader.segments(gpx), at("08:05"), at("08:32"))

        val reparsed = GpxReader.segments(GpxExport.write(selection))

        assertEquals(selection.map { it.movement }, reparsed.map { it.movement })
        assertEquals(selection.map { it.points }, reparsed.map { it.points })
        assertEquals(selection.map { it.times }, reparsed.map { it.times })
    }

    @Test fun anExportedNameSaysWhichSliceItIs() {
        val whole = GpxExport.fileName("2026-08-13.drive.gpx", null, null, whole = true)
        val part = GpxExport.fileName("2026-08-13.drive.gpx", at("08:12"), at("09:07"), whole = false)

        assertEquals("2026-08-13.drive.gpx", whole)
        assertTrue(part, part.startsWith("2026-08-13.drive.") && part.endsWith(".gpx"))
        assertTrue(part, part != whole)
    }

    @Test fun aSelectionOfNothingSummarisesAsNothing() {
        val summary = summarise(GpxExport.trim(GpxReader.segments(gpx), at("10:00"), at("11:00")))

        assertEquals(0, summary.points)
        assertNull(summary.from)
    }

    @Test fun theMovementNameIsEscapedRatherThanBreakingTheDocument() {
        val awkward = TraceSegment(
            movement = "Driving & <turning>",
            points = listOf(com.steele.looky.model.GeoPoint(35.0, -88.0)),
            times = listOf(at("08:00")),
            firstFix = at("08:00"),
            lastFix = at("08:00"),
            distanceM = 0.0,
        )

        val written = GpxExport.write(listOf(awkward))

        assertTrue(written.contains("&amp;"))
        assertTrue(written.contains("&lt;turning&gt;"))
    }

    private val sensorGpx = """
        <trk><name>Walking</name><trkseg>
        <trkpt lat="35.0" lon="-88.0"><time>2026-08-13T08:00:00Z</time><extensions><speed>1.4</speed><accuracy>5.0</accuracy><accel_variance>0.42</accel_variance><accel_freq>1.9</accel_freq><accel_steps>12</accel_steps><gpxtpx:TrackPointExtension><gpxtpx:cad>114</gpxtpx:cad></gpxtpx:TrackPointExtension></extensions></trkpt>
        <trkpt lat="35.01" lon="-88.0"><time>2026-08-13T08:01:00Z</time><extensions><speed>1.5</speed><accel_variance>0.51</accel_variance></extensions></trkpt>
        </trkseg></trk>
    """.trimIndent()

    @Test fun sensorsAreKeptVerbatimRatherThanRemodelled() {
        // Parsed as raw payload, so a field the exporter was never taught about
        // still survives the round trip.
        val segment = GpxReader.segments(sensorGpx).single()

        assertEquals(2, segment.sensors.size)
        assertTrue(segment.sensors[0]!!.contains("<accel_freq>1.9</accel_freq>"))
        assertTrue(segment.sensors[0]!!.contains("gpxtpx:cad>114<"))
        assertTrue(segment.sensors[1]!!.contains("<accel_variance>0.51</accel_variance>"))
    }

    @Test fun anExportWithoutSensorsIsJustTheTrack() {
        val written = GpxExport.write(GpxReader.segments(sensorGpx), includeSensors = false)

        assertTrue(written.contains("<trkpt"))
        assertTrue("no extensions when not asked for", !written.contains("accel_variance"))
        assertTrue("no unused namespace either", !written.contains("gpxtpx"))
    }

    @Test fun anExportWithSensorsCarriesThemAndDeclaresTheirNamespaces() {
        val written = GpxExport.write(GpxReader.segments(sensorGpx), includeSensors = true)

        assertTrue(written.contains("<accel_variance>0.42</accel_variance>"))
        assertTrue(written.contains("<gpxtpx:cad>114</gpxtpx:cad>"))
        // A cad element with no declaration makes the whole document invalid.
        assertTrue("gpxtpx must be declared", written.contains("xmlns:gpxtpx="))
    }

    @Test fun trimmingCarriesEachFixesOwnSensorsWithIt() {
        val kept = GpxExport.trim(GpxReader.segments(sensorGpx), at("08:00:30"), at("08:02"))
            .single()

        assertEquals(1, kept.points.size)
        assertTrue(kept.sensors.single()!!.contains("0.51"))
    }

    @Test fun sensorsSurviveAnExportAndReadBackUnchanged() {
        val original = GpxReader.segments(sensorGpx)

        val reparsed = GpxReader.segments(GpxExport.write(original, includeSensors = true))

        assertEquals(original.map { it.sensors }, reparsed.map { it.sensors })
    }
}
