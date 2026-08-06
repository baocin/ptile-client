# PTILES demo (legacy)

A static web page that pans/zooms a Leaflet map over real `.ptiles` data
hosted at `https://maps.mydatatimeline.com/maps/{STATE}.{layer}.ptiles`.

`index.html` calls into `ptiles-wasm` for record decoding, but does the
framing itself: header parsing, both index entry widths, offset-base
selection, merged-block slicing and the PTCI coarse index are all
hand-written JavaScript here. That duplication is the reason `web-demo/`
exists and eventually replaces this page -- every format bug this project
has had came from these two implementations disagreeing, and the failure
mode is a silently empty layer rather than an error.

> `js/app.js`, `js/ptiles-remote.js` and `pkg/` are **dead**. Nothing in the
> repo references them; they are scaffolding from an earlier arrangement in
> which `app.js` was the entry point and the wasm package lived in `pkg/`.
> The live page is `index.html` against `lib/client/`. Delete them once
> someone has confirmed no bookmark or external doc points at them.

## Local development

1. Build the wasm package (from the repo root):

   ```sh
   PATH="$HOME/.cargo/bin:$PATH" wasm-pack build wasm --target web \
     --out-dir ../demo/lib/client --out-name ptiles_client
   ```

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

**This page moved to <https://steele.red/ptiles-legacy/> on 2026-08-04.**
`/ptiles` now serves `web-demo/`, the wasm client. This one is kept for
live comparison until that has proven itself.

Deploy chain:

```
ptile-client/demo/          <- edit here
  ^
  |  absolute symlink, tracked in the steele.red repo (mode 120000)
steele.red/ptiles-legacy  ->  <ptile-client checkout>/demo
steele.red/ptiles         ->  <ptile-client checkout>/web-demo
  |
  |  build.py STATIC_DIRS includes both; shutil.copytree defaults to
  |  symlinks=False, so the links are dereferenced and the real files land in
  v
projects/steele.red/output/ptiles-legacy/
  |
  |  AWS_PROFILE=steele-red-deploy aws s3 sync <that>/ s3://steele.red/<same>/
  |  then invalidate CloudFront E1X2E2N30TVNGX on /ptiles-legacy/*
  v
S3 behind CloudFront   <- what actually serves it
```

Two consequences worth knowing:

- The symlink target is **absolute**, so it only resolves on `hino`.
  `build.py` must run on that machine; a build from a fresh clone anywhere
  else would see a dangling link.
- The live site is a **snapshot**, not a live view of `demo/`. Changes here
  are not visible until `build.py` runs *and* the output is synced *and*
  CloudFront is invalidated.

The `steele-red-deploy` credentials can create an invalidation but cannot read
one back: `cloudfront:GetInvalidation` and `GetDistribution` are both denied,
so a deploy script cannot wait for propagation. Poll the live URL instead.

There is no longer a GitHub Pages workflow -- `.github/workflows/pages.yml`
was removed in commit `193bd75`. `.github/workflows/ci.yml` is still there.

There is deliberately **no `index.html` at the repo root**. One existed as
a stale orphan copy of this file, referenced by nothing -- every doc in
this repo (`AGENTS.md`, `docs/INTEGRATION.md`,
`docs/HANDOFF-browser-routing.md`) points at `demo/index.html`, and
`build.py` never read it. It was deleted on 2026-07-26 and survives only in
git history. Do not recreate it: a second copy of this UI is always a
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
