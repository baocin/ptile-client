package com.steele.looky.offline

import com.steele.looky.model.GeoPoint
import com.steele.looky.model.MapFeature
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The draw budget has to be shared between sample cells, not won by whichever
 * cell holds the longest geometry. A starved cell is a blank H3 tile beside a
 * drawn one, which reads as a map that failed rather than a budget that was
 * spent elsewhere.
 */
class CellBudgetTest {
    /**
     * A road of [vertices] points, so length-based ranking has something to
     * sort on, named for the cell it belongs to.
     *
     * Named because two roads of equal length are otherwise value-equal, and a
     * test that asks "did this cell survive" by membership would then count a
     * neighbour's road as its own -- the same trap the cap itself avoids by
     * ranking indices rather than features.
     */
    private fun road(cell: String, vertices: Int, kind: String = "residential"): MapFeature =
        MapFeature(
            points = List(vertices) { GeoPoint(35.0 + it * 1e-4, -88.0) },
            kind = kind,
            name = cell,
        )

    private fun cell(name: String, count: Int, vertices: Int) =
        List(count) { road("$name#$it", vertices) }

    private fun from(kept: List<MapFeature>, name: String) =
        kept.count { it.name?.startsWith("$name#") == true }

    @Test fun aCellOfShortRoadsIsNotEvictedByACellOfLongOnes() {
        // One cell of very long geometry, eight of short: exactly the case that
        // left one tile drawn and its neighbours empty.
        val cells = listOf(cell("long", 2_000, 200)) + List(8) { cell("short$it", 500, 3) }

        val kept = PtilesRepository.capAcrossCells(cells, max = 3_000)

        assertTrue("the long cell drew nothing", from(kept, "long") > 0)
        repeat(8) { assertTrue("cell short$it drew nothing", from(kept, "short$it") > 0) }
    }

    @Test fun theBudgetIsRespected() {
        val kept = PtilesRepository.capAcrossCells(
            List(10) { cell("c$it", 1_000, 5) }, max = 3_000,
        )

        assertTrue(kept.size.toString(), kept.size <= 3_000)
    }

    @Test fun aCellThatCannotUseItsShareGivesItToTheDenseOnes() {
        // Nine cells of one feature each cannot use 300 apiece; the tenth can.
        val sparse = List(9) { cell("sparse$it", 1, 4) }
        val dense = cell("dense", 5_000, 4)

        val kept = PtilesRepository.capAcrossCells(sparse + listOf(dense), max = 3_000)

        repeat(9) { assertEquals(1, from(kept, "sparse$it")) }
        // The dense cell gets the rest rather than a flat tenth of the budget.
        assertTrue(from(kept, "dense").toString(), from(kept, "dense") > 2_000)
    }

    @Test fun everythingSurvivesWhenNothingIsOverBudget() {
        val cells = List(5) { cell("c$it", 10, 4) }

        assertEquals(50, PtilesRepository.capAcrossCells(cells, max = 3_000).size)
    }

    /** Footprints are drawn in two paths, so they never take strokes from roads. */
    @Test fun buildingsAreBudgetedApartFromTheRest() {
        val cells = List(4) { index ->
            cell("c$index", 500, 4) + List(500) { road("b$index#$it", 6, "building_area") }
        }

        val kept = PtilesRepository.capAcrossCells(cells, max = 1_000)

        assertEquals(2_000, kept.count { it.kind == "building_area" })
        assertTrue(kept.count { it.kind != "building_area" } <= 1_000)
    }
}
