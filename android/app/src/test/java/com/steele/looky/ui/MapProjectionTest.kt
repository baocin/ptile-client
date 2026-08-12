package com.steele.looky.ui

import androidx.compose.ui.geometry.Offset
import com.steele.looky.model.GeoPoint
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class MapProjectionTest {
    private val center = GeoPoint(35.73377, -88.03220)
    private val width = 1080f
    private val height = 1920f

    @Test fun unprojectInvertsProject() {
        val pan = Offset(-140f, 85f)
        val point = GeoPoint(35.74000, -88.02000)

        val screen = MapProjection.project(point, center, pan, width, height, scale = 2.5f)
        val back = MapProjection.unproject(screen, center, pan, width, height, scale = 2.5f)

        assertEquals(point.lat, back.lat, 1e-9)
        assertEquals(point.lon, back.lon, 1e-9)
    }

    @Test fun anUnpannedViewportIsCenteredOnTheAnchor() {
        val viewport = MapProjection.viewportCenter(center, Offset.Zero, width, height, scale = 1f)

        assertEquals(center.lat, viewport.lat, 1e-9)
        assertEquals(center.lon, viewport.lon, 1e-9)
    }

    @Test fun draggingRightMovesTheViewportWest() {
        // Dragging the map rightwards pulls western ground into view, so the
        // centre longitude must decrease.
        val viewport = MapProjection.viewportCenter(center, Offset(200f, 0f), width, height, scale = 1f)

        assertTrue("expected ${viewport.lon} < ${center.lon}", viewport.lon < center.lon)
        assertEquals(center.lat, viewport.lat, 1e-9)
    }

    @Test fun draggingDownMovesTheViewportNorth() {
        val viewport = MapProjection.viewportCenter(center, Offset(0f, 200f), width, height, scale = 1f)

        assertTrue("expected ${viewport.lat} > ${center.lat}", viewport.lat > center.lat)
    }

    @Test fun zoomingInNarrowsTheSpan() {
        assertEquals(MapProjection.BASE_SPAN_LAT, MapProjection.spanLat(1f), 1e-9)
        assertEquals(MapProjection.BASE_SPAN_LAT / 4, MapProjection.spanLat(4f), 1e-9)
    }

    @Test fun longitudeSpanWidensWithLatitude() {
        // A degree of longitude covers less ground further from the equator, so
        // the same screen width must span more degrees.
        val equator = MapProjection.spanLon(0.0, 1f, 1080f, 1080f)
        val alaska = MapProjection.spanLon(64.0, 1f, 1080f, 1080f)

        assertTrue("expected $alaska > $equator", alaska > equator)
    }

    @Test fun theCosineGuardKeepsPolarSpansFinite() {
        // cos(89 degrees) is near zero; without the 0.25 floor the span would
        // explode and the projection would divide by ~0.
        val polar = MapProjection.spanLon(89.0, 1f, 1080f, 1080f)

        assertEquals(MapProjection.BASE_SPAN_LAT / 0.25, polar, 1e-9)
    }

    @Test fun aSquareOnTheGroundIsSquareOnTheScreen() {
        // 1080x1742, the canvas the map actually gets. Before the aspect term
        // this was 1.6x too wide.
        val width = 1080f
        val height = 1742f
        val metresPerPixelY = MapProjection.spanLat(1f) * 111_320.0 / height
        val metresPerPixelX = MapProjection.spanLon(center.lat, 1f, width, height) *
            111_320.0 * kotlin.math.cos(Math.toRadians(center.lat)) / width

        assertEquals(metresPerPixelY, metresPerPixelX, metresPerPixelY * 0.01)
    }
}
