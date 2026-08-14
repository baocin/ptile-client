package com.steele.looky.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File
import java.time.Instant
import java.time.LocalDate
import java.time.ZoneId

/** Grouping a flat segment list into today plus an archive of whole days. */
class RecordingsListTest {
    private val zone = ZoneId.of("UTC")
    private val today = LocalDate.of(2026, 8, 14)

    private fun segment(
        day: String,
        session: String = "background",
        movement: String = "Driving",
        clock: String = "08:00",
        points: Int = 3,
        distanceM: Double = 1_000.0,
    ): TraceSegment {
        val at = Instant.parse("${day}T$clock:00Z")
        return TraceSegment(
            movement = movement,
            points = List(points) { com.steele.looky.model.GeoPoint(35.0, -88.0) },
            times = List(points) { at.plusSeconds(it * 60L) },
            sensors = List(points) { null },
            firstFix = at,
            lastFix = at.plusSeconds((points - 1) * 60L),
            distanceM = distanceM,
            file = File("/traces/$day.$session.gpx"),
        )
    }

    @Test fun todayKeepsItsStretchesAndOlderDaysCollapse() {
        val split = recordingsOf(
            listOf(
                segment("2026-08-14", movement = "Walking", clock = "09:00"),
                segment("2026-08-14", movement = "Driving", clock = "07:00"),
                segment("2026-08-13", clock = "10:00"),
                segment("2026-08-13", movement = "Walking", clock = "12:00"),
                segment("2026-08-11", clock = "06:00"),
            ),
            today,
            zone,
        )

        assertEquals(2, split.today.size)
        assertEquals(listOf(LocalDate.of(2026, 8, 13), LocalDate.of(2026, 8, 11)), split.archive.map { it.date })
        assertEquals(2, split.archive.first().segments.size)
    }

    @Test fun todayIsNewestStretchFirstAndAnArchivedDayReadsForwards() {
        val split = recordingsOf(
            listOf(
                segment("2026-08-14", clock = "07:00"),
                segment("2026-08-14", movement = "Walking", clock = "09:00"),
                segment("2026-08-13", movement = "Walking", clock = "15:00"),
                segment("2026-08-13", clock = "08:00"),
            ),
            today,
            zone,
        )

        assertEquals("Walking", split.today.first().movement)
        assertEquals("Driving", split.archive.single().segments.first().movement)
    }

    /** A day is one row even when the recorder wrote it across three files. */
    @Test fun aDaysSessionsRollIntoOneRow() {
        val day = recordingsOf(
            listOf(
                segment("2026-08-13", session = "drive", clock = "08:00", distanceM = 12_000.0),
                segment("2026-08-13", session = "trail", movement = "Walking", clock = "13:00", distanceM = 3_000.0),
                segment("2026-08-13", session = "background", movement = "Stationary", clock = "18:00", points = 9, distanceM = 0.0),
            ),
            today,
            zone,
        ).archive.single()

        assertEquals(15_000.0, day.distanceM, 0.001)
        assertEquals(3, day.files.size)
        assertEquals(Instant.parse("2026-08-13T08:00:00Z"), day.firstFix)
        assertEquals(Instant.parse("2026-08-13T18:08:00Z"), day.lastFix)
        // Largest share first: the row's headline is what the day was mostly.
        assertEquals("Stationary" to 9, day.breakdown.first())
    }

    /** The file name is the recorder's own answer about which day it is. */
    @Test fun theFileNameDatesAStretchBeforeItsClockDoes() {
        // A fix just after local midnight UTC-side still belongs to the file's day.
        val late = segment("2026-08-13", clock = "23:50").copy(
            firstFix = Instant.parse("2026-08-14T00:10:00Z"),
        )

        assertEquals(LocalDate.of(2026, 8, 13), dayOf(late, zone))
    }

    @Test fun aStretchWithNoFileFallsBackToItsFirstFix() {
        val loose = segment("2026-08-13").copy(file = null)

        assertEquals(LocalDate.of(2026, 8, 13), dayOf(loose, zone))
    }

    @Test fun yesterdayIsNamedAndOlderDaysAreDated() {
        assertEquals("Today", dayLabel(today, today))
        assertEquals("Yesterday", dayLabel(today.minusDays(1), today))
        assertEquals("Tue 11 Aug", dayLabel(LocalDate.of(2026, 8, 11), today))
    }

    @Test fun aRefreshOnlyLandsAtTheTopOfAnUntouchedList() {
        assertTrue(refreshIsSafe(firstVisibleIndex = 0, firstVisibleOffset = 0))
        assertFalse(refreshIsSafe(firstVisibleIndex = 4, firstVisibleOffset = 0))
        // Part-scrolled off the first row still counts as scrolled.
        assertFalse(refreshIsSafe(firstVisibleIndex = 0, firstVisibleOffset = 30))
        assertFalse(refreshIsSafe(firstVisibleIndex = 0, firstVisibleOffset = 0, query = "kroger"))
    }

    /** A live re-read replaces one file's stretches and leaves the rest alone. */
    @Test fun rereadingTheOpenFileTouchesNoOtherDay() {
        val active = File("/traces/2026-08-14.drive.gpx")
        val existing = listOf(
            segment("2026-08-14", session = "drive", clock = "09:00"),
            segment("2026-08-13", clock = "10:00"),
        )
        val grown = listOf(segment("2026-08-14", session = "drive", clock = "09:00", points = 40))

        val merged = mergeReread(existing, grown, active)

        assertEquals(2, merged.size)
        assertEquals(40, merged.first().points.size)
        assertEquals(3, merged.last().points.size)
    }
}
