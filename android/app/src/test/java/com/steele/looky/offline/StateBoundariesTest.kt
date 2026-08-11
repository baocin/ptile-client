package com.steele.looky.offline

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Test

/**
 * The baked boundaries need a device to read the asset, so these pin the
 * failures they exist to fix: the bbox table answers the wrong state in every
 * river city, because the boxes overlap and the tie-break is a box centre
 * rather than a border.
 */
class StateBoundariesTest {
    @Test fun theBboxTableMisplacesMemphisIntoArkansas() {
        assertEquals("AR", StateResolver.stateAt(35.1495, -90.0490))
        assertNotEquals("TN", StateResolver.stateAt(35.1495, -90.0490))
    }

    @Test fun theBboxTableMisplacesCincinnatiIntoKentucky() {
        assertEquals("KY", StateResolver.stateAt(39.1031, -84.5120))
    }

    @Test fun theBboxTableMisplacesPortlandIntoWashington() {
        assertEquals("WA", StateResolver.stateAt(45.5150, -122.6780))
    }

    @Test fun wellInsideAStateEvenTheBoxesAgree() {
        assertEquals("TN", StateResolver.stateAt(35.9, -86.8))
        assertEquals("CO", StateResolver.stateAt(39.0, -105.5))
    }

    @Test fun theHintBreaksTiesWhenSeveralBoxesClaimThePoint() {
        assertEquals("TN", StateResolver.stateAt(35.1495, -90.0490, preferred = "TN"))
        assertEquals("AR", StateResolver.stateAt(35.1495, -90.0490, preferred = "AR"))
    }
}
