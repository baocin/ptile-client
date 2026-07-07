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

## GitHub Pages deployment

`.github/workflows/pages.yml` builds the wasm package with a pinned
`wasm-pack` version and publishes `demo/` (including the built `pkg/`) via
`actions/deploy-pages`. Enable Pages for the repo with source "GitHub
Actions" and pushes to `main`/`master` touching `demo/`, `wasm/`, or
`core/` will redeploy automatically (or trigger manually via
"Run workflow").

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
