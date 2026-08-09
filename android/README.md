# Looky for Android

Looky is a native, Android-only, offline-first map, route, and movement recorder.
Drive and Trail are the primary experiences. The broader PTiles inspection tools
live behind **Settings → Developer map**, which is enabled by default while the
product is in development.

## Architecture

- Jetpack Compose UI and a custom vector canvas; there is no WebView or online
  basemap SDK.
- `ptiles-ffi` UniFFI bindings for decoding local layers, nearby road context,
  adaptive motion classification, and bounded driving/foot routing.
- A sticky location foreground service records in Drive, Trail, and background
  states. The notification switches modes without opening the app.
- `filesDir/traces/YYYY-MM-DD.gpx`, one valid GPX 1.1 file per local day, using
  the Rook extension namespaces and one track per debounced movement run.
- `filesDir/ptiles/` for installed packs. Imports are written to a pending file
  and renamed into place so a partial layer is never opened as complete.
- A tiny western Tennessee conformance pack ships only so a fresh install can exercise
  real PTiles decoding without connectivity. Install full state/region layers
  for useful coverage.

The app does not upload traces and route computation does not call a service.
`android:allowBackup` remains enabled deliberately, matching the Rook durability
position: day files can move through Android backup/device transfer. They are
sensitive location history, so this should become an explicit onboarding choice
before a public release.

## Build

Regenerate the binding and native libraries after changing `ffi/`:

```sh
cargo build -p ptiles-ffi
cargo run -p ptiles-ffi --bin uniffi-bindgen --features uniffi/cli -- \
  generate --library target/debug/libptiles_ffi.so \
  --language kotlin --out-dir ffi/bindings/kotlin

export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/android-ndk-r28"
cargo ndk -t arm64-v8a -t x86_64 -o android/app/src/main/jniLibs \
  build -p ptiles-ffi --release

cd android
./gradlew :app:assembleDebug
```

## GPX guarantees

- Base GPX remains readable by clients that ignore Looky/Rook extensions.
- Missing altitude, speed, accuracy, heart rate, or cadence is omitted rather
  than written as zero.
- Accelerometer summaries come from `ptiles-motion`; raw 50 Hz samples are not
  written.
- The tail is rewritten after every fix, keeping the file valid during a long
  session. A restart always opens a new movement segment.
- Files older than 30 days are pruned when the recorder starts.
- Segment context follows the points and is bound when the movement segment
  begins.

See [CANNOT_DO_YET.md](CANNOT_DO_YET.md) for the intentionally honest boundary
between this first native client and a mature Google Maps/AllTrails replacement.
