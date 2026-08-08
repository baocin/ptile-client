# GPX trace fixtures (copied from the server)

Real OpenStreetMap public GPS traces, copied verbatim from
`server/src/location/test-fixtures/gpx/` so `GpxRouteTest` can replay them through the
Android movement classifier hermetically (no cross-module build path). They are immutable
third-party downloads — keep them byte-identical to the server copies.

Licensed **ODbL** (OpenStreetMap GPS trace data). See the server directory's `README.md` for
the per-file OSM trace ids, uploaders, and areas. Five are foot routes (hikes/trails in
NC/TN); `tn-middle-tennessee-3605997.gpx` is a Nashville roads (vehicular) trace.

## In this repo

`motion/tests/gpx_replay.rs` replays these through `ptiles-motion`, the Rust port of the same
Android classifier. It parses them with a regex, per the house style of `../parse_gpx.py`, and
skips silently if this directory is absent.

`../parsed.json` holds the same six traces as lat/lon arrays with **no timestamps**, which is why
the raw files are here as well: motion classification is temporal, so `parsed.json` cannot drive
it. `core/tests/gpx_snap.rs` uses `parsed.json`; the motion tests use these.

| file | points | kind | recorded |
| --- | --- | --- | --- |
| `nc-sals-branch-1191748.gpx` | 721 | foot | 2012-03-09 |
| `nc-mine-creek-1184364.gpx` | 838 | foot | 2012-02-22 |
| `nc-umstead-trails-1184467.gpx` | 1957 | foot | 2012-02-24 |
| `tn-maryville-hike-1063250.gpx` | 1124 | foot | 2011-07-24 |
| `tn-maryville-trails-1283272.gpx` | 442 | foot | 2012-07-13 |
| `tn-middle-tennessee-3605997.gpx` | 1187 | vehicular | 2020-06-15 |

None carry speed, accuracy or accelerometer data — speed is derived from position deltas, so these
exercise the smoothing and speed-band paths and never the accel fallback. Labeled fixtures with
sensor fields come from `label-gpx/`.
