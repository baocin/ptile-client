package com.steele.looky.location

import android.content.Context
import android.location.Location
import androidx.test.core.app.ApplicationProvider
import com.steele.looky.offline.NearbyContext
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import uniffi.ptiles_ffi.AccelStats
import java.io.File
import java.time.Instant

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34])
class TraceRecorderTest {
    private val context: Context = ApplicationProvider.getApplicationContext()
    private val traceDir by lazy { File(context.filesDir, "traces") }

    @Before
    fun cleanTraceDirectory() {
        traceDir.deleteRecursively()
    }

    @Test
    fun `day file stays valid and follows the Rook extension contract`() {
        val recorder = TraceRecorder(context)
        val first = fix("2026-08-09T12:00:00Z", 35.73377, -88.03220).apply {
            altitude = 127.25
            speed = 4.5f
            accuracy = 3.0f
        }
        val stats = AccelStats(
            variance = 2.75,
            meanMagnitude = 9.81,
            dominantFrequency = 1.5,
            stepCount = 9u,
            windowDurationS = 4.0,
        )
        val result = recorder.append(
            first,
            "Walking",
            stats,
            NearbyContext("Bob & Sons", "residential", 2.4),
        )

        var xml = result.file.readText()
        assertTrue(xml.startsWith("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"))
        assertTrue(xml.endsWith("</gpx>\n"))
        assertTrue(xml.contains("creator=\"Looky\""))
        assertTrue(xml.contains("xmlns:rook=\"https://rookery.local/gpx/1\""))
        assertTrue(xml.contains("<trk><name>Walking</name><trkseg>"))
        assertTrue(xml.contains("<ele>127.25</ele>"))
        assertTrue(xml.contains("<speed>4.5</speed>"))
        assertTrue(xml.contains("<accuracy>3.0</accuracy>"))
        assertTrue(xml.contains("<accel_variance>2.75</accel_variance>"))
        assertTrue(xml.contains("<accel_freq>1.5</accel_freq>"))
        assertTrue(xml.contains("<accel_steps>9</accel_steps>"))
        assertTrue(xml.contains("<gpxtpx:cad>90</gpxtpx:cad>"))
        assertTrue(xml.contains("<name>Bob &amp; Sons</name>"))
        assertEquals(1, "<rook:context ".toRegex().findAll(xml).count())
        assertTrue(xml.indexOf("<rook:context ") > xml.lastIndexOf("</trkpt>"))

        val absent = fix("2026-08-09T12:00:07Z", 35.73400, -88.03230)
        recorder.append(absent, "Walking", stats.copy(dominantFrequency = 0.0), NearbyContext(null, null, null))
        xml = result.file.readText()
        val secondPoint = "<trkpt ".toRegex().findAll(xml).toList()[1].range.first
        val secondPointXml = xml.substring(secondPoint, xml.indexOf("</trkpt>", secondPoint) + 8)
        assertFalse(secondPointXml.contains("<ele>"))
        assertFalse(secondPointXml.contains("<speed>"))
        assertFalse(secondPointXml.contains("<accuracy>"))
        assertFalse(secondPointXml.contains("gpxtpx:TrackPointExtension"))
        assertEquals(1, "<rook:context ".toRegex().findAll(xml).count())

        recorder.append(absent, "Driving", stats, NearbyContext(null, "tertiary", 1.0))
        xml = result.file.readText()
        assertEquals(2, "<trk><name>".toRegex().findAll(xml).count())
        assertTrue(xml.contains("<trk><name>Driving</name><trkseg>"))
        assertEquals(2, "<rook:context ".toRegex().findAll(xml).count())
        assertTrue(xml.endsWith("</trkseg></trk>\n</gpx>\n"))
        recorder.close()
    }

    @Test
    fun `reopening starts a new track and appends to the existing day`() {
        val stats = AccelStats(0.0, null, 0.0, 0u, null)
        val point = fix("2026-08-09T15:00:00Z", 35.73, -88.03)
        val first = TraceRecorder(context)
        val file = first.append(point, "Stationary", stats, NearbyContext(null, null, null)).file
        first.close()

        val reopened = TraceRecorder(context)
        val result = reopened.append(point, "Stationary", stats, NearbyContext(null, null, null))
        val xml = file.readText()
        assertEquals(2, result.pointsToday)
        assertEquals(2, "<trk><name>Stationary</name>".toRegex().findAll(xml).count())
        assertEquals(2, "<trkpt ".toRegex().findAll(xml).count())
        assertTrue(xml.endsWith("</gpx>\n"))
        reopened.close()
    }

    private fun fix(instant: String, lat: Double, lon: Double) = Location("test").apply {
        latitude = lat
        longitude = lon
        time = Instant.parse(instant).toEpochMilli()
    }
}
