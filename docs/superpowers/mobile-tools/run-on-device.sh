#!/usr/bin/env bash
# Founder-serialized P0 gate run: push the aarch64 `stream` harness + its
# libc++_shared.so + the model/adapter GGUFs to a connected device and run it
# in adb shell, printing tokens/sec + RSS. No APK, no Tauri — the pure engine.
#
# Usage:
#   ./run-on-device.sh <hero.gguf> <behavioral.gguf> <contract.gguf> <voice.gguf> "<prompt>"
#
# Prereqs: USB debugging on, `adb devices` shows the phone, and the env from
# docs/superpowers/mobile-dev-setup.md is set (ANDROID_NDK_ROOT, CARGO_TARGET_DIR).
set -euo pipefail

HERO="${1:?hero base gguf}"; BEH="${2:?behavioral gguf}"; CON="${3:?contract gguf}"; VOICE="${4:?voice gguf}"
PROMPT="${5:-Say hello in one short sentence.}"
: "${ANDROID_NDK_ROOT:?}"; : "${CARGO_TARGET_DIR:?}"

DEV=/data/local/tmp/cleophis
BIN="$CARGO_TARGET_DIR/aarch64-linux-android/debug/examples/stream"
LIBCXX="$ANDROID_NDK_ROOT/toolchains/llvm/prebuilt/linux-x86_64/sysroot/usr/lib/aarch64-linux-android/libc++_shared.so"

[ -f "$BIN" ] || { echo "build first: cargo ndk -t arm64-v8a -P 24 build --features real --example stream"; exit 1; }

echo "== pushing to $DEV =="
adb shell "mkdir -p $DEV"
adb push "$BIN" "$DEV/stream"
adb push "$LIBCXX" "$DEV/libc++_shared.so"
for f in "$HERO" "$BEH" "$CON" "$VOICE"; do adb push "$f" "$DEV/$(basename "$f")"; done
adb shell "chmod 755 $DEV/stream"

echo "== running (full behavioral->contract->voice stack, greedy) =="
adb shell "cd $DEV && LD_LIBRARY_PATH=$DEV ./stream $(basename "$HERO") \
  --behavioral $(basename "$BEH") --contract $(basename "$CON") --voice $(basename "$VOICE") \
  --template llama3 --temp 0 --prompt \"$PROMPT\""

echo
echo "Gate checklist: capture tokens/sec + VmRSS/VmHWM above, then re-run with the"
echo "four Stage-5 probe prompts (fake-entity refusal, 5+5=9 pushback, concession,"
echo "medical boundary) and record the 4/4 transcript."
