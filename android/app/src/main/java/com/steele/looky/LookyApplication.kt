package com.steele.looky

import android.app.Application
import com.steele.looky.offline.PackManager

class LookyApplication : Application() {
    override fun onCreate() {
        super.onCreate()
        // Early debug builds copied the decoder conformance corpus into app
        // storage and presented it as a Tennessee map. Those files are sparse
        // test slices, not routable packs; real maps come from the dated R2
        // snapshot selected in MapPackDownloader.
        PackManager(this).removeBundledConformanceSlices()
    }
}
