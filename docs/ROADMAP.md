# ptiles-client roadmap / format-vNext requests

Things that need a change to the `.ptiles` *format* (the encoder side, in the
`ptiles` repo), not just the client. Recorded here so they aren't lost.

## Roads: store intersection node topology (degree)

**Status:** deferred to the next roads `.ptiles` format version.

Today a v2 roads block's intersection table stores only
`(lon, lat, intersection_type)` per intersection (see `core/src/roads.rs`
`Intersection`). There is **no road-to-node topology** — no node id on road
segments, no incident-road count (degree) on intersections. As a result
`nearest_intersection` (the "am I at an intersection?" query) can report the
nearest mapped intersection *point* and its traffic-control type, but **cannot
distinguish a true multi-way junction from a tagged road endpoint**.

Reconstructing degree geometrically (counting how many road-segment vertices
coincide with an intersection point) is possible but heuristic and was
deliberately not implemented.

**Request for the next roads format version:** add, per intersection, either
- a `degree: u8` (count of incident road segments), and/or
- a node id shared between the intersection and the `RoadSegment`s that meet
  there (enabling true graph/topology queries and routing).

Once the encoder emits this, `nearest_intersection` can return degree and the
client can answer real junction/turn questions.

## Address: consider per-record coordinates

The address layer (`PTILESA2`) stores `{osm_id, housenumber, street}` per H3
res-7 cell with **no per-record coordinates** — a record's location is only its
cell. Reverse lookup is therefore cell-granular and forward lookup is a linear
scan. If sub-cell precision is wanted, a future version could add a
`(lon, lat)` delta per record.

## Testing: a native Rust encoder for round-trip property tests

The suite's differential coverage is one-directional: golden fixtures decoded
from the Python reference (now including a reference-encoder-generated address
fixture), plus 12 `cargo-fuzz` targets over every decoder + the whole-file open
path, plus per-layer prefix-sweep "never panics" property tests. What's still
missing is a *native* Rust `encode_*` for at least one layer, enabling true
`decode(encode(x)) == x` round-trip properties without shelling out to Python.
Worth adding for water (simplest framing) and business (u32-framed).

## Admin/address magic collision

`US.admin.ptiles` (`PTILESA`) and `{STATE}.address.ptiles` (`PTILESA2`) collide
on the 7-byte on-disk magic (`write_header` packs `magic[:7]`, truncating
`PTILESA2`→`PTILESA`). The client disambiguates by structure (`block_count`,
`aux_length`) and filename. A future format version could give address a
distinct 7-byte magic to remove the ambiguity.
