package com.steele.looky.ui

import com.steele.looky.model.GeoPoint
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Leaving the route used to do nothing at all: the turn card went red, the
 * line on the map stayed where it was, and the app kept giving directions to a
 * road the driver was no longer on. That is worse than no guidance, because it
 * still looks like guidance.
 */
class RerouteTest {
    private val start = 1_000_000L

    @Test fun stayingOnTheRouteNeverReplans() {
        assertFalse(shouldReroute(false, start, 0L, start + 60_000, busy = false))
    }

    @Test fun oneFixOffTheRouteOnlyStartsTheClock() {
        // GPS through a tunnel or a car park comes back on its own.
        assertFalse(shouldReroute(true, 0L, 0L, start, busy = false))
        assertFalse(shouldReroute(true, start, 0L, start + 3_000, busy = false))
    }

    @Test fun aSustainedDepartureReplans() {
        assertTrue(shouldReroute(true, start, 0L, start + REROUTE_AFTER_MS + 1, busy = false))
    }

    @Test fun replanningDoesNotRepeatWhileTheCooldownHolds() {
        val now = start + REROUTE_AFTER_MS + 1
        assertFalse(shouldReroute(true, start, now - 1_000, now, busy = false))
        assertTrue(
            shouldReroute(true, start, now - REROUTE_COOLDOWN_MS - 1, now, busy = false),
        )
    }

    /** A road the map does not have would otherwise replan on every fix. */
    @Test fun aRouteAlreadyBeingPlannedIsLeftAlone() {
        assertFalse(shouldReroute(true, start, 0L, start + 60_000, busy = true))
    }

    @Test fun aReplanContinuesTheJourneyRatherThanRestartingIt() {
        val cafe = Stop("Cafe", GeoPoint(35.0, -88.0))
        val park = Stop("Park", GeoPoint(35.1, -88.0))
        val home = Stop("Home", GeoPoint(35.2, -88.0))

        val left = remainingStops(listOf(cafe, park, home), setOf(cafe))

        assertEquals(listOf(park, home), left)
    }

    /** Past every stop, the destination is still where you are going. */
    @Test fun aChainWithEverythingVisitedKeepsTheDestination() {
        val cafe = Stop("Cafe", GeoPoint(35.0, -88.0))
        val home = Stop("Home", GeoPoint(35.2, -88.0))

        val left = remainingStops(listOf(cafe, home), setOf(cafe, home))

        assertEquals(listOf(home), left)
    }
}
