package com.steele.looky.offline

import com.steele.looky.model.GeoPoint
import com.steele.looky.model.MapFeature
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
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

    @Test fun theRingStaysAtTheOnlyValueTheFfiAccepts() {
        // ffi/src/lib.rs::validate_ring rejects anything above 1, and the
        // runCatching blocks in featuresAround turn that error into an empty
        // map rather than a crash. Raising this silently blanks the map.
        assertEquals(1u.toUByte(), PtilesRepository.RING)
    }

    @Test fun oneSampleCentreWhenThereIsNoSpread() {
        assertEquals(1, PtilesRepository.sampleCenters(35.0, -88.0, 0).size)
    }

    @Test fun neighbouringViewportsShareTheirSampleCentres() {
        // The cache is keyed on the grid, so a pan that keeps most of the
        // screen must ask for most of the same centres. Unsnapped centres
        // moved with the viewport and nothing was ever asked for twice.
        val before = PtilesRepository.sampleCenters(35.0, -88.0, 2)
        val after = PtilesRepository.sampleCenters(35.004, -88.004, 2)

        assertEquals(before.toSet(), after.toSet())
        val stepped = PtilesRepository.sampleCenters(35.0 + PtilesRepository.SAMPLE_STEP_LAT, -88.0, 2)
        assertEquals(20, stepped.intersect(before.toSet()).size)
    }

    @Test fun theFetchLeadsThePan() {
        val previous = GeoPoint(35.0, -88.0)
        val now = GeoPoint(35.0 + PtilesRepository.SAMPLE_STEP_LAT, -88.0)
        val lead = PtilesRepository.leadCenters(previous, now, 1)

        // One row beyond the far edge in the direction of travel, and nothing
        // at all when the viewport has not crossed a centre.
        assertEquals(3, lead.size)
        assertTrue(lead.all { it.lat > now.lat })
        assertTrue(PtilesRepository.leadCenters(previous, previous, 1).isEmpty())
        assertTrue(PtilesRepository.leadCenters(null, now, 1).isEmpty())
    }

    @Test fun aCellIsCachedPerQuestionAsked() {
        // Developer mode adds rail, places add businesses: the same ground
        // decoded without them is not an answer to the question with them.
        assertNotEquals(
            PtilesRepository.cacheFlags(developer = false, places = false, skipMinorRoads = false),
            PtilesRepository.cacheFlags(developer = false, places = true, skipMinorRoads = false),
        )
        assertNotEquals(
            PtilesRepository.cacheFlags(developer = false, places = false, skipMinorRoads = true),
            PtilesRepository.cacheFlags(developer = false, places = false, skipMinorRoads = false),
        )
    }

    @Test fun spreadRingsTheCentreOnEverySideIncludingTheDiagonals() {
        val centers = PtilesRepository.sampleCenters(35.0, -88.0, 1)
        val centre = centers.first()

        assertEquals(9, centers.size)
        // Snapped to the grid, so the centre is the nearest node rather than
        // the coordinate itself -- within half a step of it.
        assertTrue(kotlin.math.abs(centre.lat - 35.0) <= PtilesRepository.SAMPLE_STEP_LAT / 2)
        assertTrue(kotlin.math.abs(centre.lon + 88.0) <= PtilesRepository.SAMPLE_STEP_LON / 2)
        assertTrue(centers.any { it.lat > centre.lat && it.lon == centre.lon })
        assertTrue(centers.any { it.lat < centre.lat && it.lon == centre.lon })
        assertTrue(centers.any { it.lon > centre.lon && it.lat == centre.lat })
        assertTrue(centers.any { it.lon < centre.lon && it.lat == centre.lat })
        // The corners are the whole point: on a plus they were 2.7 km from any
        // centre, and a ring reaches about 2 km.
        assertTrue(centers.any { it.lat > centre.lat && it.lon > centre.lon })
        assertTrue(centers.any { it.lat < centre.lat && it.lon < centre.lon })
        assertEquals(centers.size, centers.distinct().size)
    }

    @Test fun sampleStepsStayInsideOneCellSoNoGapOpensBetweenRings() {
        // A res-7 cell is ~1.4 km across and a ring-1 query reaches ~2 km, so
        // steps of this size overlap rather than leaving blank paper between.
        assertTrue(PtilesRepository.SAMPLE_STEP_LAT * 111_320 < 4_000)
        assertTrue(PtilesRepository.SAMPLE_STEP_LON * 91_000 < 4_000)
    }

    @Test fun footprintsAreNotRationedAgainstTheRoadNetwork() {
        // One town's worth of buildings against a city's worth of roads: the
        // roads must not evict the footprints, and the footprints must not
        // eat the road budget.
        val roads = (1..5_000).map {
            MapFeature(listOf(GeoPoint(35.0, -88.0), GeoPoint(35.1, -88.1)), "residential", "St $it")
        }
        val buildings = (1..5_000).map {
            MapFeature(
                listOf(GeoPoint(35.0, -88.0), GeoPoint(35.0, -88.001), GeoPoint(35.001, -88.001)),
                "building_area",
                null,
            )
        }

        val capped = PtilesRepository.capFeatures(roads + buildings)

        assertEquals(5_000, capped.count { it.kind == "building_area" })
        assertEquals(PtilesRepository.MAX_DRAWN_FEATURES, capped.count { it.kind != "building_area" })
    }

    @Test fun aFeatureBelongsToExactlyOneSampleCentre() {
        // Ring-1 patches overlap about three to one, so the same road comes
        // back from several centres. Ownership by first vertex is what keeps
        // it drawn once without a de-duplication pass over 60,000 features.
        val road = GeoPoint(35.0, -88.0)
        val centres = PtilesRepository.sampleCenters(road.lat, road.lon, 2)

        assertEquals(1, centres.count { PtilesRepository.owns(it, road) })
        // And a road a step away belongs to the neighbour, not to this centre.
        val away = GeoPoint(road.lat + PtilesRepository.SAMPLE_STEP_LAT, road.lon)
        assertEquals(1, centres.count { PtilesRepository.owns(it, away) })
        assertFalse(PtilesRepository.owns(centres.first(), away))
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

    /**
     * Flights and gates are gone before a hit reaches Kotlin: the rule lives in
     * `core::flight_nodes`, applied to every decoded block and to indexed
     * search, so the client no longer carries its own copy to drift from.
     * Covered by `core/src/flight_nodes.rs`.
     */

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

    @Test fun everyFetchCoversItsCornersAndNotJustACross() {
        // The viewport corner sits 2.7 km from the nearest arm of a plus, and
        // a ring reaches about 2 km, so the corners came back empty at every
        // zoom -- the map read as one tile surrounded by paper.
        val near = PtilesRepository.sampleCenters(35.0, -88.0, 1)
        val wide = PtilesRepository.sampleCenters(35.0, -88.0, 2)
        val centre = near.first()

        assertEquals(9, near.size)
        assertEquals(25, wide.size)
        assertTrue(
            "the diagonal must be sampled",
            near.any { it.lat > centre.lat && it.lon > centre.lon },
        )
    }

    @Test fun oneCentreIsStillOneCentre() {
        assertEquals(1, PtilesRepository.sampleCenters(35.0, -88.0, 0).size)
    }
}
