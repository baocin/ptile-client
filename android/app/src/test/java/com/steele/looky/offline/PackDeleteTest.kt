package com.steele.looky.offline

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File

/**
 * `PackManager.delete` needs a Context for its directory, so these cover the
 * selection rule it delegates to. The rule is the region prefix: the whole
 * segment before the first dot.
 */
class PackDeleteTest {
    @get:Rule
    val temporary = TemporaryFolder()

    private fun write(name: String, bytes: Int): File =
        temporary.newFile(name).also { it.writeBytes(ByteArray(bytes)) }

    @Test fun aStatesLayersAndItsJsonSidecarAreSelectedTogether() {
        val roads = write("TN.roads_v2.ptiles", 10)
        val categories = write("TN.business_categories.json", 5)

        val selected = PackManager.regionFiles(temporary.root.listFiles().orEmpty(), "TN")

        assertTrue(selected.containsAll(listOf(roads, categories)))
        assertEquals(15L, selected.sumOf(File::length))
    }

    @Test fun nationalLayersAndOtherStatesAreNotSelected() {
        write("TN.roads_v2.ptiles", 10)
        val admin = write("US.admin.ptiles", 20)
        val california = write("CA.roads_v2.ptiles", 30)

        val selected = PackManager.regionFiles(temporary.root.listFiles().orEmpty(), "TN")

        assertEquals(1, selected.size)
        assertTrue(admin.exists())
        assertTrue(california.exists())
    }

    @Test fun aRegionCodeThatPrefixesAnotherIsNotSweptUp() {
        write("IN.roads_v2.ptiles", 10)
        write("INX.roads_v2.ptiles", 10)

        val selected = PackManager.regionFiles(temporary.root.listFiles().orEmpty(), "IN")

        assertEquals(listOf("IN.roads_v2.ptiles"), selected.map(File::getName))
    }

    @Test fun anAbsentRegionSelectsNothing() {
        write("TN.roads_v2.ptiles", 10)

        assertTrue(PackManager.regionFiles(temporary.root.listFiles().orEmpty(), "WY").isEmpty())
    }
}
