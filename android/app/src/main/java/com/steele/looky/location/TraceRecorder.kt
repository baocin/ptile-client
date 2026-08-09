package com.steele.looky.location

import android.content.Context
import android.location.Location
import android.os.BatteryManager
import android.os.PowerManager
import com.steele.looky.offline.NearbyContext
import uniffi.ptiles_ffi.AccelStats
import java.io.File
import java.io.RandomAccessFile
import java.time.Instant
import java.time.LocalDate
import java.time.ZoneId

data class TraceAppendResult(val file: File, val pointsToday: Int)

class TraceRecorder(private val context: Context) : AutoCloseable {
    companion object {
        const val RETENTION_DAYS = 30L
        private const val HEADER = """<?xml version="1.0" encoding="UTF-8"?>
<gpx version="1.1" creator="Looky" xmlns="http://www.topografix.com/GPX/1/1" xmlns:rook="https://rookery.local/gpx/1" xmlns:gpxtpx="http://www.garmin.com/xmlschemas/TrackPointExtension/v1">
"""
        private const val FOOTER = "</gpx>\n"
    }

    private val dir = File(context.filesDir, "traces").apply { mkdirs() }
    private var file: File? = null
    private var raf: RandomAccessFile? = null
    private var date: LocalDate? = null
    private var openType: String? = null
    private var tailOffset = 0L
    private var contextXml = ""
    private var points = 0

    init { prune() }

    @Synchronized
    fun append(
        location: Location,
        movement: String,
        accel: AccelStats,
        nearby: NearbyContext,
    ): TraceAppendResult {
        val localDate = Instant.ofEpochMilli(location.time).atZone(ZoneId.systemDefault()).toLocalDate()
        if (date != localDate) openDay(localDate)
        val out = requireNotNull(raf)

        if (openType != movement) {
            if (openType != null) {
                out.seek(tailOffset)
                out.writeUtf8(contextXml + "</trkseg></trk>\n")
            } else {
                out.seek(tailOffset)
            }
            out.writeUtf8("<trk><name>${xml(movement)}</name><trkseg>\n")
            openType = movement
            contextXml = segmentContext(location, nearby)
        } else {
            out.seek(tailOffset)
        }

        out.writeUtf8(trackPoint(location, accel))
        tailOffset = out.filePointer
        out.writeUtf8(contextXml + "</trkseg></trk>\n" + FOOTER)
        out.setLength(out.filePointer)
        points++
        return TraceAppendResult(requireNotNull(file), points)
    }

    private fun openDay(day: LocalDate) {
        close()
        date = day
        file = File(dir, "$day.gpx")
        val out = RandomAccessFile(file, "rw")
        raf = out
        if (out.length() == 0L) {
            out.writeUtf8(HEADER + FOOTER)
            tailOffset = HEADER.toByteArray().size.toLong()
        } else {
            val bytes = ByteArray(out.length().coerceAtMost(Int.MAX_VALUE.toLong()).toInt())
            out.seek(0); out.readFully(bytes)
            val text = bytes.toString(Charsets.UTF_8)
            val footerAt = text.lastIndexOf("</gpx>")
            tailOffset = if (footerAt >= 0) text.substring(0, footerAt).toByteArray().size.toLong() else out.length()
            points = Regex("<trkpt ").findAll(text).count()
        }
        openType = null
        contextXml = ""
    }

    private fun trackPoint(location: Location, accel: AccelStats): String = buildString {
        append("<trkpt lat=\"").append(location.latitude).append("\" lon=\"")
            .append(location.longitude).append("\">")
        if (location.hasAltitude()) append("<ele>").append(location.altitude).append("</ele>")
        append("<time>").append(Instant.ofEpochMilli(location.time)).append("</time><extensions>")
        if (location.hasSpeed()) append("<speed>").append(location.speed).append("</speed>")
        if (location.hasAccuracy()) append("<accuracy>").append(location.accuracy).append("</accuracy>")
        append("<accel_variance>").append(accel.variance).append("</accel_variance>")
        append("<accel_freq>").append(accel.dominantFrequency).append("</accel_freq>")
        append("<accel_steps>").append(accel.stepCount).append("</accel_steps>")
        val cadence = (accel.dominantFrequency * 60.0).toInt()
        if (cadence >= 1) {
            append("<gpxtpx:TrackPointExtension><gpxtpx:cad>").append(cadence)
                .append("</gpxtpx:cad></gpxtpx:TrackPointExtension>")
        }
        append("</extensions></trkpt>\n")
    }

    private fun segmentContext(location: Location, nearby: NearbyContext): String {
        val battery = context.getSystemService(BatteryManager::class.java)
        val power = context.getSystemService(PowerManager::class.java)
        return buildString {
            append("<extensions><rook:context lat=\"").append(location.latitude)
                .append("\" lon=\"").append(location.longitude)
                .append("\" resolved=\"").append(Instant.now()).append("\">")
            if (nearby.roadClass != null) {
                append("<rook:road>")
                nearby.roadName?.let { append("<name>").append(xml(it)).append("</name>") }
                append("<class>").append(xml(nearby.roadClass)).append("</class>")
                nearby.roadDistanceM?.let { append("<distance_m>").append(it).append("</distance_m>") }
                append("</rook:road>")
            }
            append("<rook:device><battery_percent>")
                .append(battery?.getIntProperty(BatteryManager.BATTERY_PROPERTY_CAPACITY) ?: -1)
                .append("</battery_percent><charging>")
                .append(battery?.isCharging == true)
                .append("</charging><screen_on>").append(power?.isInteractive == true)
                .append("</screen_on><automotive>false</automotive></rook:device>")
            append("</rook:context></extensions>\n")
        }
    }

    fun prune(now: LocalDate = LocalDate.now()) {
        dir.listFiles().orEmpty().filter { it.extension == "gpx" }.forEach { candidate ->
            val day = runCatching { LocalDate.parse(candidate.nameWithoutExtension) }.getOrNull()
            if (day != null && day.isBefore(now.minusDays(RETENTION_DAYS))) candidate.delete()
        }
    }

    override fun close() {
        raf?.close()
        raf = null
        file = null
        date = null
        openType = null
        points = 0
    }

    private fun RandomAccessFile.writeUtf8(value: String) = write(value.toByteArray(Charsets.UTF_8))

    private fun xml(value: String): String = value
        .replace("&", "&amp;")
        .replace("<", "&lt;")
        .replace(">", "&gt;")
        .replace("\"", "&quot;")
        .replace("'", "&apos;")
}
