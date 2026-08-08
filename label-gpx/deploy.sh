#!/usr/bin/env bash
# Deploy the labeling tool to https://steele.red/ptile-label-gpx/.
#
# Same shape as web-demo/deploy.sh, and for the same reasons -- read that one's
# comments for why the content types and cache headers are set explicitly rather
# than left to `aws s3 sync`'s mimetype guessing.
#
#   ./label-gpx/deploy.sh            # dry run, prints what would change
#   ./label-gpx/deploy.sh --apply    # actually deploy
#
# Two one-time changes live in the steele.red repo, not here (see README.md):
#   1. a `ptile-label-gpx` symlink at its root pointing at this directory
#   2. "ptile-label-gpx" added to STATIC_DIRS in its build.py
set -euo pipefail

SITE="${SITE_REPO:-$HOME/kino/projects/steele.red}"
BUCKET="${BUCKET:-s3://steele.red}"
export AWS_PROFILE="${AWS_PROFILE:-steele-red-deploy}"

SRC="$SITE/output/ptile-label-gpx"
DEST="$BUCKET/ptile-label-gpx"

# See web-demo/deploy.sh: the filenames are not content-hashed, so no-cache
# (store, but revalidate) is what lets a redeploy be picked up at all.
CACHE="no-cache"

echo "==> building $SITE"
(cd "$SITE" && python3 build.py >/dev/null)

# The sync below passes --delete, so an $SRC that does not exist would empty the
# live prefix instead of deploying to it. That is the failure mode of forgetting
# the two steele.red changes, so it is checked rather than discovered.
if [ ! -d "$SRC" ]; then
  echo "!! $SRC does not exist."
  echo "   steele.red has not been told about this directory yet:"
  echo "     ln -s $(cd "$(dirname "$0")" && pwd) $SITE/ptile-label-gpx"
  echo "     add \"ptile-label-gpx\" to STATIC_DIRS in $SITE/build.py"
  exit 1
fi

# Not for serving. steele.red's build copies the whole directory into output/,
# so anything that should stay off the site has to be excluded here. SCHEMA.md
# is deliberately NOT excluded: it is the published definition of the format the
# page emits, and a fixture consumer should be able to read it.
NOPUB=(--exclude "deploy.sh" --exclude "README.md" --exclude "test/*")

if [ "${1:-}" != "--apply" ]; then
  echo "==> dry run (pass --apply to deploy)"
  aws s3 sync "$SRC/" "$DEST/" "${NOPUB[@]}" --dryrun --delete
  exit 0
fi

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
  h=$(curl -sI --max-time 30 "https://steele.red/ptile-label-gpx/$1")
  local ct cc
  ct=$(printf '%s' "$h" | grep -i '^content-type:' | tr -d '\r' | cut -d' ' -f2-)
  cc=$(printf '%s' "$h" | grep -i '^cache-control:' | tr -d '\r' | cut -d' ' -f2-)
  printf "  %-38s %-32s %s\n" "/$1" "$ct" "$cc"
  case "$ct" in *"$2"*) ;; *) echo "     WRONG content-type (want $2)"; fail=1 ;; esac
  case "$cc" in *no-cache*) ;; *) echo "     missing Cache-Control"; fail=1 ;; esac
}
check "" "text/html"
check "js/app.js" "javascript"
check "js/gpx.js" "javascript"
check "js/segments.js" "javascript"
check "js/ptiles.js" "javascript"
check "lib/client/ptiles_client.js" "javascript"
check "lib/client/ptiles_client_bg.wasm" "application/wasm"
[ "$fail" -eq 0 ] && echo "==> ok" || { echo "==> FAILED"; exit 1; }
