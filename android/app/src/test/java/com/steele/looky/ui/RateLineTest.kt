package com.steele.looky.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class RateLineTest {
    @Test fun aRateThatMatchesIsReportedOnce() {
        assertEquals("50 Hz", rateLine(50.0, 50))
        assertEquals("50 Hz", rateLine(49.6, 50))
    }

    /** The gap is the diagnostic: 2 Hz against a configured 50 is the bug. */
    @Test fun aRateThatFallsShortShowsBothNumbers() {
        val line = rateLine(2.1, 50)

        assertTrue(line, line.contains("2.1"))
        assertTrue(line, line.contains("50"))
    }

    @Test fun beforeAnySampleItSaysSoRatherThanClaimingTheSetting() {
        val line = rateLine(null, 50)

        assertTrue(line, line.contains("nothing measured"))
    }
}
