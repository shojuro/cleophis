#!/usr/bin/env bash
# run-vision-probes.sh — Stage 3 of the Vision Input work (2026-11-13).
#
# The probe set on the phone: pushes the aarch64 `vision_bench`, libc++_shared, one
# model + projector and the whole fixture directory probes/vision/fixtures/v1/ (18 PNGs,
# items.json, manifest.json), verifies EVERY file's sha256 ON THE DEVICE (an interrupted
# adb push leaves a short file and still exits 0), runs the bench through the engine
# (`--via-engine --json`, the flags Task 5's CI job uses), pulls the JSONL and scores it
# with the DEVICE gates (probes/vision/score.mjs --device). Re-runs skip pushes whose
# device sha already matches. Fixture files are pinned by manifest.json: a local file
# whose sha differs from the manifest aborts the run before anything is pushed.
#
# Usage (the founder's A22 run):
#   ADB=adbw SERIAL=R9WR50C0W1J docs/superpowers/mobile-tools/run-vision-probes.sh \
#     --model ~/cleophis-artifacts/vision/granite-docling-258M-Q8_0.gguf \
#     --mmproj ~/cleophis-artifacts/vision/mmproj-granite-docling-258M-Q8_0.preproc512.gguf \
#     --bench <path to the aarch64 vision_bench binary> \
#     --out ~/cleophis-artifacts/vision/runs/a22-s3-v1.jsonl
#   Add --dry-run to print every adb command (and the score command) without a phone.
#   Optional: --fixtures <dir> (default probes/vision/fixtures/v1 of this checkout),
#   `-- extra vision_bench flags` after the script's own.
#
# Outputs next to --out (a.jsonl): a.log (device stdout/stderr), a.md + a.json (the
# score report), a.meminfo.txt (one `dumpsys meminfo <pid>` of the running bench,
# taken ~20 s in; also appended to a.md).
#
# Which adb (the same caveat run-vision-bench.sh documents): the Linux `adb` on this
# laptop sees no USB device. The Windows adb does, via the wrapper `adbw`
# (~/.local/bin/adbw -> adb.exe), and adb.exe reads LOCAL paths as WINDOWS paths, so
# a WSL path like /home/... fails to push. When ADB is adbw or *.exe (or STAGE is set)
# every file that is pushed is first copied under STAGE (a /mnt/c directory this script
# creates) and pushed by its Windows path (wslpath -w); pulls land in STAGE and are
# copied to --out. With a native Linux adb (ADB=adb) nothing is staged and WSL paths
# are used directly.
#
# Memory: the bench is a plain binary, so this script runs it in the BACKGROUND on the
# device (nohup, via a small runner script), polls pidof vision_bench, takes one
# `dumpsys meminfo <pid>` at ~MEM_AT seconds, then waits for the exit-code file. If the
# bench ends before the snapshot, the report says so and falls back to the VmHWM the
# JSONL records carry (vmhwm_kb).
#
# Env: ADB (default adb), SERIAL (required when several devices), STAGE (default
#      /mnt/c/Users/JM505 Computers/dev/cleophis-vision-probes), ANDROID_NDK_ROOT
#      (for libc++_shared.so), CARGO_TARGET_DIR (default ~/cleophis-mobile-target; the
#      default --bench is its aarch64 release example), NICE (default "nice -n 0"),
#      MEM_AT (default 20), BENCH_TIMEOUT (default 3600 s).
# Exit: the score's — 0 all gates pass, 1 a gate missed, 2 a record missing; 1 for any
#       setup/device failure (including the INCOMPATIBLE-CPU verdict).
set -euo pipefail

ADB="${ADB:-adb}"
SERIAL="${SERIAL:-}"
NDK="${ANDROID_NDK_ROOT:-$HOME/android-ndk-r27c}"
TARGET="${CARGO_TARGET_DIR:-$HOME/cleophis-mobile-target}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
LIBCXX="$NDK/toolchains/llvm/prebuilt/linux-x86_64/sysroot/usr/lib/aarch64-linux-android/libc++_shared.so"
DEV=/data/local/tmp/cleophis-vision-probes
MEM_AT="${MEM_AT:-20}"
BENCH_TIMEOUT="${BENCH_TIMEOUT:-3600}"

MODEL=""; MMPROJ=""; BIN="$TARGET/aarch64-linux-android/release/examples/vision_bench"
FIXTURES="$ROOT/probes/vision/fixtures/v1"; OUT=""; DRY=0
while [ $# -gt 0 ]; do
  case "$1" in
    --model) MODEL="$2"; shift 2;;
    --mmproj) MMPROJ="$2"; shift 2;;
    --bench) BIN="$2"; shift 2;;
    --fixtures) FIXTURES="$2"; shift 2;;
    --out) OUT="$2"; shift 2;;
    --dry-run) DRY=1; shift;;
    --) shift; break;;
    *) echo "unknown flag $1" >&2; exit 2;;
  esac
done
EXTRA=("$@")
[ -n "$MODEL" ] && [ -n "$MMPROJ" ] && [ -n "$OUT" ] || { echo "need --model --mmproj --out (and --bench unless the default build exists)" >&2; exit 2; }
case "$OUT" in *.jsonl) ;; *) echo "--out must end in .jsonl" >&2; exit 2;; esac
BASE="${OUT%.jsonl}"; LOG="$BASE.log"; MD="$BASE.md"; JSON="$BASE.json"; MEMFILE="$BASE.meminfo.txt"
ITEMS="$FIXTURES/items.json"; MANIFEST="$FIXTURES/manifest.json"

# Missing local inputs: fatal on a real run, a warning in --dry-run (the plan is still shown).
need_file() {
  [ -f "$1" ] && return 0
  if [ "$DRY" = 1 ]; then echo "  (dry-run: $1 is not present here)" >&2; return 0; fi
  echo "FATAL: $2" >&2; exit 1
}
need_file "$BIN" "$BIN not built/found (cargo ndk -t arm64-v8a -P 24 build --release -p kpack-engine --features vision --example vision_bench), or pass --bench"
need_file "$LIBCXX" "libc++_shared.so not found at $LIBCXX (set ANDROID_NDK_ROOT)"
need_file "$MODEL" "$MODEL missing"
need_file "$MMPROJ" "$MMPROJ missing"
[ -f "$ITEMS" ] && [ -f "$MANIFEST" ] || { echo "FATAL: $FIXTURES needs items.json and manifest.json" >&2; exit 1; }

# adb wrapper. In --dry-run it prints the command and returns success with no output.
adbs() {
  if [ "$DRY" = 1 ]; then
    printf '+ %s%s' "$ADB" "${SERIAL:+ -s $SERIAL}" >&2; printf ' %q' "$@" >&2; printf '\n' >&2; return 0
  fi
  if [ -n "$SERIAL" ]; then "$ADB" -s "$SERIAL" "$@"; else "$ADB" "$@"; fi
}

SHA_DEV="sha256sum"
if [ "$DRY" != 1 ]; then
  state="$(adbs get-state 2>/dev/null | tr -d '\r' || true)"
  [ "$state" = "device" ] || { echo "FATAL: device state is '${state:-none}' (ADB=$ADB SERIAL=${SERIAL:-unset})" >&2; exit 1; }
  # The device's sha tool (toybox on Samsung).
  if adbs shell "command -v sha256sum >/dev/null 2>&1 && echo yes" 2>/dev/null | grep -q yes; then SHA_DEV="sha256sum"
  elif adbs shell "toybox sha256sum </dev/null >/dev/null 2>&1 && echo yes" 2>/dev/null | grep -q yes; then SHA_DEV="toybox sha256sum"
  else echo "FATAL: no sha256sum on the device; the post-push check would be blind" >&2; exit 1; fi
fi

# Staging for a Windows adb (see the header). STAGE_ON = 1 when files go through STAGE.
STAGE_ON=0
case "$(basename "$ADB")" in adbw|*.exe) STAGE_ON=1;; esac
[ -z "${STAGE:-}" ] || STAGE_ON=1   # an explicit STAGE forces staging, whatever ADB is
STAGE="${STAGE:-/mnt/c/Users/JM505 Computers/dev/cleophis-vision-probes}"
if [ "$STAGE_ON" = 1 ]; then
  command -v wslpath >/dev/null || { echo "FATAL: staging for a Windows adb needs wslpath" >&2; exit 1; }
  [ "$DRY" = 1 ] || mkdir -p "$STAGE"
fi
host_path() {  # <wsl path> -> the path adb may read (copies into STAGE when staging)
  local src="$1" name; name="$(basename "$src")"
  if [ "$STAGE_ON" = 0 ]; then printf '%s' "$src"; return; fi
  if [ "$DRY" != 1 ]; then
    # Byte-compare, not size-compare: a stale staged copy of a same-sized file once reached the phone.
    if [ ! -f "$STAGE/$name" ] || ! cmp -s "$STAGE/$name" "$src"; then cp "$src" "$STAGE/$name"; fi
    wslpath -w "$STAGE/$name"
  else
    printf 'C:\\<stage>\\%s' "$name"
  fi
}

# Local sha of a file; a placeholder in --dry-run when the file is absent.
local_sha() { if [ -f "$1" ]; then sha256sum "$1" | cut -d' ' -f1; else echo "<sha of $(basename "$1")>"; fi; }

# push_verified <local> <remote> [<expected sha>]: skip when the device sha already
# matches; verify ON THE DEVICE after a push.
push_verified() {
  local src="$1" dst="$2" want="${3:-}" got
  [ -n "$want" ] || want="$(local_sha "$src")"
  got="$(adbs shell "$SHA_DEV $dst 2>/dev/null" | tr -d '\r' | cut -d' ' -f1 || true)"
  if [ "$got" = "$want" ]; then echo "  skip (sha ok on device): $(basename "$src")"; return 0; fi
  echo "  push $(basename "$src")"
  adbs push "$(host_path "$src")" "$dst" >/dev/null || { echo "FATAL: adb push of $(basename "$src") failed" >&2; exit 1; }
  [ "$DRY" = 1 ] && return 0
  got="$(adbs shell "$SHA_DEV $dst" | tr -d '\r' | cut -d' ' -f1)"
  [ "$got" = "$want" ] || { echo "FATAL: $(basename "$src") sha on the DEVICE is $got, expected $want" >&2; exit 1; }
}

# The manifest is the pin: every fixture PNG's local sha must equal it before any push.
# An array, not a pipe into `while read`: adb inside a loop reads stdin and swallows the rest of the list.
mapfile -t MANIFEST_LINES < <(python3 -c 'import json,sys
m=json.load(open(sys.argv[1]))
for f,h in sorted(m["files"].items()): print(f+" "+h)' "$MANIFEST")
[ "${#MANIFEST_LINES[@]}" -gt 0 ] || { echo "FATAL: no files in $MANIFEST" >&2; exit 1; }
for line in "${MANIFEST_LINES[@]}"; do
  f="${line% *}"; h="${line#* }"
  [ -f "$FIXTURES/$f" ] || { echo "FATAL: $FIXTURES/$f listed in manifest.json but missing" >&2; exit 1; }
  [ "$(sha256sum "$FIXTURES/$f" | cut -d' ' -f1)" = "$h" ] || { echo "FATAL: $f differs from the manifest pin (regenerate or restore the fixture)" >&2; exit 1; }
done
echo "== fixtures match manifest.json (${#MANIFEST_LINES[@]} files) =="

adbs shell "mkdir -p $DEV/fixtures" >/dev/null
echo "== pushing to $DEV (sha-verified on the device) =="
push_verified "$BIN" "$DEV/vision_bench"
push_verified "$LIBCXX" "$DEV/libc++_shared.so"
push_verified "$MODEL" "$DEV/$(basename "$MODEL")"
push_verified "$MMPROJ" "$DEV/$(basename "$MMPROJ")"
push_verified "$ITEMS" "$DEV/fixtures/items.json"
push_verified "$MANIFEST" "$DEV/fixtures/manifest.json"
for line in "${MANIFEST_LINES[@]}"; do
  f="${line% *}"; h="${line#* }"
  push_verified "$FIXTURES/$f" "$DEV/fixtures/$f" "$h" </dev/null
done
adbs shell "chmod 755 $DEV/vision_bench" >/dev/null

# Runner script on the device: the bench in the background needs its own exit-code file.
# Extra flags are single-quoted into this LOCAL file, so multi-word values survive.
DEVLOG="$DEV/bench.log"; DEVRC="$DEV/bench.rc"; DEVJSON="$DEV/out-$(date -u +%Y%m%dT%H%M%SZ).jsonl"
EXTRA_Q=""
for arg in "${EXTRA[@]+"${EXTRA[@]}"}"; do EXTRA_Q+=" '${arg//\'/\'\\\'\'}'"; done
RUNNER_LOCAL="$(mktemp)"; trap 'rm -f "$RUNNER_LOCAL"' EXIT
cat > "$RUNNER_LOCAL" <<RUN
#!/system/bin/sh
cd $DEV
rm -f $DEVRC
LD_LIBRARY_PATH=$DEV ${BENCH_ENV:-} ${NICE:-nice -n 0} ./vision_bench --via-engine --family docling \\
  --model $DEV/$(basename "$MODEL") --mmproj $DEV/$(basename "$MMPROJ") \\
  --items $DEV/fixtures/items.json --max-side 512 --threads 4 --one-pass \\
  --json $DEVJSON$EXTRA_Q > $DEVLOG 2>&1
echo \$? > $DEVRC
RUN
echo "== runner script (pushed to $DEV/run-bench.sh) =="; sed 's/^/  | /' "$RUNNER_LOCAL"
RUNNER_NAME="run-bench.sh"
if [ "$STAGE_ON" = 1 ] && [ "$DRY" != 1 ]; then cp "$RUNNER_LOCAL" "$STAGE/$RUNNER_NAME"; RUNNER_PUSH="$(wslpath -w "$STAGE/$RUNNER_NAME")"
elif [ "$STAGE_ON" = 1 ]; then RUNNER_PUSH="C:\\<stage>\\$RUNNER_NAME"
else RUNNER_PUSH="$RUNNER_LOCAL"; fi
adbs push "$RUNNER_PUSH" "$DEV/$RUNNER_NAME" >/dev/null || { echo "FATAL: adb push of the runner failed" >&2; exit 1; }

[ "$DRY" = 1 ] || { mkdir -p "$(dirname "$OUT")"; : > "$MEMFILE"; }
echo "== running on the device in the background (log $LOG) =="
adbs shell "rm -f $DEVLOG $DEVRC; nohup sh $DEV/$RUNNER_NAME >/dev/null 2>&1 &" >/dev/null
MEM_NOTE=""
if [ "$DRY" = 1 ]; then
  echo "  (dry-run) poll: pidof vision_bench; at ~${MEM_AT}s: dumpsys meminfo <pid> > $MEMFILE; wait for $DEVRC (timeout ${BENCH_TIMEOUT}s)"
  adbs shell "dumpsys meminfo \$(pidof vision_bench)" >/dev/null
  adbs shell "cat $DEVLOG" >/dev/null
  adbs pull "$DEVJSON" "$OUT" >/dev/null
  PIPE_RC=0
else
  pid=""; t=0
  while [ -z "$pid" ] && [ "$t" -lt 60 ]; do
    pid="$(adbs shell "pidof vision_bench" 2>/dev/null | tr -d '\r' | awk '{print $1}' || true)"
    [ -n "$pid" ] || { adbs shell "test -f $DEVRC" >/dev/null 2>&1 && break; sleep 1; t=$((t+1)); }
  done
  if [ -n "$pid" ]; then
    echo "  bench pid $pid; snapshot dumpsys meminfo in ${MEM_AT}s"
    sleep "$MEM_AT"
    if adbs shell "kill -0 $pid" >/dev/null 2>&1; then
      { echo "# dumpsys meminfo $pid (vision_bench, ~${MEM_AT}s into the run)"; adbs shell "dumpsys meminfo $pid" | tr -d '\r'; } > "$MEMFILE"
      MEM_NOTE="captured"
    else
      echo "# bench pid $pid had already exited at the ${MEM_AT}s mark; no meminfo snapshot (see VmHWM in the records)" > "$MEMFILE"
      MEM_NOTE="bench exited before the snapshot"
    fi
  else
    echo "# bench pid never seen (finished within the poll window or failed to start); no meminfo snapshot (see VmHWM in the records)" > "$MEMFILE"
    MEM_NOTE="no pid seen"
  fi
  t=0
  until adbs shell "test -f $DEVRC" >/dev/null 2>&1; do
    [ "$t" -lt "$BENCH_TIMEOUT" ] || { echo "FATAL: bench did not finish within ${BENCH_TIMEOUT}s (device log: $DEVLOG)" >&2; exit 1; }
    sleep 5; t=$((t+5))
  done
  adbs shell "cat $DEVLOG" | tr -d '\r' | tee "$LOG"
  PIPE_RC="$(adbs shell "cat $DEVRC" | tr -d '\r' | tr -d '[:space:]')"
  echo "  meminfo: $MEM_NOTE ($MEMFILE)"
fi
if grep -q "INCOMPATIBLE" "$LOG" 2>/dev/null; then
  echo "FAIL: [cpu-verdict] INCOMPATIBLE — the build's kernels exceed this CPU; the floor check is the fix, not a re-run" >&2
  exit 1
fi
[ "${PIPE_RC:-0}" = 0 ] || { echo "FATAL: vision_bench exited $PIPE_RC on the device (log $LOG)" >&2; exit 1; }

if [ "$DRY" != 1 ]; then
  if [ "$STAGE_ON" = 1 ]; then
    adbs pull "$DEVJSON" "$(wslpath -w "$STAGE/pulled.jsonl")" >/dev/null || { echo "FATAL: adb pull failed" >&2; exit 1; }
    cp "$STAGE/pulled.jsonl" "$OUT"
  else
    adbs pull "$DEVJSON" "$OUT" >/dev/null || { echo "FATAL: adb pull failed" >&2; exit 1; }
  fi
  echo "== pulled $OUT ($(wc -l < "$OUT") records) =="
fi

echo "== scoring with the device gates =="
SCORE=(node "$ROOT/probes/vision/score.mjs" --device --items "$ITEMS" --records "$OUT" --out-md "$MD" --out-json "$JSON")
if [ "$DRY" = 1 ]; then printf '+'; printf ' %q' "${SCORE[@]}"; printf '\n'; echo "== dry-run done: nothing was run on a phone =="; exit 0; fi
set +e; "${SCORE[@]}"; SCORE_RC=$?; set -e

# Memory into the report: the meminfo snapshot plus the records' VmHWM.
if [ -f "$MD" ]; then
  {
    printf '\n## Memory of the bench process\n\n'
    printf 'Snapshot: %s. The bench is a plain binary, so this is the number without a WebView.\n\n' "${MEM_NOTE:-n/a}"
    printf '```\n'; cat "$MEMFILE"; printf '```\n\n'
    python3 - "$OUT" <<'PY'
import json,sys
rows=[json.loads(l) for l in open(sys.argv[1]) if l.strip()]
v=[r['vmhwm_kb'] for r in rows if 'vmhwm_kb' in r]
print('Max VmHWM across records: %s kB' % (max(v) if v else 'n/a (no vmhwm_kb in the records)'))
PY
  } >> "$MD"
fi
echo "== report: $MD | $JSON | meminfo: $MEMFILE | records: $OUT | log: $LOG =="
echo "== score exit $SCORE_RC (0 pass, 1 gate miss, 2 missing record) =="
exit "$SCORE_RC"
