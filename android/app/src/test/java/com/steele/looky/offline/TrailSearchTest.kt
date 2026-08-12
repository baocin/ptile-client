package com.steele.looky.offline

import com.steele.looky.model.GeoPoint
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Trail names on disk are the primary OSM `name` and nothing else -- no
 * `alt_name`, no `ref` (`scripts/build_trails.py` stores `tags["name"]`, and
 * the flags byte defines only "name present"). So the only forgiveness a trail
 * search can offer is spelling, and it is the same scoring businesses get.
 */
class TrailSearchTest {
    private val here = GeoPoint(35.0, -88.0)
    private fun at(name: String, km: Double) =
        PtilesRepository.BusinessResult(name, GeoPoint(35.0 + km / 111.32, -88.0), score = 0)

    @Test fun aMisspeltTrailStillMatches() {
        assertTrue(
            PtilesRepository.nameSimilarity("greenway", "Stones River Greenwy") >=
                PtilesRepository.MIN_NAME_SIMILARITY
        )
    }

    @Test fun theNearerOfTwoEqualTrailsComesFirst() {
        val ranked = PtilesRepository.rankByNameAndDistance(
            query = "cumberland",
            hits = listOf(at("Cumberland Trail", 30.0), at("Cumberland Trail", 1.0)),
            origin = here,
            limit = 10,
        )

        assertTrue(ranked.first().point.lat < 35.1)
    }

    @Test fun anUnrelatedTrailIsDroppedRatherThanListed() {
        val ranked = PtilesRepository.rankByNameAndDistance(
            query = "cumberland",
            hits = listOf(at("Cumberland Trail", 5.0), at("Chirt Pit Road", 0.1)),
            origin = here,
            limit = 10,
        )

        assertEquals(listOf("Cumberland Trail"), ranked.map { it.name })
    }
}
