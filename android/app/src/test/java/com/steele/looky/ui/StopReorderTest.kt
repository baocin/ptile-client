package com.steele.looky.ui

import com.steele.looky.model.GeoPoint
import org.junit.Assert.assertEquals
import org.junit.Test

class StopReorderTest {
    private fun stops(vararg labels: String) = labels.map { Stop(it, GeoPoint(35.0, -88.0)) }

    @Test fun draggingAStopDownShiftsTheOnesItPasses() {
        assertEquals(
            listOf("b", "c", "a"),
            stops("a", "b", "c").move(0, 2).map { it.label },
        )
    }

    @Test fun draggingAStopUpPutsItBeforeTheTarget() {
        assertEquals(
            listOf("c", "a", "b"),
            stops("a", "b", "c").move(2, 0).map { it.label },
        )
    }

    @Test fun theLastStopIsTheDestinationAfterAReorder() {
        val reordered = stops("a", "b", "c").move(1, 2)

        assertEquals("b", reordered.last().label)
    }

    @Test fun aDropOutsideTheListChangesNothing() {
        val original = stops("a", "b")

        assertEquals(original, original.move(0, 5))
        assertEquals(original, original.move(1, 1))
    }
}
