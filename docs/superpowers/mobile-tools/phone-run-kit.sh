#!/usr/bin/env bash
# The phone-run kit for the wave-C device journey (phone-journey.md).
#
# It does the laptop-side chores of the checklist so the founder's ninety
# minutes go on the phone, not on folder names:
#
#   prepare [DIR]    make DIR (default ./device-journey-r0) with exports/,
#                    exports-after-restart/, probe/ and screenshots/; write a
#                    founder-steps.json skeleton naming every step of the
#                    checklist; check the phone is attached and com.cleophis.app
#                    is installed; push phone-journey-inputs.txt to
#                    /sdcard/Download/; print the keyboard settings to turn off.
#   pull [DIR]       pull every *-triage-log.jsonl in the phone's Downloads into
#                    DIR/exports/ (E1).
#   pull-replay [DIR]  the same into DIR/exports-after-restart/ (R1).
#   pack [DIR]       zip DIR for hand-back, after checking the layout.
#
# The skeleton's results are empty strings. The triage reader
# (pipeline/analysis/device_journey.py, `_founder`) reads anything other than
# "PASS" or "FAIL" as NOT RECORDED, so an unfilled step can never pass by
# accident. The step ids are read from phone-journey.md's `### <id> —`
# headings, so the skeleton cannot drift from the checklist.
#
# Env: ADB (default `adb`; see run-on-device.sh for wireless pairing from WSL),
#      SERIAL (pick one device when several are attached), PKG (default
#      com.cleophis.app, the Android identifier in tauri.android.conf.json).
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
CHECKLIST="$HERE/phone-journey.md"
INPUTS="$HERE/phone-journey-inputs.txt"
ADB="${ADB:-adb}"
PKG="${PKG:-com.cleophis.app}"
SUBDIRS=(exports exports-after-restart probe screenshots)

die() { echo "FAIL: $*" >&2; exit 1; }

adb_s() {
  if [ -n "${SERIAL:-}" ]; then "$ADB" -s "$SERIAL" "$@"; else "$ADB" "$@"; fi
}

one_device() {
  local n
  n="$("$ADB" devices | awk 'NR>1 && $2=="device"' | wc -l)"
  [ "$n" -ge 1 ] || die "no device attached ($ADB devices lists none). Pair it first: see run-on-device.sh."
  if [ "$n" -gt 1 ] && [ -z "${SERIAL:-}" ]; then
    "$ADB" devices
    die "$n devices attached: set SERIAL=<serial> to pick the phone."
  fi
}

step_ids() {
  grep -oE '^### [A-Z][0-9]+ ' "$CHECKLIST" | awk '{print $2}'
}

prepare() {
  local dir="$1"
  mkdir -p "$dir"
  for d in "${SUBDIRS[@]}"; do mkdir -p "$dir/$d"; done
  if [ -e "$dir/founder-steps.json" ]; then
    echo "== $dir/founder-steps.json exists: left as it is =="
  else
    local ids
    ids="$(step_ids)"
    [ -n "$ids" ] || die "no step headings found in $CHECKLIST"
    python3 - "$dir/founder-steps.json" $ids <<'PY'
import json, sys
out, ids = sys.argv[1], sys.argv[2:]
steps = {i: {"result": "", "evidence": f"screenshots/{i}.png", "note": ""} for i in ids}
with open(out, "w") as fh:
    json.dump({"steps": steps}, fh, indent=2)
    fh.write("\n")
PY
    echo "== wrote $dir/founder-steps.json ($(echo "$ids" | wc -l) steps, results empty) =="
  fi

  one_device
  adb_s shell pm path "$PKG" >/dev/null 2>&1 \
    || die "$PKG is not installed on the phone: run the runbook's install step first."
  echo "== $PKG is installed: $(adb_s shell pm path "$PKG" | tr -d '\r' | head -1) =="
  adb_s push "$INPUTS" /sdcard/Download/phone-journey-inputs.txt
  echo "== pushed the inputs to /sdcard/Download/phone-journey-inputs.txt =="

  cat <<'EOF'

Before the first input, on the phone's keyboard settings (Samsung Keyboard:
Settings > General management > Samsung Keyboard settings; Gboard: Settings >
System > Keyboard > Gboard > Text correction), turn OFF:
  - auto-capitalisation  (Samsung: "Auto capitalise"; Gboard: "Auto-capitalisation")
  - autocorrect          (Samsung: "Auto replace" / predictive text; Gboard: "Auto-correction")
  - smart punctuation    (curly apostrophes; Samsung: "Smart punctuation" if shown)
Then paste every input from the Files app, never type it.
EOF
}

pull_logs() {
  local dir="$1" sub="$2"
  [ -d "$dir/$sub" ] || die "$dir/$sub does not exist: run 'prepare' first."
  one_device
  local files
  files="$(adb_s shell ls /sdcard/Download/ | tr -d '\r' | grep -E 'triage-log\.jsonl$' || true)"
  [ -n "$files" ] || die "no *-triage-log.jsonl in /sdcard/Download/: save each export there first (E1)."
  if [ "$sub" = exports-after-restart ]; then
    # R1's re-export is a NEW file; the first exports are already in exports/.
    files="$(while IFS= read -r f; do [ -e "$dir/exports/$f" ] || echo "$f"; done <<< "$files")"
    [ -n "$files" ] || die "every triage log on the phone is already in $dir/exports/: re-export the T6 chat first (R1)."
  fi
  local n=0
  while IFS= read -r f; do
    adb_s pull "/sdcard/Download/$f" "$dir/$sub/" >/dev/null
    echo "   pulled $f"
    n=$((n + 1))
  done <<< "$files"
  echo "== pulled $n log(s) into $dir/$sub/. Delete any that are not from this journey. =="
}

pack() {
  local dir="$1"
  for d in "${SUBDIRS[@]}"; do [ -d "$dir/$d" ] || die "missing $dir/$d"; done
  [ -f "$dir/founder-steps.json" ] || die "missing $dir/founder-steps.json"
  python3 -c 'import json,sys; json.load(open(sys.argv[1]))' "$dir/founder-steps.json" \
    || die "$dir/founder-steps.json is not valid JSON"
  local unfilled
  unfilled="$(python3 -c 'import json,sys; s=json.load(open(sys.argv[1]))["steps"]; print(" ".join(k for k,v in s.items() if v.get("result") not in ("PASS","FAIL")))' "$dir/founder-steps.json")"
  [ -z "$unfilled" ] || echo "WARNING: no PASS/FAIL recorded for: $unfilled (the reader will read them NOT RECORDED)"
  [ -n "$(ls -A "$dir/exports" 2>/dev/null)" ] || echo "WARNING: $dir/exports/ is empty"
  local zipname
  zipname="$(basename "$dir").zip"
  ( cd "$(dirname "$dir")" && python3 -m zipfile -c "$zipname" "$(basename "$dir")" )
  echo "== wrote $(cd "$(dirname "$dir")" && pwd)/$zipname =="
}

cmd="${1:-}"; dir="${2:-./device-journey-r0}"
case "$cmd" in
  prepare)     prepare "$dir" ;;
  pull)        pull_logs "$dir" exports ;;
  pull-replay) pull_logs "$dir" exports-after-restart ;;
  pack)        pack "$dir" ;;
  *) echo "usage: $0 prepare|pull|pull-replay|pack [DIR]" >&2; exit 2 ;;
esac
