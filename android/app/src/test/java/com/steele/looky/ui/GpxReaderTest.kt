package com.steele.looky.ui

import com.steele.looky.model.GeoPoint
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The fixture mirrors what `TraceRecorder` writes: a GPX 1.1 document whose
 * track points carry the recorder's `<extensions>` block and whose `<trk>`
 * elements are named after the movement classification.
 */
class GpxReaderTest {
    private val recorded = """
        <?xml version="1.0" encoding="UTF-8"?>
        <gpx version="1.1" creator="Looky" xmlns="http://www.topografix.com/GPX/1/1">
        <trk><name>Walking</name><trkseg>
        <trkpt lat="35.73377" lon="-88.03220"><ele>120.0</ele><time>2026-08-09T10:00:00Z</time><extensions><accel_variance>0.4</accel_variance></extensions></trkpt>
        <trkpt lat="35.73477" lon="-88.03220"><time>2026-08-09T10:00:03Z</time><extensions><accel_variance>0.5</accel_variance></extensions></trkpt>
        </trkseg></trk>
        <trk><name>Driving</name><trkseg>
        <trkpt lat="35.73577" lon="-88.03220"><time>2026-08-09T10:00:06Z</time><extensions></extensions></trkpt>
        </trkseg></trk>
        </gpx>
    """.trimIndent()

    @Test fun readsEveryTrackPointInOrder() {
        val trace = GpxReader.parse(recorded)

        assertEquals(3, trace.points.size)
        assertEquals(35.73377, trace.points.first().lat, 1e-9)
        assertEquals(-88.03220, trace.points.first().lon, 1e-9)
        assertEquals(35.73577, trace.points.last().lat, 1e-9)
    }

    @Test fun collectsTheMovementLabelsPresent() {
        assertEquals(listOf("Walking", "Driving"), GpxReader.parse(recorded).movements)
    }

    @Test fun distanceIsTheHaversineSumOfConsecutiveFixes() {
        val trace = GpxReader.parse(recorded)

        // Two 0.001-degree steps of latitude, ~111 m each.
        assertEquals(222.0, trace.distanceM, 3.0)
    }

    @Test fun aTruncatedTailStillYieldsThePointsItCanRead() {
        // TraceRecorder appends live, so a file read mid-write can be cut off.
        val truncated = recorded.substringBefore("<trk><name>Driving")

        val trace = GpxReader.parse(truncated)

        assertEquals(2, trace.points.size)
        assertEquals(listOf("Walking"), trace.movements)
    }

    @Test fun anEmptyDocumentIsNotAnError() {
        val trace = GpxReader.parse("<gpx></gpx>")

        assertTrue(trace.points.isEmpty())
        assertEquals(0.0, trace.distanceM, 0.0)
    }

    @Test fun distanceBetweenTwoKnownPointsMatchesTheGreatCircle() {
        // Nashville to Memphis, ~320 km.
        val nashville = GeoPoint(36.1627, -86.7816)
        val memphis = GeoPoint(35.1495, -90.0490)

        assertEquals(320_000.0, GpxReader.distanceM(nashville, memphis), 5_000.0)
    }
}
