package com.steele.looky.offline

import com.steele.looky.model.GeoPoint
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class SearchRankingTest {
    private val here = GeoPoint(35.0, -88.0)
    private fun at(name: String, km: Double) =
        PtilesRepository.BusinessResult(name, GeoPoint(35.0 + km / 111.32, -88.0), score = 0)

    @Test fun anExactNameOutscoresAPartialOne() {
        assertTrue(
            PtilesRepository.nameSimilarity("waffle house", "Waffle House") >
                PtilesRepository.nameSimilarity("waffle house", "Waffle House Express")
        )
    }

    @Test fun aTypoStillMatchesTheNameItMeant() {
        assertTrue(
            "wafle huse should still find Waffle House",
            PtilesRepository.nameSimilarity("wafle huse", "Waffle House") >= PtilesRepository.MIN_NAME_SIMILARITY
        )
        assertTrue(PtilesRepository.nameSimilarity("krogr", "Kroger") >= PtilesRepository.MIN_NAME_SIMILARITY)
    }

    @Test fun anUnrelatedNameIsNotAFuzzyMatch() {
        assertTrue(PtilesRepository.nameSimilarity("waffle house", "Dollar General") < PtilesRepository.MIN_NAME_SIMILARITY)
    }

    @Test fun aCloseTypoBeatsAPerfectMatchAcrossTheState() {
        val ranked = PtilesRepository.rankByNameAndDistance(
            query = "waffle house",
            hits = listOf(at("Waffle House", 120.0), at("Waffle Housee", 0.5)),
            origin = here,
            limit = 10,
        )

        assertEquals("Waffle Housee", ranked.first().name)
    }

    @Test fun betweenEqualNamesTheNearerOneWins() {
        val ranked = PtilesRepository.rankByNameAndDistance(
            query = "kroger",
            hits = listOf(at("Kroger", 30.0), at("Kroger", 2.0)),
            origin = here,
            limit = 10,
        )

        assertTrue(ranked.isNotEmpty())
        assertTrue(ranked.first().point.lat < 35.1)
    }

    @Test fun aChainIsFoundByItsBrandNotOnlyItsSiteName() {
        val ranked = PtilesRepository.rankByNameAndDistance(
            query = "shell",
            hits = listOf(at("Shell Oil 41762", 1.0).copy(brand = "Shell"), at("Dollar General", 0.2)),
            origin = here,
            limit = 10,
        )

        assertEquals(listOf("Shell Oil 41762"), ranked.map { it.name })
    }

    @Test fun aBrandlessRecordIsScoredOnItsNameAlone() {
        assertEquals(
            PtilesRepository.nameSimilarity("shell", "Shell Oil 41762"),
            PtilesRepository.bestSimilarity("shell", "Shell Oil 41762", brand = null),
            0.0,
        )
    }

    @Test fun nonMatchesAreDroppedRatherThanRanked() {
        val ranked = PtilesRepository.rankByNameAndDistance(
            query = "kroger",
            hits = listOf(at("Kroger", 5.0), at("Shell", 0.1), at("AA 1234", 0.1)),
            origin = here,
            limit = 10,
        )

        assertEquals(listOf("Kroger"), ranked.map { it.name })
    }
}
