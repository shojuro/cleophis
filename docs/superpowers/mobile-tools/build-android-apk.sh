#!/usr/bin/env bash
# Build the Cleophis Android APK (P1 CP0 and beyond).
#
# Wraps `tauri android build` with the provisioned mobile env block
# (docs/superpowers/mobile-dev-setup.md) and tees the full output to a log on a
# WSL-native path, so the failure trail survives the session that started it.
#
# Usage:
#   docs/superpowers/mobile-tools/build-android-apk.sh            # debug APK, aarch64
#   docs/superpowers/mobile-tools/build-android-apk.sh --release  # release APK
#   docs/superpowers/mobile-tools/build-android-apk.sh --variant=triage
#       # the supervised medical-triage build: catalog.triage.json is swapped in
#       # as resources/catalog.json for this build only, and restored on exit.
#
# Notes:
#   - Rust target dir is WSL-native (drvfs is 5-10x slower and breaks builds).
#   - llama-cpp-sys-2 reads ANDROID_NDK_ROOT/NDK_ROOT/ANDROID_NDK, not the
#     ANDROID_NDK_HOME cargo-ndk sets; all four are exported.
#   - The Android bundle ships NO desktop resources: tauri.android.conf.json
#     sets bundle.resources to null. Tauri merges platform config with
#     json_patch (RFC 7386), where `{}` is a NO-OP and only `null` removes a
#     key -- an empty object silently leaves the desktop resource map in place
#     and packs ~239 MB of Windows .dll/.exe into the APK assets.
set -euo pipefail

MODE="debug"
TAURI_FLAGS=("--debug")
VARIANT="tutor"
for a in "$@"; do
  case "$a" in
    --release) MODE="release"; TAURI_FLAGS=() ;;
    --variant=*) VARIANT="${a#--variant=}" ;;
    *) echo "unknown flag: $a" >&2; exit 2 ;;
  esac
done
case "$VARIANT" in
  tutor|triage) ;;
  *) echo "unknown variant: $VARIANT (expected tutor or triage)" >&2; exit 2 ;;
esac

export CARGO_TARGET_DIR=/home/$USER/cleophis-mobile-target

export ANDROID_NDK_HOME=/home/$USER/android-ndk-r27c
export ANDROID_NDK_ROOT=/home/$USER/android-ndk-r27c
export NDK_ROOT=/home/$USER/android-ndk-r27c
export ANDROID_NDK=/home/$USER/android-ndk-r27c
# cargo-mobile2 (under `tauri android`) resolves the NDK from NDK_HOME ONLY.
# Without it the CLI aborts before any build work with the misleading
# "failed to ensure Android environment: Skipping Android Studio command line
# tools installation" -- which reads like an SDK problem, not an NDK one.
export NDK_HOME=/home/$USER/android-ndk-r27c

export LIBCLANG_PATH=/home/$USER/.local/lib/python3.10/site-packages/clang/native

export JAVA_HOME=/home/$USER/jdk/jdk-17.0.19+10
export ANDROID_HOME=/home/$USER/android-sdk-linux
export ANDROID_SDK_ROOT=/home/$USER/android-sdk-linux
export PATH="$JAVA_HOME/bin:$ANDROID_HOME/platform-tools:$ANDROID_HOME/cmdline-tools/latest/bin:$PATH"

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../../.." && pwd)"
LOG_DIR="/home/$USER/cleophis-mobile-logs"
mkdir -p "$LOG_DIR"
LOG="$LOG_DIR/apk-$MODE-$(date +%Y%m%d-%H%M%S).log"
PROV="${LOG%.log}.provenance"

# ── Provenance sidecar (ratified policy) ──────────────────────────────────
# Build and gate scripts EMIT provenance; agents do not report it.
#
# The case this closes is not "nobody looked" -- it is "somebody looked, and
# the looking left no trace a stranger can audit." An agent's recollection of
# a PRE checkpoint is exactly the class of assurance protocol v2 stopped
# accepting, and the 2.2b APK is the worked example: the checkpoint WAS taken,
# it was correct, and it survived only in a session transcript nobody else can
# re-derive. A file the build writes itself outlives the agent that started it,
# which is the same reason the [kernels] line exists inside the app rather than
# in a build log.
#
# Two details that are the whole point rather than defensive coding:
#
#   1. Every field records the EXIT STATUS of the command that produced it.
#      `git status --porcelain` on this drvfs worktree can take minutes, and an
#      empty result from a command that TIMED OUT looks exactly like a clean
#      tree. That mistake has already been made once on this track and had to
#      be walked back. `porcelain_exit 0` is the only reading that licenses
#      "clean"; 124 means the question was never answered.
#   2. The digest is computed here, by the build. The delivery failure this
#      milestone recorded -- a correct artifact nobody was told about -- was an
#      agent not reaching its verify-and-report step. A build that emits its own
#      digest cannot finish silently.
record_provenance() {
  phase="$1"
  {
    echo "[$phase]"
    echo "timestamp      $(date -Is)"

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

echo "== building $MODE APK (aarch64), variant $VARIANT =="
echo "== log: $LOG =="
echo "== provenance: $PROV =="
cd "$ROOT"

# ── Catalog variant ───────────────────────────────────────────────────────
# The app reads its catalog from `resources_root(app)/catalog.json` at runtime
# and `include_bytes!`es that same path into the Android binary, so a variant
# cannot be a second file the build picks between -- it has to BE that path for
# the duration of the build. The swap therefore happens here, before the PRE
# checkpoint, so the provenance sidecar's `porcelain` records the tree the
# compiler actually saw: a build whose catalog was swapped reports a dirty
# `resources/catalog.json`, and one whose swap silently failed does not.
#
# The trap restores on EVERY exit path -- `set -e` aborts and Ctrl-C included.
# Leaving the triage catalog installed as `catalog.json` would make the next
# ordinary build a triage build without saying so.
CAT="$ROOT/src-tauri/resources/catalog.json"
# Idempotent, so the interrupt traps and the EXIT trap can both fire without
# the second one failing on an already-moved backup.
restore_catalog() {
  if [ -f "$CAT.tutor.bak" ]; then
    mv -f "$CAT.tutor.bak" "$CAT"
    echo "== variant: resources/catalog.json restored =="
  fi
}
if [ "$VARIANT" = "triage" ]; then
  cp "$CAT" "$CAT.tutor.bak"
  cp "$ROOT/src-tauri/resources/catalog.triage.json" "$CAT"
  trap restore_catalog EXIT
  trap 'restore_catalog; exit 130' INT
  trap 'restore_catalog; exit 143' TERM
  echo "== variant: triage (catalog.triage.json swapped in for this build) =="
fi

record_provenance PRE

# Force a full repack. AGP's incremental zip (zipflinger) rewrites entries in
# place and can strand the previous copy of a large library inside the archive:
# rebuilding on top of an existing APK turned a 334 MiB output into 658 MiB, of
# which ~340 MB was an orphaned copy of the old libcleophis_lib.so that no
# central-directory entry pointed at. The APK still installs and runs -- the
# central directory is authoritative -- which is precisely why the bloat is easy
# to miss. Deleting the previous output costs a few seconds of repacking.
rm -f "$ROOT"/src-tauri/gen/android/app/build/outputs/apk/*/*/*.apk

set +e
"$ROOT/node_modules/.bin/tauri" android build "${TAURI_FLAGS[@]}" \
  --target aarch64 --apk 2>&1 | tee "$LOG"
STATUS=${PIPESTATUS[0]}
set -e

echo "== tauri exit status: $STATUS ==" | tee -a "$LOG"
echo "== APK artifacts =="        | tee -a "$LOG"
find "$ROOT/src-tauri/gen/android" -name '*.apk' -printf '%p  %s bytes\n' 2>/dev/null \
  | tee -a "$LOG"

record_provenance POST

# The digest belongs to the build, not to whoever remembers to run sha256sum
# afterwards. Recorded for every APK found, so a multi-output build cannot
# quietly attribute one artifact's identity to another.
{
  echo "[ARTIFACT]"
  echo "tauri_exit     $STATUS"
  echo "variant        $VARIANT"
  while IFS= read -r apk; do
    [ -n "$apk" ] || continue
    echo "path           $apk"
    echo "size           $(stat -c %s "$apk") bytes"
    echo "sha256         $(sha256sum "$apk" | cut -d' ' -f1)"
  done < <(find "$ROOT/src-tauri/gen/android" -name '*.apk' 2>/dev/null)
  echo
} >> "$PROV"

echo "== log written: $LOG =="
echo "== provenance written: $PROV =="
echo "---- provenance ----"
cat "$PROV"
exit "$STATUS"
