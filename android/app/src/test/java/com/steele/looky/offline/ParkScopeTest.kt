package com.steele.looky.offline

import com.steele.looky.model.GeoPoint
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The rule trail sort scopes businesses by: inside beats near, near is a
 * radius to the boundary, and park size never enters into it.
 */
class ParkScopeTest {
    private val here = GeoPoint(35.6145, -88.8139)

    /** A square ring of `sizeKm` centred `northKm` north of the origin. */
    private fun park(name: String, sizeKm: Double, northKm: Double): Pair<String, List<GeoPoint>> {
        val half = sizeKm / 2 / 111.32
        val lat = here.lat + northKm / 111.32
        val lon = here.lon
        return name to listOf(
            GeoPoint(lat - half, lon - half),
            GeoPoint(lat - half, lon + half),
            GeoPoint(lat + half, lon + half),
            GeoPoint(lat + half, lon - half),
            GeoPoint(lat - half, lon - half),
        )
    }

    @Test fun theParkYouStandInIsTheOnlyScope() {
        // A national park you are inside, and a town park a mile away. The
        // first has a centroid ten times further off and still wins.
        val scope = PtilesRepository.parkScope(
            listOf(park("Great Smoky Mountains", sizeKm = 60.0, northKm = 0.0), park("Town Green", 0.2, 1.6)),
            here,
        )

        assertEquals(listOf("Great Smoky Mountains"), scope.map { it.name })
        assertEquals(0.0, scope.first().distanceM, 0.0)
    }

    @Test fun aSmallParkNearbyOutranksABigOneFurtherOff() {
        val scope = PtilesRepository.parkScope(
            listOf(park("Cedars of Lebanon", 40.0, 30.0), park("Riverside", 0.3, 3.0)),
            here,
        )

        assertEquals("Riverside", scope.first().name)
    }

    @Test fun pastFifteenMilesAParkIsOutOfScopeWhateverItsSize() {
        val scope = PtilesRepository.parkScope(listOf(park("Distant State Park", 2.0, 40.0)), here)

        assertTrue(scope.isEmpty())
    }

    @Test fun theBoundaryIsWhatIsMeasuredNotTheCentre() {
        // Centre 30 km north, 40 km across: the near edge is 10 km away, so the
        // park is in scope though its centroid is well past the radius.
        val scope = PtilesRepository.parkScope(listOf(park("Big Ridge", 40.0, 30.0)), here)

        assertEquals(listOf("Big Ridge"), scope.map { it.name })
        assertTrue("edge distance, not centroid distance", scope.first().distanceM < 30_000.0)
    }

    @Test fun aFourVertexParkReadsFurtherAwayThanItsEdgeReallyIs() {
        // The measured ceiling: distance is to ring vertices, so this square's
        // nearest corner answers, not the midpoint of its south edge 10 km
        // away. A ring simplified this hard is the case where a park just
        // inside 15 miles falls out of scope.
        val scope = PtilesRepository.parkScope(listOf(park("Big Ridge", 40.0, 30.0)), here)

        assertTrue("corner, not edge", scope.first().distanceM > 15_000.0)
    }

    @Test fun aBusinessIsLabelledWithTheParkItStandsIn() {
        val scope = PtilesRepository.parkScope(listOf(park("Town Green", 1.0, 0.0)), here)
        val inside = GeoPoint(here.lat + 0.001, here.lon)
        val outside = GeoPoint(here.lat + 0.5, here.lon)

        assertEquals("Town Green", PtilesRepository.parkContaining(scope, inside))
        assertNull(PtilesRepository.parkContaining(scope, outside))
    }

    @Test fun trailSortPutsPathsFirstAndDropsBusinessesOutsideTheParks() {
        val scope = PtilesRepository.parkScope(listOf(park("Town Green", 1.0, 0.0)), here)
        val trails = listOf(
            PtilesRepository.BusinessResult("Cypress Loop", here, score = 1),
            PtilesRepository.BusinessResult("Ridge Trail", here.copy(lat = here.lat + 0.002), score = 0),
        )
        val businesses = listOf(
            PtilesRepository.BusinessResult("Park Cafe", GeoPoint(here.lat + 0.001, here.lon), score = 0),
            PtilesRepository.BusinessResult("Truck Stop", GeoPoint(here.lat + 0.5, here.lon), score = 0),
        )

        val ranked = PtilesRepository.rankTrailFirst(trails, businesses, scope, limit = 10)

        assertEquals(listOf("Cypress Loop", "Ridge Trail", "Park Cafe"), ranked.map { it.name })
        assertEquals("in Town Green", ranked.last().note)
    }

    @Test fun aRingIsFarWhenItHasNoVertices() {
        assertEquals(Double.MAX_VALUE, PtilesRepository.ringDistanceM(emptyList(), here), 0.0)
    }
}
