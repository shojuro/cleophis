#!/usr/bin/env python3
"""The aarch64-android CPU arch floor has three homes. Keep them equal.

WHY THIS EXISTS
===============

`GGML_CPU_ARM_ARCH` decides which instructions the C compiler may emit into
llama.cpp. On 2026-07-31 its value (`armv8.2-a+dotprod`) was measured to SIGILL
on a Galaxy A51 — Cortex-A73, ARMv8.0 — which spec §3 names as the Android floor
device class. The string therefore governs which phones the product runs on.

It cannot live in one place:

  1. `third_party/llama-cpp-sys-2/build.rs`  — the TRUTH. What cmake is told.
  2. `docs/superpowers/mobile-tools/vendor-llama-sys-dotprod.sh` — reproduces
     the vendored patch from a clean registry checkout, and its idempotency
     guard keys on the literal value. A stale value there means re-vendoring
     silently produces a DIFFERENT binary from the one in the tree.
  3. `CpuSupport.NATIVE_ARM_ARCH` (Kotlin) — the pre-load gate. It must block
     exactly what (1) licenses. Too strict locks out working phones; too loose
     lets the crash through, which is the whole thing being fixed. Kotlin runs
     before any Rust exists, so it cannot ask.

Three homes for one fact is exactly the shape that produced the defect this
guards. The standing lesson on this branch is that a rule which must be
remembered at the moment of use will not be, so this is a mechanism instead of
a comment asking for care.

WHAT IT DOES NOT CHECK, stated because a check's silence is only evidence
within its competence:

  * It does not verify what the BINARY was compiled with. Only the shipped
    artifact can say that, and it does — the runtime/compile-time differential
    printed by `kpack_engine::cpu`. This checks the sources agree; that checks
    reality agrees. Both are needed, and this one is the cheap half.
  * It does not know whether the value is the RIGHT one. That is a founder-level
    product decision about which devices are supported.

Exit 0 if all three agree and the value is parseable, 1 otherwise.
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]

BUILD_RS = ROOT / "third_party/llama-cpp-sys-2/build.rs"
VENDOR_SH = ROOT / "docs/superpowers/mobile-tools/vendor-llama-sys-dotprod.sh"
KOTLIN = ROOT / "src-tauri/gen/android/app/src/main/java/com/cleophis/app/CpuSupport.kt"

# The android-conditional define in the vendored patch. Anchored on the android
# guard rather than on the bare symbol: 0.1.152's STOCK build.rs already defines
# GGML_CPU_ARM_ARCH="armv8-a" for its Linux/aarch64 docker path, and a pattern
# that matches either one would read the wrong line. That exact confusion has
# already cost this project once -- see the vendor script's own header.
BUILD_RS_RE = re.compile(
    r'arch_target\.contains\("android"\).*?'
    r'config\.define\(\s*"GGML_CPU_ARM_ARCH"\s*,\s*"([^"]+)"\s*\)',
    re.DOTALL,
)
# Only the injected DEFINE is compared. The script's idempotency guard keys on
# a marker comment rather than on the value -- deliberately, and the reason is
# recorded in the script: keying it on the value false-positived once against
# stock code and silently skipped patching. A guard that no longer contains an
# arch string is the correct state, so this check must not demand one.
VENDOR_DEFINE_RE = re.compile(r'GGML_CPU_ARM_ARCH\\?",\s*\\?"([^"\\]+)')
KOTLIN_RE = re.compile(r'NATIVE_ARM_ARCH\s*:\s*String\s*=\s*"([^"]+)"')

# Mirrors CpuSupport.v8MinorLevel. If this cannot parse the value, the Kotlin
# gate cannot either -- and its documented behaviour on an unparseable string is
# to require NOTHING, i.e. to stop gating. That must never reach a release, so
# an unparseable floor fails here instead.
ARCH_RE = re.compile(r"^armv(\d+)(?:\.(\d+))?-a")


def find(path: Path, pattern: re.Pattern, what: str) -> str | None:
    if not path.exists():
        print(f"FAIL: {what}: {path} does not exist")
        return None
    m = pattern.search(path.read_text(encoding="utf-8", errors="replace"))
    if not m:
        print(f"FAIL: {what}: no arch string found in {path.relative_to(ROOT)}")
        print("      The file's layout changed. Re-read it before editing this check:")
        print("      a checker that stops finding its target must FAIL, never pass quietly.")
        return None
    return m.group(1)


def main() -> int:
    found = {
        "build.rs (truth)": find(BUILD_RS, BUILD_RS_RE, "build.rs define"),
        "vendor script (define)": find(VENDOR_SH, VENDOR_DEFINE_RE, "vendor injected define"),
        "CpuSupport.kt (gate)": find(KOTLIN, KOTLIN_RE, "Kotlin gate constant"),
    }
    if any(v is None for v in found.values()):
        return 1

    for name, value in found.items():
        print(f"  {name:<24} {value}")

    distinct = set(found.values())
    if len(distinct) != 1:
        print("FAIL: the CPU arch floor disagrees across its homes.")
        print("      Whichever is right, a build and its gate now describe different CPUs.")
        return 1

    arch = distinct.pop()
    if not ARCH_RE.match(arch):
        print(f"FAIL: {arch!r} is not parseable as armv<N>[.<M>]-a.")
        print("      CpuSupport.requirements() returns NOTHING for a string it cannot")
        print("      parse -- deliberately, so the gate never blocks on its own confusion.")
        print("      That default is only safe because this check refuses to ship one.")
        return 1

    print(f"PASS: aarch64-android CPU floor is {arch} in all {len(found)} homes")
    return 0


if __name__ == "__main__":
    sys.exit(main())
