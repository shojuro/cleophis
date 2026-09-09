#!/usr/bin/env bash
# tools/sync-triage-detectors.sh — copy the reference detectors from the triage
# repo and record their sha. THE ONLY WAY src/triage/detectors.mjs changes.
#
#   tools/sync-triage-detectors.sh [path-to-cleophas-triage]   # default ~/cleophas-triage
#
# Refuses if the source repo's own pin (artifacts/detectors-pin.json) does not
# match the file it points at: a pin that disagrees with its file is a repo in
# the middle of an edit, and vendoring that would freeze a half-change.
set -euo pipefail
SRC_REPO="${1:-$HOME/cleophas-triage}"
HERE="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$SRC_REPO/probes/detectors.mjs"
PIN="$SRC_REPO/artifacts/detectors-pin.json"
DST_DIR="$HERE/src/triage"
[ -f "$SRC" ] || { echo "no $SRC — run Phase 2 Task 1 in the triage repo first"; exit 1; }
[ -f "$PIN" ] || { echo "no $PIN"; exit 1; }
actual="$(sha256sum "$SRC" | cut -d' ' -f1)"
pinned="$(python3 -c "import json,sys;print(json.load(open(sys.argv[1]))['sha256'])" "$PIN")"
[ "$actual" = "$pinned" ] || { echo "REFUSING: $SRC sha $actual != its own pin $pinned"; exit 1; }
src_head="$(git -C "$SRC_REPO" rev-parse --short HEAD)"
mkdir -p "$DST_DIR"
cp "$SRC" "$DST_DIR/detectors.mjs"
python3 - "$DST_DIR/detectors.pin.json" "$actual" "$src_head" <<'PY'
import json, sys, datetime
json.dump({
  "schema": "cleophis/triage-detectors-pin/v1",
  "source": "cleophas-triage/probes/detectors.mjs",
  "source_commit": sys.argv[3],
  "sha256": sys.argv[2],
  "synced_utc": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
  "rule": "never edit src/triage/detectors.mjs by hand; run tools/sync-triage-detectors.sh",
}, open(sys.argv[1], "w"), indent=1)
PY
echo "vendored $SRC ($actual, $src_head) -> $DST_DIR"
