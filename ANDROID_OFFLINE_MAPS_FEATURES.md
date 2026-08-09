# Android-Only Offline Maps + Trails Product on PTiles

This document scopes a credible, Android-only, offline-first competitor to the
core navigation and discovery features of Google Maps plus the trail features
of AllTrails, with PTiles as its map/search/routing data substrate.

“Only PTiles” means all installed map, search, routing, trail, terrain, POI,
address, administrative, and snapshot data is distributed as independently
versioned PTiles layers or sidecars. The app can still use Android GNSS,
accelerometer and compass sensors, TextToSpeech, local user storage, and the
network to download signed packs. After a pack is installed, map browsing,
search, routes, navigation, trail recording, and emergency context must work in
airplane mode without a map/search/routing service.

## Status legend

- **Current** — supported by `ptiles-client` data and core logic now, although
  Android UI or UniFFI plumbing may still be required.
- **Compose** — buildable by combining current primitives with substantial app
  logic.
- **Extend** — requires a new PTiles layer, sidecar, schema, or core algorithm.

## Product principles

- Airplane mode is a normal operating mode, not a degraded afterthought.
- No account is required; searches, favorites, tracks, and inferred visits are
  local by default.
- Every answer carries coverage, precision, source, and freshness where those
  facts are known. Missing data remains unknown rather than being guessed.
- Pack installation is atomic, verifiable, resumable, and reversible.
- The renderer, decoder, search, and route engines never block the UI thread.
- Accessibility is part of the navigation model, not a final UI pass.
- Dynamic facts such as closures, fires, weather, traffic, transit, opening
  status, and charger availability must show an `as_of` time and expiry.

## What exists today

### PTiles access and runtime

- **Current** local files, in-memory files, and HTTP Range sources.
- **Current** format-version validation, index-layout detection, merged-block
  slicing, dictionary decompression, byte-range caching, and decompressed-cell
  caching.
- **Current** H3 point, ring-1 neighbor, bounded-viewport, and layer-coverage
  queries.
- **Current** Rust core, WebAssembly, JSON CLI/service, and Android-capable
  UniFFI surfaces.
- **Current** layer metadata: coverage bounds, schema version, block/feature
  counts, file size, ETag, and Last-Modified when hosted.
- **Current** bounded region prefetch and cache inspection/clearing through
  UniFFI.

### Present feature layers

- **Current** roads with geometry, name/reference, class, one-way state, speed
  limit, lanes, surface, bridge/tunnel state, and mapped intersection controls.
- **Current** trails and trailheads with name, type, surface, and SAC hiking
  scale.
- **Current** parks, water, rail lines/stations, buildings, businesses/POIs,
  traffic signals, cameras, administrative context, and EV chargers.
- **Current** business names, categories as raw indexes, contacts, brand,
  operating status, confidence/provenance, and optional name-index sidecars.
- **Current** building footprints, types, names/categories, POI links, and
  optional heights.
- **Current** country, state, county, ZIP, timezone, and administrative
  polygons/grid lookup.

### Present queries and inference

- **Current** nearest road, trail, trailhead, station, rail, intersection, and
  address helpers.
- **Current** park/water containment, polygon tests, distances, projections,
  bearings, and feature containment.
- **Current** local reverse context and ranked road/building/business candidates
  using GPS accuracy and speed.
- **Current** basic corridor-bounded A* driving and foot routing, with
  avoid-highway and avoid-intersection preferences for driving.
- **Current** trail-to-routing-segment conversion.
- **Current** movement classification from GPS, accelerometer, road context,
  traffic controls, debouncing, and significant speed-shift detection.
- **Current** explainable building-footprint indoor/outdoor estimation with
  `Indoor | Outdoor | Uncertain`, confidence, reason, boundary distance, and
  building ID exposed to Android through UniFFI.
- **Current** building viewsheds, camera direction/FOV, building occlusion, and
  camera-to-road proximity.
- **Current** simple EV reserve/range charge-stop planning over decoded
  chargers.

## Street-number-level geocoding

### Short answer

Yes. PTiles can carry street-number address points at roughly meter-scale
coordinate quantization and can support fully offline forward and reverse
geocoding. The container and spatial partitioning are suitable; the current
end-to-end Android API and search indexes are not yet a complete geocoder.

### What the current code can represent

- Address v1 records contain `{osm_id, housenumber, street}` within an H3
  resolution-7 cell but have no record coordinate. A cell is kilometers across,
  so its center must never be presented as a house location.
- Address v2 decoding reconstructs per-record coordinates from signed `i16`
  longitude/latitude offsets around a merged-block center at `1e-5` degree
  units. That is approximately 1.1 m north/south encoding resolution and is
  sufficient for rooftop or entrance points when the source data is equally
  precise.
- `AddressFile::addresses_at(lat, lon, ring)` spatially loads a local cell and
  optionally ring 1.
- `AddressFile::find_address` exact-matches a folded house number and
  substring-matches a folded street, but only within that already-known local
  area.
- `nearest_address` can select the closest positioned address within 150 m;
  `locate` combines that result with road/trail context.

### Gaps in the current checkout

- The supported-format registry accepts `PTILESA` version 1. Positioned
  address records are gated on header version 2, and the standalone v2 cell
  regression test bypasses `AddressFile::open`. Before claiming end-to-end v2,
  register the verified version and add a whole-file real or reference fixture.
- The UniFFI `AddressRecord` returned by `addresses_at` and `find_address`
  currently drops `lat` and `lon`. Android receives a label but not a routable
  destination. `PtilesStack::locate` is the current exception because it keeps
  core coordinates privately and returns one nearest positioned address.
- There is no state/national street or address text index. Forward lookup needs
  an approximate coordinate before it can know which cells to scan.
- Matching has no street abbreviation model, aliases, typo tolerance,
  transliteration, language handling, locality disambiguation, or serious
  ranking.
- House forms such as `12A`, `12-14`, `12 1/2`, ranges, units, and non-Western
  formats are not modeled structurally.
- Records lack locality/city/district components, street IDs, per-record ZIP,
  units, entrances, address ranges, source date, precision class, and explicit
  confidence.
- Encoding resolution is not source accuracy. An OSM point or parcel centroid
  does not become a verified doorway merely because it is stored precisely.

### Required geocoder work

1. Give address a distinct seven-byte magic and its own independent version.
2. Register and conformance-test positioned whole-file address v2/vNext.
3. Preserve coordinate, precision, confidence, and structured components
   through UniFFI.
4. Store a stable address ID, raw and normalized house number, `street_id`,
   locality/admin IDs, postcode, country, optional unit/entrance,
   language/script, source ID/date, and precision such as `entrance`,
   `rooftop`, `parcel`, `interpolated`, or `street`.
5. Add optional from/to, parity, and side-of-street interpolation records tied
   to routable road edges. Label interpolated results honestly.
6. Add shared street/locality string tables and alias tables.
7. Keep the H3 index for reverse geocoding, optionally at a finer address
   resolution or behind a multiresolution directory.
8. Add a forward-search sidecar: normalized token/FST/trie prefixes to posting
   lists of address/street IDs and block locations, sharded by region.
9. Add `autocomplete`, `geocode`, `reverse_geocode`, and batch APIs with bias,
   bounds, locale, filters, limits, cancellation, and explicit ranking evidence.
10. Test ambiguous streets, borders, antimeridian cells, ranges, aliases,
    normalization, corrupt postings, missing coordinates, and v1 fallback.

## Android architecture

### Application shell

- Kotlin, Jetpack Compose, coroutines, WorkManager, foreground navigation
  service, Android fused location, sensor APIs, TextToSpeech, and normal Android
  accessibility semantics.
- Rust `ptiles-core`, `ptiles-motion`, search, and routing behind UniFFI for
  `arm64-v8a`; include `x86_64` only for emulator/developer builds if desired.
- Room/SQLite may store only the app catalog, downloads, favorites, trips,
  user edits, and settings. Shipped map/search/routing content remains PTiles.
- All decoding/routing/search work runs on bounded background dispatchers with
  cancellation; no synchronous FFI call from the main thread.

### `PackManager`

- Reads a signed manifest containing coherent layer versions, hashes,
  dependencies, region coverage, byte sizes, source dates, attribution, and
  dynamic-data expiry.
- Supports resumable downloads, Wi-Fi/charging/storage policy, SHA-256 or
  stronger verification, free-space preflight, and download progress.
- Installs into an inactive A/B directory, validates every header and version,
  runs smoke queries, then atomically activates the pack.
- Retains the last-known-good version for rollback after corruption, failed
  migration, or an interrupted update.
- Supports pinned packs plus size-based LRU eviction of unused regions.

### `PtilesRepository`

- Owns open local layers, memory mapping where appropriate, metadata, and
  coverage checks.
- Maintains bounded caches for compressed ranges, decompressed cells, search
  postings, and routing graph shards.
- Coalesces concurrent requests for the same block and supports priority and
  cancellation as the viewport, route, or query changes.
- Groups trace points by H3 cell so a day of fixes decompresses each cell once.
- Exposes deterministic error classes: offline/download missing, pack absent,
  unsupported format, corrupt bytes, incomplete coverage, and cancelled work.

### Renderer

- Custom renderer or a MapLibre-compatible integration with a PTiles-backed
  source adapter.
- Selects cells and generalized geometry by viewport/zoom; decodes incrementally
  and discards stale work after camera movement.
- Implements line/polygon/point styling, geometry simplification, label
  placement/collision, road shields, building extrusion, and feature picking.
- Bundles offline style JSON, glyphs, fonts, sprites, icons, attribution, and
  dark, high-contrast, color-safe, and large-label variants.

### Search engine

- Unified local index for addresses, streets, POIs, buildings, parks, trails,
  trailheads, stations, administrative areas, and EV chargers.
- Autocomplete with locality/bounds bias, category and “near me” search,
  viewport search, and along-route search.
- Ranking combines normalized text quality, distance/bias, prominence,
  category, open status freshness, precision, and provenance.
- Every hit supplies a stable ID, display label, structured components,
  coordinate, precision/confidence, source layer/snapshot, and routable access
  point when available.

### Route engine

- Uses regional hierarchy shards for long routes and local detail graphs around
  endpoints and maneuvers.
- Separates driving, walking, hiking, running, cycling/MTB, and wheelchair
  profiles instead of treating every way as a slow road.
- Applies mode access, one-way rules, barriers, turn restrictions, conditional
  access, surfaces, grade, steps, ferries, private/seasonal state, and closures.
- Produces route alternatives, maneuver steps, sign/road names, roundabout
  exits, lane guidance, geometry, distance, time, ascent, and confidence.

### Foreground navigation service

- Consumes fused fixes plus optional accelerometer/compass evidence.
- Performs map matching, movement inference, indoor/outdoor inference, route
  progress, off-route detection, reroute, maneuver timing, and trip recording.
- Owns notification controls, TTS/audio focus, wake-lock discipline, screen-off
  operation, and low-battery modes.
- Persists a minimal resumable navigation state so process death does not lose
  the destination, route, or recorded trail.

## Data packs and updates

### Pack shapes

- **Base region:** generalized cartography, labels, admin, address/search, roads,
  buildings, water, parks, rail, and essential POIs.
- **Drive:** detailed road graph, restrictions, maneuvers, EV, and optional
  traffic/closure snapshot.
- **Outdoors:** trails, route relations, terrain/DEM, contours, hillshade,
  outdoor POIs, access, and hazard/closure snapshot.
- **Full:** Base + Drive + Outdoors for a region.
- **National skeleton:** small settlement/street/POI search and high-level
  routing hierarchy, with detailed cells fetched by installed region.
- **Journey corridor:** endpoints, a route halo, search, routing dependencies,
  and emergency context across every crossed region.

### Update rules

- A manifest pins a coherent snapshot across base files and sidecars. Never
  activate search postings against different feature IDs or routing shortcuts
  against different edges.
- Prefer content-addressed immutable shards or verifiable per-shard deltas so a
  small regional change does not replace a whole state.
- Cross-border routes pull a halo and all required graph/search dependencies;
  state borders must not become route walls.
- Show exact installed/download bytes, coverage, source/build date, attribution,
  and expiry before installation.
- Dynamic layers can expire without invalidating static geometry. The app must
  visibly disable or age dependent claims when they are stale.

## Feature inventory

### Map and discovery

- **Compose** fast offline pan, zoom, rotate, tilt, layer toggles, scale, compass,
  current location, and feature selection.
- **Extend** Google-quality zoom-generalized basemap, land cover, boundaries,
  road labels, place labels, contours, and hillshade layers.
- **Extend** separately licensed offline imagery packs; imagery cannot be
  inferred from existing vector layers.
- **Compose** rich place/building/park/trail/station/charger cards from decoded
  attributes and provenance.
- **Compose** nearby, viewport, category, and along-route browsing.
- **Extend** unified autocomplete for address, street, POI, locality, park,
  trail, trailhead, station, and charger.
- **Compose** recent searches, favorites, home/work labels, lists, and offline
  coordinate sharing.
- **Compose** point inspector showing road, address, building, business, park,
  water, rail, county, ZIP, timezone, camera, signal, and data freshness.
- **Compose** download/coverage overlay so users know exactly where offline
  search, routes, terrain, and dynamic snapshots will work.

### Driving navigation

- **Current** local/corridor route distance, duration, geometry, highway
  preference, and intersection preference.
- **Extend** graph topology with stable node/edge IDs, incidence, legal mode
  access, turn restrictions, roundabouts, barriers, conditional/seasonal
  access, ferries, and private roads.
- **Extend** regional hierarchical routing for dependable cross-state routes.
- **Compose** alternatives, multi-stop ordering, route overview, step list,
  route resume, auto-reroute, and route export.
- **Extend** avoid tolls, ferries, unpaved/private roads, low clearances, weight
  limits, and hazardous-material restrictions when the required data exists.
- **Extend** maneuver generation, spoken directions, signposts, lane guidance,
  roundabout exit counting, and destination-side arrival.
- **Compose** mapped speed-limit display and warnings with explicit unknown/stale
  states.
- **Compose** EV charger discovery and reserve/range planning; **Extend** live
  status, price, authentication, connector power, reliability, and charging-time
  optimization.
- **Extend** historical or downloaded traffic models and expiring incident
  snapshots. A permanently offline app cannot promise live traffic.
- **Compose** route preview of signals, rail crossings, bridges, tunnels,
  surfaces, parks, water, cameras, chargers, and coverage gaps.

### Walking, cycling, and accessibility

- **Current** basic foot routing over eligible streets and converted trails.
- **Extend** sidewalks, crossings, curb ramps, pedestrian signals, entrances,
  gates, barriers, and walking-specific access.
- **Extend** cycling/MTB access, infrastructure, stress, surface, incline,
  dismount, and direction rules.
- **Extend** wheelchair width, curb, incline, surface smoothness, steps,
  elevator, door, and accessible-entrance data.
- **Compose** quiet, shaded-proxy, park/water, low-intersection, low-complexity,
  or low-surveillance route comparisons with assumptions shown.
- **Compose** landmark-based instructions using named buildings, businesses,
  parks, stations, bridges, tunnels, and water.
- **Compose** large text, TalkBack semantics, high-contrast/color-safe styles,
  screen-reader route-step list, voice/haptic cues, and one-handed controls.
- **Compose** confidence/coverage warnings rather than assuming a missing curb,
  sidewalk, surface, or entrance is usable.

### Trail discovery and AllTrails-style use

- **Current** browse nearby trail fragments, trailheads, surfaces, SAC scale,
  parks, water, roads, and stations.
- **Extend** trail route relations joining fragments into named official routes
  with stable route IDs.
- **Extend** DEM/elevation, contours, hillshade, grade, ascent/descent, elevation
  profile, climb segments, and terrain-aware ETA.
- **Extend** official distance, difficulty, technical exposure, permitted
  activities, dogs/access rules, fees, seasonality, and source/freshness.
- **Extend** trail junctions, bridges, fords, gates, campsites, shelters,
  toilets, drinking water, viewpoints, ranger stations, and parking.
- **Compose** loop, out-and-back, and point-to-point discovery with filters for
  activity, distance, gain, difficulty, surface, season, and access.
- **Compose** park/trail detail pages with map, description, profile, waypoints,
  access caveats, downloaded hazard snapshot, and pack completeness.
- **Compose** GPX import/export, route reverse, return-to-start, breadcrumbs,
  pause/resume, wrong-way/off-route alert, progress, remaining distance/gain,
  and local trip journal.
- **Compose** compass/bearing, coordinate format conversion, distance/area
  measurement, and sunrise/sunset calculation.
- **Compose** personal stats and private notes/photos stored locally.
- **Extend** licensed reviews/photos/popularity if desired. They create
  moderation, freshness, attribution, and storage obligations and are not
  available from current PTiles layers.

### Search and places

- **Current** cell-local address and nearby business lookup.
- **Current** optional business name-index search, although broad publication
  of the sidecar is required for acceptable statewide performance.
- **Extend** common normalized category vocabulary instead of unresolved raw
  business category indexes.
- **Extend** opening hours, services, cuisine, accessibility, entrance,
  parking, payment, and source-date fields.
- **Extend** text indexes with aliases, abbreviations, typo tolerance,
  transliteration, multilingual names, locality filters, and ranking.
- **Compose** “open now” only when hours and timezone are known and the data is
  fresh enough; otherwise show the raw status limitation.
- **Compose** destination access point selection so navigation ends at an
  entrance/parking/trailhead rather than an arbitrary centroid.

### Recording, context, and indoor/outdoor inference

- **Current** walking/running/driving/stationary classification and significant
  behavior changes.
- **Current** indoor/outdoor map heuristic:
  - a valid fix inside an enclosed footprint is `Indoor` evidence;
  - roof, carport, and canopy containment is `Uncertain`;
  - an accuracy circle clear of nearby buildings is `Outdoor` only when
    building coverage is complete;
  - poor accuracy, edge overlap, invalid fixes, and missing coverage are
    `Uncertain`;
  - the result includes confidence, reason, building ID, and boundary depth or
    clearance.
- **Compose** temporal hysteresis, entry/exit dwell, repeated containment,
  road/trail/park context, movement state, business containment, and GNSS
  degradation after an entrance transition.
- **Extend** optional generic Android evidence such as satellite visibility,
  Wi-Fi environment, barometer, or ambient light, carefully permissioned and
  never required.
- **Extend** indoor maps with venue/building IDs, entrances, levels, corridors,
  rooms, stairs, elevators, escalators, accessibility, and connectors to the
  outdoor graph. The heuristic alone does not provide indoor navigation.
- **Compose** automatic arrivals/departures, battery-aware sensor cadence,
  indoor-aware GPS confidence, private visit inference, and trace annotation.

### Safety and field reliability

- **Compose** one-tap emergency card with exact coordinate, elevation when
  available, admin area, nearest trail/road/trailhead, route position, pack
  coverage, and data age.
- **Compose** breadcrumb backtrack and return-to-start that work without route
  graph availability.
- **Compose** missed-check-in timer, low-battery warning, storage warning, and
  “offline pack incomplete” trip preflight.
- **Compose** emergency contact/SMS share through Android when connectivity
  exists, with no promise that an offline message was delivered.
- **Extend** timestamped trail closures, fires, floods, avalanche, construction,
  and weather snapshots with source, confidence, and expiry.
- **Compose** route/trail deviation, wrong-way, stale-hazard, and pack-boundary
  warnings.
- **Compose** crash-safe trip recording with periodic atomic checkpoints.

### Privacy

- **Compose** no-account mode as the default and no analytics by default.
- **Compose** all search, navigation, tracks, favorites, and place inference on
  device.
- **Compose** granular retention, delete/export controls, incognito trips,
  privacy zones, and redacted GPX/shares.
- **Compose** encrypted local vault and explicit encrypted backup/export.
- **Compose** per-permission explanations and useful behavior when sensors or
  permissions are unavailable.
- **Compose** share derived place/route summaries instead of raw histories when
  the user chooses.

### Transit and other later parity

- **Extend** transit stops, routes, shapes, schedules, calendars, transfers,
  fares, accessibility, and offline trip planning.
- **Extend** expiring realtime transit snapshots; true realtime requires a
  refresh path.
- **Extend** indoor venues and routing.
- **Extend** richer reviews, photos, editorial content, and moderated user
  reports if the product chooses that business model.

## Required PTiles extensions

### Routing vNext

- Stable node and edge IDs, road-to-node incidence, true junction degree, and
  cross-cell/cross-region identity.
- Per-mode access, turn restrictions and costs, roundabouts, barriers/gates,
  ferries, conditional/seasonal/private access, sidewalks/crossings/curbs,
  bicycle attributes, and elevation-aware edges.
- Maneuver/sign/lane metadata and a regional contraction-hierarchy or
  multilevel-partition sidecar.
- Current coordinate-merging graph construction, corridor budget, and 250,000
  node cap are useful for bounded routes but not nationwide navigation parity.

### Search vNext

- Unified indexed search over POIs, addresses, streets, buildings, parks,
  trails, trailheads, stations, chargers, and admin areas.
- Normalized tokens, aliases, language/script, categories, locality/postcode
  filters, text/distance/prominence ranking, and stable posting IDs.
- Coherent version dependency between postings and feature layers.

### Cartography vNext

- Zoom-generalized geometry and labels, settlement/place decoder, boundaries,
  land cover, terrain, contours, and hillshade sources.
- Offline styles, fonts, glyphs, sprites, shields, icons, and attribution.

### Outdoors vNext

- Elevation/DEM, trail relations, route IDs, grades, access/seasonality,
  supported activities, technical/difficulty metadata, outdoor facilities,
  hazards, and provenance/freshness.

### Dynamic overlay protocol

- Small timestamped, expiring updates for closures, hazards, construction,
  traffic/weather snapshots, business/charger status, and transit realtime.
- Base geometry remains immutable and usable after the overlay expires; the
  app simply stops claiming the dynamic fact is current.

### Pack manifest

- Region/release ID, compatible client range, every layer/sidecar magic and
  version, hashes, sizes, coverage, dependencies, generated/source dates,
  attribution/license, expiry, and signature.
- Signed atomic manifests are the difference between “some files downloaded”
  and a coherent offline map release.

## Delivery phases

### Phase 0 — substrate and honesty

- Register and conformance-test positioned whole-file address v2/vNext.
- Preserve address coordinates and precision through UniFFI.
- Ship the indoor/outdoor heuristic and Android integration tests.
- Define signed coherent manifests and atomic A/B pack installation.
- Establish renderer, cold-start, corruption, fuzz, memory, storage, and battery
  baselines.
- Put coverage/precision/freshness in every result model and UI prototype.

### MVP — useful offline regional app

- Install/update/remove a region and pass an airplane-mode cold-start test.
- Render an attractive regional basemap and inspect features.
- Search installed POIs plus locally biased addresses.
- Reverse-context “where am I?” and emergency card.
- Browse parks, trails, trailheads, surfaces, and SAC difficulty.
- Basic driving and walking corridor routes, route line, ETA/distance, simple
  spoken steps, reroute, and arrival.
- Record, pause/resume, and export GPX; breadcrumbs, off-route, and backtrack.
- Favorites, recents, privacy controls, attribution, and explicit limitation
  UI for search/routing/freshness.

### Phase 2 — dependable navigation

- Structured address vNext and regional forward-search sidecars.
- Routing topology, restrictions, hierarchy, maneuvers, lanes/roundabouts,
  cross-region routes, map matching, and robust reroute.
- Bike and wheelchair data/profiles, destination access points, and stronger
  accessibility UI.
- Delta/sharded atomic updates and bounded production caches.

### Phase 3 — AllTrails-grade outdoors

- Elevation, topo/contours/hillshade, route-relation trail catalog, ascent and
  elevation profiles, grades, difficulty, activity/access/season metadata, and
  outdoor facilities.
- Park bundles, trail filters, return-to-start, remaining gain, wrong-way and
  deviation alerts, check-in timer, and expiring closure/hazard snapshots.

### Phase 4 — broader Maps parity

- Transit packs and offline trip planning.
- Indoor venue packs and indoor/outdoor graph connections.
- Richer POIs, imagery/media/reviews where licensed, and optional explicit
  user-content synchronization.
- Downloaded historical/snapshot traffic and optional refresh paths for facts
  that cannot truthfully remain live offline.

## Acceptance scenarios

The product is not “offline” until these work with radios disabled:

1. Cold start after process death, search an installed street-number
   destination, route there, miss a turn, reroute, hear maneuvers, and arrive.
2. Cross an installed regional boundary without the route or search suddenly
   ending at the state line.
3. Open a downloaded park, choose a trail, inspect surface/difficulty/profile,
   record it, trigger an off-route warning, backtrack, and export GPX.
4. Ask “where am I?” with poor GNSS and receive an explainable uncertain answer
   rather than a fabricated address, building, or indoor/outdoor state.
5. Interrupt and corrupt an update; the old pack remains usable and the new
   pack never activates.
6. Approach a pack boundary or expired hazard snapshot and receive a visible
   coverage/freshness warning before depending on it.
7. Navigate with TalkBack, large text, high contrast, the screen off, and
   denied optional sensor permissions.
8. Delete all trips/search history and verify no account or remote service
   retained them.

