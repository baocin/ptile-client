# Labeled GPX: the rook flavor

The file format `label-gpx` reads and writes, and the source of truth for anything that consumes it
(the motion tests in `motion/tests/`, the Rookery Android exporter, whatever comes next).

It is plain GPX 1.1. Everything added lives in `<extensions>`, which the spec exists for, so any GPX
reader still sees a valid track. Two namespaces:

```xml
<gpx version="1.1" creator="Rook"
     xmlns="http://www.topografix.com/GPX/1/1"
     xmlns:rook="https://rookery.local/gpx/1">
```

## Reading rules, which come first

The producer of the interesting files is an Android app whose format is still moving. So the reader's
contract is deliberately weaker than the writer's:

1. **Every element and attribute is optional.** A missing field means "not known", never a default.
2. **Unknown elements are ignored, not errors.** New fields from a newer app version must not break
   an older reader.
3. **Match on local name, ignore the namespace prefix.** `<speed>`, `<rook:speed>` and
   `<gpxtpx:speed>` are the same field. The Android sample prefixes the context blocks but not their
   leaf children; that inconsistency is not worth a parse failure.
4. **A field present but empty (`<accuracy/>`) means known-absent.** Same as missing. It exists so a
   writer can be explicit about what it could not fill.
5. **Never substitute zero for absent.** `0.0` accuracy means a perfect fix; `0` accel variance means
   a phone lying perfectly still. Both are real measurements the classifier acts on, so writing them
   in place of "no sensor" corrupts the fixture. See *Absent vs zero* below.

## Per-point sensor extensions

```xml
<trkpt lat="35.9606" lon="-83.9207">
  <ele>264.0</ele>
  <time>2026-08-08T09:00:07Z</time>
  <extensions>
    <speed>0.1</speed>
    <accuracy>8.0</accuracy>
    <accel_variance>0.02</accel_variance>
    <accel_freq>0.3</accel_freq>
    <accel_steps>0</accel_steps>
  </extensions>
</trkpt>
```

| element | unit | maps to |
| --- | --- | --- |
| `speed` | m/s, `>= 0` | `MovementTracker.push(speed_mps)`. Negative/NaN is treated as absent. |
| `accuracy` | m, horizontal, `>= 0` | `push(accuracy_m)`. Above 30 m the classifier stops trusting GPS entirely. |
| `accel_variance` | (m/s²)² | `AccelStats.variance` |
| `accel_freq` | Hz | `AccelStats.dominant_frequency` — step cadence, not a sample rate |
| `accel_steps` | count | `AccelStats.step_count` |
| `accel_mean` | m/s² | `AccelStats.mean_magnitude` — **see the gap below** |
| `accel_window_s` | s | `AccelStats.window_duration_s` — **see the gap below** |

`<time>` is required for a point to be usable: motion classification is temporal. Points without one
are dropped on read.

### The accel gap

`AccelStats` has five fields. The Android format carries three: `variance`, `dominant_frequency`,
`step_count`. The two it omits are exactly the two that `AccelStats::EMPTY` also sets to zero, so
"3-field reading" and "no accelerometer at all" are indistinguishable to anything that fills the
gaps with `0`.

Consequences, in order of preference:

- **Best**: the Android exporter adds `accel_mean` and `accel_window_s`. They are already computed
  (`AccelStats::calculate` returns all five), so this is a writer change, not new sensor work.
- **Until then**: a reader must leave them absent rather than zero, and callers should know that
  `mean_magnitude` and `window_duration_s` are structurally unavailable on rook input. Nothing in
  `classify` currently reads either — the accel table uses only `dominant_frequency`, `variance` and
  `step_count` — so the degradation is latent, not active. It becomes real the moment a rule uses
  mean magnitude to tell a phone in a pocket from a phone on a car seat.

## Per-segment label and context

One `<trk>` per labeled segment. The label is the track name, so a plain GPX viewer shows it.

`<rook:context>` and `<rook:segment>` may sit in the `<extensions>` of either the `<trk>` or its
`<trkseg>` — the Android app writes them under `trkseg`, the example below puts them under `trk`, and
a reader must accept both. The one position that is *not* track-level is inside a `<trkpt>`, where
they would be that point's data. `label-gpx` writes them under `<trk>`.

```xml
<trk>
  <name>driving</name>
  <extensions>
    <rook:segment source="auto" confidence="0.81" edited="false"
                  start_time="2020-06-15T17:56:20Z" end_time="2020-06-15T18:00:14Z"/>
    <rook:context lat="35.9606" lon="-83.9207" resolved="2026-08-08T09:00:02Z"
                  snapshot="2026-08-07">
      <rook:admin><country>US</country><state>Tennessee</state><county>Knox</county>
                  <zip>37902</zip><timezone>America/New_York</timezone></rook:admin>
      <rook:building><osm_id>1314765907</osm_id><name>Bob &amp; Sons</name>
                     <type>retail</type><category>shop</category></rook:building>
      <rook:road><osm_id>42</osm_id><name>Gay St</name><class>residential</class>
                 <distance_m>2.4</distance_m></rook:road>
      <rook:intersection><lat>35.96</lat><lon>-83.92</lon><distance_m>18.0</distance_m>
                         <type>signals</type></rook:intersection>
      <rook:addresses>
        <rook:address><housenumber>36</housenumber><street>Market Sq</street></rook:address>
      </rook:addresses>
      <rook:businesses>
        <rook:business><osm_id>11</osm_id><name>Taco Bell</name><category_idx>7</category_idx>
                       <phone>+1 865 555 0100</phone>
                       <website>https://example.com/?a=1&amp;b=2</website>
                       <status>open</status><distance_m>34.0</distance_m></rook:business>
      </rook:businesses>
      <rook:device><battery_percent>74</battery_percent><charging>false</charging>
                   <screen_on>true</screen_on><automotive>false</automotive></rook:device>
    </rook:context>
  </extensions>
  <trkseg>…</trkseg>
</trk>
```

### `<name>`: the label vocabulary

Exactly the five `MovementType` values, lowercase (`motion/src/movement.rs`):

`unknown` · `stationary` · `walking` · `running` · `driving`

`unknown` is the debouncer's initial state and should not appear as a human label — a labeler who
cannot tell should leave the segment as the classifier proposed it and mark it `source="auto"`.

A `<name>` that is not one of the five is kept verbatim on read and treated as unlabeled.

### `<rook:segment>`

| attribute | values | meaning |
| --- | --- | --- |
| `source` | `auto` \| `human` | Who decided the label. `auto` = whatever the classifier proposed. |
| `edited` | `true` \| `false` | Whether a human touched this segment at all (split, merged, relabeled). |
| `confidence` | `0..1` | The classifier's own confidence when `source="auto"`. Meaningless for `human`. |
| `start_time`, `end_time` | ISO 8601 UTC | Redundant with the points; there so a consumer can index segments without parsing them. |

`source` and `edited` are what make this a ground-truth file rather than a recording of the
classifier's opinion. **A test that asserts classifier output against `source="auto"` labels is
asserting that the classifier agrees with itself.** Only `human`/`edited="true"` segments are
evidence.

### `<rook:context>`

Resolved map context for the segment, from the ptiles layers.

- `lat`/`lon` — the point the context was resolved at. Not necessarily a trace point: the tool
  samples a few points per segment and reports the representative one.
- `resolved` — when the lookup ran.
- `snapshot` — **which map snapshot answered** (e.g. `2026-08-07`). Load-bearing: the OSM fixture
  traces are from 2011-2020 and the snapshot is years newer, so a resolved road may not have existed
  when the trace was recorded. A consumer that cares about historical accuracy needs this to know it
  cannot trust the context.

Child blocks, all optional, all "absent means nothing mapped there":

| block | fields | source |
| --- | --- | --- |
| `rook:admin` | `country`, `state`, `county`, `zip`, `timezone` | `AdminReader.admin_at` |
| `rook:road` | `osm_id`, `name`, `class`, `distance_m` | `nearest_road`. `class` is the OSM `highway` tag and feeds the classifier's road priors — the two fields it reads are `class` and `distance_m`. |
| `rook:intersection` | `lat`, `lon`, `distance_m`, `type` | `nearest_intersection`. `type` is `signals` \| `stop` \| `give_way` \| `roundabout` \| `junction`, from the numeric `intersection_type` 1-4/0. The first three extend the "still driving" window; the others do not. |
| `rook:building` | `osm_id`, `name`, `type`, `category` | `decode_buildings` |
| `rook:addresses` | `rook:address` × n with `housenumber`, `street` | `address_cell` |
| `rook:businesses` | `rook:business` × n with `osm_id`, `name`, `category_idx`, `phone`, `website`, `status`, `distance_m` | `decode_business`. **`osm_id` can exceed 2^53** — keep it a string/BigInt, never a JS `Number`. |
| `rook:device` | `battery_percent`, `charging`, `screen_on`, `automotive` | Device state at capture. Only the app can supply this; the labeler never synthesizes it. |

`label-gpx` v1 writes `rook:admin`, `rook:road` and `rook:intersection`. The rest are read and
preserved but not generated — see `README.md`.

## Absent vs zero, and synthetic vs measured

A fixture that cannot distinguish a measured value from a computed one will happily train a
classifier on its own output. Three mechanisms:

**1. `derived="true"`** on any element the tool computed rather than read:

```xml
<speed derived="true">11.42</speed>
```

Absent attribute = as published by the capturing device. Present = this repo computed it (for
`speed`, from consecutive positions through `MotionClassifier`'s smoother). A consumer that ignores
the attribute still reads a plausible number, which is the right failure mode.

**2. Omit, never zero.** If a value is unknown, write `<accuracy/>` or nothing at all. Never `0`.

**3. `<rook:provenance>`**, once, in `<metadata>`:

```xml
<metadata>
  <rook:provenance tool="label-gpx" version="1" snapshot="2026-08-07"
                   derived="speed" synthetic="" context_samples_per_segment="5"/>
</metadata>
```

| attribute | meaning |
| --- | --- |
| `tool`, `version` | What wrote the file. |
| `snapshot` | Map snapshot every `rook:context` in the file resolved against. |
| `derived` | Comma-separated fields computed from other real data (e.g. `speed` from positions). Still trustworthy, just not measured. |
| `synthetic` | Comma-separated fields **invented** (e.g. accel profiles generated to match a label). Empty in normal use. A fixture with a non-empty `synthetic` must never be used to validate the classifier on those fields — it was generated from the answer. |
| `context_samples_per_segment` | How many points per segment were resolved against the map. Fewer samples = a coarser road context. |

## Minimal valid file

Everything except structure is optional, so this is legal input:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<gpx version="1.1" xmlns="http://www.topografix.com/GPX/1/1">
<trk><trkseg>
<trkpt lat="35.9606" lon="-83.9207"><time>2026-08-08T09:00:00Z</time></trkpt>
<trkpt lat="35.9607" lon="-83.9207"><time>2026-08-08T09:00:07Z</time></trkpt>
</trkseg></trk>
</gpx>
```

That is also the shape of the OSM fixture traces in `test-fixtures/gpx/`: position and time, nothing
else. Speed is derived, accuracy and accel are absent, and the classifier runs on speed alone — which
is exactly why the road context matters (`motion/tests/road_context.rs`).

## Escaping

`&`, `<`, `>` and quotes in names, phone numbers and URLs are XML-escaped normally
(`Bob &amp; Sons`, `?a=1&amp;b=2`). `label-gpx` produces this with `XMLSerializer` rather than string
concatenation, so it is correct in both attribute and text position. Readers should use a real XML
parser for the same reason; the one exception in this repo is `motion/tests/gpx_replay.rs`, which
scans for two attributes and a timestamp in files known not to contain escapes.
