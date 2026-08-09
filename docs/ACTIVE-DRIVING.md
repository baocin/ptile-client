# Active driving mode — design

Turn-by-turn navigation in `web-demo`, off the same `.ptiles` files everything
else reads. The bar the requester set is "never use Google Maps again", which
is a useful bar because it rules out a demo: it has to work on a phone, in a
car, on a road you have not driven, when the route is wrong and you need it to
recover without you looking at it.

This document is the plan, not the implementation. It states what exists, what
is missing, how each missing piece should work, and in what order to build it.
Where a limit is real it is written down rather than designed around.

## The honest inventory

What is already true, verified rather than assumed:

| Have | Where |
| --- | --- |
| A* road routing within a corridor of cells | `core::route_roads_with`, `wasm.route_from_segments` |
| Foot routing over trails | `RouteProfile::Foot`, `wasm.route_trails` |
| Road name, ref, class, one-way, speed limit, lanes, surface | `RoadSegment`, per segment |
| Junctions and their control type | `decode_road_block` v2 table, `{ST}.signals.ptiles` |
| EV chargers, 51 states, with power/connector/network | `{ST}.ev_v1.ptiles`, `core::ev` |
| Fuel stations | business layer, categories `11` (`Travel and Transportation > Fuel Station`) and `18` (`gas_station`) |
| Food, coffee, lodging, parking, pharmacy | business layer, same category table |
| Address search, forward and reverse | `core::locate`, `match_addresses` |
| Charge-stop planning against a range | `core::plan_charge_stops` |
| Speed smoothing and movement classification | `motion` crate, `MovementTracker` |
| Real GPS traces for replay | `test-fixtures/gpx/`, 6 traces |
| Range-request reads, ETag-keyed cache | `LayerReader`, Cache API |

What is missing. This list was longer in the first draft of this document, and
three of its five entries were wrong: they described work that the existing
primitives already did. `point_to_linestring_distance_m` reports which segment
a point snapped to and how far off it was, which *is* snapping and *is*
off-route detection; and a turn is a bearing change along a polyline that the
router already returns. Those are now built (`core::nav`, below). What is
actually left:

1. **Long routes fail.** The corridor router finds no path beyond roughly
   180 km — Nashville to Chattanooga returns nothing, with no EV or trails
   involved. A navigator that cannot cross a state is not a navigator. This is
   the real blocker.
2. **Position is one-shot.** The GPS button calls `getCurrentPosition` once.
   Navigation needs `watchPosition` feeding the navigator, and
   `motion::MovementTracker` smoothing the speed.
3. **Rerouting is a policy, not a primitive.** `off_route` is answered per fix;
   deciding *when* that means re-route (consecutive fixes, debounce, from the
   snapped position or the raw one) is page-side work.
4. **There is no driving UI.**

### Already built: `core::nav`

- `turn_queue(path, roads, radius)` → `Depart`, every manoeuvre, `Arrive`.
  Bearing changes measured across a 25 m window, so a curve is not a sequence
  of turns; consecutive same-direction turns within 20 m merge into one
  junction; each is named after the road it turns *onto*, sampled 15 m past
  the corner, preferring a named road over a nearer unnamed stub.
- **Naming is lazy by default.** Every turn carries the probe point naming
  needs, so a route can be built with no roads at all and each turn named as
  it comes up: read the one cell holding the probe, decode it, call
  `name_turn`. One block per turn — almost always already cached, being a cell
  the route drives through — instead of holding a corridor's roads in memory
  for the length of a drive. Name before the first announcement, or a
  manoeuvre called at 2 km and again at 200 m reads as two turns.
- `navigate(path, cum, turns, lat, lon, accuracy, last_index)` → snapped
  position, distance along and remaining, next turn and distance to it, and
  `off_route`.
- **The predicted heading.** `NavState::bearing_deg` is the bearing of the next
  60 m of route from the snapped point — not `coords.heading`, which is absent
  when stationary and noise below walking speed. It leads into a corner before
  the vehicle does, which is what a heading-up map needs and what a GPS
  heading cannot give.
- Snapping is windowed (forward 500 m, back 100 m around the last index) so a
  route that doubles back cannot snap to the wrong leg, with a full-scan
  fallback that recovers after a tunnel.
- `off_route` scales with the fix: `max(35 m, 3 × accuracy)`. A 60 m error on a
  5 m fix is a wrong turn; the same error on a 30 m urban-canyon fix is not.
- `wasm.Navigator` holds path, cumulative distances and turns on the Rust side,
  so a fix costs one small call instead of re-serialising the route at 1 Hz.

## Architecture

Two rules, both inherited from how the rest of this client is built.

**Anything that decides something lives in `core`.** Maneuver generation,
snapping, off-route detection and ETA are decisions. They go in Rust, get unit
tests that run without a browser, and reach the page through wasm. The page
decides nothing; it draws, speaks, and handles input.

**Active driving is a mode of the existing page, not a second page.** It shares
the wasm instance, the open `LayerReader`s and their warmed caches. A separate
page would re-fetch every header, dictionary and index on entry — the exact
cost this format exists to avoid — and would duplicate the layer plumbing.
Full-screen is a CSS state and a different render path, not a different app.

```
  GPS fix ──► core::navigate(fix, route)  ─┬─► snapped position, heading
                                           ├─► current step, distance to it
                                           ├─► ETA, distance remaining
                                           └─► off-route: yes/no + why
                                                    │
  page: draw, speak, and on off-route ◄─────────────┘
        ask core for a new route from here
```

## The five pieces

### 1. Maneuvers

`route_roads_with` builds a graph whose edges carry the road they came from,
walks it, and stitches the geometry. The name, class and ref of every edge are
in hand at that moment. The fix is to keep them.

**API.** `RouteResult` gains `steps: Vec<RouteStep>`:

```rust
pub struct RouteStep {
    pub kind: Maneuver,        // Depart, Continue, TurnLeft, TurnSlightRight, ...
    pub road_name: Option<String>,   // "Broadway"
    pub road_ref: Option<String>,    // "US-431"
    pub road_class: String,
    pub distance_m: f64,       // length of this step
    pub duration_s: f64,
    pub start_index: usize,    // into RouteResult::path
    pub junction: Option<JunctionKind>, // signals / stop / roundabout, when known
}
```

Adding a field to a serde struct is backward compatible for the existing
callers, which read `path` and the two totals.

**Segmentation.** A step boundary is any of:

- **A name or ref change.** "Broadway becomes 4th Avenue" is a step even with
  no turn, because that is what the sign says and what a driver looks for.
- **A bearing change above 25°** at a node where more than two edges meet. The
  degree threshold alone is wrong: a motorway curve is 40° over a kilometre and
  is not a turn. Junction degree comes from the road graph the router already
  built, so this is free.
- **A class change into or out of a link road** (`*_link`), which is how ramps
  and slip roads present.

**Naming the maneuver.** Bearing delta at the boundary, signed:

| delta | maneuver |
| --- | --- |
| < 25° | Continue (only emitted on a name change) |
| 25–60° | Slight left / right |
| 60–150° | Left / right |
| > 150° | U-turn |

Roundabouts are the one shape this cannot infer from bearings — a roundabout is
a sequence of small left turns. The `intersection_type == 4` (roundabout) flag
in the v2 road table marks the entry node; the exit is the first edge leaving
the roundabout way. Exit numbering counts the roundabout-classed edges passed.
Where the flag is absent, it degrades to a sequence of slight turns, which is
wrong-sounding but not wrong-headed. **Stated limit:** exit numbering will be
missing wherever the junction table is.

**Distance phrasing** is metric or imperial by locale, rounded the way people
speak: 1000, 800, 500, 300, 200, 100, 50 m — never "483 metres".

### 2. Position, snapping, heading

`core::snap_to_route(path, lat, lon, last_index) -> Snapped`:

```rust
pub struct Snapped {
    pub lat: f64, pub lon: f64,   // on the route
    pub offset_m: f64,            // how far the fix was from it
    pub along_m: f64,             // distance travelled along the route
    pub step_index: usize,
    pub distance_to_step_end_m: f64,
    pub bearing_deg: f64,         // of the route here, not of the fix
}
```

Searching the whole polyline per fix is O(n) on a 600-point route at 1 Hz,
which is nothing — but it also lets the snap jump backwards on a hairpin where
two legs pass within 30 m. So the search is **windowed**: start at
`last_index`, scan forward 500 m and back 100 m, and only fall back to a full
scan when the best offset in the window exceeds the off-route threshold. That
is also what makes a figure-of-eight route behave.

**Heading** comes from the route bearing when snapped and moving, not from
`coords.heading`, which is absent when stationary and noisy below walking
speed. When off-route, heading falls back to the GPS value, then to
consecutive-fix bearing.

**Speed** goes through `motion::MovementTracker`, which already smooths and
already distinguishes stationary from crawling. Raw `coords.speed` jumps to
zero under bridges and would restart the ETA every time.

### 3. Off-route and rerouting

Off-route is declared when **either**:

- `offset_m > max(35, 3 × horizontal_accuracy_m)` on **three consecutive
  fixes**, or
- the driver has moved more than 60 m against the route's direction while
  `offset_m > 25`.

Three fixes, not one: a single bad fix in a parking garage is not a wrong turn,
and a navigator that reroutes on noise is worse than one that waits two
seconds. Scaling by accuracy is what stops a 60 m urban-canyon fix from
declaring an error on a straight road.

**Rerouting** is from the last *good* snapped position, not the raw fix, unless
off-route has been true for more than 15 s — by then the raw fix is the truth.
It keeps the destination and any remaining waypoints, reuses the corridor cache
(the cells are almost always already warm), and is debounced to one attempt
per 10 s. On failure it says so and keeps guiding to the old route, because a
stale line is more useful than a blank screen.

### 4. The 180 km ceiling

This is a prerequisite, not a nice-to-have. Present behaviour: routes up to
~103 km succeed, 180 km returns nothing. The cause is the corridor — beyond
100 km it samples one cell every 12 km at width 1, and an arterial-only middle
that is too thin to connect.

`{ST}.highways_v2.ptiles` is published and the demo has never used it. It is
the national road spine: a graph small enough to hold for a whole state,
already filtered to the classes a long route uses.

**Plan:** for legs over ~80 km, route in three parts — local roads from the
origin to the nearest highway node, the highway layer across the middle, local
roads at the destination. This is the standard hierarchical routing shape and
it makes the middle cost bounded by highway density rather than by cell count.
Needs its own investigation; sized in M1 rather than assumed.

### 5. The driving UI

Full-screen, one thumb, sunlight, moving vehicle. Every number is either large
or absent.

```
┌─────────────────────────────────────────┐
│  ⬅  In 400 m                            │   maneuver banner
│     Turn left onto Broadway             │   (largest type on screen)
├─────────────────────────────────────────┤
│                                         │
│                                         │
│              map, rotated               │   heading-up, snapped
│           to heading, 3D-ish            │   position pinned at 1/3
│                                         │
│              ▲ you                      │
│                                         │
├──────┬──────┬──────┬──────┬─────────────┤
│  ⛽  │  ⚡  │  🍔  │  🅿  │  ✕ End      │   POI strip
├──────┴──────┴──────┴──────┴─────────────┤
│  18:42 arrival · 34 min · 41 km    ⬆︎    │   ETA bar (tap = expand)
└─────────────────────────────────────────┘
```

- **Maneuver banner.** Direction arrow, distance, road name. Turns amber inside
  200 m, green when the maneuver completes. Second-next maneuver appears as a
  small "then ↰" when it is within 400 m of the first, which is the case that
  actually catches people out.
- **Map.** Heading-up, position at the lower third so most of the screen is
  where you are going. Route drawn thick; the traversed part dimmed. Only the
  layers a driver needs: roads, water, the route, and whichever POI class is
  active. Buildings off by default — they cost the most and mean the least at
  60 mph.
- **POI strip.** Four icons, tap to toggle. Fuel and EV are the two the
  requester named and get the left-most slots.
- **ETA bar.** Arrival clock time first, because that is the number people
  actually want; duration and distance after. Tap expands to the step list.
- **Night mode** follows `prefers-color-scheme`, with a manual override that
  sticks. Amber-on-black at night, not white-on-grey.
- **Wake lock** via the Screen Wake Lock API while navigating, released on
  exit. Without it the screen sleeps mid-route, which alone makes the mode
  unusable.
- **Voice** through `SpeechSynthesis`: announcements at 2 km, 800 m, 200 m and
  at the maneuver, scaled by speed (at 25 mph the 2 km call is noise, at 70 mph
  the 200 m call is too late). Mute toggle, remembered.

### POI quick lookups and adding a stop

The interaction the requester asked for, in full:

1. Tap ⛽ or ⚡. The page loads that layer for the cells the *remaining* route
   passes through — not the viewport, since what matters is what is ahead.
2. Results draw as markers and as a sorted sheet: distance **along the route**,
   not straight-line, plus the detour each one costs. A station 200 m away
   across a divided highway is a 4 km detour and must not be listed second.
3. Fuel comes from the business layer by category (`11`, `18`), EV from
   `ev_v1`. EV rows show power, connectors and network; a filter for
   fast-charge-only uses `is_fast_connector`, which core already owns.
4. Tap one → **"Add stop"** shows `+7 min · +3.2 km` *before* committing.
   Accepting re-routes as origin → stop → destination and returns to guidance.
5. With an EV range set, `plan_charge_stops` already produces the mandatory
   stops; those appear pinned in the sheet with the leg distance to each.

**Icons** are inline SVG in a sprite, monochrome, sized 28 px with a 44 px
touch target: fuel pump, charging plug, fork-and-knife, parking P, plus the
maneuver arrow set (8 directions, U-turn, roundabout, depart, arrive). No icon
font, no external requests — the page has a strict no-CDN posture and offline
is the point.

## What this will not do

Stated plainly, because a navigator that pretends is worse than one that
declines:

- **No live traffic.** Nothing in these files carries it. ETAs are free-flow
  from speed limits and class defaults, and will be optimistic in rush hour.
- **No lane guidance.** `lanes` is a count, not a turn-lane map. "Use the
  right two lanes" is not derivable.
- **No live charger or fuel availability, and no prices.** OSM says a charger
  exists, not whether it works or is occupied.
- **No incidents, closures or seasonal roads.**
- **No speed-limit enforcement claims.** The `camera` layer is surveillance and
  ALPR, not speed enforcement, and must not be labelled as such.
- **Address search stays corridor-scoped.** There is no national name index
  published, so destination entry is by address, map tap, or business name
  within the loaded region.

## Milestones

Each ends in something demonstrable and a test that fails if it breaks.

**M1 — Turn queue and snapping.** *Done.* `core::nav` and `wasm.Navigator`,
13 unit tests. Verified from JS: a 2 km L-shaped route yields
Depart/Left/Arrive named Broadway → 4th Avenue, and the heading reads 17°
30 m short of the corner where a GPS heading still says 90°.

**M2 — Long routes.** Hierarchical routing over `highways_v2` for legs beyond
80 km. Test: Nashville → Chattanooga routes at all, and its turn queue names
I-24 and its exits. *This is now the milestone that decides the rest — the
rest of navigation works on routes that exist.*

**M3 — Live position and reroute.** `watchPosition` into `Navigator.update`,
motion-smoothed speed, reroute policy on top of the per-fix `off_route`. Test:
replay `tn-middle-tennessee-3605997.gpx` and assert along-route distance is
monotonic and the turn index never goes backwards; splice in a wrong turn and
assert exactly one reroute fires within 3 fixes.

**M4 — The screen.** Full-screen mode, banner, heading-up map, ETA bar, wake
lock, night mode, voice. Test: the existing chromium harness drives a simulated
drive and asserts the banner text changes at the right distances.

**M5 — POI and stops.** Icon strip, along-route search, detour cost, add-stop
re-route, EV filters. Test: with a fuel search on a known corridor, assert
results are ordered by detour and that adding one changes the route length by
the amount the sheet promised.

## Testing

The harness pattern already exists and it is the reason the last three features
shipped working: `web-demo/test/route_check.py` drives the real page in
chromium against live tiles and asserts things a stub cannot fake.

Navigation adds one capability: **simulated drives**. A GPX trace is fed to the
page as synthetic `watchPosition` events at configurable speed, which makes
every claim above testable without a car — banner timing, reroute counts, ETA
convergence, voice call points. The traces in `test-fixtures/gpx/` are real
OSM traces and are already used this way by the motion crate.

The claims worth asserting, because each has an easy false pass:

- Along-route distance is monotonic across a whole trace (a snapper that jumps
  backwards passes any single-fix test).
- Exactly one reroute per wrong turn (not zero, not eleven).
- The step list's summed `distance_m` equals the route's total within 1 m.
- Announcements fire once each, in order, at the right distances for the speed.

## Open questions

1. **Roundabout exit numbering** depends on the junction table being present
   where roundabouts are. Coverage is unmeasured; M1 should measure it before
   the UI promises "take the second exit".
2. **Hierarchical routing needs a join rule** — how a local route meets the
   highway graph when the nearest highway node is 20 km away. Standard answer
   is an access-node search; sizing is part of M1.
3. **Does `highways_v2` carry names and refs** to the standard the banner
   needs? Unverified. If it carries geometry only, the middle of a long route
   will be namable only from the roads layer, which defeats part of the point.
4. **Voice on iOS Safari** requires a user gesture to unlock speech; the
   "Start" button is the natural place, but it needs testing on a real device.
5. **Battery.** Continuous GPS, wake lock and 1 Hz rendering is the worst case
   for a phone. Worth measuring in M4 and possibly dropping the render to 4 Hz
   between maneuvers.
