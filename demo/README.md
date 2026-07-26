# PTILES wasm demo

A static web page that pans/zooms a Leaflet map over real `.ptiles` data
hosted at `https://maps.mydatatimeline.com/maps/{STATE}.{layer}.ptiles`,
doing all format work (header/index parsing, H3 cell math, zstd
decompression, record decoding, nearest-road lookup, business search)
through this repo's `ptiles-wasm` crate. The only things the JS in `js/`
does are: HTTP Range requests, and turning wasm's decoded plain objects
into Leaflet layers. No PTILES decoder logic is duplicated in JavaScript.

## Local development

1. Build the wasm package (from the repo root):

   ```sh
   cd wasm
   wasm-pack build --target web --out-dir ../demo/pkg
   ```

   This produces `demo/pkg/{ptiles_wasm.js, ptiles_wasm_bg.wasm, ...}`,
   which `demo/js/app.js` imports directly as an ES module -- no bundler
   step needed.

2. Serve `demo/` over HTTP (ES module imports and `fetch()` both require a
   real origin, not `file://`):

   ```sh
   cd demo
   python3 -m http.server 8000
   ```

3. Open `http://localhost:8000/`.

## CORS finding (important)

`maps.mydatatimeline.com` **does** send permissive CORS headers, confirmed
via:

```sh
curl -s -I -H "Origin: https://example.github.io" -H "Range: bytes=0-255" \
  https://maps.mydatatimeline.com/maps/TN.roads.ptiles
# access-control-allow-origin: *
# access-control-expose-headers: Content-Range,Accept-Ranges,Content-Length

curl -s -i -X OPTIONS -H "Origin: https://example.github.io" \
  -H "Access-Control-Request-Method: GET" -H "Access-Control-Request-Headers: range" \
  https://maps.mydatatimeline.com/maps/TN.roads.ptiles
# HTTP/2 204, access-control-allow-methods: GET, HEAD
# access-control-allow-headers: range
```

Both the actual GET/Range response and the `OPTIONS` preflight are
CORS-permissive (`allow-origin: *`, `Range` explicitly allowed in
`Access-Control-Allow-Headers`, and `Content-Range`/`Accept-Ranges`/
`Content-Length` exposed so the browser can read them back). This means
**cross-origin Range requests from a GitHub Pages origin work as-is** --
no proxy or CORS workaround is needed for this demo to fetch real data
directly from the browser.

(The host does *not* serve `{STATE}.business_name_index.ptiles` for every
state -- some states 404 on that sidecar. The demo's business search falls
back to brute-force scanning `{STATE}.business.ptiles` in that case, which
is slow over the network -- see `docs/INTEGRATION.md`'s pitfalls section.)

## Canonical source and deployment

**`demo/index.html` is the only source of truth for this UI.** Edit it here;
do not edit a copy elsewhere and sync back.

Deploy chain to <https://steele.red/ptiles/>:

```
projects/ptile-client/demo/          <- edit here
  ^
  |  absolute symlink, tracked in the steele.red repo (mode 120000)
projects/steele.red/ptiles  ->  /home/aoi/kino/projects/ptile-client/demo
  |
  |  build.py STATIC_DIRS includes "ptiles"; shutil.copytree defaults to
  |  symlinks=False, so the link is dereferenced and the real files land in
  v
projects/steele.red/output/ptiles/   <- what Cloudflare Pages serves
```

Two consequences worth knowing:

- The symlink target is **absolute**, so it only resolves on `hino-omarchy`.
  `build.py` must run on that machine; a Cloudflare Pages build from a fresh
  clone would see a dangling link.
- The live site is a **snapshot**, not a live view of `demo/`. Changes here
  are not visible at steele.red/ptiles until `build.py` runs and the output
  is published.

There is no longer a GitHub Pages workflow -- `.github/workflows/pages.yml`
was removed in kino commit `193bd75`, and `.github/` is now empty.

There is deliberately **no `index.html` at the repo root**. One existed as
a stale orphan copy of this file, referenced by nothing -- every doc in
this repo (`AGENTS.md`, `docs/INTEGRATION.md`,
`docs/HANDOFF-browser-routing.md`) points at `demo/index.html`, and
`build.py` never read it. It was deleted on 2026-07-26 and survives only in
kino git history. Do not recreate it: a second copy of this UI is always a
bug.

## What's implemented

- Pan/zoom loading of roads/water/parks/rail/buildings per viewport, using
  `cells_for_bounds` (wasm) to turn the visible bbox into H3 res-7 cells,
  then one HTTP Range request per (layer, cell) for cells not already
  fetched.
- Click on the map for nearest-road lookup (name + highlighted geometry),
  via wasm's `nearest_road`, checking the clicked cell plus its ring-1
  neighbors (`neighbor_cells`).
- Business search box (state selector + query), preferring the
  `business_name_index.ptiles` sidecar (`key_for_business_name_query` +
  `match_business_name_block`) and falling back to a brute-force scan of
  `business.ptiles` if the sidecar isn't present for that state.

## Verification performed (see task report for full evidence)

- `cargo test --workspace` passes with the new wasm exports added.
- `wasm-pack build --target web` succeeds and produces a valid `pkg/`.
- The built page's JS module graph was checked for resolvable imports
  (Node's ES module resolver against the built files), since no headless
  browser was available in this environment -- actual in-browser
  rendering/interaction (map tiles, clicking, search results appearing)
  was **not** visually verified and should be checked manually before
  relying on this demo.
