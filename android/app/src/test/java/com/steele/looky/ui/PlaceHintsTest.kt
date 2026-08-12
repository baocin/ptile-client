package com.steele.looky.ui

import com.steele.looky.model.GeoPoint
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class PlaceHintsTest {
    private val here = GeoPoint(35.0, -88.0)

    @Test fun bearingsReadAsTheCompassPointsTheyAre() {
        assertEquals("N", compassPoint(bearingDeg(here, GeoPoint(35.2, -88.0))))
        assertEquals("S", compassPoint(bearingDeg(here, GeoPoint(34.8, -88.0))))
        assertEquals("E", compassPoint(bearingDeg(here, GeoPoint(35.0, -87.8))))
        assertEquals("W", compassPoint(bearingDeg(here, GeoPoint(35.0, -88.2))))
    }

    @Test fun aDiagonalReadsAsTheDiagonal() {
        assertEquals("NE", compassPoint(bearingDeg(here, GeoPoint(35.15, -87.85))))
    }

    @Test fun aPlaceBesideTheRouteCountsAsOnIt() {
        val route = listOf(GeoPoint(35.0, -88.0), GeoPoint(35.05, -88.0), GeoPoint(35.1, -88.0))

        // ~200 m east of the middle of the line.
        assertTrue(nearRoute(route, GeoPoint(35.05, -87.9978), ON_ROUTE_M))
    }

    @Test fun aPlaceAKilometreOffTheRouteDoesNot() {
        val route = listOf(GeoPoint(35.0, -88.0), GeoPoint(35.05, -88.0))

        assertFalse(nearRoute(route, GeoPoint(35.05, -87.985), ON_ROUTE_M))
    }

    @Test fun withNoRoutePlannedNothingIsOnIt() {
        assertFalse(nearRoute(emptyList(), here, ON_ROUTE_M))
    }

    @Test fun caloriesCountOnlyWhatYouMovedYourselfThrough() {
        assertEquals(0, estimateCalories(50_000.0, "Driving"))
        assertEquals(0, estimateCalories(50_000.0, "Stationary"))
        assertEquals(55, estimateCalories(1_000.0, "Walking"))
        assertEquals(70, estimateCalories(1_000.0, "Running"))
    }

    @Test fun anUnclassifiedStretchBurnsNothingRatherThanGuessing() {
        assertEquals(0, estimateCalories(5_000.0, "Unknown"))
    }
}
