package com.steele.looky.location

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import com.steele.looky.AppSettings

class BootReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val settings = AppSettings(context)
        if (intent.action == Intent.ACTION_BOOT_COMPLETED && settings.continuousRecording) {
            TraceService.start(context, settings.activeMode)
        }
    }
}
