#!/usr/bin/env bash
# Founder-serialized gate run: push the aarch64 harness + libc++_shared.so +
# the model/adapter GGUFs to EVERY attached phone and run it in adb shell —
# tokens/sec + RSS (stream) and the behavioral probes (probe). No APK, no Tauri.
#
# ── 🔴 MULTI-DEVICE, AND WHY THAT WAS A BLOCKER RATHER THAN DEBT ─────────────
#
# This script used to assume exactly one phone: bare `$ADB` with no `-s`, and
# `adb get-serialno`, which does not merely pick one when two are attached — it
# **fails outright**. The 5.4 gate needs three devices and the founder is
# acquiring the second and third now, so the first thing that would have
# happened when they arrived is that the existing tooling broke, during the
# scarcest resource on this track.
#
# `discover_devices` + `adbs` are the pattern `airplane-mode.sh` already uses,
# and the two definitions are deliberately IDENTICAL. They are copied rather
# than extracted to a shared file because `airplane-mode.sh` is A2's evidence,
# verified against eleven mock scenarios whose mock no longer exists — editing
# it to hoist two functions would re-open that verification for a tidiness win.
# That leaves one fact with two homes (D-4), which is recorded rather than
# hidden: the extraction is the right end state and should happen the next time
# A2's harness has a mock to be re-verified against.
#
# Three properties the loop has that a single-device script did not need:
#   * a half-attached phone must not silently drop out. `discover_devices`
#     excludes `unauthorized` / `offline` / `no permissions`, and a device that
#     was never in the list is not a device that passed.
#   * one phone failing must not abort the others, and must not be lost. Each
#     device's failure is collected and named in the summary, and the script
#     exits non-zero if ANY device failed.
#   * every line of output is attributed to a serial. Three devices producing
#     one undifferentiated transcript is a gate you cannot act on.
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
#     4) adb devices  → every phone you mean to gate must be listed first.
#   Alternative (Windows-side adb sees USB natively): ADB=adb.exe ./run-on-device.sh …
#   NOTE: with adb.exe, LOCAL file paths are interpreted by Windows — keep the
#   binary + GGUFs on a Windows-visible path, or prefer wireless (Linux adb),
#   which handles WSL paths natively.
#
# ── Where the GGUFs come from ──
#   Fetch the hero stack from the signed catalog (public GET, sha256-verified):
#     ./fetch-artifacts.sh          # → ~/cleophis-artifacts/*.gguf
#   or point --model/--behavioral/etc. at any local GGUFs you already have.
#
# Usage:
#   ./run-on-device.sh --model P [--behavioral P] [--contract P] [--voice P] \
#                      [--template llama3|chatml|auto] [--prompt "..."] \
#                      [--serial R58N30ABCD]     # one device instead of all
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
#
# `-e` is deliberately NOT set, and the reason is NOT the one this comment
# first claimed.
#
# I asserted here that `[ -x "$TGT/probe" ] && $ADB push …` had been exiting
# the whole script when the probe binary was absent. **Measured, and it is
# false**: bash exempts every command in an `&&` list except the one after the
# final `&&`, so a failing left-hand side does not fire errexit and the list's
# non-zero status does not either. `set -e; false && echo x; echo REACHED`
# prints REACHED and exits 0. The old form did exactly what it looked like.
# Recorded rather than quietly deleted, because the write-up was already
# drafted and only running it caught the difference — the same shape as the two
# negative controls in this session that failed for a cause other than the one
# they were named for.
#
# The real reason is the loop. A three-device gate must not let the first
# phone's failure abort the other two, so `run_on` has to RETURN non-zero to a
# collector rather than exit — and a function called as an `if !` condition has
# errexit suspended for its whole body anyway (measured too). So `-e` would
# give no protection precisely where the work happens, while looking as though
# it did. Every failure that matters is checked explicitly instead.
set -uo pipefail

ADB="${ADB:-adb}"                      # override: ADB=adb.exe for Windows-side USB
: "${ANDROID_NDK_ROOT:?set ANDROID_NDK_ROOT}"; : "${CARGO_TARGET_DIR:?set CARGO_TARGET_DIR}"

MODEL="" BEH="" CON="" VOICE="" TEMPLATE="auto" ONLY_SERIAL=""
PROMPT="Say hello in one short sentence."
while [ $# -gt 0 ]; do case "$1" in
  --model) MODEL="$2"; shift 2;; --behavioral) BEH="$2"; shift 2;;
  --contract) CON="$2"; shift 2;; --voice) VOICE="$2"; shift 2;;
  --template) TEMPLATE="$2"; shift 2;; --prompt) PROMPT="$2"; shift 2;;
  --serial) ONLY_SERIAL="$2"; shift 2;;
  -h|--help) sed -n '1,70p' "$0"; exit 0;;
  *) echo "unknown arg: $1"; exit 2;; esac; done
[ -n "$MODEL" ] || { echo "--model <base.gguf> required"; exit 2; }

DEV=/data/local/tmp/cleophis
# RELEASE binaries only — a debug harness sandbags tok/s (same invalid-perf trap
# as scalar kernels). And the aarch64 build MUST carry the dotprod patch (see
# vendor-llama-sys-dotprod.sh) or quantized matmuls run scalar on the A22.
TGT="$CARGO_TARGET_DIR/aarch64-linux-android/release/examples"
LIBCXX="$ANDROID_NDK_ROOT/toolchains/llvm/prebuilt/linux-x86_64/sysroot/usr/lib/aarch64-linux-android/libc++_shared.so"
[ -x "$TGT/stream" ] || { echo "build first (RELEASE + dotprod patch applied): cargo ndk -t arm64-v8a -P 24 build --release --features real --example stream --example probe"; exit 1; }

# ── The hero systemPrompt (catalog data) — what the desktop probe script sends.
#
# 🔴 NO FALLBACK, and the fallback it replaces was worse than a crash. This
# used to end in `|| echo 'You are a helpful, honest tutor.'`, so a moved
# catalog, a JSON error or a missing python3 produced a DIFFERENT system prompt
# and the run continued looking exactly like a real one. The behavioral adapter
# is trained to answer to the catalog prompt; probing without it measures a
# configuration no user runs, and Stage-5 transcripts gathered that way would
# be compared against in-app ones as though they were the same gate.
CAT="$(cd "$(dirname "$0")/../../.." && pwd)/src-tauri/resources/catalog.json"
SYS="$(python3 -c "import json;c=json.load(open(r'$CAT'));h=[e for e in c if e.get('real')][0];print(h['systemPrompt'].strip())")" || {
  echo "FATAL: could not read the hero systemPrompt from $CAT." >&2
  echo "  A probe run with a substituted system prompt is a DIFFERENT GATE and" >&2
  echo "  would not be labelled as one, so this refuses rather than guesses." >&2
  exit 1
}

# ── Device discovery ──────────────────────────────────────────────────────────
#
# Byte-identical to `airplane-mode.sh`'s. Prints one serial per line for every
# device in state `device`, deliberately excluding `unauthorized`, `offline`
# and `no permissions`: a half-attached phone that silently drops out of the
# run is how a three-device gate reports two passes and no failures.
discover_devices() {
  "$ADB" devices | awk 'NR>1 && $2=="device" {print $1}'
}

adbs() { "$ADB" -s "$1" "${@:2}"; }

# Read into an array through process substitution, NOT `discover_devices |
# while read`. A piped `while` runs in a SUBSHELL, so anything it sets is lost
# and any `exit` inside it kills only the subshell — the bug that made
# airplane-mode.sh print a failure and exit 0, twice, in two disguises.
DEVICES=()
while IFS= read -r s; do [ -n "$s" ] && DEVICES+=("$s"); done < <(discover_devices)

if [ -n "$ONLY_SERIAL" ]; then
  # Verified against the discovered list rather than trusted. A typo would
  # otherwise sail through to a pile of "device not found" from adb, which
  # reads like a broken phone rather than a broken argument.
  found=0
  for s in "${DEVICES[@]:-}"; do [ "$s" = "$ONLY_SERIAL" ] && found=1; done
  [ "$found" = 1 ] || {
    echo "FATAL: --serial $ONLY_SERIAL is not attached (or is unauthorized/offline)." >&2
    echo "  attached and ready: ${DEVICES[*]:-none}" >&2
    exit 1
  }
  DEVICES=("$ONLY_SERIAL")
fi

[ "${#DEVICES[@]}" -gt 0 ] || {
  echo "FATAL: no device is attached and ready." >&2
  echo "  \`$ADB devices\` lists nothing in state 'device'. An unauthorized or" >&2
  echo "  offline phone is excluded on purpose — see the connection note above." >&2
  exit 1
}

push_to() { # serial, local path (may be empty)
  [ -n "$2" ] || return 0
  adbs "$1" push "$2" "$DEV/$(basename "$2")" >/dev/null
}

# Everything one phone needs, returning non-zero rather than aborting the run.
run_on() {
  local serial="$1"
  local model_name; model_name="$(adbs "$serial" shell getprop ro.product.model 2>/dev/null | tr -d '\r')"
  echo
  echo "############################################################"
  echo "## DEVICE $serial  (${model_name:-unknown model})"
  echo "############################################################"

  adbs "$serial" shell "mkdir -p $DEV" || { echo "[$serial] FAIL: cannot mkdir $DEV"; return 1; }
  push_to "$serial" "$TGT/stream" || { echo "[$serial] FAIL: pushing stream"; return 1; }
  # `if`, not `&&`, for legibility only — the old `&&` form was correct, see
  # the errexit note in the header. What IS new is the `|| return 1`: the push
  # itself was unchecked before, so a failed copy produced a harness that ran
  # the previous device's binary.
  if [ -x "$TGT/probe" ]; then
    push_to "$serial" "$TGT/probe" || { echo "[$serial] FAIL: pushing probe"; return 1; }
  fi
  push_to "$serial" "$LIBCXX" || { echo "[$serial] FAIL: pushing libc++_shared.so"; return 1; }
  # The libc++ push lands as its basename already; both harnesses cd to $DEV,
  # so adapters are passed as relative basenames.
  for f in "$MODEL" "$BEH" "$CON" "$VOICE"; do
    push_to "$serial" "$f" || { echo "[$serial] FAIL: pushing $f"; return 1; }
  done

  local lora=""
  [ -n "$BEH" ]   && lora="$lora --behavioral $(basename "$BEH")"
  [ -n "$CON" ]   && lora="$lora --contract $(basename "$CON")"
  [ -n "$VOICE" ] && lora="$lora --voice $(basename "$VOICE")"
  adbs "$serial" shell "chmod 755 $DEV/stream $DEV/probe 2>/dev/null || true"

  echo "== [$serial] [1] tokens/sec + RSS (stream, greedy) =="
  adbs "$serial" shell "cd $DEV && LD_LIBRARY_PATH=$DEV ./stream $(basename "$MODEL") $lora --template $TEMPLATE --temp 0 --prompt \"$PROMPT\"" \
    || { echo "[$serial] FAIL: stream harness exited non-zero"; return 1; }

  if adbs "$serial" shell "test -x $DEV/probe" 2>/dev/null; then
    echo "== [$serial] [2] behavioral probes (probe, honesty + socratic) =="
    adbs "$serial" shell "cd $DEV && LD_LIBRARY_PATH=$DEV ./probe --model $(basename "$MODEL") $lora --system \"$SYS\" --template $TEMPLATE --temp 0 --set all" \
      || { echo "[$serial] FAIL: probe harness exited non-zero"; return 1; }
  fi
  return 0
}

echo "== ${#DEVICES[@]} device(s) ready: ${DEVICES[*]} =="
FAILED=()
for serial in "${DEVICES[@]}"; do
  if ! run_on "$serial"; then FAILED+=("$serial"); fi
done

echo
echo "Gate checklist:"
echo " - KERNEL CHECK (spec H3): the '[kernels]' line printed at startup MUST show"
echo "   DOTPROD = 1 (and ideally MATMUL_INT8 = 1). If they are 0/absent, or tok/s"
echo "   is implausibly low (< ~5 tok/s on a modern phone), the ggml-cpu build is"
echo "   BASELINE armv8-a (scalar) — the numbers are invalid; rebuild with"
echo "   GGML_CPU_ALL_VARIANTS / GGML_CPU_ARM_ARCH before trusting perf."
echo " - Record tokens/sec + VmRSS/VmHWM and the probe transcript PER DEVICE. The"
echo "   5.4 gate is about the SPREAD across tiers, so a number without a serial"
echo "   beside it is not a result."
echo " - The catalog has no voice adapter yet — fullest real stack is"
echo "   behavioral+contract (Qwen3-4B). Pass --voice only once one exists."
echo " - This CLI probe is not the A3 gate. §11 A3 wants the probes IN-APP, which"
echo "   is chat_stage5_probe; these transcripts are for calibrating it."

if [ "${#FAILED[@]}" -gt 0 ]; then
  echo
  echo "FAILED on ${#FAILED[@]} of ${#DEVICES[@]} device(s): ${FAILED[*]}" >&2
  echo "  A device that failed is NOT a device that passed quietly — the run" >&2
  echo "  continued so the others still produced evidence, and this exit status" >&2
  echo "  is what stops that from reading as a clean sweep." >&2
  exit 1
fi
echo
echo "== all ${#DEVICES[@]} device(s) completed =="
