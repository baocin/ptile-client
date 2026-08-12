package com.steele.looky.offline

import com.steele.looky.model.GeoPoint
import com.steele.looky.model.MapFeature
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class RouteAndSearchTest {
    private val start = GeoPoint(35.0, -88.0)
    private val middle = GeoPoint(35.5, -88.5)
    private val other = GeoPoint(36.0, -89.0)
    private val end = GeoPoint(36.5, -89.5)

    @Test fun noWaypointsIsASingleLeg() {
        assertEquals(
            listOf(start to end),
            PtilesRepository.routeLegs(start, emptyList(), end),
        )
    }

    @Test fun waypointsBecomeConsecutiveLegsInOrder() {
        assertEquals(
            listOf(start to middle, middle to other, other to end),
            PtilesRepository.routeLegs(start, listOf(middle, other), end),
        )
    }

    @Test fun joiningLegsDropsTheSharedJointPoint() {
        val first = PtilesRepository.RouteResult(listOf(start, middle), 100.0, 60.0, 3)
        val second = PtilesRepository.RouteResult(listOf(middle, end), 200.0, 90.0, 4)

        val joined = PtilesRepository.joinLegs(listOf(first, second))

        assertEquals(listOf(start, middle, end), joined.points)
        assertEquals(300.0, joined.distanceM, 0.0)
        assertEquals(150.0, joined.durationS, 0.0)
        assertEquals(7, joined.decodedSegments)
    }

    @Test fun asingleLegJoinsToItself() {
        val only = PtilesRepository.RouteResult(listOf(start, end), 100.0, 60.0, 3)

        assertEquals(only.points, PtilesRepository.joinLegs(listOf(only)).points)
    }

    @Test fun exactMatchesOutrankPrefixAndSubstringHits() {
        val hits = listOf(
            PtilesRepository.BusinessResult("Waffle House 12", GeoPoint(35.1, -88.1), score = 0),
            PtilesRepository.BusinessResult("Waffle House", GeoPoint(35.2, -88.2), score = 2),
            PtilesRepository.BusinessResult("Waffle Houses", GeoPoint(35.3, -88.3), score = 1),
        )

        val merged = PtilesRepository.mergeBusinessHits(hits, limit = 10)

        assertEquals(listOf(2, 1, 0), merged.map { it.score })
    }

    @Test fun theSameStoreFoundInTwoStateIndexesAppearsOnce() {
        // Bordering states' bounding boxes overlap, so one store can sit in
        // both name indexes.
        val hits = listOf(
            PtilesRepository.BusinessResult("Border Cafe", GeoPoint(36.500001, -89.500002), score = 2),
            PtilesRepository.BusinessResult("Border Cafe", GeoPoint(36.500001, -89.500002), score = 2),
        )

        assertEquals(1, PtilesRepository.mergeBusinessHits(hits, limit = 10).size)
    }

    @Test fun twoStoresWithTheSameNameAtDifferentPlacesBothSurvive() {
        val hits = listOf(
            PtilesRepository.BusinessResult("Waffle House", GeoPoint(35.2, -88.2), score = 2),
            PtilesRepository.BusinessResult("Waffle House", GeoPoint(36.4, -87.1), score = 2),
        )

        assertEquals(2, PtilesRepository.mergeBusinessHits(hits, limit = 10).size)
    }

    @Test fun theLimitCapsTheMergedList() {
        val hits = (1..30).map {
            PtilesRepository.BusinessResult("Store $it", GeoPoint(35.0 + it / 100.0, -88.0), score = 1)
        }

        assertEquals(5, PtilesRepository.mergeBusinessHits(hits, limit = 5).size)
    }

    @Test fun theBuildingSampleGridWidensWithTheSpreadButStaysBounded() {
        assertEquals(3, PtilesRepository.buildingSampleSpan(1))
        assertEquals(4, PtilesRepository.buildingSampleSpan(2))
        assertTrue(PtilesRepository.buildingSampleSpan(50) <= 6)
    }

    @Test fun theRingStaysAtTheOnlyValueTheFfiAccepts() {
        // ffi/src/lib.rs::validate_ring rejects anything above 1, and the
        // runCatching blocks in featuresAround turn that error into an empty
        // map rather than a crash. Raising this silently blanks the map.
        assertEquals(1u.toUByte(), PtilesRepository.RING)
    }

    @Test fun oneSampleCentreWhenThereIsNoSpread() {
        assertEquals(
            listOf(GeoPoint(35.0, -88.0)),
            PtilesRepository.sampleCenters(35.0, -88.0, 0),
        )
    }

    @Test fun spreadAddsFourArmsPerStepAroundTheCentre() {
        val centers = PtilesRepository.sampleCenters(35.0, -88.0, 1)

        assertEquals(5, centers.size)
        assertEquals(GeoPoint(35.0, -88.0), centers.first())
        assertTrue(centers.any { it.lat > 35.0 && it.lon == -88.0 })
        assertTrue(centers.any { it.lat < 35.0 && it.lon == -88.0 })
        assertTrue(centers.any { it.lon > -88.0 && it.lat == 35.0 })
        assertTrue(centers.any { it.lon < -88.0 && it.lat == 35.0 })
    }

    @Test fun sampleStepsStayInsideOneCellSoNoGapOpensBetweenRings() {
        // A res-7 cell is ~1.4 km across and a ring-1 query reaches ~2 km, so
        // steps of this size overlap rather than leaving blank paper between.
        assertTrue(PtilesRepository.SAMPLE_STEP_LAT * 111_320 < 4_000)
        assertTrue(PtilesRepository.SAMPLE_STEP_LON * 91_000 < 4_000)
    }

    @Test fun aFeatureReturnedByTwoOverlappingCentresIsDrawnOnce() {
        val road = MapFeature(listOf(GeoPoint(35.0, -88.0), GeoPoint(35.1, -88.1)), "primary", "Broadway")

        assertEquals(1, PtilesRepository.dedupeFeatures(listOf(road, road.copy())).size)
    }

    @Test fun twoDistinctRoadsBothSurviveDeduplication() {
        val first = MapFeature(listOf(GeoPoint(35.0, -88.0), GeoPoint(35.1, -88.1)), "primary", "Broadway")
        val second = MapFeature(listOf(GeoPoint(36.0, -87.0), GeoPoint(36.1, -87.1)), "primary", "Church St")

        assertEquals(2, PtilesRepository.dedupeFeatures(listOf(first, second)).size)
    }

    @Test fun hitsSortNearestFirstWhenAnOriginIsKnown() {
        val here = GeoPoint(35.0, -88.0)
        val hits = listOf(
            PtilesRepository.BusinessResult("Far exact", GeoPoint(35.5, -88.0), score = 2),
            PtilesRepository.BusinessResult("Near substring", GeoPoint(35.001, -88.0), score = 0),
        )

        val merged = PtilesRepository.mergeBusinessHits(hits, limit = 10, origin = here)

        assertEquals(listOf("Near substring", "Far exact"), merged.map { it.name })
    }

    @Test fun withNoOriginTheMatchQualityStillDecides() {
        val hits = listOf(
            PtilesRepository.BusinessResult("Near substring", GeoPoint(35.001, -88.0), score = 0),
            PtilesRepository.BusinessResult("Far exact", GeoPoint(35.5, -88.0), score = 2),
        )

        assertEquals("Far exact", PtilesRepository.mergeBusinessHits(hits, limit = 10).first().name)
    }

    @Test fun flightNumbersFromTheAerowayImportAreNotBusinesses() {
        assertTrue(PtilesRepository.isFlightNode("AA 1234"))
        assertTrue(PtilesRepository.isFlightNode("DL2201"))
        assertFalse(PtilesRepository.isFlightNode("Gate A12 Cafe"))
        assertFalse(PtilesRepository.isFlightNode("Waffle House"))
    }

    @Test fun flightNodesAreDroppedFromMergedHits() {
        val hits = listOf(
            PtilesRepository.BusinessResult("AA 1234", GeoPoint(35.0, -88.0), score = 2),
            PtilesRepository.BusinessResult("Airport Diner", GeoPoint(35.0, -88.0), score = 1),
        )

        assertEquals(listOf("Airport Diner"), PtilesRepository.mergeBusinessHits(hits, limit = 10).map { it.name })
    }

    @Test fun bothCorridorFailuresAreWorthSplittingFor() {
        assertTrue(
            PtilesRepository.isSplittableFailure(
                IllegalStateException("bad bounding box: bounding box (35, -88)..(36, -87) is too large: covers more than 512 H3 res-7 cells")
            )
        )
        // Measured on the TN pack: Savannah to Camden fails this way whole and
        // routes as two halves.
        assertTrue(PtilesRepository.isSplittableFailure(IllegalStateException("offline route failed: Disconnected")))
    }

    @Test fun failuresASmallerCorridorCannotFixAreNotSplit() {
        assertFalse(PtilesRepository.isSplittableFailure(IllegalStateException("no roads layer is installed")))
        assertFalse(PtilesRepository.isSplittableFailure(IllegalStateException("offline route failed: StartNotSnapped")))
        assertFalse(PtilesRepository.isSplittableFailure(IllegalStateException("offline route failed: EmptyGraph")))
    }

    @Test fun aWrappedCorridorErrorIsStillRecognised() {
        val wrapped = RuntimeException("route failed", IllegalStateException("bad bounding box: too large, 512 cells"))

        assertTrue(PtilesRepository.isSplittableFailure(wrapped))
    }

    @Test fun theStreetGridOutranksTheFootwaysBesideIt() {
        // A town has a footway per street; ranked above roads they filled the
        // draw cap and deleted the grid.
        assertTrue(PtilesRepository.featureRank("residential") < PtilesRepository.featureRank("trail:footway"))
        assertTrue(PtilesRepository.featureRank("motorway") < PtilesRepository.featureRank("residential"))
        assertTrue(PtilesRepository.featureRank("trail:path") < PtilesRepository.featureRank("building_area"))
    }

    @Test fun theCapKeepsRoadsWhenFootwaysOutnumberThem() {
        val streets = (1..10).map { feature("residential", "Street $it") }
        val footways = (1..100).map { feature("trail:footway", null) }

        val kept = PtilesRepository.capFeatures(footways + streets, max = 20)

        assertTrue(kept.count { it.kind == "residential" } >= 8)
    }

    @Test fun aTownFullOfRoadsStillLeavesRoomForTrails() {
        // The bug this exists for: a city viewport decoded thousands of roads,
        // and a single global ranking evicted every trail before it drew.
        val roads = (1..3_000).map { feature("residential", "Street $it") }
        val trails = (1..40).map { feature("trail:path", "Path $it") }

        val kept = PtilesRepository.capFeatures(roads + trails, max = 1_000)

        assertTrue("trails must survive a road-dense viewport", kept.any { it.kind == "trail:path" })
        assertTrue(kept.count { it.kind == "residential" } > 300)
        assertEquals(1_000, kept.size)
    }

    @Test fun unusedBudgetGoesBackToWhoeverCanUseIt() {
        // Only roads present: they should take the whole budget, not 40% of it.
        val roads = (1..500).map { feature("residential", null) }

        assertEquals(100, PtilesRepository.capFeatures(roads, max = 100).size)
    }

    private fun feature(kind: String, name: String?) =
        com.steele.looky.model.MapFeature(listOf(GeoPoint(35.0, -88.0), GeoPoint(35.1, -88.0)), kind, name)

    @Test fun theDrawCapHoldsMoreThanItUsedTo() {
        // 3,000 features is ~60k vertices, well inside the frame budget that
        // the uncapped 34k-path case blew through.
        assertEquals(3_000, PtilesRepository.MAX_DRAWN_FEATURES)
    }

    @Test fun pavementAndParkingAislesAreTheClassesWorthSkipping() {
        assertTrue("footway" in PtilesRepository.MINOR_ROAD_CLASSES)
        assertTrue("service" in PtilesRepository.MINOR_ROAD_CLASSES)
        assertFalse("residential" in PtilesRepository.MINOR_ROAD_CLASSES)
        assertFalse("motorway" in PtilesRepository.MINOR_ROAD_CLASSES)
    }

    @Test fun theNewestAdminPackWins() {
        val files = listOf(
            java.io.File("/packs/US.admin.ptiles"),
            java.io.File("/packs/US.admin_v2.ptiles"),
            java.io.File("/packs/TN.roads_v2.ptiles"),
        )

        assertEquals("US.admin_v2.ptiles", PtilesRepository.newestAdminPack(files)?.name)
    }

    @Test fun anUnversionedAdminPackIsStillUsableOnItsOwn() {
        val files = listOf(java.io.File("/packs/US.admin.ptiles"))

        assertEquals("US.admin.ptiles", PtilesRepository.newestAdminPack(files)?.name)
        assertEquals(null, PtilesRepository.newestAdminPack(listOf(java.io.File("/packs/TN.roads_v2.ptiles"))))
    }
}
