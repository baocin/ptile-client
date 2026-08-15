package com.steele.looky.offline

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * American place names are written both ways and searched for either. Folding
 * is query-side on purpose: the business name index's buckets were built with
 * `core::business_search::fold_name`'s rule and a pack already on a phone
 * cannot be re-bucketed, so what this changes is which returned rows count as
 * matches -- never which bucket is opened.
 */
class NameNormalizationTest {
    @Test fun punctuationAndCaseStopMattering() {
        assertEquals(
            PtilesRepository.normalizeName("St. Mary's Loop"),
            PtilesRepository.normalizeName("st marys loop"),
        )
        assertEquals(
            PtilesRepository.normalizeName("Smith & Sons"),
            PtilesRepository.normalizeName("Smith and Sons"),
        )
    }

    @Test fun theCommonAbbreviationsCollapseOntoOneSpelling() {
        val pairs = listOf(
            "Mount LeConte" to "Mt LeConte",
            "Fort Loudoun" to "Ft Loudoun",
            "Cedar Creek" to "Cedar Crk",
            "North Ridge Road" to "N Ridge Rd",
            "Saint Marys" to "St Marys",
        )
        pairs.forEach { (long, short) ->
            assertEquals(
                "$long and $short should fold alike",
                PtilesRepository.normalizeName(long),
                PtilesRepository.normalizeName(short),
            )
        }
    }

    @Test fun anAbbreviatedQueryIsAlsoAskedTheLongWay() {
        // Scoring only forgives spelling among rows that came back, and both
        // far-reaching paths match literally.
        assertEquals(listOf("St Marys", "saint Marys"), PtilesRepository.queryVariants("St Marys"))
        assertEquals(listOf("greenway"), PtilesRepository.queryVariants("greenway"))
        assertTrue(PtilesRepository.queryVariants("  ").isEmpty())
    }

    @Test fun aFoldedNameStillMatchesTheWayItWasTyped() {
        assertTrue(
            PtilesRepository.nameSimilarity("st marys", "St. Mary's Church") >=
                PtilesRepository.MIN_NAME_SIMILARITY
        )
        assertTrue(
            PtilesRepository.nameSimilarity("mount leconte", "Mt. LeConte Trail") >=
                PtilesRepository.MIN_NAME_SIMILARITY
        )
    }
}
