package com.steele.looky.model

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class MotionDiagnosticsTest {
    private fun fix(atMs: Long, speed: Double?) = MotionFix(atMs, speed, null, null)

    @Test fun aWindowWithNoFixesHasNoAverage() {
        val windows = speedWindows(listOf(fix(0L, 4.0)), nowMs = 100_000L)
        assertNull(windows.first { it.seconds == 1 }.meanMps)
        assertEquals(0, windows.first { it.seconds == 1 }.samples)
    }

    @Test fun aThinWindowStillAveragesButReportsItsSampleCount() {
        val now = 100_000L
        val history = listOf(fix(now - 2_500L, 10.0), fix(now - 1_500L, 20.0))
        val three = speedWindows(history, now).first { it.seconds == 3 }
        assertEquals(2, three.samples)
        assertEquals(15.0, three.meanMps!!, 1e-9)
        // The same data over a wider window is the same average -- only the
        // sample count tells the user it is two fixes, not a settled figure.
        assertEquals(2, speedWindows(history, now).first { it.seconds == 610 }.samples)
    }

    @Test fun fixesWithoutSpeedAreExcludedFromTheAverage() {
        val now = 10_000L
        val history = listOf(fix(now - 500L, null), fix(now - 400L, 6.0))
        val one = speedWindows(history, now).first { it.seconds == 1 }
        assertEquals(1, one.samples)
        assertEquals(6.0, one.meanMps!!, 1e-9)
    }

    @Test fun historyDropsAnythingOlderThanTheWidestWindow() {
        var history = emptyList<MotionFix>()
        // 20 minutes at 1 Hz, twice the widest window.
        for (t in 0 until 1_200) history = appendFix(history, fix(t * 1_000L, 1.0))
        assertTrue(history.size <= MOTION_HISTORY_CAP)
        assertEquals(611, history.size)
        assertEquals(589_000L, history.first().atMs)
    }

    @Test fun aBurstOfFixesCannotOutgrowTheCap() {
        var history = emptyList<MotionFix>()
        // Same millisecond, so age trimming cannot help: the cap is the guard.
        repeat(5_000) { history = appendFix(history, fix(1_000L, 1.0)) }
        assertEquals(MOTION_HISTORY_CAP, history.size)
    }

    @Test fun gpsIsFreshUpToThreePollingIntervalsAndStaleAfter() {
        val limit = 3 * 3_000L
        assertFalse(motionStaleness(nowMs = 1L + limit, lastFixAtMs = 1L, lastAccelAtMs = 1L, 3, 50).gpsStale)
        assertTrue(motionStaleness(nowMs = 2L + limit, lastFixAtMs = 1L, lastAccelAtMs = 1L, 3, 50).gpsStale)
    }

    @Test fun accelerometerStalenessScalesWithTheConfiguredRate() {
        // 10 Hz: 100 periods is 10 s. 100 Hz: the floor takes over at 1.5 s.
        assertEquals(10_000L, motionStaleness(0L, 1L, 1L, 3, 10).accelLimitMs)
        assertEquals(1_500L, motionStaleness(0L, 1L, 1L, 3, 100).accelLimitMs)
        assertFalse(motionStaleness(11_000L, 11_000L, 1_000L, 3, 10).accelStale)
        assertTrue(motionStaleness(11_001L, 11_000L, 1_000L, 3, 10).accelStale)
    }

    @Test fun anInputThatNeverReportedIsStaleWithNoAge() {
        val stale = motionStaleness(5_000L, lastFixAtMs = 0L, lastAccelAtMs = 0L, 3, 50)
        assertTrue(stale.gpsStale)
        assertTrue(stale.accelStale)
        assertTrue(stale.any)
        assertEquals(-1L, stale.gpsAgeMs)
    }

    @Test fun aReadingNewerThanTheCallersClockIsFreshNotMissing() {
        // The UI clock ticks at 1 Hz; the bus updates faster than that.
        val stale = motionStaleness(10_000L, lastFixAtMs = 10_095L, lastAccelAtMs = 10_095L, 3, 50)
        assertEquals(0L, stale.accelAgeMs)
        assertFalse(stale.any)
    }

    @Test fun bothInputsFreshMeansNoWarning() {
        assertFalse(motionStaleness(10_000L, 9_500L, 9_900L, 3, 50).any)
    }

    @Test fun aWindowReportsHowItWasTravelled() {
        val now = 10_000L
        val history = listOf(
            MotionFix(now - 9_000, 22.0, null, null, "Driving"),
            MotionFix(now - 2_000, 21.0, null, null, "Driving"),
            MotionFix(now - 1_000, 1.3, null, null, "Walking"),
        )

        val windows = speedWindows(history, now)

        // One stray verdict must not relabel a minute of driving.
        assertEquals("Driving", windows.first { it.seconds == 13 }.movement)
        assertEquals("Walking", windows.first { it.seconds == 1 }.movement)
    }

    @Test fun aWindowWithNoVerdictSaysNothingRatherThanGuessing() {
        val now = 5_000L
        val history = listOf(MotionFix(now - 1_000, 2.0, null, null, null))

        assertNull(speedWindows(history, now).first().movement)
    }
}
