package com.steele.looky.location

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import java.io.File
import java.time.LocalDate

class TraceRecorderSessionTest {
    @Test fun aDayFileNamesItsSession() {
        assertEquals(
            "2026-08-09.trail.gpx",
            TraceRecorder.fileName(LocalDate.of(2026, 8, 9), TraceRecorder.SESSION_TRAIL),
        )
    }

    @Test fun sessionAndDateRoundTripThroughTheFileName() {
        val file = File(TraceRecorder.fileName(LocalDate.of(2026, 8, 9), TraceRecorder.SESSION_DRIVE))

        assertEquals(TraceRecorder.SESSION_DRIVE, TraceRecorder.sessionOf(file))
        assertEquals(LocalDate.of(2026, 8, 9), TraceRecorder.dateOf(file))
    }

    @Test fun filesWrittenBeforeSessionsExistedReadBackAsBackground() {
        val legacy = File("2026-08-09.gpx")

        assertEquals(TraceRecorder.SESSION_BACKGROUND, TraceRecorder.sessionOf(legacy))
        assertEquals(LocalDate.of(2026, 8, 9), TraceRecorder.dateOf(legacy))
    }

    @Test fun anUndatedFileHasNoDateToPruneOn() {
        assertNull(TraceRecorder.dateOf(File("notes.gpx")))
    }
}
