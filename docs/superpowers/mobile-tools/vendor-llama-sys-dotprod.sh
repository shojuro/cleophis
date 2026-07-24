#!/usr/bin/env bash
# Vendor llama-cpp-sys-2 and patch its build.rs so the aarch64-android ggml-cpu
# compiles with ARM **dotprod** kernels (spec H3). The stock cross-build leaves
# ggml-cpu at -march=armv8-a BASELINE (no dotprod/i8mm) → quantized matmuls run
# scalar on device → garbage tok/s. The crate exposes no cargo feature or env
# for this (only a CMAKE_-prefixed env passthrough, and GGML_CPU_ARM_ARCH is not
# CMAKE_-prefixed), so a build.rs patch is the only lever.
#
# WHY armv8.2-a+dotprod and NOT +i8mm: Cortex-A55 (the 4-6GB budget floor tier
# through ~2023) has dotprod but NOT i8mm — hardcoding +i8mm SIGILLs on exactly
# that tier. armv8.2-a+dotprod is the safe single-static-build floor. (i8mm /
# full runtime dispatch would need GGML_CPU_ALL_VARIANTS, which requires
# GGML_BACKEND_DL + BUILD_SHARED_LIBS — incompatible with our static in-process
# link; that's a separate, later architecture decision.)
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
if "armv8.2-a+dotprod" in src:
    print("build.rs already patched"); raise SystemExit(0)
anchor = "    // extract the target-cpu config value, if specified"
assert anchor in src, "anchor not found — build.rs layout changed; re-inspect before patching"
patch = (
    "    // Cleophis mobile patch: floor-tier Android devices (Cortex-A55+) need ARM\n"
    "    // dotprod kernels; the stock cross-build leaves ggml-cpu at -march=armv8-a\n"
    "    // baseline, running quantized matmuls scalar (spec H3). Force the arch to\n"
    "    // armv8.2-a+dotprod. i8mm is DELIBERATELY EXCLUDED: A55 (the 4-6GB budget\n"
    "    // floor through ~2023) has dotprod but NOT i8mm, so hardcoding +i8mm would\n"
    "    // SIGILL on exactly that tier. i8mm belongs in a runtime-dispatched variant.\n"
    "    {\n"
    "        let arch_target = env::var(\"TARGET\").unwrap_or_default();\n"
    "        if arch_target.starts_with(\"aarch64\") && arch_target.contains(\"android\") {\n"
    "            config.define(\"GGML_CPU_ARM_ARCH\", \"armv8.2-a+dotprod\");\n"
    "        }\n"
    "    }\n\n"
)
open(path, "w").write(src.replace(anchor, patch + anchor, 1))
print("patched build.rs (GGML_CPU_ARM_ARCH=armv8.2-a+dotprod for aarch64-android)")
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
