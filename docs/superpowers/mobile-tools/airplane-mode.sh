#!/usr/bin/env bash
# A2 — the airplane-mode acceptance harness (spec §11 A2, hazard H9).
#
# ── THE FAILURE THIS EXISTS TO NOT BE ─────────────────────────────────────────
#
# A2's own consequence string says the item "would be satisfied by a human
# remembering to turn radios off". So a harness that PRINTS "please enable
# airplane mode" is the failure, not the fix. This script therefore:
#
#   1. DISCOVERS attached devices (`discover_devices`) rather than assuming a
#      serial, because two more phones are arriving and the 5.4 gate needs
#      three. `run-on-device.sh` is the counter-example: bare `$ADB` with no
#      `-s`, and `adb get-serialno` fails outright once a second device is
#      attached.
#   2. DRIVES the radios itself over adb, and
#   3. FAILS if it cannot VERIFY they are off (`verify_radios_off`).
#
# (3) is the whole point. The assertion A2 makes is "no socket attempt after
# setup", and that assertion is worth nothing if the offline state is taken on
# trust — an unverifiable precondition makes the whole suite a check that
# cannot fail.
#
# ── WHY VERIFICATION IS EMPIRICAL, NOT A SETTINGS READ ────────────────────────
#
# `settings get global airplane_mode_on` returns the CONFIG. This branch's
# standing rule is that claims are verified against the artifact, never against
# the config that was meant to produce it — the same reason bundle claims are
# checked against the built APK. A device can report `airplane_mode_on=1` while
# a VPN, a tethered link, or a stubborn OEM radio still carries packets. So the
# setting is necessary and NOT sufficient: `verify_radios_off` also requires
# that no packet can actually leave.
#
# ── AND WHY THERE IS A POSITIVE CONTROL ───────────────────────────────────────
#
# "The egress probe failed" does not mean "the radios are off" — it also means
# "the probe is broken", "ping is missing on this OEM image", or "the network
# was already down". Those are indistinguishable from a single failing probe,
# and this project has a standing rule that an absent result is not a negative
# finding.
#
# So `verify_egress_works` runs the IDENTICAL probe with the radios ON and
# REQUIRES it to succeed, before airplane mode is ever enabled. Only a probe
# demonstrated capable of succeeding is allowed to prove anything by failing.
# A run that cannot get egress in the first place aborts and says so, rather
# than sailing through to a green that means nothing.
#
# ── WHAT THIS HARNESS DOES AND DOES NOT AUTOMATE (stated, not implied) ────────
#
# Fully machine-driven and machine-verified: device discovery, radio control,
# the offline precondition, the per-uid egress assertion, and restore.
#
# NOT automated: exercising the app's features. Driving a chat turn needs UI
# automation that would have to hardcode per-device coordinates — the exact
# device-specific assumption requirement (1) forbids. `--interactive` opens a
# monitored window for a human to use the app in.
#
# That is a different thing from the failure A2 names, and the distinction is
# the point: the HUMAN part is "use the app", never "remember to turn the
# radios off". The precondition and the verdict stay machine-verified whether a
# human is present or not.
#
# Usage:
#   ./airplane-mode.sh                     # every attached device
#   ./airplane-mode.sh --serial R58N30ABCD # one device
#   ./airplane-mode.sh --interactive 180   # 180 s window to exercise the app
#   ./airplane-mode.sh --soak 60           # unattended: launch, watch 60 s
set -uo pipefail

ADB="${ADB:-adb}"
PKG="${PKG:-com.cleophis.app}"
# 8.8.8.8 rather than a hostname: a DNS failure and a routing failure are
# different facts, and resolving first would conflate them.
PROBE_HOST="${PROBE_HOST:-8.8.8.8}"
SOAK_SECS=60
INTERACTIVE=0
ONLY_SERIAL=""

while [ $# -gt 0 ]; do case "$1" in
  --serial)      ONLY_SERIAL="$2"; shift 2;;
  --soak)        SOAK_SECS="$2"; shift 2;;
  --interactive) INTERACTIVE=1; SOAK_SECS="${2:-180}"; shift 2;;
  -h|--help)     sed -n '1,70p' "$0"; exit 0;;
  *) echo "airplane-mode: unknown arg $1" >&2; exit 2;;
esac; done

fail() { echo "FAIL (A2): $*" >&2; exit 1; }
note() { echo "  $*"; }

# ── Device discovery ──────────────────────────────────────────────────────────
#
# Prints one serial per line for every device in state `device`. Deliberately
# excludes `unauthorized`, `offline` and `no permissions`: a half-attached phone
# that silently drops out of the run is how a three-device gate reports two
# passes and no failures.
discover_devices() {
  "$ADB" devices | awk 'NR>1 && $2=="device" {print $1}'
}

adbs() { "$ADB" -s "$1" "${@:2}"; }

# ── The egress probe, used in BOTH directions ────────────────────────────────
#
# Returns 0 when a packet reached the outside world. The SAME function is the
# positive control (must succeed, radios on) and the verification (must fail,
# radios off) — one implementation, so the two cannot drift into testing
# different things, which is the coupling bug D-4 is about.
egress_reaches_internet() {
  local serial="$1"
  adbs "$serial" shell "ping -c 2 -W 2 $PROBE_HOST >/dev/null 2>&1 && echo UP || echo DOWN" \
    2>/dev/null | tr -d '\r' | grep -q UP
}

# The positive control. An egress probe that has never been seen to succeed
# cannot prove anything by failing.
verify_egress_works() {
  local serial="$1"
  if ! egress_reaches_internet "$serial"; then
    fail "$serial: NO egress before airplane mode was enabled. The probe is \
therefore not demonstrated capable of succeeding, so its later failure would \
prove nothing. Connect the device to a working network and re-run. \
(If this device is deliberately offline, that is not an A2 run.)"
  fi
  note "positive control: egress WORKS with radios on (probe can succeed)"
}

# ── Radio control ────────────────────────────────────────────────────────────
set_airplane_mode() {
  local serial="$1" on="$2"
  local verb="enable"; [ "$on" = "1" ] || verb="disable"
  # Android 11+ (API 30). The A22 and both incoming devices are past this.
  if adbs "$serial" shell "cmd connectivity airplane-mode $verb" >/dev/null 2>&1; then
    return 0
  fi
  # Fallback for images without `cmd connectivity`: write the setting and
  # broadcast the change. Not silently trusted — `verify_radios_off` still has
  # to pass afterwards, which is exactly why a best-effort fallback is safe here.
  adbs "$serial" shell "settings put global airplane_mode_on $on" >/dev/null 2>&1
  adbs "$serial" shell "su -c 'am broadcast -a android.intent.action.AIRPLANE_MODE --ez state $on'" \
    >/dev/null 2>&1 || true
}

# ── The verification that A2 turns on ────────────────────────────────────────
#
# Two independent facts, both required:
#   (a) the device REPORTS airplane mode on   — necessary, not sufficient
#   (b) no packet can actually leave          — the artifact, not the config
#
# A check that cannot be PERFORMED is a failure, never a skip. That is the
# absent-result rule: "the setting read returned nothing" and "the setting is 0"
# must not collapse into the same outcome.
verify_radios_off() {
  local serial="$1"
  local reported
  reported="$(adbs "$serial" shell 'settings get global airplane_mode_on' 2>/dev/null | tr -d '\r\n')"

  case "$reported" in
    1) note "reported: airplane_mode_on=1" ;;
    0) fail "$serial: airplane mode did NOT take (airplane_mode_on=0). The \
radios were not turned off, so nothing this run observes is an offline result." ;;
    *) fail "$serial: could not READ the airplane-mode setting (got '${reported:-<empty>}'). \
An unverifiable precondition is a failure, not a skip — the whole point of A2 \
is that the offline state is machine-verified rather than assumed." ;;
  esac

  # Settle: radios take a moment to actually drop after the setting flips, and
  # a probe fired too early passes for the wrong reason.
  sleep 5

  if egress_reaches_internet "$serial"; then
    fail "$serial: airplane_mode_on=1 but packets STILL REACH $PROBE_HOST. The \
config says offline and the artifact says otherwise — a VPN, a tethered link, \
or an OEM radio that ignores the setting. This is precisely why the setting \
alone is not accepted."
  fi
  note "verified: no egress reaches $PROBE_HOST (radios genuinely down)"
}

# ── The A2 assertion: no socket attempt after setup ──────────────────────────
app_uid() {
  local serial="$1"
  adbs "$serial" shell "dumpsys package $PKG | grep -m1 userId=" 2>/dev/null \
    | tr -d '\r' | sed -n 's/.*userId=\([0-9]*\).*/\1/p'
}

# Per-uid byte counters. Zero delta across the window is the machine half of
# "no socket attempt"; the logcat scan is the half that catches an attempt that
# failed before it moved any bytes (which, in airplane mode, is most of them).
#
# ⚠ Prints nothing when the uid has no rows at all, and the CALLER MUST TREAT
# THAT AS A FAILURE. An `awk ... {print s+0}` that reports 0 for "parsed
# nothing" and 0 for "measured zero bytes" is a counter that can never rise —
# i.e. an assertion that cannot fail, which is the exact disease A2 exists to
# not have. The two outcomes are kept distinguishable here and separated by
# `read_uid_bytes` below.
uid_bytes_raw() {
  local serial="$1" uid="$2"
  adbs "$serial" shell "dumpsys netstats detail" 2>/dev/null | tr -d '\r' \
    | grep -a "uid=$uid" \
    | awk -F'[= ]' '{for(i=1;i<=NF;i++){if($i=="rb"||$i=="tb"){s+=$(i+1);seen=1}}}
                    END{if(seen) print s; else print ""}'
}

# Same measurement, with "could not measure" separated from "measured zero".
#
# ⚠ RETURNS non-zero instead of calling `fail`, and the caller MUST propagate.
# `fail` runs `exit`, and this function is only ever used inside `$( … )` — a
# command substitution is a SUBSHELL, so that exit would kill the substitution
# and leave the script running. Demonstrated: the first version printed
# "FAIL (A2): could not read per-uid byte counters" and then exited **0**. That
# is the same green-under-red as the `while` loop below, in a second disguise,
# which is why the rule is written here rather than remembered: NEVER call
# `fail` from a function used in a command substitution.
read_uid_bytes() {
  local serial="$1" uid="$2" phase="$3" v
  v="$(uid_bytes_raw "$serial" "$uid")"
  case "$v" in
    ''|*[!0-9]*)
      echo "FAIL (A2): $serial: could not read per-uid byte counters for uid=$uid ($phase). \
\`dumpsys netstats detail\` produced no parsable rows, so the zero-bytes \
assertion would be measuring nothing and could never fail. Unverifiable is a \
FAILURE here, not a pass." >&2
      return 1 ;;
  esac
  printf '%s' "$v"
}

run_one_device() {
  local serial="$1"
  echo "== $serial =="

  local uid; uid="$(app_uid "$serial")"
  [ -n "$uid" ] || fail "$serial: $PKG is not installed — nothing to assert about."
  note "app uid=$uid"

  verify_egress_works "$serial"

  note "enabling airplane mode…"
  # Registered BEFORE the radios are touched, so a failure between the two
  # still restores. A `trap ... RETURN` was the first version of this and was
  # wrong: `fail` calls `exit`, and an exit does not run a RETURN trap — the
  # phone would have been left in airplane mode by exactly the failing runs
  # that need a human to pick it up next.
  TOUCHED_DEVICES+=("$serial")
  set_airplane_mode "$serial" 1

  verify_radios_off "$serial"

  local before; before="$(read_uid_bytes "$serial" "$uid" "before")" || exit 1
  adbs "$serial" logcat -c >/dev/null 2>&1 || true
  adbs "$serial" shell "monkey -p $PKG -c android.intent.category.LAUNCHER 1" >/dev/null 2>&1 \
    || fail "$serial: could not launch $PKG"

  if [ "$INTERACTIVE" = "1" ]; then
    echo "  >> ${SOAK_SECS}s window: USE THE APP now (open a chat, send a turn)."
    echo "  >> Radio state is already machine-verified; only the exercising is manual."
  else
    note "unattended soak: ${SOAK_SECS}s"
  fi
  sleep "$SOAK_SECS"

  local after; after="$(read_uid_bytes "$serial" "$uid" "after")" || exit 1
  local delta=$(( after - before ))

  local netlog
  netlog="$(adbs "$serial" logcat -d 2>/dev/null | tr -d '\r' \
    | grep -iE "$PKG|cleophis" | grep -iE 'UnknownHost|ECONNREFUSED|ENETUNREACH|SocketException|Network is unreachable|failed to connect' || true)"

  echo "  --- verdict ---"
  note "uid byte delta: $delta"
  if [ "$delta" -gt 0 ]; then
    fail "$serial: the app moved $delta bytes while the radios were verified \
down — it attempted network I/O offline."
  fi
  if [ -n "$netlog" ]; then
    echo "$netlog" >&2
    fail "$serial: the app attempted a connection offline (see the logcat lines above)."
  fi
  echo "  PASS (A2): $serial — radios machine-verified down, zero uid bytes, no connection attempts"
}

main() {
  local devices
  if [ -n "$ONLY_SERIAL" ]; then
    devices="$ONLY_SERIAL"
  else
    devices="$(discover_devices)"
  fi
  # No devices is a FAILURE, not a quiet success. A suite that passes because
  # it ran against nothing is the "green-because-unexecuted" shape this branch
  # has already been bitten by twice.
  [ -n "$devices" ] || fail "no attached devices in state 'device'. \
\`$ADB devices\` shows nothing usable — a run against zero devices is not a pass."

  # ⚠ READ THIS BEFORE "TIDYING" THE LOOP.
  #
  # The obvious spelling is `echo "$devices" | while read -r s; do …; done`, and
  # it is CATASTROPHICALLY WRONG here: a pipe puts the loop in a SUBSHELL, so
  # `fail`'s `exit 1` kills only the subshell. The script then carries on and
  # prints "all discovered devices passed" — with exit status 0. Demonstrated,
  # not theorised: a five-line reproduction printed FAIL and then the success
  # line and exited 0.
  #
  # That is a green under a red, the same shape as verify-apk.py's, arriving in
  # the harness whose entire reason for existing is that A2's assertion must be
  # able to fail. Read into an array instead — no pipe, no subshell.
  local -a serials=()
  while IFS= read -r s; do
    [ -n "$s" ] && serials+=("$s")
  done <<< "$devices"

  local s
  for s in "${serials[@]}"; do
    run_one_device "$s"
  done
  echo
  echo "airplane-mode: PASS on ${#serials[@]} device(s): ${serials[*]}"
}

# Restore every device this run put into airplane mode, on ANY exit path —
# success, assertion failure, or Ctrl-C. Leaving a founder's phone offline
# because an assertion tripped is a harness that costs more than it proves.
TOUCHED_DEVICES=()
restore_all() {
  local d
  for d in "${TOUCHED_DEVICES[@]:-}"; do
    [ -n "$d" ] || continue
    echo "  restoring radios on $d…"
    set_airplane_mode "$d" 0
  done
}
trap restore_all EXIT INT TERM

main
