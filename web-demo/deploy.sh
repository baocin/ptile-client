#!/usr/bin/env bash
# Deploy the demo to https://steele.red/ptiles/.
#
# Pushing the repo deploys nothing: the site is a static S3 bucket with no
# watcher on git, and steele.red builds into an output/ that is gitignored.
# This script is what users see.
#
#   ./web-demo/deploy.sh            # dry run, prints what would change
#   ./web-demo/deploy.sh --apply    # actually deploy
set -euo pipefail

SITE="${SITE_REPO:-$HOME/kino/projects/steele.red}"
BUCKET="${BUCKET:-s3://steele.red}"
export AWS_PROFILE="${AWS_PROFILE:-steele-red-deploy}"

SRC="$SITE/output/ptiles"
DEST="$BUCKET/ptiles"

# Caching. These filenames are NOT content-hashed -- ptiles_client_bg.wasm keeps
# its name across every deploy -- so `immutable` or a long max-age would pin a
# stale build in browsers with no way to bust it short of renaming the file.
#
# `no-cache` does not mean "do not store": it means store, but revalidate before
# reuse. S3 sends an ETag on every object, so an unchanged 516 KB wasm costs a
# 304 and no body, while a redeploy is picked up on the next load. Without any
# Cache-Control at all -- the state before this script -- browsers apply
# heuristic freshness and can serve a stale page for hours.
CACHE="no-cache"

echo "==> building $SITE"
(cd "$SITE" && python3 build.py >/dev/null)

# Not for serving. steele.red's build copies web-demo/ wholesale into output/,
# so anything that should not be public has to be excluded here rather than
# assumed absent -- deploy.sh names the bucket and the profile, the README's
# deploy section repeats them, and test/ is the harness 43979bf wanted kept off
# the site. All three had been published before this list existed.
NOPUB=(--exclude "deploy.sh" --exclude "README.md" --exclude "test/*")

if [ "${1:-}" != "--apply" ]; then
  echo "==> dry run (pass --apply to deploy)"
  aws s3 sync "$SRC/" "$DEST/" "${NOPUB[@]}" --dryrun --delete
  exit 0
fi

# Content types are set explicitly rather than left to `aws s3 sync`, which
# guesses from the local mimetypes database. A .wasm served as
# application/octet-stream makes WebAssembly.instantiateStreaming fail with the
# page still loading and the map still drawing -- every layer simply stays
# empty, which looks like a data problem rather than a deploy problem.
echo "==> uploading"
aws s3 sync "$SRC/" "$DEST/" "${NOPUB[@]}" \
  --exclude "*.wasm" --exclude "*.html" --exclude "*.js" \
  --cache-control "$CACHE" --only-show-errors

aws s3 cp "$SRC/index.html" "$DEST/index.html" \
  --content-type "text/html; charset=utf-8" --cache-control "$CACHE" --only-show-errors

find "$SRC" -name "*.js" | while read -r f; do
  aws s3 cp "$f" "$DEST/${f#"$SRC/"}" \
    --content-type "text/javascript; charset=utf-8" --cache-control "$CACHE" --only-show-errors
done

find "$SRC" -name "*.wasm" | while read -r f; do
  aws s3 cp "$f" "$DEST/${f#"$SRC/"}" \
    --content-type "application/wasm" --cache-control "$CACHE" --only-show-errors
done

echo "==> verifying live headers"
fail=0
check() { # url, expected content-type
  local h
  h=$(curl -sI --max-time 30 "https://steele.red/ptiles/$1")
  local ct cc
  ct=$(printf '%s' "$h" | grep -i '^content-type:' | tr -d '\r' | cut -d' ' -f2-)
  cc=$(printf '%s' "$h" | grep -i '^cache-control:' | tr -d '\r' | cut -d' ' -f2-)
  printf "  %-38s %-32s %s\n" "/$1" "$ct" "$cc"
  case "$ct" in *"$2"*) ;; *) echo "     WRONG content-type (want $2)"; fail=1 ;; esac
  case "$cc" in *no-cache*) ;; *) echo "     missing Cache-Control"; fail=1 ;; esac
}
check "" "text/html"
check "js/ptiles.js" "javascript"
check "lib/client/ptiles_client.js" "javascript"
check "lib/client/ptiles_client_bg.wasm" "application/wasm"
[ "$fail" -eq 0 ] && echo "==> ok" || { echo "==> FAILED"; exit 1; }
