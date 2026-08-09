# What Looky cannot do yet

This is the product boundary, not a wishlist disguised as completed work.
Looky already uses PTiles locally for real decoding, motion classification,
nearby road context, map geometry, and bounded routes. The gaps below must stay
visible in UI and engineering decisions.

## Routing and navigation

- Routes are bounded to 512 H3 resolution-7 cells. This is appropriate for a
  city or trail region, not coast-to-coast routing. PTiles has no contracted
  hierarchical routing graph, contraction hierarchy, or state-seam pack index.
- Looky does not yet produce turn-by-turn maneuvers, spoken instructions,
  lane guidance, speed-camera warnings, automatic off-route rerouting, or route
  alternatives. The core has useful navigator primitives, but their full native
  Android orchestration is not wired in this first pass.
- No traffic, closures, construction, weather, wildfire, transit schedules,
  ferry schedules, or live charger availability exists in the installed data.
- Foot routing combines pedestrian-legal roads and an installed trails layer.
  It cannot infer a missing connector or safely route across incomplete packs.
- Elevation gain, contour routing, avalanche exposure, accessibility grades,
  and offline DEM terrain are not in the current route cost model.

## Map and discovery

- The native canvas intentionally starts small: roads, trails, the active route,
  and trace. It has no cartographic label collision, vector-tile style language,
  satellite imagery, traffic overlay, indoor maps, Street View, or production
  3D renderer.
- Address and business lookup primitives exist, but there is no complete
  country-wide forward-geocode/search index or polished native search UI yet.
- On first-run, Looky downloads Tennessee and Montana from the 2026-08-07 My
  Data Timeline snapshot. Offline maps also offer a built-in all-US download
  covering every state/DC and the US-wide admin, camera, and signals layers.
  Automatic GPS-based state selection and resumable downloads are not wired yet.
- Pack delivery lacks signed manifests, resumable downloads, delta updates,
  storage forecasts, automatic adjacent-region selection, and atomic multi-layer
  version activation. Single imported files are atomically installed.
- PTiles coverage and source freshness can be reported, but the format itself
  does not carry a universal build date or completeness guarantee.

## Background behavior and sensors

- Android ultimately controls background execution. Looky uses a sticky
  location foreground service and a persistent notification, but force-stop,
  revoked background location, aggressive vendor battery policies, or a user
  disabling the notification can stop collection. No app can honestly promise
  otherwise.
- Boot restart depends on Android allowing the foreground location service and
  on background location remaining granted.
- There is no wearable heart-rate source yet, so `<gpxtpx:hr>`, `<rook:rr>`,
  and `<rook:hr_contact>` are omitted. Cadence is derived from accelerometer
  frequency as specified.
- Segment context currently records nearby road observations and device state.
  Admin, building, address, business, intersection, and full automotive context
  require their layers and resolver wiring.
- The GPX cannot faithfully reclassify movement later because it stores the
  classifier's accelerometer summary, not raw samples. That matches the Rook
  format and is a deliberate size/privacy tradeoff.
- GPS and accelerometer polling rates are user-adjustable, but the PTiles
  adaptive sampler is not yet allowed to change those preferences on its own.

## PTiles layer coverage

- The native decoder currently has no separate `highways` layer kind. Looky
  downloads and retains `highways_v2.ptiles` for forward compatibility; routing
  and motion classification use the roads layer's OSM `highway` tags.

## GPX compatibility and privacy

- The inherited Rook extension vocabulary uses unprefixed leaves inside GPX
  extensions. Common readers accept it, but a strict GPX 1.1 schema validator
  may reject those leaves because they inherit the GPX namespace.
- There is no published XSD for `https://rookery.local/gpx/1`.
- Traces are private app files but Android backup is enabled. They are not
  end-to-end encrypted with a user-held key, and there is no per-file consent,
  redaction, home-zone masking, or retention UI beyond the fixed 30-day prune.
- No server currently consumes the Rook-specific extensions. Sharing a file
  preserves them, but downstream services generally read only lat/lon/time.

## Platform/product scope

- Android only. There is no iOS, Android Auto, Wear OS, CarPlay, web sync,
  account, collaborative sharing, cloud history, or cross-device handoff.
- Looky is not ready to claim emergency-navigation reliability. Packs can be
  incomplete or stale, GNSS can fail, and no SOS/weather/rescue integration is
  present.
