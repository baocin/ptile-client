package com.steele.looky.offline

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Every name here was read out of the shipped `TN.business.ptiles`, so the
 * rule is judged against what the layer actually holds rather than against
 * what an airport import is imagined to look like.
 */
class FlightNodeTest {
    private fun dropped(name: String) = PtilesRepository.isFlightNode(name)

    @Test fun bareDesignatorsAreFlights() {
        listOf("DL3208", "AA 1087", "DL4795", "UA 45").forEach {
            assertTrue(it, dropped(it))
        }
    }

    /** The common shape, and the one the anchored rule used to miss entirely. */
    @Test fun aDesignatorCarryingItsRouteIsStillAFlight() {
        listOf(
            "AA 1445 BNA-LAX",
            "DL 1656 - BNA to DTW",
            "UA6157 To DEN",
            "AA3908 BNA - ORD",
            "AA 2903 CHA/DFW",
            "AA 2999 (TYS > ORD)",
            "AA 3027 BNA>ORD",
            "AA 2926 CHA/DFW Seat 11A",
            "AA 1735 MEM to DFW Non-stop",
        ).forEach { assertTrue(it, dropped(it)) }
    }

    @Test fun aFlightSpelledOutIsOneToo() {
        assertTrue(dropped("Delta Flight 973 - MCI to ATL"))
        assertTrue(dropped("American Airlines Flight 1221"))
    }

    /**
     * Real places the old rule deleted. 174 of the 232 names it dropped in
     * Tennessee were of this kind.
     */
    @Test fun highwaysAndUnitNumbersAreNotFlights() {
        listOf(
            "HWY 54", "HWY 385", "HWY 45N", "US 51", "TN0106",
            "BAC 41", "BAC 45", "BAS 128", "AMB 210", "AMG 116",
            "ACT 1", "ASP 2011", "ABC24", "FOX 16", "OR 7", "PT2", "MW3", "KU4K",
        ).forEach { assertFalse(it, dropped(it)) }
    }

    @Test fun realBusinessesKeepingANumberSurvive() {
        listOf(
            "HWY 55 Burgers", "US 43 Drag Raceway", "ONE9 Travel Center",
            "FPC 731 Lexington", "HWY 191 Recycling & Auto Salvage",
            "VFW 4840 - Ray Pinner Post, Tipton County, TN",
        ).forEach { assertFalse(it, dropped(it)) }
    }

    /** Airside furniture: 204 in Tennessee, all of it inside an airport. */
    @Test fun gatesAndConcoursesAreNotDestinations() {
        listOf("Gate 5", "Gate 12", "Gate B7", "Gate C20", "gate a3", "Terminal 2", "Concourse C4")
            .forEach { assertTrue(it, dropped(it)) }
    }

    /** The trailing number is what separates a gate from a company. */
    @Test fun aBusinessNamedGateSomethingIsKept() {
        listOf("Gate Communications", "Gateway Tire", "Golden Gate Cafe")
            .forEach { assertFalse(it, dropped(it)) }
    }

    @Test fun aCarrierCodeInsideANameIsNotADesignator() {
        // The match is anchored, so a name merely containing one is safe.
        listOf("Delta Dental of Tennessee", "United Grocery Outlet", "Alaska Roadhouse")
            .forEach { assertFalse(it, dropped(it)) }
    }
}
