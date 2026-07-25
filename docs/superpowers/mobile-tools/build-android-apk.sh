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
if [ "${1:-}" = "--release" ]; then
  MODE="release"
  TAURI_FLAGS=()
fi

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

echo "== building $MODE APK (aarch64) =="
echo "== log: $LOG =="
cd "$ROOT"

set +e
"$ROOT/node_modules/.bin/tauri" android build "${TAURI_FLAGS[@]}" \
  --target aarch64 --apk 2>&1 | tee "$LOG"
STATUS=${PIPESTATUS[0]}
set -e

echo "== tauri exit status: $STATUS ==" | tee -a "$LOG"
echo "== APK artifacts =="        | tee -a "$LOG"
find "$ROOT/src-tauri/gen/android" -name '*.apk' -printf '%p  %s bytes\n' 2>/dev/null \
  | tee -a "$LOG"
echo "== log written: $LOG =="
exit "$STATUS"
