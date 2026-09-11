#!/usr/bin/env bash
# Phase 1c A3 — the device half of the med-triage device-fidelity bar.
#
# Pushes the catalog's base + adapter GGUFs (sha-verified ON THE DEVICE), the
# prompt file and the aarch64 `probe` harness; runs the harness in `--json`
# mode with the flags the app uses; pulls the result; prints the parity lines.
#
# The pod scores `endpoint(Q4 base + f16 LoRA)` on the same items. This script
# produces the other side of that comparison. Everything it refuses to do is a
# way the two runs could look alike and not be alike.
#
# ── WHAT THIS SCRIPT WILL NOT DO ─────────────────────────────────────────────
#
#   * run without a device (`adb devices` in state `device`);
#   * push a GGUF whose LOCAL sha256 is not the catalog's — a 1.2 GB copy of
#     the wrong file is worse than a refusal, because the run that follows
#     looks exactly like a real one;
#   * accept a GGUF whose sha256 ON THE DEVICE is not the catalog's after the
#     push — an interrupted `adb push` leaves a short file and adb still exits
#     0 often enough that this has to be checked rather than assumed;
#   * run on a device with no sha256 tool, because then the point above cannot
#     be checked at all;
#   * paper over a `[cpu-verdict] INCOMPATIBLE` line. The harness prints the
#     runtime AT_HWCAP verdict; a device that cannot run the dotprod kernels
#     produces either a SIGILL or scalar numbers, and neither is a result.
#
# ── IDEMPOTENT, AND WHY THAT MATTERS HERE ────────────────────────────────────
#
# The GGUFs are ~1.3 GB together and a wireless `adb push` of that is minutes.
# A re-run therefore hashes what is already on the device and pushes only what
# differs. `--force-push` is the escape hatch. The harness binary and the prompt
# file are small and are pushed every time, because those are the two things a
# re-run is usually re-running BECAUSE they changed.
#
# ── AIRPLANE MODE ────────────────────────────────────────────────────────────
#
# Attempted, verified, and reported either way — never assumed. If the script
# cannot set it, it says so in a line that ends up in the log beside the
# numbers, so a reader can see which of the two the run was. Restored on exit
# only if this script was the thing that changed it.
#
# Usage:
#   ./run-device-probes.sh --prompts work/device-probes/m7.jsonl
#   ./run-device-probes.sh --label m7 --repeat 2 --serial R58N30ABCD
#
#   --label L         prompts default to work/device-probes/L.jsonl and outputs
#                     are named after L; defaults to the prompt file's basename
#   --prompts P       the JSONL prompt file (overrides --label's default)
#   --catalog P       default src-tauri/resources/catalog.triage.json
#   --catalog-id ID   default: the first entry with "real": true
#   --artifacts DIR   where the GGUFs are (default ~/cleophis-artifacts, or
#                     $CLEOPHIS_ARTIFACTS)
#   --out DIR         default work/device-probes
#   --n-ctx N         default 4096 (matches probes/lib.mjs; see the note below)
#   --repeat N        run the whole suite N times and diff the runs (default 1)
#   --serial S        one device instead of every attached one
#   --force-push      re-push the GGUFs even if the on-device sha already matches
#   --no-airplane     do not touch the radios
#
# ── N-CTX, STATED BECAUSE IT IS A DIFFERENCE ─────────────────────────────────
# The app opens 2048 on a `low`-tier DEVICE and 4096 above it; the pod serves
# 4096. 4096 is the default here because it is the pod's, and because the
# triage prompts are ~200 tokens against a 320-token cap, so neither value can
# truncate anything. If that stops being true, this is the knob, and a run at a
# different n-ctx should say so in its write-up.
#
# `-e` is deliberately not set, for the reason `run-on-device.sh`'s header
# records: the per-device function must RETURN non-zero to a collector rather
# than abort the other devices, and errexit is suspended inside a function
# called as an `if !` condition anyway. Every failure that matters is checked.
set -uo pipefail

ADB="${ADB:-adb}"
HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"

: "${CARGO_TARGET_DIR:=/home/$USER/cleophis-mobile-target}"
: "${ANDROID_NDK_ROOT:=/home/$USER/android-ndk-r27c}"

PROMPTS=""
LABEL=""
CATALOG="$REPO/src-tauri/resources/catalog.triage.json"
CATALOG_ID=""
ARTIFACTS="${CLEOPHIS_ARTIFACTS:-$HOME/cleophis-artifacts}"
OUT="$REPO/work/device-probes"
N_CTX=4096
REPEAT=1
ONLY_SERIAL=""
FORCE_PUSH=0
DO_AIRPLANE=1

while [ $# -gt 0 ]; do case "$1" in
  --prompts)     PROMPTS="$2"; shift 2;;
  --label)       LABEL="$2"; shift 2;;
  --catalog)     CATALOG="$2"; shift 2;;
  --catalog-id)  CATALOG_ID="$2"; shift 2;;
  --artifacts)   ARTIFACTS="$2"; shift 2;;
  --out)         OUT="$2"; shift 2;;
  --n-ctx)       N_CTX="$2"; shift 2;;
  --repeat)      REPEAT="$2"; shift 2;;
  --serial)      ONLY_SERIAL="$2"; shift 2;;
  --force-push)  FORCE_PUSH=1; shift;;
  --no-airplane) DO_AIRPLANE=0; shift;;
  -h|--help)     sed -n '1,80p' "$0"; exit 0;;
  *) echo "unknown arg: $1" >&2; exit 2;; esac; done

if [ -z "$PROMPTS" ]; then
  [ -n "$LABEL" ] || { echo "FATAL: --prompts <file.jsonl> or --label <name> required." >&2; exit 2; }
  PROMPTS="$REPO/work/device-probes/$LABEL.jsonl"
fi
[ -n "$LABEL" ] || LABEL="$(basename "$PROMPTS" .jsonl)"
[ -f "$PROMPTS" ] || { echo "FATAL: prompt file not found: $PROMPTS" >&2; exit 1; }
[ -f "$CATALOG" ] || { echo "FATAL: catalog not found: $CATALOG" >&2; exit 1; }
mkdir -p "$OUT" || exit 1

DEV=/data/local/tmp/cleophis-probes
TGT="$CARGO_TARGET_DIR/aarch64-linux-android/release/examples"
LIBCXX="$ANDROID_NDK_ROOT/toolchains/llvm/prebuilt/linux-x86_64/sysroot/usr/lib/aarch64-linux-android/libc++_shared.so"

[ -x "$TGT/probe" ] || {
  echo "FATAL: no aarch64 harness at $TGT/probe." >&2
  echo "  build it RELEASE, with the dotprod patch applied:" >&2
  echo "    cargo ndk -t arm64-v8a -P 24 build -p kpack-engine --release --features real --example probe" >&2
  exit 1
}
[ -f "$LIBCXX" ] || { echo "FATAL: libc++_shared.so not found at $LIBCXX" >&2; exit 1; }

# ── Catalog, read once, with NO fallback ─────────────────────────────────────
#
# `run-on-device.sh`'s header records why: a substituted value produces a
# DIFFERENT gate and would not be labelled as one. Everything below comes out of
# the published catalog entry or the run does not happen.
read_catalog() {
  python3 - "$CATALOG" "$CATALOG_ID" <<'PY'
import json, sys, hashlib
cat = json.load(open(sys.argv[1]))
want = sys.argv[2]
if want:
    hits = [e for e in cat if e.get("id") == want]
else:
    hits = [e for e in cat if e.get("real")]
if not hits:
    sys.exit("no matching catalog entry")
e = hits[0]
fp = hashlib.sha256(e["systemPrompt"].encode()).hexdigest()[:12]
for k, v in [
    ("ENTRY_ID", e["id"]),
    ("MODEL_FILE", e["modelFile"]),
    ("MODEL_SHA", e["sha256"]),
    ("ADAPTER_FILE", e["adapterFile"]),
    ("ADAPTER_SHA", e["adapterSha256"]),
    ("MAX_TOKENS", e["sampling"]["maxTokens"]),
    ("TEMPERATURE", e["sampling"]["temperature"]),
    ("CHAT_TEMPLATE", e["chatTemplate"]),
    ("CATALOG_FP", e["promptFingerprint"]),
    ("COMPUTED_FP", fp),
]:
    print(f"{k}={v}")
PY
}

CAT_VARS="$(read_catalog)" || {
  echo "FATAL: could not read the triage entry from $CATALOG." >&2
  echo "  A probe run against a substituted model, adapter or token budget is a" >&2
  echo "  DIFFERENT gate and would not be labelled as one, so this refuses." >&2
  exit 1
}
# Read into named variables WITHOUT `eval`. The catalog is repo-controlled, so
# this is not a threat model — it is that a value containing a quote or a `$`
# would make `eval` fail in a way that reads like a broken phone rather than a
# broken catalog, and the whole point of this block is to fail legibly.
ENTRY_ID="" MODEL_FILE="" MODEL_SHA="" ADAPTER_FILE="" ADAPTER_SHA=""
MAX_TOKENS="" TEMPERATURE="" CHAT_TEMPLATE="" CATALOG_FP="" COMPUTED_FP=""
while IFS='=' read -r k v; do
  case "$k" in
    ENTRY_ID)      ENTRY_ID="$v";;
    MODEL_FILE)    MODEL_FILE="$v";;
    MODEL_SHA)     MODEL_SHA="$v";;
    ADAPTER_FILE)  ADAPTER_FILE="$v";;
    ADAPTER_SHA)   ADAPTER_SHA="$v";;
    MAX_TOKENS)    MAX_TOKENS="$v";;
    TEMPERATURE)   TEMPERATURE="$v";;
    CHAT_TEMPLATE) CHAT_TEMPLATE="$v";;
    CATALOG_FP)    CATALOG_FP="$v";;
    COMPUTED_FP)   COMPUTED_FP="$v";;
    *) echo "FATAL: unexpected catalog field $k" >&2; exit 1;;
  esac
done <<< "$CAT_VARS"
for k in ENTRY_ID MODEL_FILE MODEL_SHA ADAPTER_FILE ADAPTER_SHA MAX_TOKENS TEMPERATURE CHAT_TEMPLATE CATALOG_FP COMPUTED_FP; do
  [ -n "${!k}" ] || { echo "FATAL: catalog field $k came back empty" >&2; exit 1; }
done

[ "$CATALOG_FP" = "$COMPUTED_FP" ] || {
  echo "FATAL: the catalog's promptFingerprint ($CATALOG_FP) is not the sha of its own" >&2
  echo "  systemPrompt ($COMPUTED_FP). The catalog contradicts itself; fix it before" >&2
  echo "  any device number is taken from it." >&2
  exit 1
}

# Local GGUFs: the catalog names a bundle-relative path; accept it either as a
# path under --artifacts or as a bare basename there.
resolve_local() { # catalog path -> local file, or empty
  local p="$1"
  if [ -f "$ARTIFACTS/$p" ]; then echo "$ARTIFACTS/$p"; return; fi
  if [ -f "$ARTIFACTS/$(basename "$p")" ]; then echo "$ARTIFACTS/$(basename "$p")"; return; fi
  if [ -f "$p" ]; then echo "$p"; return; fi
  echo ""
}
MODEL_LOCAL="$(resolve_local "$MODEL_FILE")"
ADAPTER_LOCAL="$(resolve_local "$ADAPTER_FILE")"
for pair in "MODEL:$MODEL_LOCAL:$MODEL_FILE" "ADAPTER:$ADAPTER_LOCAL:$ADAPTER_FILE"; do
  what="${pair%%:*}"; rest="${pair#*:}"; local_path="${rest%%:*}"; cat_path="${rest#*:}"
  [ -n "$local_path" ] || {
    echo "FATAL: $what GGUF not found. The catalog names \"$cat_path\"; looked under" >&2
    echo "  $ARTIFACTS. Fetch it with ./fetch-artifacts.sh or pass --artifacts DIR." >&2
    exit 1
  }
done

echo "== local sha256 (before any 1.2 GB push) =="
check_local_sha() { # label, file, expected
  local got; got="$(sha256sum "$2" | cut -d' ' -f1)"
  if [ "$got" != "$3" ]; then
    echo "FATAL: $1 sha256 mismatch on the LOCAL file $2" >&2
    echo "  expected $3" >&2
    echo "  got      $got" >&2
    return 1
  fi
  echo "  OK $1 $got  $(basename "$2")"
}
check_local_sha "base"    "$MODEL_LOCAL"   "$MODEL_SHA"   || exit 1
check_local_sha "adapter" "$ADAPTER_LOCAL" "$ADAPTER_SHA" || exit 1

# ── Device discovery — byte-identical to airplane-mode.sh / run-on-device.sh ──
discover_devices() {
  "$ADB" devices | awk 'NR>1 && $2=="device" {print $1}'
}
adbs() { "$ADB" -s "$1" "${@:2}"; }

DEVICES=()
while IFS= read -r s; do [ -n "$s" ] && DEVICES+=("$s"); done < <(discover_devices)

if [ -n "$ONLY_SERIAL" ]; then
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
  echo "  \`$ADB devices\` lists nothing in state 'device'. An unauthorized or offline" >&2
  echo "  phone is excluded on purpose — see run-on-device.sh's connection notes." >&2
  exit 1
}

# ── Per-device helpers ───────────────────────────────────────────────────────

# The sha256 tool, resolved once per device. NO fallback to "skip the check":
# the whole reason the sha is verified after the push is that adb can leave a
# short file, and a run that cannot check is a run that cannot claim.
device_sha_cmd() { # serial -> prints a command prefix, or empty
  local s="$1"
  if adbs "$s" shell "command -v sha256sum >/dev/null 2>&1 && echo yes" 2>/dev/null | grep -q yes; then
    echo "sha256sum"
  elif adbs "$s" shell "command -v toybox >/dev/null 2>&1 && toybox sha256sum </dev/null >/dev/null 2>&1 && echo yes" 2>/dev/null | grep -q yes; then
    echo "toybox sha256sum"
  else
    echo ""
  fi
}

device_sha() { # serial, sha-cmd, device path
  adbs "$1" shell "$2 '$3' 2>/dev/null" | tr -d '\r' | awk '{print $1}'
}

push_verified() { # serial, sha-cmd, local, device basename, expected sha
  local s="$1" shacmd="$2" src="$3" name="$4" want="$5"
  local have=""
  if [ "$FORCE_PUSH" = 0 ]; then
    have="$(device_sha "$s" "$shacmd" "$DEV/$name")"
  fi
  if [ "$have" = "$want" ]; then
    echo "  [$s] already present, sha matches: $name"
    return 0
  fi
  echo "  [$s] pushing $name ($(du -h "$src" | cut -f1)) ..."
  adbs "$s" push "$src" "$DEV/$name" >/dev/null || { echo "  [$s] FAIL: push $name" >&2; return 1; }
  local got; got="$(device_sha "$s" "$shacmd" "$DEV/$name")"
  if [ "$got" != "$want" ]; then
    echo "  [$s] FAIL: $name sha256 on the DEVICE is $got, expected $want" >&2
    echo "         (an interrupted adb push leaves a short file and still exits 0)" >&2
    return 1
  fi
  echo "  [$s] pushed and verified: $name $got"
}

AIRPLANE_SET_BY_US=()
enable_airplane() { # serial -> prints a status word
  local s="$1"
  local before; before="$(adbs "$s" shell settings get global airplane_mode_on 2>/dev/null | tr -d '\r')"
  if [ "$before" = "1" ]; then echo "already-on"; return; fi
  adbs "$s" shell "cmd connectivity airplane-mode enable" >/dev/null 2>&1
  local after; after="$(adbs "$s" shell settings get global airplane_mode_on 2>/dev/null | tr -d '\r')"
  if [ "$after" = "1" ]; then
    AIRPLANE_SET_BY_US+=("$s")
    echo "enabled-by-script"
  else
    echo "NOT-SET"
  fi
}

restore_airplane() {
  for s in "${AIRPLANE_SET_BY_US[@]:-}"; do
    [ -n "$s" ] || continue
    adbs "$s" shell "cmd connectivity airplane-mode disable" >/dev/null 2>&1
  done
}
trap restore_airplane EXIT

run_on() { # serial
  local serial="$1"
  local model_name; model_name="$(adbs "$serial" shell getprop ro.product.model 2>/dev/null | tr -d '\r')"
  echo
  echo "############################################################"
  echo "## DEVICE $serial  (${model_name:-unknown model})"
  echo "############################################################"

  local shacmd; shacmd="$(device_sha_cmd "$serial")"
  [ -n "$shacmd" ] || {
    echo "[$serial] FAIL: no sha256sum on this device. The post-push integrity check" >&2
    echo "  is the reason this script can claim the bytes that ran are the bytes the" >&2
    echo "  catalog names, so a device that cannot do it does not get to produce a" >&2
    echo "  result." >&2
    return 1
  }
  echo "[$serial] sha tool: $shacmd"

  adbs "$serial" shell "mkdir -p $DEV" || { echo "[$serial] FAIL: mkdir $DEV" >&2; return 1; }

  push_verified "$serial" "$shacmd" "$MODEL_LOCAL"   "$(basename "$MODEL_FILE")"   "$MODEL_SHA"   || return 1
  push_verified "$serial" "$shacmd" "$ADAPTER_LOCAL" "$(basename "$ADAPTER_FILE")" "$ADAPTER_SHA" || return 1

  # Small, and re-pushed every run: these are the two things a re-run usually
  # exists because of.
  for f in "$TGT/probe" "$LIBCXX" "$PROMPTS" "$CATALOG"; do
    adbs "$serial" push "$f" "$DEV/$(basename "$f")" >/dev/null \
      || { echo "[$serial] FAIL: pushing $(basename "$f")" >&2; return 1; }
  done
  adbs "$serial" shell "chmod 755 $DEV/probe" >/dev/null 2>&1

  local air="skipped"
  if [ "$DO_AIRPLANE" = 1 ]; then air="$(enable_airplane "$serial")"; fi
  echo "[$serial] airplane mode: $air"
  if [ "$air" = "NOT-SET" ]; then
    echo "[$serial] NOTE: the radios are NOT verified off for this run. That is a fact" >&2
    echo "  about the run, not a warning to be dismissed — record it beside the numbers." >&2
  fi

  # `nice` needs a raised priority to be worth anything and a raised priority
  # needs root, so it is probed rather than assumed. An unniced run is fine; a
  # run that silently failed to start because `nice` exited 1 is not.
  local NICE=""
  if adbs "$serial" shell "nice -n -10 true >/dev/null 2>&1 && echo yes" 2>/dev/null | grep -q yes; then
    NICE="nice -n -10"
  elif adbs "$serial" shell "nice -n 0 true >/dev/null 2>&1 && echo yes" 2>/dev/null | grep -q yes; then
    NICE="nice -n 0"
  fi
  echo "[$serial] nice: ${NICE:-none}"

  local rc=0
  for run in $(seq 1 "$REPEAT"); do
    local tag="$LABEL.$serial"
    [ "$REPEAT" = 1 ] || tag="$LABEL.$serial.run$run"
    local devjson="$DEV/$tag.json"
    local outjson="$OUT/$tag.json"
    local outlog="$OUT/$tag.log"

    # `--model-sha`/`--behavioral-sha` make the ENGINE hash the artifacts too,
    # which is the second sha check on the same bytes and is deliberate: the
    # script's check answers "did the push land", the engine's is the very
    # fail-closed gate the app applies before it will load anything. It costs a
    # re-hash of ~1.3 GB on the phone at every run; that is the price of the
    # device running the same load path the product runs.
    echo "[$serial] == run $run/$REPEAT -> $outjson =="
    local t0; t0=$(date +%s)
    adbs "$serial" shell "cd $DEV && LD_LIBRARY_PATH=$DEV $NICE ./probe \
        --model $(basename "$MODEL_FILE") --model-sha $MODEL_SHA \
        --behavioral $(basename "$ADAPTER_FILE") --behavioral-sha $ADAPTER_SHA \
        --prompts-file $(basename "$PROMPTS") \
        --catalog $(basename "$CATALOG") $([ -n "$CATALOG_ID" ] && echo "--catalog-id $CATALOG_ID") \
        --json $devjson --n-ctx $N_CTX --gpu-layers 0" 2>&1 | tee "$outlog"
    local harness_rc=${PIPESTATUS[0]}
    local t1; t1=$(date +%s)
    echo "[$serial] elapsed: $((t1 - t0))s"

    if [ "$harness_rc" != 0 ]; then
      echo "[$serial] FAIL: harness exited $harness_rc (log: $outlog)" >&2
      rc=1
      # Pull whatever it managed to write; a partial run is still evidence.
    fi
    adbs "$serial" pull "$devjson" "$outjson" >/dev/null 2>&1 \
      || { echo "[$serial] FAIL: could not pull $devjson" >&2; rc=1; continue; }

    if grep -q "INCOMPATIBLE" "$outlog"; then
      echo "[$serial] FAIL: [cpu-verdict] INCOMPATIBLE — the compile-time kernels are not" >&2
      echo "  the ones this CPU has. Numbers from such a run are scalar at best and a" >&2
      echo "  SIGILL at worst; neither is a result." >&2
      rc=1
    fi

    echo "[$serial] ---- parity lines ----"
    grep -E '^\[(probe-json|prompt)\]' "$outlog" \
      || echo "[$serial] (no parity lines: the harness did not reach its header)"

    # THE LINE THE FOUNDER COMPARES WITH THE POD.
    #
    # `[prompt] … sha256=` is printed by the engine itself, once per session,
    # over the exact bytes it tokenised — pre-closed think block included. Task
    # A2's own byte-equality test ran against a Qwen3-4B GGUF because the 1.7B
    # is not on that box, so THIS run, on the catalog's real 1.7B stack, is the
    # first time the number is produced by the model that ships. It is the
    # number to put beside the pod's `rendered_prompt_sha256`.
    if ! grep -q '^\[prompt\]' "$outlog"; then
      echo "[$serial] FAIL: the engine printed no [prompt] line, so this run cannot say" >&2
      echo "  what it served. Without it the device-vs-pod delta is not attributable." >&2
      rc=1
    else
      echo "[$serial] ---- the sha to compare with the pod (real 1.7B stack) ----"
      grep -m1 '^\[prompt\]' "$outlog"
      echo "[$serial]   Put this beside the pod's rendered_prompt_sha256 for the same"
      echo "[$serial]   {system,user}. Equal shas exclude rendering as an explanation for"
      echo "[$serial]   any disagreement; unequal shas mean nothing else in the run counts."
    fi
    verify_sha_independently "$outjson" || rc=1
  done

  if [ "$REPEAT" -gt 1 ]; then
    compare_runs "$serial" || rc=1
  fi
  return $rc
}

# The harness computes prompt_sha from its own render. This recomputes it here,
# from the prompt file, with a second implementation — so a harness that renders
# the prompt wrongly and hashes it consistently is still caught. It is the
# cheapest available check on the one number the pod comparison rests on.
verify_sha_independently() { # out json
  python3 - "$1" "$PROMPTS" <<'PY'
import hashlib, json, sys
out, prompts = sys.argv[1], sys.argv[2]
want = {}
for line in open(prompts, encoding="utf-8"):
    line = line.strip()
    if not line:
        continue
    r = json.loads(line)
    rendered = (
        "<|im_start|>system\n" + r["system"] + "<|im_end|>\n"
        "<|im_start|>user\n" + r["user"] + "<|im_end|>\n"
        "<|im_start|>assistant\n<think>\n\n</think>\n\n"
    )
    want[r["id"]] = hashlib.sha256(rendered.encode("utf-8")).hexdigest()
bad = 0
n = 0
first = None
for line in open(out, encoding="utf-8"):
    line = line.strip()
    if not line:
        continue
    rec = json.loads(line)
    n += 1
    if first is None:
        first = rec
    if want.get(rec["id"]) != rec.get("prompt_sha"):
        bad += 1
        if bad <= 3:
            print(f"  PROMPT-SHA MISMATCH {rec['id']}: harness {rec.get('prompt_sha')} vs script {want.get(rec['id'])}")
print(f"  records: {n} of {len(want)}")
if first:
    print(f"  prompt_sha[{first['id']}] = {first['prompt_sha']}")
    print( "  PARITY: this sha must equal the pod's rendered_prompt_sha256 for the same")
    print( "          {system,user}. It is computed from the rendered prompt STRING on")
    print( "          both sides; until Task A2 exposes the engine's own render, it")
    print( "          proves the pod<->harness half and not the harness<->engine half.")
states = {}
for line in open(out, encoding="utf-8"):
    line = line.strip()
    if line:
        s = json.loads(line).get("state", "?")
        states[s] = states.get(s, 0) + 1
print("  states: " + " ".join(f"{k}={v}" for k, v in sorted(states.items())))
sys.exit(1 if bad else 0)
PY
}

# Greedy decoding is deterministic within one device, so two runs must be
# identical in everything but wall-clock. The plan review makes the point that
# if they are NOT, that fact matters more than the device-vs-pod delta.
compare_runs() { # serial
  local serial="$1"
  python3 - "$OUT" "$LABEL" "$serial" "$REPEAT" <<'PY'
import json, sys, os
out, label, serial, repeat = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4])
def load(run):
    p = os.path.join(out, f"{label}.{serial}.run{run}.json")
    if not os.path.exists(p):
        print(f"  run{run} produced no file ({p}); the runs cannot be compared.")
        return None
    d = {}
    with open(p, encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if line:
                r = json.loads(line)
                d[r["id"]] = r
    return d
base = load(1)
if base is None:
    sys.exit(1)
worst = 0
for run in range(2, repeat + 1):
    other = load(run)
    if other is None:
        worst = max(worst, 1)
        continue
    diffs = [i for i in base if i in other and (base[i]["raw"] != other[i]["raw"])]
    missing = sorted(set(base) ^ set(other))
    print(f"  run1 vs run{run}: {len(diffs)} of {len(base)} replies differ; {len(missing)} ids only in one run")
    for i in diffs[:5]:
        print(f"    DIFFERS: {i}")
    worst = max(worst, len(diffs) + len(missing))
if worst:
    print("  NON-DETERMINISM ON ONE DEVICE AT TEMPERATURE 0. Read this before reading any")
    print("  device-vs-pod delta: a device that does not repeat itself cannot be compared")
    print("  with anything.")
sys.exit(1 if worst else 0)
PY
}

echo
echo "== catalog: $ENTRY_ID  (fingerprint $CATALOG_FP, maxTokens $MAX_TOKENS, temperature $TEMPERATURE, template $CHAT_TEMPLATE)"
echo "== prompts: $PROMPTS ($(grep -cve '^[[:space:]]*$' "$PROMPTS") records)"
echo "== harness: $TGT/probe"
echo "== ${#DEVICES[@]} device(s) ready: ${DEVICES[*]}"

# ── HOW LONG THIS TAKES, SAID BEFORE IT STARTS ───────────────────────────────
#
# The real M7 set is 260 prompts: 200 endpoint arms, 20 crisis-embedded, 40
# crisis. At the catalog's 320-token cap and the tok/s a 1.7B Q4_K_M gets on the
# workhorse tier, a run is HOURS, not minutes — and `--repeat 2` doubles it.
# Printed here rather than discovered at minute forty, because the two things a
# founder does with that number are "start it before bed" and "do not start it
# on a laptop that sleeps".
PROMPT_N="$(grep -cve '^[[:space:]]*$' "$PROMPTS")"
python3 - "$PROMPT_N" "$REPEAT" <<'PY'
import sys
n, repeat = int(sys.argv[1]), int(sys.argv[2])
# 8 and 25 tok/s bracket what this project has measured for a 1.7B Q4_K_M on
# the floor and workhorse tiers; 320 is the cap, and most replies come in well
# under it, so the low end is pessimistic on purpose.
for label, tps in (("fast", 25.0), ("slow", 8.0)):
    mins = n * (320.0 / tps) / 60.0 * repeat
    print(f"== estimate ({label}, {tps:.0f} tok/s, 320-token cap): {mins:.0f} min for "
          f"{n} prompts x {repeat} run(s)")
print("== the harness prints a live ETA per item once the first one lands.")
if n * repeat > 150:
    print("== LONG RUN: keep the host awake (a suspended laptop kills the adb shell) and")
    print("==   prefer a USB or a stable wireless link. Records are flushed one per line,")
    print("==   so a dropped connection still leaves everything up to that point in")
    print("==   /data/local/tmp/cleophis-probes, and a re-run skips the GGUF push.")
PY

WALL0=$(date +%s)
FAILED=()
for serial in "${DEVICES[@]}"; do
  if ! run_on "$serial"; then FAILED+=("$serial"); fi
done
WALL1=$(date +%s)

echo
echo "== total elapsed: $((WALL1 - WALL0))s =="
echo "Next: score the replies, and score what the PATIENT reads, not the raw text:"
echo "  node $REPO/probes/device-guard.mjs --in $OUT/$LABEL.<serial>.json \\"
echo "       --out $OUT/$LABEL.<serial>.guard.json"
echo "The raw number is the model contract; the post-guard number is the product"
echo "contract, and the product one is the bar that disqualifies at release."

if [ "${#FAILED[@]}" -gt 0 ]; then
  echo
  echo "FAILED on ${#FAILED[@]} of ${#DEVICES[@]} device(s): ${FAILED[*]}" >&2
  exit 1
fi
echo
echo "== all ${#DEVICES[@]} device(s) completed =="
