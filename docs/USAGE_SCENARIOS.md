# Ten usage scenarios, and where Looky falls down

Written against the app as it actually is at commit `14694c0` — the Kotlin in
`android/app/src/main/java/com/steele/looky/`, not `HANDOFF.md`, which is stale
on three points (it says there is no turn-by-turn; there is. It says a map
long-press sets a destination; nothing wires `onLongPress`. It says
classification runs every 2 s; it runs every 1 s).

Baseline facts every scenario inherits:

- Packs are downloaded per **US state**, from
  `https://maps.mydatatimeline.com/maps/2026-08-07/{STATE}.{layer}.ptiles`
  (`offline/MapPackDownloader.kt:23-30`). 13 layers per state, plus three
  US-wide layers (`admin`, `camera`, `signals`).
- The map is a hand-drawn Compose canvas over decoded PTiles features
  (`ui/OfflineMap.kt`). No tiles, no raster, no network. North-up, pinch zoom
  0.6–18.
- You can only pick a destination from a **business-name search** (Drive) or a
  **trail-name search** (Trail). There is no address entry, no coordinate
  entry, no map long-press, no saved places.
- Recording writes one GPX per day per session kind into `filesDir/traces/`.
  Nothing in the app exports, shares, or deletes a trace.

---

## 1. Road trip: Nashville to Asheville

**What they do.** Two days before leaving, they open Offline maps, download
Tennessee, and — thinking ahead — also download North Carolina. In the car they
search "Loveless Cafe", start the drive, and follow the turn card.

**What they get.** This is the scenario the app is built for and it largely
works. Tennessee resolves from GPS, the state's packs install, business search
finds the cafe from the name index, `PtilesStack.offlineRoute` builds a driving
route, and `DriveScreen.kt:128-136` drives a native `Navigator` that prints
"Turn left onto …" with distance to turn and distance remaining. Background
recording is already running, so the whole drive lands in a GPX.

**Where it falls down.** The 250-mile leg is far past the 512-cell corridor cap,
so `PtilesRepository.kt:403-422` bisects at the snapped geometric midpoint, up
to 3 levels deep — 8 legs maximum. When a midpoint lands somewhere no road
snaps, the whole route fails with the raw exception text in a peach banner.
Routing runs on `Dispatchers.Default` with no cancel button, so a failing
long route is a long wait for bad news. Then the real problem: the route is
computed against **one state's roads layer at a time**, chosen by the state the
current fix resolves to (`PtilesRepository.kt:496-508`). A route that crosses
the TN/NC line has no single graph to run on. And there is no reroute — going
off-route only turns the turn card red (`LookyApp.kt:300-331`); nothing
recomputes, so one missed exit ends navigation for the rest of the trip.

## 2. Day hike in Glacier, no signal

**What they do.** They download Montana in the hotel the night before, drive to
the Logan Pass trailhead, and switch to Trail. They search "Highline" and start.

**What they get.** The trail search does find it: `PtilesRepository.kt:292-316`
sweeps the trails layer across five sample centres and collapses the segments
into one row per trail name. Trailheads are always drawn regardless of zoom
(`OfflineMap.kt`), the trail routes on foot with pedestrian-legal roads merged
in, and the GPX records the whole walk with movement classification.

**Where it falls down.** Trail mode has **no turn card at all** — deliberately
(`TrailScreen.kt:65-72`) — so once you are walking you have a blue line and a
dot and nothing else. There is no elevation profile, no distance-remaining, no
"you have left the trail" signal, no total ascent. Trail search is plain
substring, not fuzzy (unlike business search), so "highline trail" finds
nothing if the data says "Highline". And nothing surfaces a trail's difficulty:
`sac_scale` and `surface` **are** decoded and shown, but only on the Developer
map's detail sheet, not in Trail search results or on the route card. Elevation
is the harder gap — the packs carry no terrain layer at all, so an elevation
profile or a grade-aware foot route needs a new layer, not a UI change.

## 3. Delivery driver, suburban Memphis

**What they do.** Forty drops a day. They get an address from their dispatch
app and want to route to it.

**What they cannot do at all.** There is no address search. The `address_v2`
pack is downloaded on every state install and then **never opened** —
`PtilesRepository.kt:439` passes `addresses = null` to `PtilesStack`. So "4212
Elvis Presley Blvd" cannot be typed anywhere in the app. The only destinations
are business names. For a delivery driver, that is the whole job.

**Where else it falls down.** No stop persistence — the reorderable stop chain
(`ui/PlacePicker.kt`, `StopList`) is in-memory only, cleared on recomposition
and mode change, never written to disk. So a 40-stop day cannot be built once
and worked through. No arrival detection, no "next stop", no ETA in clock time.
The layer is there; the wiring is not. This is the single largest gap in the
app and it is entirely client-side work — the address layer is already
published, already downloaded, and `AddressLayer::find_address` already exists
in the FFI (`ffi/src/lib.rs:1982`).

## 4. Birder at a wildlife refuge

**What they do.** They walk a loop, want to record where each sighting was,
and want to get the track off the phone into eBird afterwards.

**What they get.** The track: yes, in detail. Every trackpoint carries lat,
lon, elevation, time, speed, accuracy, accelerometer variance, dominant
frequency, step count and derived cadence (`TraceRecorder.kt:115-131`), and
each segment carries a `rook:context` block with nearest road name, class,
distance and battery state. Recording detail (`ui/RecordingDetail.kt`) draws
the track over the decoded surroundings with a movement-share breakdown.

**Where it falls down.** Two hard stops. First, **there is no way to mark a
point.** "Waypoints" in this app means route stops, not dropped pins; there is
no drop-pin, no note, no photo, no timestamped marker of any kind. Second,
**there is no export.** The GPX files sit in `filesDir` with no FileProvider,
no share intent, no export button anywhere in the source. The data is
excellent and completely trapped. Neither gap needs a pack change — the first
is a UI affordance plus a GPX `<wpt>` write, the second is a FileProvider and
a share intent.

## 5. Tracking a daily commute

**What they do.** Turn it on, forget about it, look at the week on Friday.

**What they get.** This works better than most of the app. `continuousRecording`
defaults on (`AppSettings.kt`), so background recording starts on first launch
after permissions and again on boot (`BootReceiver.kt`). The service is
`START_STICKY`, holds a partial wake lock, and has a watchdog that
re-subscribes to location if no fix arrives for `interval × 6` seconds clamped
to 60–300 (`TraceService.kt:70-71, 236-243`). Movement is reclassified every
second. Ending a drive falls back to background recording rather than stopping
(`TraceService.kt:88-90`).

**Where it falls down.** Recordings is one **flat list of every movement
segment across every day file**, newest first (`LookyApp.kt:468-543`). There
is no per-day rollup, no week view, no totals, no "your commute is usually 34
minutes", no trend. Fifty segments a day means scrolling. There is also no
delete — a bad day, an accidental recording, a trip you would rather not keep,
all permanent until the 30-day auto-prune
(`TraceRecorder.kt:157-162`) removes them. And the phone is recording GPS all
day into unencrypted files with `allowBackup="true"` and no redaction, which
`CANNOT_DO_YET.md` acknowledges but the settings screen does not mention.

## 6. Weekend camper packing for a trip through three states

**What they do.** Friday evening, hotel wifi, planning to drive TN → GA → FL.
They open Offline maps and start downloading.

**Where it falls down immediately.** They cannot find out what this will cost
before starting: the size of a state's packs is only known **after** install
(the card reads bytes off installed files). There is no manifest, no size
forecast, no free-space check. Downloads are sequential `HttpURLConnection`
GETs on the screen's coroutine scope (`MapPackDownloader.kt:58-82`), so
navigating away from Packs or backgrounding the app **kills the download
mid-file** and leaves a `.{name}.pending` file on disk that nothing ever cleans
up. There is no resume, no retry, no WorkManager, no Wi-Fi-only option, and the
first layer that fails aborts the whole run with `"{code} for {layer}"` in the
status line. Three states is 39 files that must all complete while the user
stares at the screen. "Download all US PTiles layers" is roughly 666 sequential
files under the same rules.

## 7. Visiting family, looking for somewhere to eat

**What they do.** Sitting in a relative's house in Knoxville. Open Drive, type
"thai".

**What they get.** Better than expected. `PtilesRepository.kt:232-260` merges
two passes — the spatial business layer within 8 km, and every installed
`{STATE}.business_name_index.ptiles` — then ranks by
`nameSimilarity − distancePenalty` (`:753-772`): exact 1.0, prefix 0.92,
substring 0.85, word-prefix 0.8, else a Levenshtein ratio, floored at 0.55,
with 40 km of distance costing 0.45 of similarity. Results show distance, an
8-point compass bearing, and "on your route" when within 500 m of a planned
path. Airline flight-number nodes are filtered out (`:695`).

**Where it falls down.** You cannot search by **category** — "restaurants near
me", "coffee", "gas station" — because business categories arrive as a raw
integer index and the `business_categories.json` sidecar is downloaded and
never parsed. The Developer map's detail sheet literally shows the integer.
Nothing carries opening hours, so "is it open now" is unanswerable. And the
search origin is always the GPS anchor, never the panned viewport: you cannot
scroll the map to downtown and ask what is there. `places_v1` and `ev_v1` are
downloaded and never read by anything either. All three of these are unused
already-published data, not missing data.

## 8. Cyclist on a rail-trail

**What they do.** Ride 30 miles out and back on a converted rail corridor.

**Where it falls down.** There is no cycling mode. `OfflineRouteMode` has
exactly two values, Driving and Trail (`ffi/src/lib.rs:155-160`), and Trail
means the foot profile with the trails layer merged in. A bike is neither: it
wants paved surfaces, avoids stairs, and will happily use a road a pedestrian
route would down-rank. `surface` and `trail_type` **are** in the trails layer
and decoded — so the data supports the distinction and the routing profile does
not. Nothing shows grade, because there is no terrain layer. The rail-trail
itself renders (rail is a drawn layer), but distinguishing an active rail line
from a converted trail depends on which layer the feature was published into,
which the app never surfaces. Movement classification has no Cycling category
either — `MovementType` is Unknown/Stationary/Walking/Running/Driving
(`ffi/src/motion.rs:44-50`), so a 15 mph ride is labelled Driving in every
recording.

## 9. Landing at an airport with a rental car

**What they do.** Fly into Denver, pick up a car, open Looky, want to get to
the hotel.

**Where it falls down.** Colorado is not installed and airport wifi is what it
is. The failure surface is the app's weakest area. `mapsReady` requires a GPS
fix, so before the first fix arrives the "Downloads Needed" chip shows
regardless of what is installed (`LookyApp.kt:155-166`). With no pack for the
viewport, the map does not say so — every query is wrapped in `runCatching`,
layer lookup returns null, and you get **blank paper with grid lines and a
compass**. The only textual signal is in the search picker
(`ui/PlacePicker.kt:142`, "No maps downloaded for this area. Tap to fix."). Then
the hotel is an address, so even with Colorado installed the destination cannot
be entered (see scenario 3). County lines only draw below zoom 0.9
(`AdminBoundaries.COUNTY_LINES_BELOW`), so there is not even a coarse "you are
here" reference at street zoom on an empty map.

## 10. Anyone outside the United States

**What they do.** Install the app in Vancouver, or Bristol, or anywhere else.

**What they get.** Nothing. This is worth stating plainly rather than treating
as a footnote. `US_STATES` is 50 states plus DC (`MapPackDownloader.kt:32`);
packs are addressable only by those codes and `downloadCurrentState` throws on
anything else. `StateResolver.kt:22-65` is a table of US state bounding boxes
with an Alaska antimeridian special case. The APK bakes in 1.6 MB of US Census
cartographic boundaries (`us_state_bounds.txt`, `us_county_bounds.txt`) as its
only coarse-zoom orientation layer. Imperial units default to on, commented
"this ships to a US-only pack set" (`AppSettings.kt:27`). Outside the US: no
state resolves, no packs download, every search returns NoMaps, and the map is
blank paper without even county lines.

This is a **data-pipeline** limit, not an app limit — the library itself has no
US assumption in its query surface (see `docs/LIBRARY_INDEPENDENCE.md`). What
would need to change is the pack publishing scheme: packs keyed by an arbitrary
region id rather than a state code, and a manifest the app can read to discover
what regions exist, instead of a hardcoded 51-element array.

---

## Gaps, ranked by how often they would bite

1. **No address or coordinate destination entry.** Bites every user, every
   session, in every scenario except a named-business errand. The address layer
   is downloaded on every state install and never opened
   (`PtilesRepository.kt:439`), and the FFI call already exists. Highest
   value-per-line-of-code fix in the app.
2. **Pack downloading is fragile and blind.** No size shown before you start,
   no free-space check, no resume, no retry, no background service, dies when
   you leave the screen, leaks `.pending` files, and one failed layer aborts the
   run (`MapPackDownloader.kt:58-82`). Every user hits this on day one and
   again for every new state.
3. **No reroute.** Going off-route recolours the turn card and does nothing
   else (`LookyApp.kt:300-331`). One missed turn ends useful navigation. Bites
   every drive of any length.
4. **State-keyed packs and a state-keyed router.** Routing and search work
   against whichever single state the current fix resolves to
   (`PtilesRepository.kt:496-508`), so any journey that crosses a line — and
   plenty of ordinary local ones near a border — has no single graph. Needs a
   multi-pack routing path, or region packs that are not states.
5. **No export, share, or delete of recordings.** The app collects unusually
   rich GPX and offers no way to get it out or throw it away
   (`TraceRecorder.kt`, no FileProvider in the manifest). Bites everyone who
   records anything they care about, and everyone who records something they
   do not.

Then, in roughly descending order of frequency:

6. **Long routes fail unpredictably.** The 512-cell corridor cap plus 3-deep
   midpoint bisection (`PtilesRepository.kt:403-422`) means any interstate-scale
   route is a coin flip, with a long uncancellable wait before the answer.
7. **Business search cannot search categories.** `business_categories.json` is
   downloaded and never parsed; the category surfaces as a raw integer. "Coffee
   near me" is impossible.
8. **Search origin is locked to the GPS anchor.** You cannot pan the map and
   search where you are looking.
9. **No map long-press destination.** `OfflineMap.kt:205` exposes
   `onLongPress` and no caller passes it — while the map's accessibility string
   at `:221` tells screen-reader users "Long press to add a stop to the route".
   That one should be fixed on honesty grounds regardless.
10. **Blank paper is the no-data failure mode.** No pack for the viewport
    produces an empty canvas rather than a message (`LookyApp.kt:155-166` also
    mis-signals before the first GPS fix).
11. **No voice guidance.** Turn-by-turn that requires looking at the phone is
    turn-by-turn you should not use while driving.
12. **Recordings has no rollup.** A flat segment list with no day, week or
    total view.
13. **No cycling profile and no Cycling movement class.** Rides are routed on
    foot and labelled Driving.
14. **Four downloaded layers are never read** — `places_v1`, `ev_v1`,
    `highways_v2`, `signals` — costing bandwidth and storage for nothing.
15. **US-only**, by pack scheme rather than by code.

### Where the packs, not the app, are the limit

Most of the list above is client-side wiring against data that already ships.
Three gaps are not:

- **Elevation and grade** (scenarios 2, 8). No terrain layer exists. An
  elevation profile, total ascent, or a grade-aware foot/bike cost function all
  need a new published layer.
- **Opening hours, and anything time-varying** (scenario 7) — traffic,
  closures, seasonal trail status. The business layer carries phone, website,
  operating status and confidence, but no hours; nothing in any layer is
  time-indexed.
- **Non-US coverage** (scenario 10). Needs region-keyed packs and a discovery
  manifest, replacing the hardcoded 51-state array and the baked US Census
  boundary assets.
