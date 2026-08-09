# Awesome Projects Enabled by `ptiles-client`

`ptiles-client` is more than a map-tile decoder. It is a compact geospatial
query engine that can read local files, memory buffers, or small HTTP byte
ranges and turn them into buildings, roads, businesses, trails, water, parks,
rail, traffic controls, cameras, addresses, and administrative context. It can
run in native Rust, browsers through WebAssembly, mobile apps through UniFFI,
command-line pipelines, and constrained `no_std + alloc` environments.

This is a deliberately broad inventory of projects that could be built with
that foundation. It includes direct applications, reusable infrastructure,
research tools, civic projects, games, art, and speculative extensions.

## Feasibility legend

- **Ready** — the core data and query primitives already exist. The remaining
  work is primarily product code, presentation, and deployment.
- **Compose** — combines existing primitives but needs meaningful new
  application logic, indexing, caching, analysis, or algorithms.
- **Extend** — needs additional PTiles data, an encoder or format revision, or
  a substantial new capability. It is still a natural project for this
  ecosystem.

The tags describe technical proximity, not production readiness. Any public
product would still need to address data freshness, incomplete source tags,
accessibility, privacy, licensing, and validation for its use case.

## What the library makes available

- H3 resolution-7 point, neighbor-ring, and bounded-viewport cell queries.
- Efficient local, in-memory, and HTTP Range access with header, dictionary,
  index, and byte-range caching.
- Detection of the published 19-byte and 38-byte index layouts, all known
  offset bases, and merged-block cell slicing.
- Building footprints, types, names, categories, POI links, and optional
  heights.
- Road geometry, class, name, reference, one-way state, speed limit, lane
  count, surface, bridge/tunnel state, and intersection controls.
- Businesses and POIs with names, categories, contact details, addresses,
  brands, operating status, provenance, and optional indexed name search.
- Park, water, rail, trail, trailhead, station, traffic-control, surveillance
  camera, administrative, and address records.
- Nearest-feature, containment, distance, projection, point-in-polygon,
  reverse-location, local forward-address, and GPS-candidate scoring helpers.
- Corridor-bounded A* driving routes, estimated duration, and preferences to
  avoid highways or intersection-heavy paths.
- Building-height estimation, line-of-sight/viewshed calculations, camera aim
  and occlusion checks, and camera-to-road proximity.
- GPS/accelerometer movement classification, debouncing, road-context priors,
  traffic-control context, and statistically significant speed-shift detection.
- Rust, WebAssembly, JSON CLI/service, and Swift/Kotlin/Python-friendly UniFFI
  integration surfaces.

## 1. Maps and visual explorers

- **Ready — Pocket vector map.** Render nearby roads, buildings, water, parks,
  rail, trails, signals, cameras, and businesses without a conventional tile
  server.
- **Ready — Layer laboratory.** Toggle layers, inspect decoded records, view
  H3 boundaries, and compare feature counts cell by cell.
- **Ready — PTiles file inspector.** Drop in a file and display its header,
  format version, coverage, detected index width, offset base, block count,
  HTTP validators, and populated cells.
- **Ready — Map-under-the-cursor panel.** Continuously explain the nearest
  road or trail, containing park or water body, nearby building, business,
  address, jurisdiction, ZIP, and timezone.
- **Ready — State atlas.** Build one downloadable atlas per state with a layer
  catalog, coverage bounds, metadata, and offline browsing.
- **Ready — Building explorer.** Click a footprint to show its OSM identity,
  type, name, category, height provenance, and associated POIs.
- **Ready — Street anatomy viewer.** Inspect class, lanes, speed, surface,
  one-way restrictions, bridges, tunnels, and nearby controls along a road.
- **Ready — Trail and greenway viewer.** Style natural trails separately from
  developed cycleways and footways and identify trailheads.
- **Ready — Rail explorer.** Browse named tracks, rail types, stations, and
  halts and inspect the nearest rail feature.
- **Ready — Water and park explorer.** Show named polygons and lines, identify
  what contains a point, and report distance to the nearest boundary.
- **Ready — Surveillance infrastructure map.** Display cameras and ALPRs with
  operator, placement, direction, field of view, and uncertainty.
- **Ready — Traffic-control atlas.** Map signals, crossing signals, stops,
  give-way controls, railway signals, and road-block intersection tables.
- **Ready — Administrative boundary explorer.** Resolve a point to country,
  state, county, ZIP, and timezone and optionally draw decoded polygons.
- **Compose — Time-travel layer comparison.** Store snapshots and visualize
  added, removed, or changed features between PTiles builds.
- **Compose — Side-by-side decoder workbench.** Compare Rust/Wasm, legacy
  JavaScript, and reference-decoder output field by field.
- **Compose — Semantic zoom renderer.** Select layer density, label priority,
  and generalization based on zoom and cell occupancy.
- **Compose — Map style playground.** Derive colors, strokes, extrusions, and
  labels from every decoded attribute and export shareable styles.
- **Compose — Embeddable mini-map component.** Package the reader, range
  loading, cell cache, renderer, and point inspector as a reusable web widget.
- **Compose — Desktop GIS plug-in.** Expose PTiles as a lazy data source for
  QGIS or another GIS without first converting entire state files.
- **Compose — Terminal map browser.** Render coarse nearby context and feature
  summaries in a TUI for SSH and low-bandwidth environments.

## 2. Offline, edge, and resilient geography

- **Ready — Offline-first mobile map.** Bundle selected state files and answer
  nearby queries with no server, account, or network connection.
- **Ready — Progressive web app atlas.** Cache headers, dictionaries, indexes,
  and visited ranges using ETags so repeat visits are nearly network-free.
- **Ready — Field laptop toolkit.** Query local PTiles files from the CLI and
  pipe JSON into survey, journalism, or incident-response scripts.
- **Ready — Air-gapped site context service.** Serve newline-delimited JSON
  location queries from a local directory inside a restricted network.
- **Ready — Bring-your-own-map SDK.** Let users point an application at local
  or remote `.ptiles` files instead of depending on one hosted provider.
- **Ready — Regional download manager.** Use layer bounds, versions, byte
  lengths, ETags, and Last-Modified values to plan and verify offline packs.
- **Compose — Journey packager.** Prefetch only cells intersecting a planned
  route plus a safety corridor rather than downloading a whole state.
- **Compose — Emergency-area packager.** Turn an incident bounding box into a
  bounded set of cells and cache the relevant layers before deployment.
- **Compose — Peer-to-peer map sharing.** Exchange content-addressed state
  files or route corridors between nearby devices when the internet is down.
- **Compose — Edge range proxy.** Coalesce, cache, and validate PTiles byte
  ranges close to users while keeping the origin as static file hosting.
- **Compose — Intermittent-network query queue.** Return cached nearby context
  immediately and refresh missing cells when connectivity reappears.
- **Compose — Storage-budget pack builder.** Choose layers and regions under a
  byte budget using file metadata and observed cell access frequency.
- **Compose — Embedded trip computer.** Use the `no_std + alloc` decoder core
  for local context on dedicated navigation hardware.
- **Compose — Privacy-preserving local geocoder.** Keep coordinates and query
  history on-device while resolving roads, addresses, jurisdictions, and POIs.
- **Compose — Static-host geospatial backend.** Provide useful spatial queries
  from CDN-hosted immutable files, with no spatial database or application
  server.
- **Extend — Signed map packs.** Add manifests and signatures so devices can
  verify origin, version, and integrity before trusting offline data.

## 3. Navigation and route planning

- **Ready — Local driving router.** Calculate a corridor-bounded A* route with
  distance, estimated duration, and a displayable path.
- **Ready — Avoid-highways router.** Prefer arterials and local roads while
  retaining highways when they are the only reasonable connection.
- **Ready — Low-intersection router.** Prefer routes with fewer junction
  penalties for simpler driving and fewer stops.
- **Ready — Route-on-a-map demo.** Select endpoints, fetch only corridor cells,
  calculate a route, and animate the result entirely in the browser.
- **Compose — Offline turn preview.** Break a route into named roads and show
  speed, lanes, surface, bridge/tunnel, and controls along each segment.
- **Compose — Route context ribbon.** Summarize upcoming parks, waterways,
  rail crossings, tunnels, bridges, signals, cameras, and business landmarks.
- **Compose — Scenic route scorer.** Prefer paths near water and parks and
  penalize monotonous high-speed roads.
- **Compose — Surface-aware route scorer.** Avoid or prefer particular road
  surfaces for motorcycles, vintage vehicles, delivery carts, or weather.
- **Compose — Low-surveillance route explorer.** Compare candidate routes by
  cameras near the road and cameras estimated to see the path, with explicit
  uncertainty rather than guarantees.
- **Compose — Signal-light route estimator.** Estimate delay from mapped
  traffic controls and compare a short signal-heavy route with a longer
  free-flowing one.
- **Compose — Bridge and tunnel itinerary.** Find routes that seek or avoid
  bridges and tunnels for tourism, vehicle restrictions, or user comfort.
- **Compose — One-way validation tool.** Trace a route and flag suspicious
  direction changes against decoded one-way attributes.
- **Compose — Route coverage auditor.** Identify corridor cells with missing
  road blocks before an offline trip begins.
- **Compose — Multi-stop local route planner.** Combine pairwise routes with a
  stop-order heuristic for errands or service calls.
- **Compose — Landmark navigation.** Generate instructions using named
  buildings, parks, stations, water, and businesses instead of distances only.
- **Compose — Route replay debugger.** Overlay a GPS trace, candidate snaps,
  chosen route, speed transitions, and traffic controls to diagnose bad
  navigation behavior.
- **Compose — Safe route-corridor handoff.** Export just the geometry and
  contextual records needed by another app or device.
- **Extend — Walking router.** Build pedestrian topology from roads, footways,
  trails, crossings, and access rules.
- **Extend — Cycling router.** Add bicycle access, grade, stress, and dedicated
  infrastructure attributes and a cycling cost profile.
- **Extend — Wheelchair router.** Add curb cuts, sidewalk continuity, incline,
  surface smoothness, steps, elevator status, and accessible entrances.
- **Extend — True turn-by-turn directions.** Add stable road-node topology,
  turn restrictions, maneuver generation, and roundabout semantics.
- **Extend — Long-distance hierarchical routing.** Add a routing sidecar or
  contraction hierarchy that crosses state and corridor boundaries cheaply.
- **Extend — Live-traffic routing.** Overlay expiring speed and closure data on
  immutable road geometry.

## 4. Position understanding and GPS enrichment

- **Ready — “Where am I?” resolver.** Return the nearest/on-road or trail,
  nearest address, containing park or water, nearby station, and jurisdiction.
- **Ready — GPS candidate ranker.** Rank roads, buildings, and businesses
  using distance, reported accuracy, speed, and configurable priors without
  mutating the original fix.
- **Ready — Road snap preview.** Show the nearest projected point and distance
  without forcing a possibly incorrect snap.
- **Ready — Trail snap preview.** Distinguish a user on a trail from one merely
  near it and retain the original accuracy context.
- **Ready — Intersection context detector.** Report the nearest mapped control
  and whether it is a signal, stop, give-way, roundabout, or generic junction.
- **Ready — Address-nearby resolver.** Find the closest sufficiently precise
  address record inside a configurable threshold.
- **Ready — Jurisdiction annotator.** Add country, state, county, ZIP, timezone,
  and boundary flags to every location event.
- **Compose — Batched trace enricher.** Group thousands of points by H3 cell,
  decompress each cell once, and attach map context to an entire day.
- **Compose — Probabilistic map matcher.** Extend per-fix candidate scores with
  transition probabilities across a trace.
- **Compose — Indoor/outdoor heuristic.** Combine stationary fixes, building
  containment, building type, businesses, GPS accuracy, and speed.
- **Compose — Arrival/departure detector.** Detect dwell inside a building or
  near a business and separate it from waiting at a mapped traffic control.
- **Compose — Road-versus-trail disambiguator.** Use movement type, surface,
  route continuity, and previous candidates when parallel ways are close.
- **Compose — GPS drift explainer.** Show accuracy circles, geometric
  distances, emission scores, and why a different candidate won.
- **Compose — Boundary-crossing detector.** Emit events when a trace changes
  county, ZIP, state, timezone, park, or water containment.
- **Compose — Map-aware geofence engine.** Define geofences by actual building,
  park, water, road, or administrative geometry rather than circles alone.
- **Compose — Privacy-zone detector.** Keep private-place recognition on-device
  and redact or coarsen exported traces near selected buildings.
- **Compose — Context-aware sampling controller.** Reduce GPS frequency when
  stationary in a known place and increase it around significant movement or
  map-context changes.
- **Compose — Location confidence UI.** Present “likely on Main Street,”
  “possibly inside the library,” or “uncertain between road and trail” from
  actual ranked candidates.

## 5. Geocoding, search, and local discovery

- **Ready — Local reverse geocoder.** Resolve a coordinate to a nearby address,
  way, jurisdiction, ZIP, and timezone without sending it to a service.
- **Ready — Cell-local forward address search.** Match house number and street
  among already-loaded address records for a viewport or ring.
- **Ready — Indexed business search.** Search available name-index sidecars
  with case, accent, and prefix/substring matching.
- **Ready — Nearby business browser.** List businesses in the current and
  neighboring cells with contact, brand, status, category, and provenance.
- **Ready — Building-to-business directory.** Find businesses located inside
  or nearest to a selected building footprint.
- **Compose — Offline omnibox.** Merge business names, local addresses, road
  names, parks, stations, water, buildings, and jurisdictions into one search
  experience.
- **Compose — “What is this place?” card.** Combine building identity,
  contained businesses, address, admin context, and nearby transportation.
- **Compose — Open-now local finder.** Interpret available opening-hours data
  where present and filter out closed or temporarily closed records.
- **Compose — Contact directory.** Build local phone, website, email, social,
  and brand lookup with explicit upstream provenance.
- **Compose — Category browser.** Resolve category indices and offer an offline
  taxonomy for food, retail, services, recreation, and institutions.
- **Compose — Search-along-route.** Fetch a narrow route corridor and find fuel,
  food, parks, stations, or other POIs with bounded detour cost.
- **Compose — Search inside a park or district.** Filter businesses, buildings,
  trails, and stations by polygon containment.
- **Compose — Duplicate-place auditor.** Detect spatially close business
  records with similar names but differing source IDs or attributes.
- **Compose — Closed-business verifier.** Produce review queues for records
  marked closed or temporarily closed and compare later snapshots.
- **Compose — Multistate business search service.** Query per-state indexes in
  parallel, merge rankings, and return provenance and coverage gaps.
- **Compose — Voice-query local assistant.** Turn requests such as “nearest
  trailhead” or “what county am I in?” into on-device queries.
- **Extend — Statewide address index.** Add street/name buckets so forward
  geocoding does not require knowing the local cells first.
- **Extend — Fuzzy multilingual place search.** Add token indexes,
  transliteration, alternate names, language metadata, and typo tolerance.
- **Extend — Places-layer decoder and search.** Decode the published places
  layer and integrate locality or settlement names into the omnibox.

## 6. Personal mobility and timeline applications

- **Ready — Movement-mode classifier.** Classify stationary, walking, running,
  and driving from GPS, accelerometer, road context, and debounced votes.
- **Ready — Significant-speed-change detector.** Mark statistically supported
  behavior boundaries instead of relying only on fixed thresholds.
- **Ready — Traffic-light stop recognizer.** Distinguish waiting near a signal,
  stop, or give-way control from arriving at a destination.
- **Compose — Private personal timeline.** Turn local sensor history into
  trips, stops, visits, routes, and place labels entirely on-device.
- **Compose — Automatic trip diary.** Summarize origins, destinations,
  movement modes, roads, parks, trails, businesses, and jurisdictions.
- **Compose — Commute analyzer.** Compare route, duration, stop frequency,
  signal exposure, and speed-shift patterns across days.
- **Compose — Visit inference.** Rank candidate buildings and businesses for a
  stationary cluster, carrying uncertainty into the UI.
- **Compose — Walk/run session detector.** Use cadence, speed, road/trail
  context, and significant shifts to cut sessions automatically.
- **Compose — Drive segmentation.** Identify ignition-like starts/stops,
  traffic-control waits, intermediate visits, and long GPS gaps.
- **Compose — Multimodal journey splitter.** Detect transitions among walking,
  driving, and rail-adjacent segments using speed, stations, tracks, and map
  context.
- **Compose — Battery-aware capture policy.** Change sensor cadence based on
  stable movement state, accuracy, and expected map transitions.
- **Compose — GPX labeling studio.** Overlay sensor windows, movement votes,
  statistical shifts, nearest roads, controls, and editable truth labels.
- **Compose — Timeline correction assistant.** Offer ranked alternative roads,
  buildings, or businesses when the inferred visit looks wrong.
- **Compose — Daily geography digest.** Report distance by mode, parks visited,
  counties crossed, trails used, and notable route context.
- **Compose — Carbon estimate journal.** Combine classified driving distance
  and route duration with user-selected vehicle factors.
- **Compose — Routine-change detector.** Compare repeated trace corridors and
  visit patterns without uploading raw location history.
- **Compose — Family or team field log.** Synchronize only derived events and
  selected map references rather than continuous raw coordinates.
- **Extend — Transit-mode classifier.** Add schedules, routes, stop sequences,
  and stronger rail/bus evidence to distinguish transit from driving.

## 7. Urban planning and infrastructure analysis

- **Ready — Road inventory dashboard.** Count and map road class, surface,
  lanes, speed limits, one-way segments, bridges, and tunnels by region.
- **Ready — Building stock inventory.** Summarize footprint, type, name,
  category, and available height coverage by cell or jurisdiction.
- **Ready — Traffic-control inventory.** Measure density and types of signals,
  stops, give-way controls, roundabouts, and crossings.
- **Ready — Camera inventory.** Count device type, operator, placement, aim
  metadata, and road proximity with uncertainty clearly surfaced.
- **Compose — Intersection-delay proxy.** Combine controls, road class, route
  structure, and observed traces to estimate recurring friction.
- **Compose — Road completeness audit.** Find segments missing names, surfaces,
  speed limits, lanes, or one-way information.
- **Compose — Building-height coverage audit.** Map where measured heights are
  present, absent, or clamped and avoid treating missing data as zero.
- **Compose — Mixed-use intensity proxy.** Combine building types, businesses,
  roads, stations, parks, and footprint density at H3-cell scale.
- **Compose — Park-access analysis.** Estimate population-independent spatial
  access using road/trail distance, entrances, and neighborhood coverage.
- **Compose — Waterfront-access analysis.** Identify roads, trails, parks, and
  public places near water geometries.
- **Compose — Station-area study.** Inventory roads, buildings, businesses,
  parks, trails, and cameras around rail stations.
- **Compose — Last-mile connectivity audit.** Measure how station and trailhead
  points connect to nearby roads, paths, businesses, and destinations.
- **Compose — Speed-policy map.** Compare posted speed attributes across road
  class and administrative regions and flag suspicious outliers.
- **Compose — Surface equity map.** Compare paved/unpaved or unknown road and
  trail surfaces across neighborhoods.
- **Compose — Bridge/tunnel dependency analysis.** Find corridors and local
  routes whose connectivity relies on a small number of crossings.
- **Compose — Block permeability proxy.** Use road geometry, one-way state,
  building footprints, and route detours to estimate local connectivity.
- **Compose — Amenity desert finder.** Identify cells far from selected
  business categories, parks, trailheads, or stations.
- **Compose — Infrastructure-change dashboard.** Diff periodic snapshots to
  track new roads, buildings, controls, cameras, parks, or businesses.
- **Extend — True junction analytics.** Add road-to-node identities and degree
  so planners can distinguish real multiway junctions from tagged endpoints.
- **Extend — Parcel and zoning overlay.** Add parcel, land-use, and zoning
  layers for development-capacity and compliance studies.

## 8. Transportation, fleets, and logistics

- **Ready — Fleet GPS context API.** Enrich vehicle fixes with nearest road,
  speed limit, class, lanes, one-way state, surface, and jurisdiction.
- **Compose — Route compliance monitor.** Compare actual traces with planned
  corridors and report meaningful deviations.
- **Compose — Speed-limit context logger.** Annotate observations with mapped
  limits while treating missing or stale attributes as unknown.
- **Compose — Delivery stop verifier.** Rank the destination building,
  business, and address for a stationary cluster.
- **Compose — Service-area packager.** Cache only layers and cells covering a
  crew’s assigned region.
- **Compose — Dispatch context panel.** Show roads, controls, bridges, tunnels,
  buildings, addresses, and jurisdictions around an incident or job.
- **Compose — Road-surface risk flagger.** Warn configured vehicle classes
  about potentially unsuitable surfaces along a route.
- **Compose — Large-vehicle route preflight.** Flag narrow lane counts,
  tunnels, bridges, residential segments, and missing metadata for review.
- **Compose — Mileage-by-jurisdiction calculator.** Split traces at state,
  county, ZIP, or other decoded administrative changes.
- **Compose — Depot placement explorer.** Score candidate areas by road access,
  business density, route duration, and station proximity.
- **Compose — Technician route organizer.** Order stops, provide offline
  corridors, and retain building/business context at each job.
- **Compose — Map-aware proof of service.** Store a privacy-conscious derived
  statement such as “stationary at candidate building” with confidence.
- **Compose — Roadside asset survey app.** Attach field observations to stable
  OSM road IDs, intersections, signals, or cameras.
- **Compose — Rail-adjacent work planner.** Identify tracks and stations near
  sites and include them in safety briefings.
- **Compose — Geospatial anomaly detector.** Flag impossible jumps, driving
  through buildings, prolonged off-road motion, or metadata inconsistencies.
- **Extend — Commercial restriction routing.** Add height, weight, axle,
  hazardous-material, and time-window constraints.

## 9. Outdoors, recreation, and environment

- **Ready — Offline hiking companion.** Show nearby trail, trailhead, surface,
  hiking difficulty scale, parks, water, roads, and jurisdiction.
- **Ready — Greenway finder.** Highlight developed footways and cycleways
  separately from natural paths, tracks, bridleways, and steps.
- **Ready — Trailhead locator.** Find and describe the nearest trailhead point
  rather than confusing it with the nearest trail segment.
- **Ready — “Am I in the park?” tool.** Use polygon containment and nearest
  boundary distance instead of a place-name approximation.
- **Ready — Waterside explorer.** Identify nearby lakes, rivers, streams, and
  named water features and their geometry type.
- **Compose — Trail journal.** Match recorded outings to named trails and parks
  and summarize surface and difficulty.
- **Compose — Urban nature route.** Prefer routes or walks near parks and water
  while keeping a bounded offline corridor.
- **Compose — Trail-road crossing audit.** Find geometry intersections that may
  need signs, crossings, or field verification.
- **Compose — Trail continuity checker.** Detect gaps, dangling endpoints, and
  abrupt surface or difficulty changes in decoded trail geometry.
- **Compose — Park perimeter walk generator.** Approximate a walk around park
  boundaries using nearby roads and trails.
- **Compose — Watershed field-notes map.** Attach observations to stable water
  features and cache only the study area.
- **Compose — Quiet-space finder.** Use distance from major roads, business
  density, parks, water, and building enclosure as a transparent proxy.
- **Compose — Picnic or rest-stop finder.** Rank parks near a route using
  detour, water proximity, business access, and building shade proxies.
- **Compose — Outdoor line-of-sight explorer.** Evaluate how nearby buildings
  block views toward urban landmarks, parks, railways, or water edges.
- **Compose — Trail rescue context card.** Package nearest trail, trailhead,
  road access, water, jurisdiction, and offline coordinates for responders.
- **Extend — Elevation-aware hiking.** Add terrain/elevation tiles for grade,
  ascent, hillshade, and terrain occlusion.
- **Extend — Environmental sensor layer.** Associate air, water, heat, noise,
  or weather observations with compact spatial cells and static geometry.
- **Extend — Flood exposure explorer.** Add floodplains and elevation, then
  intersect them with buildings, roads, businesses, and routes.

## 10. Accessibility and inclusive navigation

- **Compose — Low-complexity driving routes.** Prefer fewer intersections,
  highways only when needed, and clearly preview controls and maneuvers.
- **Compose — Cognitive-accessibility navigator.** Use memorable buildings,
  parks, stations, bridges, and businesses as landmarks.
- **Compose — Surface-aware mobility map.** Expose known and unknown road or
  trail surfaces so users can make personal equipment decisions.
- **Compose — Step-aware trail browser.** Distinguish steps and difficult
  hiking scales from developed footways and cycleways.
- **Compose — Accessible destination context card.** Provide precise building,
  business, address, nearby road, station, and park context in one screen.
- **Compose — Offline orientation assistant.** Speak nearest way, intersection
  control, station, park, water, and jurisdiction without a network request.
- **Compose — Safer crossing review map.** Inventory road intersections and
  mapped controls near common pedestrian routes.
- **Compose — Route uncertainty reporter.** Highlight missing surfaces,
  incomplete topology, and unverified access information rather than silently
  assuming accessibility.
- **Compose — Caregiver travel pack.** Preload a bounded journey and landmark
  set with simplified, shareable instructions.
- **Compose — Haptic context prototype.** Convert distance and bearing to
  selected roads, trails, stations, or entrances into vibration cues.
- **Extend — Sidewalk network.** Add sidewalks, crossings, curb ramps,
  pedestrian signals, barriers, and entrances as routable topology.
- **Extend — Wheelchair metadata.** Add width, incline, smoothness, steps,
  elevators, doors, and accessible-toilet or entrance attributes.
- **Extend — Indoor transition layer.** Connect outdoor paths to entrances,
  floors, elevators, and indoor destinations.
- **Extend — Audio landmark layer.** Curate reliably perceivable landmarks and
  crossing cues for blind and low-vision navigation.

## 11. Safety, emergency, and field operations

- **Ready — Incident context lookup.** Given a coordinate, return roads,
  controls, buildings, addresses, water, rail, parks, and jurisdiction.
- **Ready — Offline responder map.** Bundle region files for use when cellular
  infrastructure is unavailable or overloaded.
- **Compose — Dispatch map card.** Summarize nearest named road, cross-control,
  building footprint, business, address, county, ZIP, and timezone.
- **Compose — Evacuation corridor preflight.** Inspect bridges, tunnels,
  one-way segments, road class, surface, controls, and missing coverage.
- **Compose — Rail and water hazard proximity.** Warn field teams when a job
  site or route lies near tracks or water.
- **Compose — Wildland/park search context.** Provide trail, trailhead, park,
  water, and road-access information from an offline pack.
- **Compose — Camera-aware incident reconstruction aid.** Identify documented
  cameras that may face an incident point, including occlusion and aim
  uncertainty, for lawful follow-up.
- **Compose — Building visibility planner.** Estimate which nearby buildings
  have line of sight to a public warning point or visual signal.
- **Compose — Jurisdiction routing assistant.** Determine responsible county,
  state, ZIP, and timezone at a boundary-sensitive incident.
- **Compose — Field-team rendezvous selector.** Pick named, reachable landmarks
  near multiple parties and cache their corridors.
- **Compose — Road closure overlay.** Apply temporary closures to local route
  graph edges while retaining static PTiles geometry.
- **Compose — Damage survey collector.** Attach assessments to stable building,
  road, bridge/tunnel, park, or water feature identifiers.
- **Compose — Coverage-gap report.** Prove which layers and cells were or were
  not available when an offline decision was made.
- **Extend — Emergency facilities layer.** Add hospitals, shelters, hydrants,
  fire stations, emergency phones, and response-specific attributes.
- **Extend — Dynamic hazard layer.** Overlay fires, floods, storms, closures,
  chemical incidents, or exclusion zones with expiration and provenance.
- **Extend — Building entrances and occupancy.** Add safe access points and
  validated capacity information for response planning.

## 12. Privacy, surveillance, and visibility

- **Ready — Camera field-of-view explainer.** For a selected point, show which
  mapped cameras are close enough, aimed appropriately, unobstructed by known
  buildings, and subject to assumed metadata.
- **Ready — Camera-to-road association.** Find cameras within a chosen distance
  of road geometry.
- **Ready — Reciprocal viewshed demo.** Ask which buildings are visible from a
  point or which buildings could see a selected feature.
- **Compose — Surveillance exposure route comparison.** Compare routes using
  mapped camera proximity and estimated visibility without claiming complete
  coverage.
- **Compose — Public-space camera audit.** Summarize camera types, placements,
  operators, directions, and missing metadata by neighborhood.
- **Compose — ALPR transparency map.** Present documented ALPR locations,
  operators, references, and snapshot history for civic oversight.
- **Compose — Camera metadata quality queue.** Prioritize fixed cameras missing
  direction, angle, operator, name, or placement for verification.
- **Compose — Urban visibility sandbox.** Move observer height and radius and
  explore how building footprints and uncertain heights affect sight lines.
- **Compose — Window-view proxy.** Estimate which nearby parks, water, rail,
  landmarks, or businesses could be visible from a building.
- **Compose — Public-art siting explorer.** Find building-facing locations with
  broad estimated visibility and explain occluders.
- **Compose — Privacy-preserving visit inference.** Run business/building
  matching locally and export only user-approved summaries.
- **Compose — Location redaction engine.** Replace precise trace sections with
  building, park, cell, or jurisdiction labels according to user rules.
- **Compose — Data minimization benchmark.** Compare what can be answered from
  cached ranges or derived events versus retaining full files and raw traces.
- **Compose — Sight-line test suite.** Generate adversarial footprint/height
  scenes and verify monotonic behavior as eye height and radius change.
- **Extend — Terrain-aware visibility.** Add elevation and vegetation or other
  occluders; current viewsheds account for buildings, not terrain.
- **Extend — Explicit uncertainty model.** Carry per-feature source date,
  confidence, height bounds, and camera metadata quality into visibility
  probabilities.

## 13. Buildings, property, and 3D experiences

- **Ready — Building facts viewer.** Inspect footprint, centroid, type, name,
  category, height, POI link, and OSM identity.
- **Ready — Lightweight 3D city map.** Extrude footprints using measured
  height when available and transparent type-based estimates otherwise.
- **Compose — Height uncertainty visualizer.** Distinguish measured, clamped,
  estimated, and missing height rather than rendering them identically.
- **Compose — Building-to-amenity matcher.** Associate businesses with
  containing or nearest footprints and expose ambiguous matches.
- **Compose — Landmark prominence score.** Combine building size, height,
  name, category, surrounding density, and line of sight.
- **Compose — Skyline explorer.** Estimate visible building silhouettes from
  a street point within the local viewshed radius.
- **Compose — View-corridor planner.** Test whether a proposed public marker,
  mural, or sign is visible from selected observation points.
- **Compose — Rooftop radio pre-screen.** Estimate building-to-building visual
  line of sight as an initial survey aid, never as an RF guarantee.
- **Compose — Solar/shadow teaching prototype.** Use footprints and heights as
  a starting geometry set for a separate sun-angle model.
- **Compose — Building-density atlas.** Aggregate footprint count, area,
  types, height availability, and businesses by H3 cell.
- **Compose — Address/building consistency audit.** Find addresses far from
  likely buildings or buildings with conflicting address/business context.
- **Compose — Building snapshot diff.** Track additions, removals, footprint
  changes, height changes, and renamed landmarks.
- **Compose — Real-estate neighborhood context.** Summarize nearby parks,
  water, rail, road class, businesses, cameras, and jurisdiction without
  generating opaque suitability scores.
- **Extend — True 3D city model.** Add roof shapes, levels, terrain, facade
  semantics, and multipolygon structure.
- **Extend — Parcel research tool.** Join footprints to parcels, ownership,
  assessment, zoning, and permit data from authoritative sources.
- **Extend — Indoor directory.** Link building footprints to entrances,
  floors, units, businesses, and accessible routes.

## 14. Local economy and business intelligence

- **Ready — Offline local directory.** Browse business names, categories,
  contacts, brands, addresses, status, and data provenance.
- **Ready — Business provenance explorer.** Compare Overture/Foursquare source
  IDs and confidence where the records carry them.
- **Compose — Category-density map.** Aggregate selected business types by
  H3 cell, road corridor, station area, park boundary, or jurisdiction.
- **Compose — Main-street profile.** Summarize businesses, brands, building
  types, road attributes, controls, and walkable destinations along a road.
- **Compose — Commercial cluster detector.** Find concentrations of related
  businesses and the buildings or corridors they occupy.
- **Compose — Brand footprint explorer.** Search a brand, map its locations,
  and compare operating status and source provenance.
- **Compose — Business data cleanup workbench.** Flag duplicates, missing
  contacts, implausible coordinates, inconsistent status, or weak confidence.
- **Compose — Route-based market explorer.** Count reachable amenities within
  a driving-time or corridor budget using local routes.
- **Compose — Station commerce study.** Compare business mix around stations
  and along connecting roads.
- **Compose — Park-edge economy study.** Inventory businesses and buildings
  around park entrances or boundaries.
- **Compose — Local closure tracker.** Diff snapshots for removed records or
  status changes and keep provenance visible.
- **Compose — Site-context report generator.** Produce a reproducible nearby
  inventory for a candidate location without uploading its coordinates.
- **Extend — Published business-name indexes.** Deploy the existing sidecar
  format broadly so state search avoids expensive brute-force scans.
- **Extend — Business hours and category sidecars.** Publish normalized
  taxonomies and richer temporal availability for reliable filtering.

## 15. Civic technology and community projects

- **Ready — “What district am I in?” widget.** Resolve available jurisdiction
  fields locally and link to the relevant public resources.
- **Ready — Neighborhood map kiosk.** Run a static, low-maintenance local map
  from bundled files in a library, school, or community center.
- **Compose — Community asset map.** Curate buildings, parks, trails, water,
  stations, and local businesses into a shareable atlas.
- **Compose — Open data story map.** Use compact local layers to tell stories
  about roads, parks, rail, water, growth, or surveillance.
- **Compose — Map feedback collector.** Let residents select a stable feature
  ID and submit corrections or observations to the appropriate maintainer.
- **Compose — Missing-name campaign.** Identify unnamed parks, roads,
  buildings, stations, water features, or trails for community mapping.
- **Compose — Missing-attribute campaign.** Create field-survey queues for road
  surfaces, speed limits, lanes, camera metadata, and trail details.
- **Compose — Public meeting map pack.** Package only the cells and layers
  relevant to a planning proposal for offline distribution.
- **Compose — Jurisdiction boundary explainer.** Show why services, timezones,
  or addresses may change across a nearby boundary.
- **Compose — Civic route audit.** Compare access to parks, stations, and local
  businesses from selected neighborhoods using transparent geometry.
- **Compose — Community change journal.** Diff periodic snapshots and invite
  verification of added or removed local features.
- **Compose — Local-history walking tour.** Attach stories to stable roads,
  buildings, parks, rail lines, and water features and make it offline-first.
- **Compose — Participatory safety walk.** Log observations at intersections,
  crossings, cameras, trail-road connections, and road segments.
- **Extend — Election and service districts.** Add authoritative district
  boundaries with source dates and identifiers.
- **Extend — Public-facility layer.** Add libraries, schools, clinics,
  shelters, public toilets, drinking water, and accessibility fields.

## 16. Developer tools and geospatial infrastructure

- **Ready — PTiles-to-JSON bridge.** Use the CLI one-shot or serve mode as a
  stable integration boundary for non-Rust programs.
- **Ready — Browser decoder SDK.** Package the Wasm decoders and cell-slicing
  functions for JavaScript applications.
- **Ready — Mobile geospatial SDK.** Ship UniFFI bindings for Swift, Kotlin,
  and Python applications over local or remote files.
- **Ready — Custom source backend.** Implement `PtilesSource` for an object
  store, package archive, database blob, encrypted store, or platform API.
- **Ready — In-memory fuzz corpus runner.** Open adversarial bytes through
  `MemorySource` and exercise every parser without filesystem dependencies.
- **Ready — Format-version linter.** Validate magic/version pairs against the
  generated supported-format table and fail closed on unknown schemas.
- **Ready — Index-layout diagnostic.** Report entry width, source of detection,
  offset base, corrected offsets, and header inconsistencies.
- **Ready — Merged-block debugger.** List cells inside a compressed block and
  extract exactly one cell payload for decoder inspection.
- **Compose — HTTP Range benchmark.** Measure open/query request counts, bytes,
  latency, cache hits, ETag behavior, and server Range compliance.
- **Compose — PTiles CDN validator.** Probe hosted files for byte lengths,
  partial responses, validators, supported formats, and readable sample cells.
- **Compose — Layer health dashboard.** Track version, bounds, feature count,
  block count, file size, last modification, sample decode success, and known
  header anomalies.
- **Compose — GeoJSON streaming adapter.** Decode only requested cells and
  stream features into standard GIS tooling.
- **Compose — Arrow/Parquet exporter.** Batch-decode selected cells or regions
  into analytical columnar formats while preserving provenance.
- **Compose — Local query microservice.** Wrap the core or CLI in HTTP/gRPC and
  retain per-layer and per-cell caches.
- **Compose — Serverless range-query function.** Open static PTiles in object
  storage and answer bounded point or viewport queries without a database.
- **Compose — Multi-language conformance kit.** Reuse golden fixtures and
  corpus files to verify independent decoders field for field.
- **Compose — Differential fuzzing system.** Feed identical generated bytes to
  Rust, JavaScript, and reference decoders and minimize disagreements.
- **Compose — Prefix-sweep safety harness.** Verify that every truncation of a
  valid record fails safely and never panics.
- **Compose — Coverage heatmap generator.** Visualize populated versus absent
  H3 cells and compare header bounds with actual index contents.
- **Compose — Byte-cost query planner.** Estimate which ranges and compressed
  blocks a point, viewport, route, or trace will require before fetching them.
- **Compose — Cache policy simulator.** Replay realistic pans or traces to tune
  byte-range, decompressed-cell, and layer LRU budgets.
- **Compose — PTiles virtual filesystem.** Mount layers as metadata, cells,
  JSON, or GeoJSON paths for shell exploration.
- **Extend — Native Rust encoder.** Implement at least water and business
  writers to enable round-trip property tests and Rust-native pipelines.
- **Extend — General PTiles builder toolkit.** Define schemas, dictionaries,
  merged blocks, indexes, coarse indexes, validation, and reproducible output.
- **Extend — Cross-layer manifest.** Describe a regional release, per-layer
  versions, hashes, provenance, build dates, and compatible clients.
- **Extend — Query sidecars.** Add optional name, category, route, spatial,
  statistical, or temporal indexes without changing base feature files.

## 17. Data quality, auditing, and observability

- **Ready — Published-layer smoke tester.** Open every known layer, require a
  real fixture, read representative cells, and record decoded feature counts.
- **Ready — Header truthfulness checker.** Compare declared counts, bounds,
  offsets, and lengths with decoded reality and catalog known builder bugs.
- **Ready — Unsupported-version sentinel.** Monitor hosted files and alert when
  a new magic/version pair appears before clients silently misread it.
- **Compose — Cross-layer consistency audit.** Check businesses against
  buildings, addresses against roads/buildings, cameras against roads, and
  stations against rail geometry.
- **Compose — Geometry validity scanner.** Find empty, degenerate, out-of-range,
  self-crossing, or implausibly long geometries.
- **Compose — Coordinate-order trap detector.** Catch accidental `[lat, lon]`
  versus `[lon, lat]` swaps using bounds and known fixtures.
- **Compose — Offset-layout regression corpus.** Preserve examples of both
  index widths, all offset bases, merged blocks, and misleading headers.
- **Compose — Snapshot reproducibility checker.** Verify deterministic bytes,
  dictionaries, indexes, file hashes, and metadata for repeated builds.
- **Compose — Source-provenance dashboard.** Expose which business records have
  stable upstream IDs, confidence values, and snapshot validators.
- **Compose — Attribute coverage report.** Quantify names, heights, surfaces,
  speed limits, lanes, directions, operators, addresses, and contact fields by
  region.
- **Compose — Silent-empty detector.** Guard against layouts or merged blocks
  that decode to plausible empty results instead of explicit errors.
- **Compose — Spatial outlier finder.** Flag features outside header bounds or
  far from related geometry and cells.
- **Compose — Real-world golden fixture curator.** Select small, license-safe
  examples covering every optional field and known irregularity.
- **Compose — Format migration dashboard.** Track adoption of new per-layer
  schema versions independently rather than inventing a global release number.
- **Extend — Record-level quality metadata.** Carry source timestamps,
  confidence, licenses, and validation status beside individual features.

## 18. AI assistants and automation

- **Ready — Tool-calling location assistant.** Give an agent deterministic
  tools for nearest road, park, water, station, trailhead, address,
  jurisdiction, business, and route queries.
- **Ready — Map-grounded answer verifier.** Require an assistant to cite the
  decoded feature ID, distance, source layer, and snapshot metadata behind a
  location claim.
- **Compose — Offline travel assistant.** Answer nearby and routing questions
  locally from a downloaded pack when no network model or map API is available.
- **Compose — Natural-language map query layer.** Translate “show cameras near
  the road by the park” into bounded cell reads and explicit spatial filters.
- **Compose — Field-survey copilot.** Generate the next verification task from
  missing names, surfaces, controls, camera attributes, or geometry anomalies.
- **Compose — Trip-summary writer.** Turn derived movement segments and map
  context into a readable diary while keeping raw coordinates local.
- **Compose — Route explanation agent.** Explain why a route chose or avoided
  highways, intersections, surfaces, controls, or landmarks.
- **Compose — Data-debugging agent.** Inspect metadata, layout detection,
  decoded sample cells, and conformance results before proposing a format fix.
- **Compose — Civic question-answering tool.** Answer transparent questions
  about nearby public geography while stating coverage and freshness limits.
- **Compose — Spatial retrieval layer for RAG.** Retrieve only records in the
  relevant H3 cells before a model generates prose.
- **Compose — Uncertainty-aware place labeler.** Present ranked candidates and
  ask for confirmation when GPS evidence does not justify a single answer.
- **Compose — Automated map release reviewer.** Compare snapshots, summarize
  material changes, and flag unsupported schemas or suspicious count shifts.
- **Extend — Declarative spatial query planner.** Compile higher-level spatial
  expressions into minimal PTiles cell/range reads and reusable operators.
- **Extend — Learned map matcher.** Train transition and emission models while
  retaining deterministic geometry and provenance as inspectable features.

## 19. Education, research, games, and art

- **Ready — Binary-format teaching lab.** Explore headers, varints, zigzag
  deltas, string tables, zstd dictionaries, indexes, and random access in one
  real format.
- **Ready — H3 spatial-index tutorial.** Visualize coordinate-to-cell mapping,
  neighbor rings, viewport coverage, and cell-normalization behavior.
- **Ready — WebAssembly performance lab.** Compare decoding across Rust,
  JavaScript, and the Wasm boundary with identical bytes.
- **Ready — Fuzzing workshop.** Use the existing per-decoder harnesses to teach
  structured fuzzing, corpus design, and crash minimization.
- **Compose — Routing algorithm playground.** Visualize graph construction,
  bidirectional A*, heuristic search, corridor budgets, and preference costs.
- **Compose — Map-matching research bench.** Compare nearest-feature,
  Gaussian-candidate, sequence, and movement-context methods on GPX traces.
- **Compose — Change-point statistics demo.** Show Welch tests, multiple-test
  correction, effect-size gates, and debouncer transitions on real movement.
- **Compose — Viewshed geometry classroom.** Demonstrate projection, angular
  horizons, occlusion, height uncertainty, and reciprocity.
- **Compose — Urban morphology notebook.** Explore footprint density, road
  networks, parks, water, rail, and business mix at a consistent H3 scale.
- **Compose — Geospatial compression research.** Benchmark dictionaries,
  merged blocks, entry widths, coordinate deltas, and query-local byte cost.
- **Compose — City exploration game.** Reveal cells as the player moves and
  award discoveries for parks, waterways, trailheads, stations, and landmarks.
- **Compose — Offline scavenger hunt builder.** Define clues against stable
  feature IDs, containment, distance, bearing, and local routes.
- **Compose — Procedural city narrative.** Generate stories from nearby roads,
  buildings, businesses, rail, water, controls, and historic snapshots.
- **Compose — Data-driven map art.** Turn road classes, footprints, rail,
  water, trails, or H3 occupancy into posters, plots, animations, or textiles.
- **Compose — Visibility puzzle game.** Ask players to find a point or eye
  height from which selected buildings or landmarks are visible.
- **Compose — Route optimization puzzle.** Balance time, distance, highways,
  intersections, cameras, parks, and bridges under a fixed budget.
- **Compose — Neighborhood “fingerprint” cards.** Generate comparable visual
  signatures from local geometry and attribute distributions.
- **Compose — Map data escape room.** Solve clues by inspecting binary layouts,
  offsets, H3 cells, names, and spatial relationships.
- **Extend — Historical PTiles archive.** Encode consistent dated snapshots for
  reproducible urban-change research and time-based games.
- **Extend — Synthetic city generator.** Produce valid PTiles fixtures for
  imaginary worlds, benchmarks, education, and property tests.

## 20. High-leverage ecosystem extensions

These are projects whose main output would expand what every other project can
do.

- **Extend — Precise address vNext.** Give each address record coordinates and
  a distinct on-disk magic, eliminating cell-only positions and the current
  admin/address magic collision.
- **Extend — Road topology vNext.** Add stable node IDs, junction degree, and
  road-to-node relationships for reliable turns and network analysis.
- **Extend — Turn-restriction sidecar.** Encode prohibited, mandatory, and
  conditional maneuvers independently from road geometry.
- **Extend — Global routing hierarchy.** Build cross-cell and cross-state
  shortcuts while retaining base PTiles as the inspectable geometry source.
- **Extend — Elevation and terrain layer.** Enable grade-aware routing,
  terrain viewsheds, flood analysis, and outdoor planning.
- **Extend — Sidewalk and crossing layer.** Unlock pedestrian, wheelchair,
  school-route, and crossing-safety projects.
- **Extend — Transit layer.** Add routes, stops, schedules, service calendars,
  accessibility, and realtime overlays.
- **Extend — Land-use, parcel, and zoning layers.** Support planning, property,
  development, and environmental analysis.
- **Extend — Dynamic overlay protocol.** Define small, expiring updates for
  traffic, closures, hazards, opening status, and sensor observations without
  rebuilding static geometry.
- **Extend — Unified search sidecars.** Index roads, addresses, buildings,
  parks, water, rail stations, trails, and businesses by normalized names.
- **Extend — Per-feature provenance.** Include source, license, observation
  date, confidence, and revision identifiers in every layer schema.
- **Extend — Signed regional manifests.** Make sets of independently versioned
  layers discoverable, verifiable, cacheable, and reproducible as one release.
- **Extend — Delta updates.** Distribute verified changes between snapshots so
  offline clients need not replace entire state files.
- **Extend — Query-planning metadata.** Store layer density and cell byte-cost
  summaries for better viewport, route, trace, and storage planning.
- **Extend — Bounded decompressed-cell LRU.** Turn the current efficient
  per-trace cache pattern into a configurable long-lived mobile/server cache.
- **Extend — First-class GeoJSON/Arrow adapters.** Make PTiles a drop-in lazy
  source for mainstream web maps, notebooks, and analytics engines.

## Choosing a first project

A few especially strong starting points, chosen because they exercise distinct
strengths of the workspace without first requiring a format change:

1. **Offline “Where am I?” app** — demonstrates local files, point-to-cell
   lookup, cross-layer decoding, address/admin context, and mobile bindings.
2. **Route context explorer** — combines HTTP Range reads, corridor routing,
   road attributes, controls, parks, water, cameras, and businesses.
3. **Private trip diary** — combines batched cell caching, candidate scoring,
   motion classification, significant shifts, and local place inference.
4. **PTiles developer inspector** — makes layouts, versions, ranges, merged
   blocks, metadata, and decoded records visible and debuggable.
5. **Community data audit map** — turns missing attributes and cross-layer
   inconsistencies into concrete, verifiable mapping tasks.
6. **Urban visibility and camera audit** — showcases the unusual combination
   of building geometry, uncertain heights, camera aim, and line of sight.

The common architectural advantage is the same in each case: the application
can fetch or retain only the compact cells it needs, run deterministic spatial
logic close to the user, and expose the underlying feature and provenance
instead of hiding every answer behind a remote black-box map service.
