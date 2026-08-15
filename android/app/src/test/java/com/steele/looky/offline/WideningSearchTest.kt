package com.steele.looky.offline

import com.steele.looky.model.GeoPoint
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * A search that finds nothing and a search that looked nowhere read the same on
 * screen, and that was the whole failure: a trail twenty miles out was not
 * ranked low, it was absent. These cover the widening that replaced it -- how
 * far a search reaches before it stops, what it says it reached, and that a hit
 * further out is held rather than dropped.
 */
class WideningSearchTest {
    private val here = GeoPoint(35.6145, -88.8139) // Jackson, TN

    /** A hit `miles` due north, which flat and spherical agree on closely. */
    private fun at(name: String, miles: Double) = PtilesRepository.BusinessResult(
        name,
        GeoPoint(here.lat + miles * 1.609344 / 111.32, here.lon),
        score = 0,
    )

    @Test fun theNearestRungWithEnoughResultsWins() {
        val hits = (1..PtilesRepository.ENOUGH_RESULTS).map { at("Trail $it", 5.0) } +
            listOf(at("Far Trail", 300.0))
        val outcome = PtilesRepository.widenToEnough(hits, here)

        assertEquals(PtilesRepository.SEARCH_LADDER_M.first(), outcome.reachM, 1.0)
        assertEquals(PtilesRepository.ENOUGH_RESULTS, outcome.hits.size)
        // Not discarded -- counted, so the panel can offer to go and get it.
        assertEquals(1, outcome.beyondReach)
    }

    @Test fun tooFewNearMeansTheSearchWidensRatherThanStops() {
        val outcome = PtilesRepository.widenToEnough(listOf(at("Virgin Falls Trail", 196.0)), here)

        assertEquals(1, outcome.hits.size)
        assertTrue("reached ${outcome.reachM}", outcome.reachM >= 196 * 1_609.344)
        assertEquals(0, outcome.beyondReach)
    }

    @Test fun theLadderStopsAtAThousandMilesAndSaysWhatIsLeft() {
        val outcome = PtilesRepository.widenToEnough(listOf(at("Pacific Crest Trail", 1_500.0)), here)

        assertEquals(PtilesRepository.SEARCH_LADDER_M.last(), outcome.reachM, 1.0)
        assertTrue(outcome.hits.isEmpty())
        assertEquals(1, outcome.beyondReach)
    }

    @Test fun searchingFartherLiftsTheCeilingRatherThanRaisingIt() {
        val hits = listOf(at("Pacific Crest Trail", 1_500.0))
        val ceiling = PtilesRepository.SEARCH_LADDER_M.size
        val outcome = PtilesRepository.widenToEnough(hits, here, fromRung = ceiling)

        assertEquals(1, outcome.hits.size)
        assertEquals(0, outcome.beyondReach)
    }

    @Test fun nothingFoundStillReportsHowFarNothingWasTrueFor() {
        val outcome = PtilesRepository.widenToEnough(emptyList(), here)

        assertEquals(PtilesRepository.SEARCH_LADDER_M.last(), outcome.reachM, 1.0)
        assertEquals(0, outcome.beyondReach)
    }

    /**
     * The reach is printed to the user, so it is measured on a sphere. Flat
     * earth is what ranking uses and is several percent out at ladder scale.
     */
    @Test fun theReachIsMeasuredWellEnoughToPrint() {
        val thousandMiles = GeoPoint(here.lat + 1_000 * 1.609344 / 111.32, here.lon)
        val measured = PtilesRepository.distanceM(here, thousandMiles)

        assertEquals(1_609_344.0, measured, 20_000.0)
    }

    @Test fun aStrongMatchFarAwayRanksBelowAWeakerOneNearby() {
        val ranked = PtilesRepository.rankByNameAndDistance(
            query = "cumberland trail",
            hits = listOf(
                at("Cumberland Trail", 200.0),
                at("Cumberland River Trail", 2.0),
            ),
            origin = here,
            limit = 10,
        )

        assertEquals("Cumberland River Trail", ranked.first().name)
        // Below, not gone: an exhaustive search that hides its far hits is the
        // failure this replaced.
        assertEquals(2, ranked.size)
    }

    @Test fun theDistancePenaltyKeepsGrowingPastTheFalloff() {
        val near = PtilesRepository.distancePenalty(40.0)
        val far = PtilesRepository.distancePenalty(400.0)
        val further = PtilesRepository.distancePenalty(4_000.0)

        assertTrue("$near < $far", near < far)
        assertTrue("$far < $further", far < further)
    }
}
