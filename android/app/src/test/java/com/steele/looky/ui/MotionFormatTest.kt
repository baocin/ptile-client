package com.steele.looky.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class MotionFormatTest {
    @Test fun compassWrapsAroundNorthInBothDirections() {
        assertEquals("N", compassPoint(0.0))
        assertEquals("N", compassPoint(359.0))
        assertEquals("N", compassPoint(-1.0))
        assertEquals("NE", compassPoint(45.0))
        assertEquals("W", compassPoint(270.0))
        assertEquals("N", compassPoint(720.0))
    }

    @Test fun speedConvertsToTheUnitTheUserPicked() {
        assertEquals("22.4 mph", formatSpeed(10.0, imperial = true))
        assertEquals("36.0 km/h", formatSpeed(10.0, imperial = false))
    }

    @Test fun agesReadAtTheScaleTheyHappen() {
        assertEquals("400ms", formatAge(400))
        assertEquals("9.0s", formatAge(9_000))
        assertEquals("2m 5s", formatAge(125_000))
        assertEquals("1h 1m", formatAge(3_660_000))
    }

    @Test fun anInputThatNeverReportedSaysSoRatherThanPrintingANegativeAge() {
        assertTrue(staleLine("GPS", -1L, 9_000L, "3s polling").contains("nothing"))
        assertTrue(staleLine("GPS", 20_000L, 9_000L, "3s polling").contains("20.0s"))
    }
}
