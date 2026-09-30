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
#   pull [DIR]       pull every *-triage-log.jsonl (and "* (N).jsonl") in the
#                    phone's Downloads into DIR/exports/ (E1).
#   pull-replay DIR TITLE
#                    pull the newest export of ONE chat, named by its title or
#                    exact file name, into DIR/exports-after-restart/ as
#                    <stem>-triage-log-after-restart.jsonl (R1).
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
  echo "== $PKG is installed: $(adb_s shell pm path "$PKG" | tr -d '\r' | sed -n 1p) =="
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

# The app saves each export as `<stem>-triage-log.jsonl`, where <stem> is the
# chat title through `safe_file_stem` (src-tauri/src/mobile_native/pure.rs:
# anything but ASCII letters, digits, space, - and _ becomes _, cut to 60
# characters, trimmed, "chat" when nothing is left). Saving a second file of
# the same name to Downloads either overwrites it or gets " (1)" appended, so
# both forms are matched.
LOG_RE='-triage-log( \([0-9]+\))?\.jsonl$'

# Newest first; -1 because `adb shell` allocates a pty from a terminal and a
# bare `ls` would then print columns.
phone_logs() {
  adb_s shell ls -1t /sdcard/Download/ | tr -d '\r' | grep -E -- "$LOG_RE" || true
}

safe_stem() {
  python3 - "$1" <<'PY2'
import sys
t = "".join(c if (c.isascii() and c.isalnum()) or c in "-_ " else "_" for c in sys.argv[1])[:60].strip()
print(t if any(c.isascii() and c.isalnum() for c in t) else "chat")
PY2
}

pull_logs() {
  local dir="$1"
  [ -d "$dir/exports" ] || die "$dir/exports does not exist: run 'prepare' first."
  one_device
  local files
  files="$(phone_logs)"
  [ -n "$files" ] || die "no *-triage-log.jsonl in /sdcard/Download/: save each export there first (E1)."
  local n=0
  while IFS= read -r f; do
    adb_s pull "/sdcard/Download/$f" "$dir/exports/$f" >/dev/null || die "could not pull $f"
    echo "   pulled $f"
    n=$((n + 1))
  done <<< "$files"
  echo "== pulled $n log(s) into $dir/exports/. Delete any that are not from this journey. =="
}

# R1: the re-export of ONE chat after a restart. Its file name is the same as
# its E1 copy's (or that name plus " (1)"), so it is found by name or chat
# title, never by "not already pulled", and the NEWEST match is taken. It is
# saved under a distinct local name so it can never be mistaken for, or
# overwrite, the E1 copy in exports/.
pull_replay() {
  local dir="$1" want="${2:-}"
  [ -d "$dir/exports-after-restart" ] || die "$dir/exports-after-restart does not exist: run 'prepare' first."
  [ -n "$want" ] || die "name the re-exported chat: pull-replay DIR '<chat title>' (or its exact file name)."
  one_device
  local stem pick="" logs f
  if [[ "$want" =~ \.jsonl$ ]]; then
    stem="${want%.jsonl}"; stem="${stem% (*)}"; stem="${stem%-triage-log}"
  else
    stem="$(safe_stem "$want")"
  fi
  # A loop that stops at the first (newest) match, not `| head -1`: under
  # pipefail and set -e, head closing the pipe early kills the script silently
  # as soon as there are two matches, which is exactly the " (1)" case.
  logs="$(phone_logs)"
  while IFS= read -r f; do
    if [[ "$want" =~ \.jsonl$ ]]; then
      [ "$f" = "$want" ] && { pick="$f"; break; }
    else
      case "$f" in "$stem-triage-log.jsonl"|"$stem-triage-log ("*").jsonl") pick="$f"; break;; esac
    fi
  done <<< "$logs"
  [ -n "$pick" ] || die "no triage log for '$want' in /sdcard/Download/ (looked for $stem-triage-log[ (N)].jsonl): re-export that chat and save it to Downloads first (R1)."
  local local_name="$stem-triage-log-after-restart.jsonl"
  adb_s pull "/sdcard/Download/$pick" "$dir/exports-after-restart/$local_name" >/dev/null || die "could not pull $pick"
  echo "== pulled the newest match, $pick, as $dir/exports-after-restart/$local_name =="
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
  pull)        pull_logs "$dir" ;;
  pull-replay) pull_replay "$dir" "${3:-}" ;;
  pack)        pack "$dir" ;;
  *) echo "usage: $0 prepare|pull|pack [DIR]   or   $0 pull-replay DIR '<chat title or file name>'" >&2; exit 2 ;;
esac
