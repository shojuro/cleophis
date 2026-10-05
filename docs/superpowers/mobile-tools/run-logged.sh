#!/usr/bin/env bash
# Run a long command with its exit status preserved, its output teed, and its
# provenance written down. Use this INSTEAD of an ad-hoc pipe.
#
#   run-logged.sh <label> -- <command...>
#
#   run-logged.sh ndk-check -- cargo ndk -t arm64-v8a -P 24 check -p cleophis --all-targets
#   run-logged.sh kotlin    -- ./gradlew :app:compileUniversalDebugKotlin --console=plain
#
# Exits with the COMMAND's status, never the pipeline's.
#
# ============================================================================
# WHY THIS EXISTS: six incidents of one family in about a day.
# ============================================================================
#
#   1. A gate log lost, so a run that happened could not be shown to have.
#   2. A build piped through `tail` wrote nothing until it finished, so an empty
#      log read as a stalled build.
#   3. `head` truncated a caller list, so a count was taken of a prefix.
#   4. `git status --porcelain` timed out: exit 124, EMPTY OUTPUT -- byte
#      identical to a clean tree.
#   5. A guard's exit status swallowed by the shell that called it.
#   6. `./gradlew ... | tail -40` reported **exit 0 while the build FAILED**,
#      because that was `tail`'s status. The failure was only visible because
#      `${PIPESTATUS[0]}` had been captured separately.
#
# Every one of those was diagnosed correctly, written up well, and generalised
# into a rule -- and the next one still arrived. This project's own repeatedly
# proven finding is that **a rule which must be remembered at the moment of use
# will not be**, and that only a mechanism closes that gap. So this is the
# mechanism: the correct behaviour is the default, and getting it right no
# longer depends on anyone recalling `PIPESTATUS` at the exact moment they are
# thinking about something else.
#
# The provenance half follows the ratified policy that SCRIPTS EMIT PROVENANCE
# AND AGENTS DO NOT REPORT IT, and reuses `build-android-apk.sh`'s proven
# `record_provenance` shape rather than inventing a second one -- including its
# hard-won detail that only `porcelain_exit 0` licenses the word "clean".
set -uo pipefail

if [ "$#" -lt 3 ] || [ "$2" != "--" ]; then
  echo "usage: $(basename "$0") <label> -- <command...>" >&2
  exit 2
fi

LABEL="$1"; shift 2

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
LOG_DIR="/home/$USER/cleophis-mobile-logs"
mkdir -p "$LOG_DIR"
LOG="$LOG_DIR/$LABEL-$(date +%Y%m%d-%H%M%S).log"
PROV="${LOG%.log}.provenance"

record_provenance() {
  phase="$1"; shift   # without this the phase leaks into the recorded command
  {
    echo "[$phase]"
    echo "timestamp      $(date -Is)"
    echo "label          $LABEL"
    echo "command        $*"

    prov_head="$(git -C "$ROOT" rev-parse HEAD 2>&1)" && prov_rc=0 || prov_rc=$?
    echo "head           $prov_head"
    echo "head_exit      $prov_rc"

    prov_porc="$(timeout 900 git -C "$ROOT" status --porcelain 2>&1)" && prov_rc=0 || prov_rc=$?
    echo "porcelain_exit $prov_rc   # 0 = answered; 124 = TIMED OUT, empty output below proves NOTHING"
    if [ "$prov_rc" -eq 0 ] && [ -z "$prov_porc" ]; then
      echo "porcelain      <empty: tree clean>"
    else
      echo "porcelain      <<<"
      printf '%s\n' "$prov_porc"
      echo ">>>"
    fi
    echo
  } >> "$PROV"
}

echo "== run-logged: $LABEL =="
echo "== log:        $LOG =="
echo "== provenance: $PROV =="
record_provenance PRE "$@"

# stderr merged into stdout deliberately: a compiler's warnings and errors go to
# stderr, and a log that omits them is a log that reads clean on a failed build.
"$@" 2>&1 | tee "$LOG"
STATUS=${PIPESTATUS[0]}

{
  echo
  echo "== exit status: $STATUS =="
} | tee -a "$LOG"

record_provenance POST "$@"

# The warning count is emitted here, unanchored, because `^warning` once kept
# "the report filtered it" alive for a whole round during the 1.4 dead-code
# episode. Counting is not judging: a non-zero count is a prompt to LOOK, not a
# failure -- Gradle's own deprecation advisory text matches too.
WARNS="$(grep -ic "warning" "$LOG" || true)"
echo "== unanchored 'warning' matches in log: $WARNS (inspect; not all are compiler warnings) =="

if [ "$STATUS" -ne 0 ]; then
  echo "== FAILED: $LABEL exited $STATUS -- see $LOG ==" >&2
fi
echo "== log written:        $LOG =="
echo "== provenance written: $PROV =="

exit "$STATUS"
