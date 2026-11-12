#!/usr/bin/env bash
# run-vision-bench.sh — Stage 0 of the Vision Input spike (2026-11-01).
#
# Pushes the aarch64 `vision_bench` example, libc++_shared, one candidate model +
# projector and the test images to the phone, verifies every file's sha256 ON THE
# DEVICE (an interrupted adb push leaves a short file and still exits 0 — the
# lesson run-device-probes.sh records), runs the bench, and pulls one JSONL of
# records. Re-runs skip pushes whose device sha already matches.
#
# Usage:
#   ADB=~/bin/adb-win SERIAL=R9WR50C0W1J \
#   docs/superpowers/mobile-tools/run-vision-bench.sh \
#     --model ~/cleophis-artifacts/vision/granite-docling-258M-Q8_0.gguf \
#     --mmproj ~/cleophis-artifacts/vision/mmproj-granite-docling-258M-Q8_0.gguf \
#     --family docling --items ~/cleophis-artifacts/vision/spike-images/items.json \
#     --out ~/cleophis-artifacts/vision/runs/a22-docling-512-t4.jsonl \
#     [-- extra vision_bench flags, e.g. --max-side 512 --threads 4 --repeat 3]
#
# Env: ADB (default adb), SERIAL (required when several devices), ANDROID_NDK_ROOT
#      (for libc++_shared.so), CARGO_TARGET_DIR (default ~/cleophis-mobile-target),
#      NICE (default "nice -n 0").
set -euo pipefail

ADB="${ADB:-adb}"
SERIAL="${SERIAL:-}"
NDK="${ANDROID_NDK_ROOT:-$HOME/android-ndk-r27c}"
TARGET="${CARGO_TARGET_DIR:-$HOME/cleophis-mobile-target}"
BIN="$TARGET/aarch64-linux-android/release/examples/vision_bench"
LIBCXX="$NDK/toolchains/llvm/prebuilt/linux-x86_64/sysroot/usr/lib/aarch64-linux-android/libc++_shared.so"
DEV=/data/local/tmp/cleophis-vision

MODEL=""; MMPROJ=""; FAMILY=""; ITEMS=""; OUT=""
while [ $# -gt 0 ]; do
  case "$1" in
    --model) MODEL="$2"; shift 2;;
    --mmproj) MMPROJ="$2"; shift 2;;
    --family) FAMILY="$2"; shift 2;;
    --items) ITEMS="$2"; shift 2;;
    --out) OUT="$2"; shift 2;;
    --) shift; break;;
    *) echo "unknown flag $1" >&2; exit 2;;
  esac
done
EXTRA=("$@")
[ -n "$MODEL" ] && [ -n "$MMPROJ" ] && [ -n "$FAMILY" ] && [ -n "$ITEMS" ] && [ -n "$OUT" ] \
  || { echo "need --model --mmproj --family --items --out" >&2; exit 2; }
[ -f "$BIN" ] || { echo "FATAL: $BIN not built (cargo ndk -t arm64-v8a -P 24 build --release -p kpack-engine --features vision --example vision_bench)" >&2; exit 1; }
[ -f "$LIBCXX" ] || { echo "FATAL: libc++_shared.so not found at $LIBCXX" >&2; exit 1; }
[ -f "$ITEMS" ] || { echo "FATAL: $ITEMS missing" >&2; exit 1; }

adbs() { if [ -n "$SERIAL" ]; then "$ADB" -s "$SERIAL" "$@"; else "$ADB" "$@"; fi; }
state="$(adbs get-state 2>/dev/null | tr -d '\r' || true)"
[ "$state" = "device" ] || { echo "FATAL: device state is '${state:-none}' (ADB=$ADB SERIAL=${SERIAL:-unset})" >&2; exit 1; }

# The device's sha tool (toybox on Samsung).
if adbs shell "command -v sha256sum >/dev/null 2>&1 && echo yes" 2>/dev/null | grep -q yes; then SHA_DEV="sha256sum"
elif adbs shell "toybox sha256sum </dev/null >/dev/null 2>&1 && echo yes" 2>/dev/null | grep -q yes; then SHA_DEV="toybox sha256sum"
else echo "FATAL: no sha256sum on the device; the post-push check would be blind" >&2; exit 1; fi

adbs shell "mkdir -p $DEV/images" >/dev/null

# A Windows adb reached from WSL (the only adb that sees the USB phone) cannot read
# WSL paths: `adb push /home/...` fails and an unchecked script sails on. When
# WINSTAGE is set (a WSL path under /mnt/c) every file is copied there first and
# pushed/pulled by its Windows path; WINSTAGE_WIN is the same directory as Windows
# spells it. Both unset = a native adb that reads WSL paths directly.
WINSTAGE="${WINSTAGE:-}"
WINSTAGE_WIN="${WINSTAGE_WIN:-}"
[ -z "$WINSTAGE" ] || [ -n "$WINSTAGE_WIN" ] || { echo "FATAL: WINSTAGE needs WINSTAGE_WIN (the same dir as a C:\\ path)" >&2; exit 1; }
[ -z "$WINSTAGE" ] || mkdir -p "$WINSTAGE"
host_path_for_push() {  # <wsl path> -> the path adb may read
  local src="$1"
  if [ -z "$WINSTAGE" ]; then printf '%s' "$src"; return; fi
  local name; name="$(basename "$src")"
  # Byte-compare, not size-compare: two PNGs of different shape matched in size once
  # and a stale staged copy reached the phone.
  if [ ! -f "$WINSTAGE/$name" ] || ! cmp -s "$WINSTAGE/$name" "$src"; then cp "$src" "$WINSTAGE/$name"; fi
  printf '%s\\%s' "$WINSTAGE_WIN" "$name"
}

# push_verified <local> <remote>: skip when the device sha already matches; verify after a push.
push_verified() {
  local src="$1" dst="$2" want got
  want="$(sha256sum "$src" | cut -d' ' -f1)"
  got="$(adbs shell "$SHA_DEV $dst 2>/dev/null" | tr -d '\r' | cut -d' ' -f1 || true)"
  if [ "$got" = "$want" ]; then echo "  skip (sha ok on device): $(basename "$src")"; return 0; fi
  echo "  push $(basename "$src") ($(stat -c %s "$src") B)"
  adbs push "$(host_path_for_push "$src")" "$dst" >/dev/null || { echo "FATAL: adb push of $(basename "$src") failed" >&2; exit 1; }
  got="$(adbs shell "$SHA_DEV $dst" | tr -d '\r' | cut -d' ' -f1)"
  [ "$got" = "$want" ] || { echo "FATAL: $(basename "$src") sha on the DEVICE is $got, expected $want" >&2; exit 1; }
}

echo "== pushing to $DEV (sha-verified) =="
push_verified "$BIN" "$DEV/vision_bench"
push_verified "$LIBCXX" "$DEV/libc++_shared.so"
push_verified "$MODEL" "$DEV/$(basename "$MODEL")"
push_verified "$MMPROJ" "$DEV/$(basename "$MMPROJ")"
ITEMS_DIR="$(cd "$(dirname "$ITEMS")" && pwd)"
push_verified "$ITEMS" "$DEV/images/items.json"
# An array, not a pipe into `while read`: adb inside the loop reads stdin and swallowed
# the rest of the file list, so only the first image was ever pushed (2026-11-01).
mapfile -t ITEM_FILES < <(python3 -c 'import json,sys
for it in json.load(open(sys.argv[1]))["items"]: print(it["file"])' "$ITEMS")
[ "${#ITEM_FILES[@]}" -gt 0 ] || { echo "FATAL: no items in $ITEMS" >&2; exit 1; }
echo "  ${#ITEM_FILES[@]} image(s) listed in items.json"
for f in "${ITEM_FILES[@]}"; do push_verified "$ITEMS_DIR/$f" "$DEV/images/$f" </dev/null; done
adbs shell "chmod 755 $DEV/vision_bench" >/dev/null

mkdir -p "$(dirname "$OUT")"
DEVJSON="$DEV/out-$(date -u +%Y%m%dT%H%M%SZ).jsonl"
LOG="${OUT%.jsonl}.log"
echo "== running on the device (log $LOG) =="
# The extra flags cross a second shell on the device: single-quote each one so a
# multi-word --prompt survives (an unquoted expansion split it into "flags" once).
EXTRA_Q=""
for arg in "${EXTRA[@]+"${EXTRA[@]}"}"; do EXTRA_Q+=" '${arg//\'/\'\\\'\'}'"; done
# BENCH_ENV: extra environment for the bench on the device (kept for experiments; the one-pass switch is now the bench's own --one-pass flag).
adbs shell "cd $DEV && LD_LIBRARY_PATH=$DEV ${BENCH_ENV:-} ${NICE:-nice -n 0} ./vision_bench \
  --model $DEV/$(basename "$MODEL") --mmproj $DEV/$(basename "$MMPROJ") --family $FAMILY \
  --items $DEV/images/items.json --json $DEVJSON$EXTRA_Q" 2>&1 | tee "$LOG"
if grep -q "INCOMPATIBLE" "$LOG"; then
  echo "FAIL: [cpu-verdict] INCOMPATIBLE — the build's kernels exceed this CPU; the floor check is the fix, not a re-run" >&2
  exit 1
fi
if [ -n "$WINSTAGE" ]; then
  adbs pull "$DEVJSON" "$WINSTAGE_WIN\\pulled.jsonl" >/dev/null || { echo "FATAL: adb pull failed" >&2; exit 1; }
  cp "$WINSTAGE/pulled.jsonl" "$OUT"
else
  adbs pull "$DEVJSON" "$OUT" >/dev/null || { echo "FATAL: adb pull failed" >&2; exit 1; }
fi
n="$(wc -l < "$OUT")"
echo "== pulled $OUT ($n records) =="
python3 - "$OUT" <<'EOF'
import json,sys,statistics
rows=[json.loads(l) for l in open(sys.argv[1]) if l.strip()]
ok=[r for r in rows if 'error' not in r]
if not ok: print('no successful records'); sys.exit(0)
tot=[r['total_ms'] for r in ok]; tot.sort()
p50=tot[len(tot)//2]; p95=tot[min(len(tot)-1, int(round(0.95*(len(tot)-1))))]
print('  records %d ok %d | total_ms p50 %d p95 %d | max VmHWM %d kB | load model %d ms, mmproj %d ms' % (
    len(rows), len(ok), p50, p95, max(r['vmhwm_kb'] for r in ok), ok[0]['load_model_ms'], ok[0]['load_mmproj_ms']))
for r in ok: print('  %-22s rep%d %4dx%-4d img_tok %4d | enc %s prefill %5d dec %5d (%d tok) | %s' % (
    r['id'], r['rep'], r['width'], r['height'], r['n_image_tokens'], r.get('encode_ms'), r['prefill_ms'], r['decode_ms'], r['n_gen'], r['raw'][:60].replace('\n','\\n')))
EOF
