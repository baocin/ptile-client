package com.steele.looky.location

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * The rate the accelerometer actually delivers.
 *
 * This exists because the phone that prompted it was registered at 50 Hz and
 * delivering a fraction of that, which nothing in the app could see: the
 * setting was reported as if it were the measurement.
 */
class AccelRateTest {
    @Test
    fun `no answer before the window closes`() {
        assertNull(measuredHz(50, 999))
        assertNull(measuredHz(1, 0))
    }

    @Test
    fun `counts what arrived, not what was asked for`() {
        assertEquals(50.0, measuredHz(100, 2_000)!!, 1e-9)
        // The reported symptom: registered at 50 Hz, four samples in two
        // seconds. Nothing should round that up towards the setting.
        assertEquals(2.0, measuredHz(4, 2_000)!!, 1e-9)
    }

    @Test
    fun `a longer window than expected still divides by real elapsed time`() {
        assertEquals(10.0, measuredHz(50, 5_000)!!, 1e-9)
    }
}
