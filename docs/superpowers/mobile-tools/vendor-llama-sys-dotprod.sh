#!/usr/bin/env bash
# Vendor llama-cpp-sys-2 and patch its build.rs so the aarch64-android ggml-cpu
# arch floor is set explicitly. The crate exposes no cargo feature or env for
# this (only a CMAKE_-prefixed env passthrough, and GGML_CPU_ARM_ARCH is not
# CMAKE_-prefixed), so a build.rs patch is the only lever.
#
# ── 🔴 THE FLOOR CHANGED, 2026-07-31: armv8.2-a+dotprod → armv8-a ────────────
#
# This script's original reasoning was: the stock cross-build leaves ggml-cpu at
# -march=armv8-a, quantized matmuls run scalar, so force armv8.2-a+dotprod;
# exclude +i8mm because Cortex-A55 lacks it. The mechanism was right and the
# FLOOR WAS ONE ARCHITECTURE GENERATION TOO HIGH. A55 has dotprod; Cortex-A73
# does not, and a Galaxy A51 (Exynos 9611, A73+A53, ARMv8.0) — a literal
# instance of spec §3's named Android floor device — exits 132 (SIGILL) at the
# first inference on the armv8.2 build. Measured, on the founder's phone.
#
# Two independent kill paths, not one: `+dotprod` licenses sdot/udot, AND the
# armv8.2-a LEVEL makes ARMv8.1 LSE atomics mandatory, so the compiler may emit
# casal/ldadd anywhere in the C sources. llama.cpp reports no flag for the
# second, so no amount of reading its system-info string would have shown it.
#
# ⚠ THE VALUE BELOW IS PROVISIONAL. The alternative to a universal baseline
# binary is real runtime dispatch (GGML_CPU_ALL_VARIANTS, which requires
# GGML_BACKEND_DL + BUILD_SHARED_LIBS and would replace the static in-process
# link). That is a founder/steering decision, not this script's.
#
# ⚠ AND CHANGING IT IS NOT ENOUGH ON ITS OWN. Editing GGML_CPU_ARM_ARCH does
# NOT invalidate the cmake cache in an existing CARGO_TARGET_DIR: the build
# succeeds in ~25s, reports no error, and links the PREVIOUS arch's objects —
# measured, and only caught because the on-device diagnostic reported the old
# value from the new binary. Run
#   cargo clean -p llama-cpp-sys-2 --release --target aarch64-linux-android
# after any change here, and confirm against the ARTIFACT's own [kernels] line.
#
# The floor string has three homes (here, build.rs, and the Kotlin pre-load
# gate). `mobile-tools/check-cpu-floor.py` fails if they disagree.
#
# This is a TEMPORARY vendored patch. The permanent fix is an upstream
# llama-cpp-rs PR exposing a GGML cmake-define passthrough; moving [patch] to
# the workspace root (where it also governs kpack-embed) is a gate-time,
# founder-owned change.
#
# Usage:  ./vendor-llama-sys-dotprod.sh [dest_dir]     # default: ~/vendor-llama-sys/llama-cpp-sys-2
# Then add to the consuming crate's [patch.crates-io] the path it prints.
set -euo pipefail

VER=0.1.152
DEST="${1:-$HOME/vendor-llama-sys/llama-cpp-sys-2}"
SRC="$(ls -d "$HOME"/.cargo/registry/src/*/llama-cpp-sys-2-"$VER" 2>/dev/null | head -1)"
[ -n "$SRC" ] || { echo "llama-cpp-sys-2-$VER not in the cargo registry — run 'cargo fetch' in a crate that depends on it first"; exit 1; }

echo "vendoring $SRC -> $DEST"
rm -rf "$DEST"; mkdir -p "$(dirname "$DEST")"; cp -r "$SRC" "$DEST"; chmod -R u+w "$DEST"

python3 - "$DEST/build.rs" <<'PY'
import sys
path = sys.argv[1]
src = open(path).read()
# Idempotency guard MUST key on OUR marker, not the bare "GGML_CPU_ARM_ARCH"
# symbol: llama-cpp-sys-2 0.1.152's stock build.rs already defines
# GGML_CPU_ARM_ARCH="armv8-a" (for the TargetOs::Linux+aarch64 docker path), so
# the old generic check false-positived and SILENTLY skipped patching — leaving
# aarch64-android at baseline armv8-a (scalar matmuls, garbage tok/s). Key on the
# actual value we inject instead.
if "CLEOPHIS ARM ARCH FLOOR" in src:
    print("build.rs already patched"); raise SystemExit(0)
anchor = "    // extract the target-cpu config value, if specified"
assert anchor in src, "anchor not found — build.rs layout changed; re-inspect before patching"
patch = (
    "    // CLEOPHIS ARM ARCH FLOOR (aarch64-android). armv8.2-a+dotprod SIGILLed\n"
    "    // on a Cortex-A73 ARMv8.0 phone -- spec §3's own floor device class --\n"
    "    // via BOTH sdot/udot and the mandatory ARMv8.1 LSE atomics the level\n"
    "    // implies. See this script's header and check-cpu-floor.py.\n"
    "    {\n"
    "        let arch_target = env::var(\"TARGET\").unwrap_or_default();\n"
    "        if arch_target.starts_with(\"aarch64\") && arch_target.contains(\"android\") {\n"
    "            config.define(\"GGML_CPU_ARM_ARCH\", \"armv8-a\");\n"
    "        }\n"
    "    }\n\n"
)
open(path, "w").write(src.replace(anchor, patch + anchor, 1))
print("patched build.rs (GGML_CPU_ARM_ARCH=armv8-a for aarch64-android)")
PY

cat <<EOF

Now add to the consuming crate's Cargo.toml (kpack-engine for the spike; the
workspace root at gate time):

  [patch.crates-io]
  llama-cpp-sys-2 = { path = "$DEST" }

Verify after rebuild:
  find \$CARGO_TARGET_DIR/aarch64-linux-android -path '*ggml-cpu*' -name flags.make \\
    | xargs grep -o 'march=[^ ]*'      # expect: march=armv8.2-a+dotprod
EOF
