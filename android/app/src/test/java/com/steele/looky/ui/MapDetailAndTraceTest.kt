package com.steele.looky.ui

import androidx.compose.ui.geometry.Offset
import com.steele.looky.model.GeoPoint
import com.steele.looky.model.MapFeature
import com.steele.looky.offline.PtilesRepository
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.time.Instant

class MapDetailAndTraceTest {
    @Test fun theOpeningViewDrawsEverything() {
        // The map opens at 1.0; anything culled here is culled by default.
        assertTrue(MapDetail.draws("residential", isPoint = false, scale = 1.0f))
        assertTrue(MapDetail.draws("business:5", isPoint = true, scale = 1.0f))
        // Parking aisles and driveways are the exception, and wait with the
        // pavement: at 1.0 the view is 8 km tall and they are noise the fetch
        // would otherwise pay to carry.
        assertFalse(MapDetail.draws("service", isPoint = false, scale = 1.0f))
    }

    @Test fun zoomedOutKeepsThroughRoadsAndDropsTheRest() {
        val far = MapDetail.ARTERIAL_ONLY_BELOW - 0.1f

        assertTrue(MapDetail.draws("motorway", isPoint = false, scale = far))
        assertTrue(MapDetail.draws("water", isPoint = false, scale = far))
        assertFalse(MapDetail.draws("residential", isPoint = false, scale = far))
        assertFalse(MapDetail.draws("service", isPoint = false, scale = far))
    }

    @Test fun zoomedInDrawsEveryLine() {
        assertTrue(MapDetail.draws("residential", isPoint = false, scale = MapDetail.ARTERIAL_ONLY_BELOW + 0.1f))
        assertTrue(MapDetail.draws("service", isPoint = false, scale = MapDetail.FOOTWAYS_ABOVE))
    }

    @Test fun pointsWaitForACloserZoomThanLines() {
        assertFalse(MapDetail.draws("business:5", isPoint = true, scale = MapDetail.POINTS_ABOVE - 0.1f))
        assertTrue(MapDetail.draws("business:5", isPoint = true, scale = MapDetail.POINTS_ABOVE))
    }

    private val gpx = """
        <trk><name>Driving</name><trkseg>
        <trkpt lat="35.0" lon="-88.0"><time>2026-08-09T08:00:00Z</time></trkpt>
        <trkpt lat="35.1" lon="-88.0"><time>2026-08-09T08:05:00Z</time></trkpt>
        <trkpt lat="35.2" lon="-88.0"><time>2026-08-09T08:10:00Z</time></trkpt>
        </trkseg></trk>
        <trk><name>Walking</name><trkseg>
        <trkpt lat="35.3" lon="-88.0"><time>2026-08-09T08:20:00Z</time></trkpt>
        </trkseg></trk>
    """.trimIndent()

    @Test fun aTraceBreaksDownIntoFixesPerMovement() {
        val trace = GpxReader.parse(gpx)

        assertEquals(listOf("Driving" to 3, "Walking" to 1), trace.breakdown)
        assertEquals(4, trace.points.size)
    }

    @Test fun totalsMergeRepeatedLabelsAndRankThem() {
        val totals = GpxReader.totals(listOf("Walking" to 2, "Driving" to 5, "Walking" to 4))

        assertEquals(listOf("Walking" to 6, "Driving" to 5), totals)
    }

    @Test fun theFirstAndLastFixBoundTheDay() {
        val trace = GpxReader.parse(gpx)

        assertEquals(Instant.parse("2026-08-09T08:00:00Z"), trace.firstFix)
        assertEquals(Instant.parse("2026-08-09T08:20:00Z"), trace.lastFix)
    }

    @Test fun aFileWithNoTimesHasNoSpanToShow() {
        assertNull(formatSpan(null, null))
    }

    @Test fun eachTrackBecomesOneSegmentOfTheDay() {
        val segments = GpxReader.segments(gpx)

        assertEquals(listOf("Driving", "Walking"), segments.map { it.movement })
        assertEquals(3, segments[0].points.size)
        assertEquals(Instant.parse("2026-08-09T08:00:00Z"), segments[0].firstFix)
        assertEquals(Instant.parse("2026-08-09T08:10:00Z"), segments[0].lastFix)
    }

    @Test fun aSegmentCarriesTheDistanceItsOwnPointsCover() {
        val segments = GpxReader.segments(gpx)

        assertTrue(segments[0].distanceM > 20_000)
        assertEquals(0.0, segments[1].distanceM, 0.0)
    }

    @Test fun anEmptyTrackIsNotASegment() {
        val text = "<trk><name>Stationary</name><trkseg></trkseg></trk>"

        assertTrue(GpxReader.segments(text).isEmpty())
    }

    @Test fun pavementWaitsForACloseZoomSoTheStreetsShowThrough() {
        // A town's footways trace every street; at arm's length they hid the
        // roads underneath.
        assertFalse(MapDetail.draws("trail:footway", isPoint = false, scale = 1.0f))
        assertTrue(MapDetail.draws("trail:footway", isPoint = false, scale = MapDetail.FOOTWAYS_ABOVE))
        // A named path is not pavement and draws with the rest.
        assertTrue(MapDetail.draws("trail:path", isPoint = false, scale = 1.0f))
    }

    private val center = GeoPoint(35.6145, -88.8139)
    private val canvasWidth = 1080f
    private val canvasHeight = 1920f

    private fun fitProject(point: GeoPoint, fit: Pair<Float, Offset>) =
        MapProjection.project(point, center, fit.second, canvasWidth, canvasHeight, fit.first)

    @Test fun aFitPutsEveryPointOnScreen() {
        // A corridor wider than the opening view, well off the projection anchor.
        val corner = GeoPoint(35.90, -88.50)
        val points = listOf(GeoPoint(35.55, -88.95), center, corner)

        val fit = MapFit.solve(points, center, canvasWidth, canvasHeight)!!

        points.forEach {
            val at = fitProject(it, fit)
            assertTrue("$it landed at $at", at.x in 0f..canvasWidth && at.y in 0f..canvasHeight)
        }
    }

    @Test fun aFitLeavesAMarginRatherThanTouchingTheEdge() {
        val points = listOf(GeoPoint(35.55, -88.95), GeoPoint(35.90, -88.50))

        val fit = MapFit.solve(points, center, canvasWidth, canvasHeight, marginPx = 120f)

        points.forEach {
            val at = fitProject(it, fit!!)
            assertTrue(at.x >= 100f && at.x <= canvasWidth - 100f)
            assertTrue(at.y >= 100f && at.y <= canvasHeight - 100f)
        }
    }

    @Test fun aFitCentresASinglePointWithoutSolvingAZoom() {
        val only = GeoPoint(35.70, -88.70)

        val fit = MapFit.solve(listOf(only), center, canvasWidth, canvasHeight)!!

        assertEquals(MapProjection.MAX_SCALE, fit.first, 0f)
        val at = fitProject(only, fit)
        assertEquals(canvasWidth / 2f, at.x, 0.5f)
        assertEquals(canvasHeight / 2f, at.y, 0.5f)
    }

    @Test fun thereIsNothingToFitWithoutPointsOrACanvas() {
        assertNull(MapFit.solve(emptyList(), center, canvasWidth, canvasHeight))
        assertNull(MapFit.solve(listOf(center), center, 0f, 0f))
    }

    private val roadLine = MapFeature(
        (0..10).map { GeoPoint(35.6000, -88.8000 + it * 0.001) }, "residential", "Main St",
    )
    private val trailLine = MapFeature(
        (0..10).map { GeoPoint(35.6100, -88.8000 + it * 0.001) }, "trail:path", "Ridge Trail",
    )

    @Test fun aRouteIsSplitWhereItLeavesTheRoadForTheTrail() {
        // Drive east along the road, then walk the trail one hundred metres north.
        val route = (0..5).map { GeoPoint(35.6000, -88.8000 + it * 0.001) } +
            (5..10).map { GeoPoint(35.6100, -88.8000 + it * 0.001) }

        val parts = RouteModes.classify(route, listOf(roadLine, trailLine))

        assertEquals(listOf(RouteModes.Surface.ROAD, RouteModes.Surface.TRAIL), parts.map { it.first })
        // The joint vertex belongs to both parts, so the drawn line has no gap.
        assertEquals(parts[0].second.last(), parts[1].second.first())
    }

    @Test fun aStretchNearNoMappedWayIsNotGuessedAt() {
        val route = (0..4).map { GeoPoint(35.6000, -88.8000 + it * 0.001) } +
            (0..4).map { GeoPoint(35.7000 + it * 0.001, -88.8000) }

        val parts = RouteModes.classify(route, listOf(roadLine, trailLine))

        assertEquals(listOf(RouteModes.Surface.ROAD, RouteModes.Surface.UNKNOWN), parts.map { it.first })
    }

    @Test fun withNoDecodedWaysThereIsNothingToSplitOn() {
        val route = (0..5).map { GeoPoint(35.6000, -88.8000 + it * 0.001) }

        assertTrue(RouteModes.classify(route, emptyList()).isEmpty())
        assertTrue(RouteModes.classify(route, listOf(MapFeature(roadLine.points, "water"))).isEmpty())
    }

    @Test fun onlyLinesThatCanBeTravelledAreMatchedAgainst() {
        assertEquals(RouteModes.Surface.ROAD, RouteModes.surfaceOf(roadLine))
        assertEquals(RouteModes.Surface.TRAIL, RouteModes.surfaceOf(trailLine))
        assertNull(RouteModes.surfaceOf(MapFeature(roadLine.points, "water_area")))
        assertNull(RouteModes.surfaceOf(MapFeature(listOf(center), "trailhead")))
    }

    @Test fun theGroundIsPaintedBeforeWhatSitsOnIt() {
        assertTrue(MapDetail.layer("water_area") < MapDetail.layer("residential"))
        assertTrue(MapDetail.layer("building_area") < MapDetail.layer("motorway"))
        assertTrue(MapDetail.layer("residential") < MapDetail.layer("motorway"))
        assertTrue(MapDetail.layer("motorway") < MapDetail.layer("business:5"))
    }

    @Test fun jurisdictionLinesShowWhenTheStreetsAreGone() {
        // County lines go first as you zoom in, state lines linger.
        assertTrue(MapDetail.draws("admin_county", isPoint = false, scale = 0.7f))
        assertTrue(MapDetail.draws("admin_state", isPoint = false, scale = 1.2f))
        assertFalse(MapDetail.draws("admin_county", isPoint = false, scale = 1.2f))
        assertFalse(MapDetail.draws("admin_state", isPoint = false, scale = 2.0f))
    }

    @Test fun jurisdictionLinesArePaintedUnderEverything() {
        assertTrue(MapDetail.layer("admin_state") <= MapDetail.layer("water_area"))
        assertTrue(MapDetail.layer("admin_county") < MapDetail.layer("motorway"))
    }

    @Test fun theFetchWidensAsTheViewGrows() {
        assertTrue(MapDetail.fetchSpread(4f) < MapDetail.fetchSpread(1f))
        // Zoomed out far enough to frame a route, one net width leaves blank paper.
        assertTrue(MapDetail.fetchSpread(0.3f) >= 3)
        assertEquals(MapDetail.MAX_SPREAD, MapDetail.fetchSpread(MapProjection.MIN_SCALE))
    }

    @Test fun theFetchCoversTheViewportPlusTwoRingsAtEveryZoom() {
        // The whole point of deriving the spread from the viewport: whatever
        // the zoom, the fetched grid reaches past the visible edge by the
        // margin, so a pan lands on decoded ground.
        listOf(0.5f, 1f, 2f, 6f, 18f).forEach { scale ->
            val reach = MapDetail.fetchSpread(scale) * PtilesRepository.SAMPLE_STEP_LAT
            val needed = MapProjection.spanLat(scale) / 2 + MapDetail.MARGIN_RINGS * MapDetail.R7_STEP_LAT
            assertTrue("scale $scale reaches $reach, needs $needed", reach >= needed)
        }
    }

    @Test fun aFootprintTooSmallToSeeIsNotDrawn() {
        assertFalse(MapDetail.drawsFootprint(1f, 1.5f))
        assertTrue(MapDetail.drawsFootprint(1f, 12f))
        assertTrue(MapDetail.drawsFootprint(40f, 30f))
    }

    @Test fun footprintsArriveOnceTheyAreOneBatchNotOnePathEach() {
        assertTrue(MapDetail.draws("building_area", isPoint = false, scale = 1.6f))
        assertFalse(MapDetail.draws("building_area", isPoint = false, scale = 1.2f))
    }

    @Test fun pavementIsSkippedExactlyWhereItIsNotDrawn() {
        // Fetch and draw must agree, or the fetch pays for features the map
        // then hides: 15,000 of them at the opening zoom.
        listOf(0.5f, 1f, 1.5f, 2.9f, 3.5f, 8f).forEach { scale ->
            PtilesRepository.MINOR_ROAD_CLASSES.forEach { kind ->
                assertEquals(
                    "$kind at $scale",
                    MapDetail.skipsMinorRoads(scale),
                    !MapDetail.draws(kind, isPoint = false, scale = scale),
                )
            }
        }
    }
}
