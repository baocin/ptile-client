package com.steele.looky.offline

import com.steele.looky.model.GeoPoint
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * A routing failure has to say which of its causes happened.
 *
 * The FFI reports the enum variant, so "offline route failed: Disconnected" is
 * what reached the screen -- one sentence covering "there is no road between
 * these places", "the road exists and we did not load it", and "you have not
 * downloaded the next state". Those need different things from the user, and
 * one of them needs nothing at all.
 */
class RouteFailureMessageTest {
    private val jackson = GeoPoint(35.6145, -88.8139)
    private val nashville = GeoPoint(36.1627, -86.7816)

    private fun ffi(variant: String) = IllegalStateException("Routing { message: \"$variant\" }")

    private fun message(variant: String, sameCoverage: Boolean = true, trail: Boolean = false) =
        PtilesRepository.explain(ffi(variant), jackson, nashville, trail, sameCoverage)

    @Test fun anUnsnappedStartTalksAboutWhereYouAre() {
        val said = message("StartNotSnapped")

        assertTrue(said, said.contains("where you are"))
        assertFalse(said, said.contains("StartNotSnapped"))
    }

    @Test fun anUnsnappedEndTalksAboutTheDestination() {
        val said = message("EndNotSnapped")

        assertTrue(said, said.contains("destination"))
        // The most common real reason, and the one that tells them what to do.
        assertTrue(said, said.contains("track") || said.contains("on foot"))
    }

    @Test fun onATrailAnUnsnappedEndTalksAboutTrails() {
        val said = message("EndNotSnapped", trail = true)

        assertTrue(said, said.contains("trail") || said.contains("path"))
    }

    /** The case a user can fix, and the one they must be told about. */
    @Test fun aDisconnectionOutsideCoverageAsksForTheMissingState() {
        val said = message("Disconnected", sameCoverage = false)

        assertTrue(said, said.contains("Download"))
    }

    /**
     * Inside one pack the same failure is far more likely to be our corridor
     * than a genuinely unreachable place, and the message should not claim
     * more certainty than that.
     */
    @Test fun aDisconnectionInsideCoverageSaysItIsProbablyTheData() {
        val said = message("Disconnected")

        assertTrue(said, said.contains("map data"))
        // Hedged on purpose: from inside the router this is indistinguishable
        // from a genuinely unreachable place, so it must not claim either.
        assertTrue(said, said.contains("usually"))
    }

    @Test fun anOversizedCorridorAsksForAStopPartway() {
        val said = PtilesRepository.explain(
            IllegalStateException("InvalidBounds: bounding box too large (812 cells)"),
            jackson, nashville, false,
        )

        assertTrue(said, said.contains("stop partway"))
    }

    @Test fun aMissingLayerSaysToDownloadTheArea() {
        val said = message("no roads layer is installed")

        assertTrue(said, said.contains("Offline maps"))
    }

    /** Splitting still has to recognise the failures it can act on. */
    @Test fun rephrasingDoesNotHideTheCauseFromTheSplitter() {
        val wrapped = RoutingProblem(
            message("Disconnected"), ffi("Disconnected"),
        )

        assertTrue(PtilesRepository.isSplittableFailure(wrapped))
    }

    @Test fun onlyAnUnsnappedEndpointClimbsTheSnapLadder() {
        assertTrue(PtilesRepository.isSnapFailure(ffi("EndNotSnapped")))
        assertTrue(PtilesRepository.isSnapFailure(ffi("StartNotSnapped")))
        assertFalse(PtilesRepository.isSnapFailure(ffi("Disconnected")))
        assertFalse(PtilesRepository.isSnapFailure(ffi("EmptyGraph")))
    }

    /** The ladder has to widen, and to start where core's own default is. */
    @Test fun theLadderStartsAtTheProfileDefaultAndWidens() {
        val ladder = PtilesRepository.SNAP_LADDER_M

        assertTrue(ladder.first() == 0.0)
        assertTrue(ladder.toString(), ladder.zipWithNext().all { (a, b) -> b > a })
    }
}
