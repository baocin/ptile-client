package com.steele.looky.offline

import org.junit.Assert.assertEquals
import org.junit.Test

class StateResolverTest {
    @Test fun resolvesRepresentativeStatesOffline() {
        assertEquals("TN", StateResolver.stateAt(36.1627, -86.7816))
        assertEquals("AK", StateResolver.stateAt(61.2181, -149.9003))
        assertEquals("DC", StateResolver.stateAt(38.9072, -77.0369))
    }

    @Test fun keepsAPreferredStateInsideAnOverlappingBorderBox() {
        assertEquals("TN", StateResolver.stateAt(35.1495, -90.0490, preferred = "TN"))
    }

    @Test fun mapsAdminNamesBackToSnapshotCodes() {
        assertEquals("NY", StateResolver.codeForName("New York"))
        assertEquals("DC", StateResolver.codeForName("District of Columbia"))
    }
}
