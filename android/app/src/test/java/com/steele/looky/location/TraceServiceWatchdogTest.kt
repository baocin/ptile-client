package com.steele.looky.location

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class TraceServiceWatchdogTest {
    @Test fun aFastPollingRateStillWaitsAMinuteBeforeResubscribing() {
        assertEquals(60_000L, TraceService.staleAfterMs(3))
        assertEquals(60_000L, TraceService.staleAfterMs(10))
    }

    @Test fun aSlowPollingRateWaitsProportionallyLonger() {
        assertEquals(180_000L, TraceService.staleAfterMs(30))
        assertTrue(TraceService.staleAfterMs(60) <= 300_000L)
    }
}
