package com.steele.looky.location

import org.junit.Assert.assertTrue
import org.junit.Test

class TraceServicePolicyTest {
    @Test fun movementClassificationRefreshesAtInteractiveCadence() {
        assertTrue(TraceService.CLASSIFICATION_INTERVAL_MS <= 2_000L)
    }
}
