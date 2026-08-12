package com.steele.looky.location

import com.steele.looky.model.LookyMode
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.time.LocalDate

/**
 * Start, stop, start again -- and alternate Drive with Trail.
 *
 * Every bug this app has had around recording came from a session label that
 * outlived its journey: a drive that stayed a drive after it ended, a walk
 * appended to the drive's day file, a tab greyed out by a session nobody was
 * on. So the whole cycle is walked here rather than one transition at a time.
 */
class SessionCyclingTest {
    private val day = LocalDate.of(2026, 8, 12)

    /** Replays actions through the pure planner, carrying the mode forward. */
    private fun walk(vararg actions: String?): List<String> {
        var activeMode = LookyMode.DRIVE
        val recording = mutableListOf<String>()
        for (action in actions) {
            when (val plan = TraceService.plan(action, continuousRecording = true, activeMode = activeMode)) {
                is SessionPlan.Record -> {
                    activeMode = plan.mode
                    recording += plan.session
                }
                is SessionPlan.Stop -> recording += "stopped"
            }
        }
        return recording
    }

    @Test fun aDriveEndsBackInTheBackgroundLog() {
        assertEquals(
            listOf(TraceRecorder.SESSION_DRIVE, TraceRecorder.SESSION_BACKGROUND),
            walk(TraceService.ACTION_DRIVE, TraceService.ACTION_END_SESSION),
        )
    }

    @Test fun driveAndTrailAlternateWithoutLeakingIntoEachOther() {
        val sessions = walk(
            TraceService.ACTION_DRIVE,
            TraceService.ACTION_END_SESSION,
            TraceService.ACTION_TRAIL,
            TraceService.ACTION_END_SESSION,
            TraceService.ACTION_DRIVE,
            TraceService.ACTION_END_SESSION,
            TraceService.ACTION_TRAIL,
            TraceService.ACTION_END_SESSION,
        )

        assertEquals(
            listOf(
                TraceRecorder.SESSION_DRIVE, TraceRecorder.SESSION_BACKGROUND,
                TraceRecorder.SESSION_TRAIL, TraceRecorder.SESSION_BACKGROUND,
                TraceRecorder.SESSION_DRIVE, TraceRecorder.SESSION_BACKGROUND,
                TraceRecorder.SESSION_TRAIL, TraceRecorder.SESSION_BACKGROUND,
            ),
            sessions,
        )
    }

    @Test fun startingTheOtherModeMidJourneyTakesOverRatherThanBlending() {
        // No End in between: pressing Start on the other tab is a takeover.
        assertEquals(
            listOf(
                TraceRecorder.SESSION_DRIVE,
                TraceRecorder.SESSION_TRAIL,
                TraceRecorder.SESSION_DRIVE,
            ),
            walk(TraceService.ACTION_DRIVE, TraceService.ACTION_TRAIL, TraceService.ACTION_DRIVE),
        )
    }

    @Test fun everySessionChangeInACycleWritesToItsOwnDayFile() {
        val files = walk(
            TraceService.ACTION_DRIVE,
            TraceService.ACTION_END_SESSION,
            TraceService.ACTION_TRAIL,
            TraceService.ACTION_END_SESSION,
        ).map { TraceRecorder.fileName(day, it) }

        assertEquals(
            listOf(
                "2026-08-12.drive.gpx",
                "2026-08-12.background.gpx",
                "2026-08-12.trail.gpx",
                "2026-08-12.background.gpx",
            ),
            files,
        )
        // A drive and a walk on the same day never share a file.
        assertEquals(3, files.toSet().size)
    }

    @Test fun theModeFollowsTheJourneyIntoTheBackgroundLog() {
        // Ending a trail leaves the classifier in walking mode, not driving.
        val plan = TraceService.plan(
            TraceService.ACTION_END_SESSION,
            continuousRecording = true,
            activeMode = LookyMode.TRAIL,
        )

        assertEquals(SessionPlan.Record(LookyMode.TRAIL, TraceRecorder.SESSION_BACKGROUND), plan)
    }

    @Test fun endingAJourneyStopsOutrightOnlyWhenRecordingIsSwitchedOff() {
        assertEquals(
            SessionPlan.Stop(forgetSetting = false),
            TraceService.plan(TraceService.ACTION_END_SESSION, continuousRecording = false, activeMode = LookyMode.DRIVE),
        )
    }

    @Test fun theSettingsSwitchStopsAndIsRemembered() {
        assertEquals(
            SessionPlan.Stop(forgetSetting = true),
            TraceService.plan(TraceService.ACTION_STOP, continuousRecording = true, activeMode = LookyMode.DRIVE),
        )
    }

    @Test fun aStickyRestartMidDriveComesBackAsBackgroundNotAsADrive() {
        val sessions = walk(TraceService.ACTION_DRIVE, null, null)

        assertEquals(
            listOf(TraceRecorder.SESSION_DRIVE, TraceRecorder.SESSION_BACKGROUND, TraceRecorder.SESSION_BACKGROUND),
            sessions,
        )
    }

    @Test fun aLongCycleNeverLeavesAJourneyRunningAtTheEnd() {
        val sessions = walk(
            TraceService.ACTION_DRIVE, TraceService.ACTION_END_SESSION,
            TraceService.ACTION_TRAIL, TraceService.ACTION_DRIVE, TraceService.ACTION_END_SESSION,
            TraceService.ACTION_BACKGROUND,
            TraceService.ACTION_TRAIL, TraceService.ACTION_END_SESSION,
        )

        assertEquals(TraceRecorder.SESSION_BACKGROUND, sessions.last())
        assertTrue(sessions.count { it == TraceRecorder.SESSION_DRIVE } == 2)
        assertTrue(sessions.count { it == TraceRecorder.SESSION_TRAIL } == 2)
    }
}
