#!/usr/bin/env bash
# Founder-serialized P0 gate run: push the aarch64 harness + libc++_shared.so +
# the model/adapter GGUFs to a connected phone and run it in adb shell —
# tokens/sec + RSS (stream) and the behavioral probes (probe). No APK, no Tauri.
#
# ── Connecting the phone from WSL2 (USB is NOT visible to WSL Linux by default) ──
#   Samsung Galaxy A22 (the P0 reference device), WIRELESS debugging:
#     1) Enable Developer options: Settings → About phone → Software information →
#        tap "Build number" 7 times.
#     2) Settings → Developer options → turn ON "Wireless debugging" → tap it →
#        "Pair device with pairing code" (shows an IP:PORT and a 6-digit code).
#     3) WSL:  adb pair <ip>:<pair-port>          (enter the 6-digit code)
#              adb connect <ip>:<debug-port>      (the port on the main Wireless-
#                                                  debugging screen, NOT the pair port)
#     4) adb devices  → the A22 must be listed before running this script.
#   Alternative (Windows-side adb sees USB natively): ADB=adb.exe ./run-on-device.sh …
#   NOTE: with adb.exe, local file paths are interpreted by Windows — prefer
#   wireless (Linux adb), which handles WSL paths natively.
#   NOTE: with adb.exe, LOCAL file paths are interpreted by Windows — keep the
#   binary + GGUFs on a Windows-visible path, or prefer wireless (Linux adb),
#   which handles WSL paths natively. `$ADB devices` must list the phone first.
#
# ── Where the GGUFs come from ──
#   Fetch the hero stack from the signed catalog (public GET, sha256-verified):
#     ./fetch-artifacts.sh          # → ~/cleophis-artifacts/*.gguf
#   or point --model/--behavioral/etc. at any local GGUFs you already have.
#
# Usage:
#   ./run-on-device.sh --model P [--behavioral P] [--contract P] [--voice P] \
#                      [--template llama3|chatml|auto] [--prompt "..."]
#
# ── P0 reference bundles for the Galaxy A22 (8 GB, workhorse tier) ──
#   A=~/cleophis-artifacts
#   [1] Gate config — 1B Llama + behavioral (Llama template):
#     ./run-on-device.sh --model $A/llama-1b-base.gguf \
#       --behavioral $A/llama-1b-behavioral.gguf --template llama3
#   [2] H14 encore — 4B Qwen + behavioral + contract (ChatML, 2-LoRA stack):
#     ./run-on-device.sh --model $A/qwen-4b-base.gguf \
#       --behavioral $A/qwen-4b-behavioral.gguf --contract $A/qwen-4b-contract.gguf \
#       --template chatml
#   The A22 is Cortex-A55-class: dotprod YES, i8mm NO. The binary MUST be built
#   from the dotprod build (vendor-llama-sys-dotprod.sh) — the [kernels] line
#   must show DOTPROD = 1 (and NOT crash, which +i8mm would).
set -euo pipefail

ADB="${ADB:-adb}"                      # override: ADB=adb.exe for Windows-side USB
: "${ANDROID_NDK_ROOT:?set ANDROID_NDK_ROOT}"; : "${CARGO_TARGET_DIR:?set CARGO_TARGET_DIR}"

MODEL="" BEH="" CON="" VOICE="" TEMPLATE="auto" PROMPT="Say hello in one short sentence."
while [ $# -gt 0 ]; do case "$1" in
  --model) MODEL="$2"; shift 2;; --behavioral) BEH="$2"; shift 2;;
  --contract) CON="$2"; shift 2;; --voice) VOICE="$2"; shift 2;;
  --template) TEMPLATE="$2"; shift 2;; --prompt) PROMPT="$2"; shift 2;;
  *) echo "unknown arg: $1"; exit 2;; esac; done
[ -n "$MODEL" ] || { echo "--model <base.gguf> required"; exit 2; }

DEV=/data/local/tmp/cleophis
TGT="$CARGO_TARGET_DIR/aarch64-linux-android/debug/examples"
LIBCXX="$ANDROID_NDK_ROOT/toolchains/llvm/prebuilt/linux-x86_64/sysroot/usr/lib/aarch64-linux-android/libc++_shared.so"
[ -x "$TGT/stream" ] || { echo "build first: cargo ndk -t arm64-v8a -P 24 build --features real --example stream --example probe"; exit 1; }

# The hero systemPrompt (catalog data) — what the desktop probe script sends.
CAT="$(cd "$(dirname "$0")/../../.." && pwd)/src-tauri/resources/catalog.json"
SYS="$(python3 -c "import json;c=json.load(open(r'$CAT'));h=[e for e in c if e.get('real')][0];print(h['systemPrompt'].strip())" 2>/dev/null || echo 'You are a helpful, honest tutor.')"

echo "== $($ADB get-serialno 2>/dev/null || echo 'no device — see connection note above') =="
$ADB shell "mkdir -p $DEV"
$ADB push "$TGT/stream" "$DEV/stream" >/dev/null
[ -x "$TGT/probe" ] && $ADB push "$TGT/probe" "$DEV/probe" >/dev/null
$ADB push "$LIBCXX" "$DEV/libc++_shared.so" >/dev/null

# Both harnesses cd to $DEV, so adapters are passed as relative basenames.
push() { [ -n "$1" ] || return 0; $ADB push "$1" "$DEV/$(basename "$1")" >/dev/null; }
push "$MODEL"; push "$BEH"; push "$CON"; push "$VOICE"
LORA=""
[ -n "$BEH" ]   && LORA="$LORA --behavioral $(basename "$BEH")"
[ -n "$CON" ]   && LORA="$LORA --contract $(basename "$CON")"
[ -n "$VOICE" ] && LORA="$LORA --voice $(basename "$VOICE")"
$ADB shell "chmod 755 $DEV/stream $DEV/probe 2>/dev/null || true"

echo "== [1] tokens/sec + RSS (stream, greedy) =="
$ADB shell "cd $DEV && LD_LIBRARY_PATH=$DEV ./stream $(basename "$MODEL") $LORA --template $TEMPLATE --temp 0 --prompt \"$PROMPT\""

if $ADB shell "test -x $DEV/probe" 2>/dev/null; then
  echo "== [2] behavioral probes (probe, honesty + socratic) =="
  $ADB shell "cd $DEV && LD_LIBRARY_PATH=$DEV ./probe --model $(basename "$MODEL") $LORA --system \"$SYS\" --template $TEMPLATE --temp 0 --set all"
fi

echo
echo "Gate checklist:"
echo " - KERNEL CHECK (spec H3): the '[kernels]' line printed at startup MUST show"
echo "   DOTPROD = 1 (and ideally MATMUL_INT8 = 1). If they are 0/absent, or tok/s"
echo "   is implausibly low (< ~5 tok/s on a modern phone), the ggml-cpu build is"
echo "   BASELINE armv8-a (scalar) — the numbers are invalid; rebuild with"
echo "   GGML_CPU_ALL_VARIANTS / GGML_CPU_ARM_ARCH before trusting perf."
echo " - Record tokens/sec + VmRSS/VmHWM and the probe transcript (4/4 Stage-5)."
echo " - The catalog has no voice adapter yet — fullest real stack is"
echo "   behavioral+contract (Qwen3-4B). Pass --voice only once one exists."
