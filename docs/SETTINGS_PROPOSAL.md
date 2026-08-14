# Settings proposal for Looky

Written against the working tree on `msp/lookie-android-app` at commit
`b862006`, plus uncommitted edits to `TraceService.kt` and `MotionEngine.kt`
that landed while this was being written. Line numbers were re-verified after
those edits; where a number could still have moved, the symbol name is given
alongside it.

Two things were asked for: an answer on "search all alternative names places
might have as well", and a real debate about what else belongs in Settings.
The second half is written as an argument, not a wishlist, because most of
these candidates lose.

---

## Part 1: alternative names

### What the data actually holds

Three name-bearing layers matter, and they are in three different states.

**Trails carry one name and nothing else.** `TrailFeature`
(`core/src/trails.rs:20-28`) is `osm_id`, `trail_type`, `geom_type`, `coords`,
`surface`, `sac_scale`, `name: Option<String>`. The record format has a flags
byte with exactly one name bit (`core/src/trails.rs:86-90`), and the builder
writes `tags["name"]` only (`scripts/build_trails.py:155` in the ptiles repo).
There is no `alt_name`, no `ref`, no `old_name` on disk. Trail search
(`PtilesRepository.trailsNearby`,
`android/app/src/main/java/com/steele/looky/offline/PtilesRepository.kt:305-335`)
therefore has nothing to widen to. Its own comment says so at
`PtilesRepository.kt:328-332`.

**Businesses carry a `brand`, and the client throws it away.** `Business`
(`core/src/business.rs:31-57`) decodes `name`, plus an optional `brand` behind
flag `0x08` (`core/src/business.rs:42`, decoded at `:110-114` and `:328-332`;
the round-trip test asserts `brand == "Waffle House Inc"` at
`core/src/business.rs:600`). There is no `alt_name` field in the business
record at all. The UniFFI record that crosses into Kotlin,
`BusinessInfo` (`ffi/src/lib.rs:537-554`), exposes `name`, `phone`, `website`,
`operating_status`, `source_type`, `source_id`, `confidence` — and **not**
`brand`. So the one genuine alternative name the business layer holds is
decoded by Rust on every query and dropped at the FFI boundary. That is a
four-line change to expose, not a format change.

**The name index is keyed on the primary name only.**
`{STATE}.business_name_index.ptiles` buckets every record by the first
character of its `name` into 28 zstd blocks
(`core/src/business_search.rs:5-27`), and `BusinessHit`
(`core/src/business_search.rs:54-71`) carries `name`, `category_idx`,
`lat`/`lon`, `cell`, `score` — no brand, no alternate. Even if the business
record's `brand` were exposed tomorrow, the statewide half of
`searchBusinesses` (`PtilesRepository.kt:245-273`) could not use it: the index
has no key for it and no field to return it in. Only the spatial half — the
8 km sweep at `PtilesRepository.kt:260` — could match on brand.

**The layer that does carry `alt_name` is downloaded and never opened.**
`alt_name` exists in exactly one PTiles layer: places (`PTILESP`), where it is
optional behind flag `0x01` (`SPEC.md:612` in the ptiles repo, written by
`scripts/build_places.py:76` and `:115-122`). `places_v1` is in
`STATE_LAYERS`, so every state install downloads it
(`offline/MapPackDownloader.kt:28`). Nothing reads it: there is no `places.rs`
in `core/src/` — the file kind is registered in `core/src/versions.rs:67` and
that is the extent of it — there is no places API in `ffi/src/lib.rs`, and the
string `"places"` appears nowhere in the Kotlin outside that downloader list.
Note also what the layer is: populated places and neighbourhoods, cities,
towns, villages, suburbs. It is a gazetteer, not a POI set. Its `alt_name`
would let someone find "Music City" and get Nashville. It has no bearing on
finding a restaurant or a trail.

### What a setting could honestly control today

Nothing worth a switch.

- Trail alternative names: impossible. Needs a trails schema change plus a
  rebuild of 51 state packs.
- Business alternative names, statewide: impossible. Needs `brand` added to
  the name index builder and a bump of the sidecar format.
- Business alternative names, within 8 km of the user: possible after a small
  FFI change (add `brand: Option<String>` to `BusinessInfo`, regenerate
  bindings, score it in `rankByNameAndDistance`,
  `PtilesRepository.kt:983-1002`). But this gives the nearby pass a recall the
  statewide pass does not have, so the same query returns brand matches close
  by and misses them further out, for reasons no user could infer.
- Place (town/neighbourhood) alternative names: possible only after writing a
  places decoder in `core`, an FFI surface for it, and a Kotlin search path —
  and it answers a question the app does not currently ask.

There is also a reason the request is less urgent than it sounds. The app is
already forgiving about names: `nameSimilarity` (`PtilesRepository.kt:937-951`)
scores exact 1.0, prefix 0.92, substring 0.85, word-prefix 0.8, then falls back
to a Levenshtein ratio against the whole string and against the best single
word, floored at `MIN_NAME_SIMILARITY = 0.55` (`:667`). "wafle huse" finds
Waffle House today. What fuzzy matching cannot do is bridge names that share no
characters — "WaHo", "Micky D's", "the Cracker Barrel on 96" — and that is
precisely the class `brand` and `alt_name` would fix.

### Recommendation

**Do not add the setting.** Add the capability, unconditionally, in this order:

1. Expose `brand` through `BusinessInfo` (`ffi/src/lib.rs:537-554`) and match
   on it in `rankByNameAndDistance`, scoring a brand match slightly below a
   name match so "Waffle House Inc" never outranks a business literally called
   what was typed. Smallest change with a real effect; also gives the business
   detail card something to show.
2. Add brand (and `alt_name` if the business builder ever collects it) as
   additional keys in `build_business_name_index.py`, so the statewide pass has
   the same recall as the nearby one. This is a pack rebuild, published under a
   new stem the way `US.admin_v2` was — old clients keep working.
3. Only then decide whether trails need a second name field. That is a schema
   change to `build_trails.py` and a rebuild of every state's trails layer, for
   a class of names ("the Greenway") that fuzzy matching already half-covers.

A toggle here would be a failure in the specific way the brief describes: a
search that finds more of what you meant is not a preference, it is correct
behaviour, and nobody would ever turn it off. If it is switched on by default
and never switched off, it is not a setting — it is a second code path that
only the default branch is ever exercised on, which is how search ranking rots.

If a switch is wanted anyway for a period of A/B comparison on a real device,
make it a developer-map row rather than a user setting, and delete it once the
comparison is done.

---

## Part 2: the other candidates

Debated with a subagent instructed to argue against every one of them. Where
the two of us disagreed on a fact, the fact was re-checked directly; one such
check is noted below.

### Ship

**Delete a recording.** An action, not a setting.

For: the app records GPS continuously by default (`AppSettings.kt:17-19`,
`continuousRecording` defaults true) and the user cannot remove any of it.
`RecordingsScreen` (`ui/LookyApp.kt:503-587`) lists segments and opens
`RecordingDetailScreen`; neither offers a delete. The only deletion in the
whole app is on the 30-day prune (`location/TraceRecorder.kt:157-162`), which
the user does not control and is not told about. Map packs already have delete
with a confirm dialog (`ui/LookyApp.kt:695-711` → `PackManager.delete`,
`offline/PackManager.kt:46-51`) — the pattern exists, it just was not applied
to the data that matters more.

Against: nothing survives contact. The strongest objection is that a day file
holds several segments and users will want to delete a segment, not a day —
which is a scoping question, not a reason to keep the gap.

Verdict: **ship**, whole-file delete first, from `RecordingDetailScreen`, with
the pack confirm dialog copied verbatim. Segment-level delete needs a GPX
rewrite and can wait for someone to ask.

**Show the achieved sensor rates.** Two rows in the diagnostics sheet.

For: this is the owner's own bug, and the app already has the answer. The
delivered accelerometer rate is measured over a 2 s window
(`MotionEngine.measuredHz`, `location/MotionEngine.kt:42`, accumulated at
`:210-220`), stored as `MotionEngine.deliveredRateHz` (`:76`), and published
onto `LiveTraceState.accelHz` (`model/Models.kt:39`, written at
`location/TraceService.kt:342`). Nothing in the UI reads it — `accelHz` in
`ui/LookyApp.kt:439` and `:462-463` is the *setting*, a different value with a
confusingly identical name. Meanwhile `MotionSheet` reports staleness and
explains it by quoting the **configured** rate:
`"${settings.accelerometerRateHz} Hz"` at `ui/MotionSheet.kt:105`, and
`"${settings.gpsIntervalSeconds}s polling"` at `:98`. On a phone delivering
2 Hz against a configured 50, that line does not just omit the diagnosis, it
prints the opposite of it. (The two agents disagreed here — one read a copy
from before the `accelHz` plumbing landed. Verified directly against the
current tree.)

The achieved GPS interval is not measured anywhere, but the data to derive it
is already in the buffer the speed-window table reads
(`MotionFix.atMs`, `model/Models.kt:56-61`; windows at `:51`). A median gap
over the last N fixes is a few lines.

Against: it is diagnostic clutter for a user who is not debugging. That is an
argument for where it goes, not whether — and the sheet is already reached by
tapping a warning icon (`ui/LookyApp.kt:243-266`), so its audience is already
self-selected.

Verdict: **ship.** Highest value per line in this entire document. It is not a
setting, it costs no new preference, and it replaces a misleading line with a
true one. Until it exists, every conversation about GPS and accelerometer
settings is conducted blind.

**Pause recording.** An action on the notification, not a setting.

For: today the only way to stop recording for ten minutes is to turn
`continuousRecording` off entirely, via `TraceService.stop()`, which also
clears the preference (`TraceService.kt:173`) — so it stays off until the user
remembers. Worse, starting a drive forces it back on: the Record plan sets
`continuousRecording = true` unconditionally (`TraceService.kt:203`). A user
who deliberately turned recording off and later taps "Start drive" is silently
re-enrolled in always-on background recording. That is not a setting behaving
as a setting.

Against: a paused recorder that silently never resumes is worse than no pause.

Verdict: **ship** as a timed pause (30/60 minutes, auto-resume) exposed as a
notification action beside the existing Drive/Trail/Stop actions
(`TraceService.kt:364-366`). And fix `TraceService.kt:203` regardless — a
preference the service overwrites is not a preference.

**Body weight, asked once during onboarding.**

For: `estimateCalories` (`ui/LookyApp.kt:913-920`) is 55 kcal/km walking, 70
running, scaled to a fixed 70 kg body mass documented at `:905-911` — which
already carries a `ponytail:` note saying to add a weight setting when someone
wants a number they would stand behind. The CAL tile is on both mode screens
all day (`:934`).

Against: this is the weakest "ship" here. The per-kilometre coefficient is
itself a rough figure; a precise weight makes an imprecise number look sourced.
The genuinely lazy alternative is to delete the calorie tile and show something
the app measures — moving time, or top speed.

Verdict: **ship, but in onboarding, not Settings**, and only if the calorie
tile is staying. One number, asked once, with no default to defend. If the
answer to "do we want CAL at all" is no, this candidate disappears, which is
the better outcome.

### Change the default instead

**Developer map, currently on.** `developerMapEnabled` defaults to `true`
(`AppSettings.kt:13-15`), so every user gets a Developer map card in More
(`ui/LookyApp.kt:404-406`) showing raw OSM ids, category integers, source
confidence, and pack filenames. The toggle's own subtitle admits it: "On by
default during development" (`ui/LookyApp.kt:455`). Note the route itself is
not gated — only the menu entry is. Verdict: **default-change** to false, or
better, delete the preference and gate the card on `BuildConfig.DEBUG`. That
removes a settings row and makes it impossible to ship the surface by accident.
One caveat: the developer map is currently the only place with per-layer
visibility toggles (`ui/LookyApp.kt:769-778`), so check nobody is relying on it
as a real feature before hiding it.

**Keep the screen on while navigating.** There is no `FLAG_KEEP_SCREEN_ON`
anywhere in the app. Turn-by-turn on a screen that sleeps after 30 seconds is
not turn-by-turn. As a setting it is unanswerable — nobody knows they want it
until the screen dies mid-turn. Verdict: **default-change**, scoped to
DriveScreen while a route is active. No row.

**Recording retention.** `RETENTION_DAYS = 30L`
(`location/TraceRecorder.kt:19`) silently deletes the user's location history
with no warning and no per-file exemption. A user-chosen number is still an
arbitrary number, only now the data loss is the user's fault. Two further facts
argue for removing rather than configuring it: the prune runs from
`TraceRecorder.init` (`:57`), so a long-lived service can go months without
pruning — the behaviour is not even reliable — and a GPX day file is kilobytes
against packs that are gigabytes, so it is not saving the space that is
actually at risk. Verdict: **default-change to never; delete `prune`**. This is
a net code deletion, and it becomes safe the moment delete-a-recording ships.
If storage ever genuinely bites, cap by total bytes with a warning, not by age
in silence.

**Wi-Fi-only downloads.** Real problem: `downloadStates`
(`offline/MapPackDownloader.kt:46-88`) is a sequential loop of plain
`HttpURLConnection` GETs — 16 files for one state, 653 for all-US — with no
retry, no `Range` resume, no size forecast, no free-space check, and no
network-type check anywhere. It runs on the screen's coroutine scope
(`ui/LookyApp.kt:607`, `:622`), so leaving the screen or rotating kills it
mid-file and leaves a `.{name}.pending` orphan (`MapPackDownloader.kt:66`) that
nothing cleans up. One non-2xx response aborts the whole remaining run
(`:65`, inside the single `runCatching` at `:55`).

But a Wi-Fi checkbox bolted onto that machine is a preference on top of a bug,
and it invents a new support case: "the download won't start" because a switch
on another screen is on. Verdict: **default-change** — move the downloader to
WorkManager with `NetworkType.UNMETERED` as the constraint and a per-run
"download on cellular anyway" confirmation at the tap. WorkManager brings
resume, retry with backoff, and survival across screen exit, so it removes the
setting and three of the bugs at once. Add a `StatFs` free-space check and a
size estimate to the same confirm dialog; both are guards, not preferences.

**Units.** Keep `imperialUnits` exactly as it is (`AppSettings.kt:27-30`). It
is the rare preference that is genuinely un-guessable — a scientist in
Tennessee, a European visitor — and it is already the smallest possible
control. Do not add locale auto-detection: an en-GB phone in Nashville would
get kilometres, which is worse than a wrong-by-default toggle the user can
flip. Seeding the initial value from locale is harmless if someone wants it.
Separately, there is a real bug worth more than any new setting: three call
sites cache the value in a keyless `remember` — `ui/LookyApp.kt:536`,
`ui/RecordingDetail.kt:248` and `:413` — so flipping units does not refresh
those screens.

### Reject

**Battery saver / adaptive GPS rate.** This would be the fourth knob on one
dial. `gpsIntervalSeconds` already exists (`AppSettings.kt:40-42`) and the
classifier already knows whether the user is stationary. If the rate should
depend on movement, the app has the movement — make it adaptive with no
setting, or leave the existing interval alone. A toggle that silently degrades
recording quality generates "why did my trace go sparse" reports that nobody
connects back to a switch flipped weeks earlier. Reject.

Related and more useful: `PRIORITY_HIGH_ACCURACY` is hardcoded at
`TraceService.kt:260` and a partial wake lock is acquired with no timeout at
`:230-233`. If battery is a real complaint, that is where to look, with the
achieved-rate readout in hand first.

**Map detail / declutter slider.** `MapDetail` (`ui/OfflineMap.kt:207-310`) is
already a considered zoom ladder — arterial-only below 0.9, points above 0.9,
buildings above 2.2, footways above 3.0, road labels above 1.8, business labels
above 3.2 — each threshold carrying a comment explaining why it is what it is,
sitting above a 3,000-feature cap with per-layer quotas
(`PtilesRepository.kt:706`, `:759-799`). A user slider fights that ladder, and
then two systems disagree and someone has to reason about both. The real
complaint is never "too much detail", it is "the map is slow" or "I cannot find
my road", and a slider fixes neither. Reject; tune the constants against a
measured frame budget.

**Cycling mode as a setting.** This is a category error: `activeMode` already
exists (`AppSettings.kt:21-25`), so cycling is a third enum value and a routing
profile, not a preference. It is also a real gap — `OfflineRouteMode` has only
Driving and Foot, and `MovementType` has no Cycling class, so a 15 mph ride is
recorded as Driving. But it multiplies against the session kinds
(`TraceRecorder.kt:21-25`), the tab logic (`ui/LookyApp.kt:283-299`), and
`SamplingIntent` (`MotionEngine.kt:84`). Reject as a setting; defer as a
feature until rerouting and address entry are done, both of which are broken
today rather than merely missing.

**Voice turn prompts as a setting.** There is no TTS in the app at all.
Debating the toggle before the feature exists is the exact failure this
document is meant to avoid. If it is built, the mute control belongs on the
navigation card as a speaker icon that remembers its last state, not as a
Settings row. Reject the setting; the feature is a separate decision.

**Privacy zone around home.** A geofence that suppresses writes needs a radius,
hysteresis, a rule for segments that straddle the boundary, and produces a
trace with a hole in it that reads as a bug. It also requires storing the
single most sensitive coordinate the app could hold. Pause plus delete cover
the same need with behaviour the user can see and verify. Reject.

**Pack storage location and cache limits.** Scoped storage makes an arbitrary
pack path a permissions problem, and `PackManager.packsDir` is assumed
throughout `PtilesRepository`. A cache limit is meaningless when every byte is
a pack the user explicitly chose, and per-state delete already exists
(`ui/LookyApp.kt:695-711`). The genuine gap is a free-space check before
download, which is a guard. Reject both.

**Notification appearance.** Importance, ongoing flag, and the Drive/Trail/Stop
actions are fixed (`TraceService.kt:356-377`). This is a foreground-service
notification Android requires to be visible; there is nothing to configure that
the platform would honour. Reject.

### Not settings, but larger than any setting here

Recorded so they are not lost in a document about preferences.

- **Address entry.** `address_v2` is downloaded on every state install
  (`MapPackDownloader.kt:27`) and `PtilesRepository.routeLeg` passes
  `addresses = null` (`PtilesRepository.kt:500`). No address can be typed
  anywhere in the app. `docs/USAGE_SCENARIOS.md` ranks this the highest
  value-per-line gap in the app and that ranking looks right.
- **Category search.** `business_categories.json` is downloaded
  (`MapPackDownloader.kt:28`, `:57`) and never parsed, so the detail sheet
  shows a raw integer and "coffee near me" is unanswerable.
- **Rerouting.** Going off route recolours the card and does nothing.

---

## What I would implement first, and why

1. **Render the achieved accelerometer rate and the achieved GPS interval in
   `MotionSheet`, and stop quoting the configured rate as the explanation of
   staleness** (`ui/MotionSheet.kt:98`, `:105`). The accelerometer measurement
   already exists and already reaches the UI layer unused
   (`MotionEngine.deliveredRateHz` → `LiveTraceState.accelHz`,
   `TraceService.kt:342`); the GPS one is a median over timestamps already in
   the buffer. First because every other decision about rates, battery, and
   sampling is currently being made without knowing what the platform actually
   delivers, and because the sheet presently states something known to be
   false on the owner's own device.

2. **Delete a recording**, whole file, from `RecordingDetailScreen`, reusing
   the pack confirm dialog. An always-on location recorder with no delete is
   the most serious gap in this list, and it is the precondition for dropping
   the prune.

3. **Retention to never; delete `TraceRecorder.prune()`**
   (`TraceRecorder.kt:19`, `:57`, `:157-162`). Net code deletion, and safe once
   step 2 exists. Do not replace it with a retention picker.

4. **Expose `brand` through `BusinessInfo`** (`ffi/src/lib.rs:537-554`) and
   score it in `rankByNameAndDistance`. This is the honest, shippable part of
   the alternative-names request: one FFI field, one scoring branch, no
   setting, and it makes brand visible on the business card as a bonus. Follow
   it with brand keys in the name-index builder when a pack rebuild is next
   scheduled, so the statewide pass matches the nearby one.

5. **Flip the developer map off, or gate it on `BuildConfig.DEBUG`**
   (`AppSettings.kt:13-15`). One line, removes a shipped debug surface and a
   settings row. Check first that its per-layer toggles are not doing real
   work for someone.

6. **Keep the screen on while a route is active.** One window flag, no row.

7. **Fix `TraceService.kt:203`**, which forces `continuousRecording = true`
   whenever a session records, then add a timed pause as a notification action.
   In that order: the override has to go before pause means anything.

8. **Move the downloader to WorkManager** with an unmetered constraint and a
   cellular confirmation, plus a free-space check and a size estimate at the
   tap. Largest of these by effort, and it deletes the Wi-Fi-only setting, the
   resume gap, the retry gap, and the leave-the-screen bug together.

Net effect on the Settings screen: one row removed (developer map), zero rows
added, one onboarding question (body weight, only if the calorie tile stays).
Everything else on the list turned out to be a default to change, a bug to fix,
or a feature to build.

## Unsure

- Whether 2 Hz on the owner's device is a platform throttle, a rounding of the
  `registerListener` microsecond hint (`MotionEngine.kt:114`), or something
  else. Rendering `deliveredRateHz` on a second device is the cheapest way to
  find out, which is why it is step 1 rather than a guess.
- Whether the GPS 60 s ceiling is still real. `TraceService.kt:262-268`
  documents a displacement filter that caused exactly this symptom and has been
  removed. If it persists, the next suspect is the watchdog's own 60 s floor
  (`staleAfterMs`, `TraceService.kt:73`, clamped `60_000..300_000`) or Doze —
  not a setting.
- Whether `business_name_index.ptiles` can carry brand keys without breaking
  clients that hold the current sidecar. The format has a version byte and the
  admin pack precedent (`US.admin_v2` published beside the old file) suggests
  the answer is "publish under a new stem", but the index builder was not read
  closely enough to say for certain.
- Whether anyone actually wants the calorie tile. `ui/LookyApp.kt:907-908`
  reads as though it is standing in for "a number that is not fix count". If
  so, replacing it beats adding a weight field.
