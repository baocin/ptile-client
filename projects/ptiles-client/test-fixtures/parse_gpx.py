#!/usr/bin/env python3
"""Extract [lat, lng] sequences from GPX trkpt elements. grep-only, no XML lib."""

import glob
import json
import os
import re

rookery_dir = os.path.expanduser(
    "~/code/rookery/server/src/server/location/test-fixtures/gpx"
)
timeline_dir = os.path.expanduser("~/kino/projects/timeline/tests/fixtures/gps")
out_path = os.path.expanduser("~/kino/projects/ptiles-client/test-fixtures/parsed.json")

# ponytail: simple regex, assumes well-formed GPX
ptn = re.compile(r'<trkpt lat="([^"]+)" lon="([^"]+)"')

result = []

for gpx_path in sorted(glob.glob(os.path.join(rookery_dir, "*.gpx"))):
    name = os.path.splitext(os.path.basename(gpx_path))[0]
    # trim the trailing numeric id for a cleaner label
    label = re.sub(r"-\d+$", "", name)
    points = [[float(m[0]), float(m[1])] for m in ptn.findall(open(gpx_path).read())]
    if points:
        result.append({"label": label, "points": points, "source": "rookery"})

for gpx_path in sorted(glob.glob(os.path.join(timeline_dir, "*.gpx"))):
    name = os.path.splitext(os.path.basename(gpx_path))[0]
    points = [[float(m[0]), float(m[1])] for m in ptn.findall(open(gpx_path).read())]
    if points:
        result.append({"label": name, "points": points, "source": "timeline"})

os.makedirs(os.path.dirname(out_path), exist_ok=True)
with open(out_path, "w") as f:
    json.dump(result, f, indent=2)

total = sum(len(r["points"]) for r in result)
print(f"Wrote {len(result)} tracks ({total} points total) to {out_path}")
