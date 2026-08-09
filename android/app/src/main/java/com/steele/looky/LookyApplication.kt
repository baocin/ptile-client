package com.steele.looky

import android.app.Application
import com.steele.looky.offline.PackManager

class LookyApplication : Application() {
    override fun onCreate() {
        super.onCreate()
        PackManager(this).installBundledDemoIfNeeded()
    }
}
