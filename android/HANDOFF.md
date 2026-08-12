# Looky Android handoff

This document is for the team taking ownership of Looky. It describes the
current product behavior, the normal engineering workflows, and the boundaries
that should not be accidentally hidden by a UI change.

## Product contract

Looky is an Android-only, offline-first map and movement recorder. Drive and
Trail are first-class modes. A foreground location service stays visible in a
persistent notification, holds a partial wake lock while active, refreshes
movement classification every second, records location and motion in the
background, and writes one Rook-compatible GPX day file per local date.

Looky does not use a WebView, Google Maps SDK, or online route service. The map
canvas and all local lookups are backed by PTiles decoded through the native
`ptiles-ffi` library. Internet is used only for optional map-pack downloads.

## Repository and runtime layout

- Branch: `msp/lookie-android-app`
- Android project: `android/`
- Application id: `com.steele.looky`
- Display name: `Looky`
- Native PTiles source: `ffi/`, `core/`, and `motion/`
- Installed layers: `filesDir/ptiles/`
- Daily recordings: `filesDir/traces/YYYY-MM-DD.gpx`
- Public debug APK: `https://android.mydatatimeline.com/looky/latest.apk`

The R2 bucket is `mydatatimeline`. Use the existing `mdt-r2` AWS profile; do
not copy credentials into the repository, CI logs, or the app.

## User workflows

### First launch

1. The onboarding stepper explains offline maps, background recording, and GPX.
2. Location/activity/notification permissions are requested.
3. The default download resolves and installs the user's current state pack
   from the `2026-08-07` My Data Timeline snapshot, including roads, trails,
   highways, parks, rail, places, water, buildings, businesses, addresses,
   EV, and lookup layers. US-wide camera/signals/admin layers are included.
4. The user can skip recording or continue into the app.

### Offline Maps

Offline Maps is reached from More. It has a prominent all-US download and a
card for every state plus DC. Each card shows installed layer count and total
bytes, has a Download/Update action, and keeps individual filenames collapsed
until expanded. Downloads stream to a pending file and rename atomically only
after completion.

The all-US action downloads every state layer plus `US.admin_v2.ptiles`,
`US.camera.ptiles`, and `US.signals.ptiles`. This is intentionally a large
operation; storage forecasting and resumable downloads are future work.

The admin pack is fetched before anything else, including on a single-state
download: it is what resolves which state you are in, and that answer chooses
every pack that follows.

### Drive and Trail

Drive and Trail are separate screens, not one screen with a flag. Drive
searches the business layers and follows turn-by-turn; Trail searches only the
trails layer and shows a walk summary. Both build a chain of stops -- the last
one is the destination -- and both record to their own GPX day file, with an
always-on background log continuing between journeys.

Destinations come from search or from the stop chain; the map's long-press
handler still exists on `OfflineMap` but no screen passes one. Both endpoints
are snapped to the nearest installed offline road/trail before the route graph
is evaluated. Layer selection is state- and coverage-aware; filesystem order
never decides which state's graph is used.

A leg that fails because its corridor is over the cell cap, or because the
corridor is connected on paper but not in the data, is split at its midpoint
and retried -- up to three times. Measured over 45 Tennessee city pairs, that
takes 14 routable pairs to 38.

If a route reports an empty graph, first verify that a versioned state roads
layer is installed for the area and that the endpoints are inside its coverage.
The native graph is bounded to 512 H3 resolution-7 cells; it is not a
country-scale routing graph.

### Recording settings

Settings exposes continuous recording, developer map, route preferences, GPS
polling interval, and accelerometer polling rate. Rate changes are sent to the
running service immediately. PTiles adaptive motion classification receives
the selected sampling intent and sensor summaries; it does not override the
user's chosen rates.

## Architecture

`MainActivity` owns permissions and onboarding. `LookyApp` owns Compose
navigation and screen state. `PtilesRepository` owns layer discovery, map
features, nearby-road context, endpoint snapping, and offline routes.
`MotionEngine` feeds accelerometer summaries and GPS observations into the
PTiles adaptive motion session. `TraceService` owns the foreground lifecycle,
location requests, notifications, and GPX append operations.

`OfflineMap` is a deliberately small vector canvas. It renders roads, trails,
parks, filled water, building footprints, state and county lines from the admin
pack, business and trailhead pins, road and business labels, active routes, and
recorded traces. Detail is gated by zoom (`MapDetail`) and the draw budget is
shared per layer, because a city's roads would otherwise evict every trail. It
is not a production vector-tile renderer and has no satellite, traffic, or 3D
layer.

## GPX and durability expectations

The writer follows the Rook GPX 1.1 day-file contract:

- one file per local day, with one `<trk>` per debounced movement type;
- movement names remain `Stationary`, `Walking`, `Running`, `Driving`, or
  `Unknown` for the classifier's initial state;
- missing readings are omitted rather than serialized as zero;
- accelerometer data is stored as summary statistics, not raw samples;
- the file tail is rewritten after each append and remains readable during a
  long session;
- files older than 30 days are pruned when recording starts;
- a process restart starts a new movement segment.

The trace files are private but Android backup is deliberately enabled. Treat
them as sensitive location history when adding export, sync, or analytics.

## Engineering workflows

### Build and test the Android client

```sh
cd android
./gradlew :app:testDebugUnitTest :app:assembleDebug
```

Generated Gradle files and build outputs are ignored by `android/.gitignore`.

### Run on an emulator/device

```sh
adb devices
adb install -r android/app/build/outputs/apk/debug/app-debug.apk
agent-device open com.steele.looky
agent-device snapshot -i
```

When a physical phone is unavailable, the Pixel emulator is the supported
validation fallback. Exercise onboarding, Offline Maps, Drive/Trail switching,
route snapping, background recording, and Settings rate changes. Inspect
`adb logcat` for `FATAL EXCEPTION` and `AndroidRuntime` after each flow.

### Change PTiles native code

After changing `ffi/`, `core/`, or `motion/`, rebuild the Rust library,
regenerate Kotlin UniFFI bindings, rebuild both Android ABIs, then run the
Android tests. The exact commands are in `android/README.md`. Also run:

```sh
cargo test -p ptiles-ffi --lib
```

Do not hand-edit generated Kotlin bindings unless repairing a known generated
artifact; regenerate them from the native interface instead.

### Deploy a debug APK

```sh
sha256sum android/app/build/outputs/apk/debug/app-debug.apk
aws --profile mdt-r2 s3 cp \
  android/app/build/outputs/apk/debug/app-debug.apk \
  s3://mydatatimeline/looky/latest.apk \
  --content-type application/vnd.android.package-archive \
  --cache-control 'public,max-age=300'
```

Verify the public object with `curl --head` and record the SHA-256 in the
handoff or release note. The APK is a debug build and must not be presented as
a signed production release.

## Data-pack operations

The downloader currently targets:

`https://maps.mydatatimeline.com/maps/2026-08-07/`

State layer stems are defined in `MapPackDownloader.STATE_LAYERS`; the US-wide
stems are in `US_LAYERS`. If the snapshot date or vocabulary changes, update
those constants and test at least two representative state downloads before
enabling a new default.

Layer files are versioned by a `_vN` suffix on the stem, and the client always
opens the highest version it has installed (`layerCandidates` for state packs,
`newestAdminPack` for the admin one). Publishing a rebuilt layer therefore
means uploading it under a new stem rather than overwriting the old file: a
device that already has the old one keeps working, and picks up the new one on
its next download.

### `US.admin_v2.ptiles`

Published 2026-08-12 alongside the untouched `US.admin.ptiles`. Two fixes, both
in `scripts/build_admin.py` in the ptiles repo:

- `boundary_flags` is populated. It was specified in SPEC.md as straddle bits
  and hardcoded to `0`, so `admin_at()` returned the H3 cell centre's
  jurisdiction with no warning and could be wrong by up to ~1.2 km. A point on
  the Tennessee side of the TN/KY line north of Clarksville resolved to
  Kentucky. 34.5% of cells carry a flag; 3.3% carry the state bit.
- County ring names come from TIGER `NAMELSAD` rather than `"{NAME} County"`,
  so Louisiana parishes, Alaska boroughs and census areas, and Virginia
  independent cities are named correctly.

The on-disk layout is unchanged and the old pack still decodes. The client does
not yet act on the flags; doing so means falling back to point-in-polygon
against `AdminLayer.polygonsIn` when the state bit is set, before choosing a
map pack. The native decoder currently has no independent `highways` layer
kind, so highway files are retained for forward compatibility while routing
and motion classification use OSM `highway` tags from the roads layer.

## Known boundaries

Read [CANNOT_DO_YET.md](CANNOT_DO_YET.md) before promising production behavior.
The important current limits are bounded routing, no rerouting once off route,
no address or coordinate destination entry, no traffic/closures, no resumable
downloads, no wearable heart rate, Android background-execution limits, and no
end-to-end trace encryption. Turn-by-turn itself now exists: `Navigator` in
`ffi/src/lib.rs` wraps `core::nav`, and the drive screen follows it.

## Handoff checklist

- Build and unit tests pass.
- PTiles FFI tests pass.
- APK installs on the validation device.
- Onboarding downloads the location-resolved current state without a file picker.
- Offline Maps exposes all states and shows sizes.
- Drive map has visible roads/buildings/water in an installed coverage area.
- Trail map has visible trail geometry.
- Route endpoints snap to offline geometry and an empty graph produces a clear
  actionable error.
- Background recording grows the current GPX file after the app is closed.
- No credentials, generated build directories, or unrelated workspace changes
  are included in the Looky commit.
