package com.steele.looky.ui

import com.steele.looky.model.GeoPoint
import java.time.Instant

/**
 * Getting a recording out of the app, whole or in part.
 *
 * A day file is written continuously and contains everything: the drive to the
 * trailhead, the walk, the hour in the car park, the drive home. What someone
 * wants to keep or send is usually a slice of that, so the trim happens before
 * the export rather than in whatever they open it with.
 *
 * The output is GPX 1.1 with nothing but the standard elements. `TraceRecorder`
 * writes a richer file -- accelerometer summaries, road context, battery -- and
 * those extensions are deliberately dropped here: an exported file is for other
 * software, and every consumer understands a track point.
 */
object GpxExport {
    private const val HEADER = """<?xml version="1.0" encoding="UTF-8"?>
<gpx version="1.1" creator="Looky" xmlns="http://www.topografix.com/GPX/1/1">
"""
    private const val FOOTER = "</gpx>\n"

    /**
     * The fixes recorded between two instants, as segments.
     *
     * Both ends are inclusive. Untimed fixes are kept when the segment they
     * belong to overlaps the window at all -- a file written before timestamps
     * existed should still export as itself rather than as nothing.
     */
    fun trim(segments: List<TraceSegment>, from: Instant, to: Instant): List<TraceSegment> {
        if (from > to) return trim(segments, to, from)
        return segments.mapNotNull { segment ->
            val kept = segment.points.indices.filter { index ->
                val at = segment.times.getOrNull(index)
                at == null || (at >= from && at <= to)
            }
            // A segment whose every fix is untimed would otherwise survive any
            // window, including one that lies entirely outside its own span.
            val timed = segment.times.filterNotNull()
            if (timed.isNotEmpty() && (timed.last() < from || timed.first() > to)) return@mapNotNull null
            if (kept.isEmpty()) return@mapNotNull null
            val points = kept.map { segment.points[it] }
            val times = kept.map { segment.times.getOrNull(it) }
            segment.copy(
                points = points,
                times = times,
                firstFix = times.firstOrNull { it != null },
                lastFix = times.lastOrNull { it != null },
                distanceM = GpxReader.pathLengthM(points),
            )
        }
    }

    /** Every timestamp in a recording, in order, for choosing a window. */
    fun timeline(segments: List<TraceSegment>): List<Instant> =
        segments.flatMap { it.times.filterNotNull() }.sorted()

    /** One `<trk>` per segment, named for how that stretch was travelled. */
    fun write(segments: List<TraceSegment>): String = buildString {
        append(HEADER)
        segments.forEach { segment ->
            if (segment.points.isEmpty()) return@forEach
            append("<trk><name>").append(xml(segment.movement)).append("</name><trkseg>\n")
            segment.points.forEachIndexed { index, point ->
                append("<trkpt lat=\"").append(point.lat).append("\" lon=\"").append(point.lon).append("\">")
                segment.times.getOrNull(index)?.let {
                    append("<time>").append(it).append("</time>")
                }
                append("</trkpt>\n")
            }
            append("</trkseg></trk>\n")
        }
        append(FOOTER)
    }

    /**
     * A filename that says what it is without being opened.
     *
     * `2026-08-13.drive.0812-0907.gpx` rather than `track.gpx`: an export lands
     * in a folder beside other exports, and the day alone does not distinguish
     * two slices of the same day.
     */
    fun fileName(source: String, from: Instant?, to: Instant?, whole: Boolean): String {
        val stem = source.removeSuffix(".gpx")
        if (whole || from == null || to == null) return "$stem.gpx"
        return "$stem.${clock(from)}-${clock(to)}.gpx"
    }

    private fun clock(at: Instant): String =
        java.time.format.DateTimeFormatter.ofPattern("HHmm")
            .withZone(java.time.ZoneId.systemDefault())
            .format(at)

    private fun xml(value: String): String = value
        .replace("&", "&amp;")
        .replace("<", "&lt;")
        .replace(">", "&gt;")
        .replace("\"", "&quot;")
        .replace("'", "&apos;")
}

/** Points and distance of a trimmed selection, for showing before exporting. */
data class TrimSummary(val points: Int, val distanceM: Double, val from: Instant?, val to: Instant?)

fun summarise(segments: List<TraceSegment>): TrimSummary {
    val points = segments.sumOf { it.points.size }
    val times = segments.flatMap { it.times.filterNotNull() }
    return TrimSummary(
        points = points,
        distanceM = segments.sumOf { it.distanceM },
        from = times.minOrNull(),
        to = times.maxOrNull(),
    )
}

/** Straight-line length of a path, exposed for callers that hold points only. */
internal fun pathLength(points: List<GeoPoint>): Double = GpxReader.pathLengthM(points)
