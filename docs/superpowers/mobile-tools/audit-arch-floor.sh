#!/usr/bin/env bash
# audit-arch-floor.sh — does an aarch64 Android binary stay at the CPU floor?
#
# check-cpu-floor.py checks that the SOURCES agree on the floor (armv8-a in all
# of its homes). This checks the ARTIFACT: it disassembles the binary and counts
# instructions the floor does not license — ARMv8.2 dot-product (sdot/udot,
# fmlal) and ARMv8.1 LSE atomics (casal, ldadd, swpal). The A51 SIGILL of
# 2026-07-31 was an ldadd in ggml's hot path on a Cortex-A73.
#
# One class of match is allowed: the compiler's outline-atomics helpers
# (`__aarch64_ldadd4_relax` and friends). They contain LSE instructions by
# design and are only ever executed after a runtime check of the CPU's
# features (`__aarch64_have_lse_atomics`), so they are safe on ARMv8.0. Every
# binary this project has shipped carries exactly those and nothing else.
#
# Usage: audit-arch-floor.sh <binary> [<binary> ...]   (exit 1 on any offender)
set -euo pipefail
NDK="${ANDROID_NDK_ROOT:-$HOME/android-ndk-r27c}"
OD="$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-objdump"
[ -x "$OD" ] || { echo "FATAL: llvm-objdump not found at $OD (set ANDROID_NDK_ROOT)" >&2; exit 2; }
[ $# -ge 1 ] || { echo "usage: $0 <aarch64 binary> [...]" >&2; exit 2; }
rc=0
for bin in "$@"; do
  [ -f "$bin" ] || { echo "FATAL: $bin missing" >&2; exit 2; }
  offenders="$("$OD" -d "$bin" | awk '
    /^[0-9a-f]+ <.*>:$/ { fn = $2 }
    /[[:space:]](sdot|udot|fmlal|fmlal2|casal|casa|cas|ldadd|ldadda|ldaddl|ldaddal|swpal|swp|stadd)[[:space:]]/ {
      if (fn !~ /^<__aarch64_/) print fn, $0
    }')"
  helpers="$("$OD" -d "$bin" | awk '/^[0-9a-f]+ <__aarch64_.*>:$/ {n++} END {print n+0}')"
  if [ -n "$offenders" ]; then
    echo "FAIL: $bin has instructions above the armv8-a floor outside the outline-atomics helpers:"
    echo "$offenders" | head -20
    rc=1
  else
    echo "PASS: $bin — nothing above the floor outside $helpers outline-atomics helper(s)"
  fi
done
exit $rc
