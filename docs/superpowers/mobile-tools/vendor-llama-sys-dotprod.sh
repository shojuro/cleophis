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
# The floor string has four homes (here, build.rs's cmake define, build.rs's
# `-march` flag on the separate cc build of llama.cpp's multimodal tool (mtmd,
# added 2026-11-01 for the vision spike), and the Kotlin pre-load gate).
# `mobile-tools/check-cpu-floor.py` fails if they disagree.
#
# ── IN-TREE PATCHES BEYOND THE FLOOR (vision spike, 2026-11-01) ──────────────
# The vendored tree carries three more edits; this script re-applies them all
# so a re-vendor reproduces the committed tree (security item P08: a vendored
# edit that lives only in the tree is invisible to review).
#   1. build.rs, mtmd cc build: `-march=armv8-a` for aarch64-android (the
#      fourth floor home; the cc build does not see the cmake define).
#   2. build.rs, mtmd glob loop: `cargo:rerun-if-changed` per mtmd source, or a
#      source patch under tools/mtmd is silently not recompiled (measured).
#   3. llama.cpp/tools/mtmd/mtmd-image.cpp, idefics3 preprocessor: with the
#      environment variable MTMD_IDEFICS3_NO_TILE set and a 1x1 tile grid,
#      emit only the global (overview) image — HF's do_image_splitting=false.
#      Upstream always adds the tile grid, so a 512-px input costs two encoder
#      passes; on the A22 that is the difference between ~16 s and ~9 s per
#      image. Opt-in; default behaviour unchanged. A product build should make
#      it a context parameter (Stage 1 of the vision plan).
# Verify after rebuild with mobile-tools/audit-arch-floor.sh on the artifact.
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

python3 - "$DEST/build.rs" <<'PY'
import sys
path = sys.argv[1]
src = open(path).read()
# Patch 1: the mtmd cc build's -march (fourth floor home). Keyed on our marker.
if "CLEOPHIS ARM ARCH FLOOR (mtmd home)" not in src:
    anchor = "        // Collect all .cpp files in tools/mtmd and its subdirectories\n"
    assert anchor in src, "mtmd anchor not found — build.rs layout changed; re-inspect before patching"
    patch = (
        "        // CLEOPHIS ARM ARCH FLOOR (mtmd home): the cmake build above pins\n"
        "        // GGML_CPU_ARM_ARCH for ggml; this cc build of the multimodal tool is\n"
        "        // separate and would otherwise take the NDK compiler's default. Keep the\n"
        "        // two in step (docs/superpowers/mobile-tools/check-cpu-floor.py checks\n"
        "        // both) so the image encoder cannot emit instructions the floor phone\n"
        "        // class lacks — the A51 SIGILL of 2026-07-31 was exactly that.\n"
        "        {\n"
        "            let arch_target = env::var(\"TARGET\").unwrap_or_default();\n"
        "            if arch_target.starts_with(\"aarch64\") && arch_target.contains(\"android\") {\n"
        "                mtmd_build.flag(\"-march=armv8-a\");\n"
        "            }\n"
        "        }\n\n"
    )
    src = src.replace(anchor, patch + anchor, 1)
    print("patched build.rs (mtmd cc build -march=armv8-a)")
# Patch 2: recompile when a vendored mtmd source changes.
if "cargo:rerun-if-changed={}\", path.display()" not in src:
    anchor = "                    mtmd_build.file(&path);\n"
    assert src.count(anchor) == 1, "mtmd_build.file anchor not unique — re-inspect"
    patch = (
        "                    // CLEOPHIS: the vendored mtmd sources are patched in-tree; cargo must\n"
        "                    // recompile when they change (found 2026-11-01: a patch was silently\n"
        "                    // not built).\n"
        "                    println!(\"cargo:rerun-if-changed={}\", path.display());\n"
    )
    src = src.replace(anchor, patch + anchor, 1)
    print("patched build.rs (rerun-if-changed per mtmd source)")
open(path, "w").write(src)
PY

python3 - "$DEST/llama.cpp/tools/mtmd/mtmd-image.cpp" <<'PY'
import sys
path = sys.argv[1]
src = open(path).read()
if "MTMD_IDEFICS3_NO_TILE" in src:
    print("mtmd-image.cpp already patched"); raise SystemExit(0)
inc = '#include "mtmd-image.h"\n'
assert src.startswith(inc), "mtmd-image.cpp no longer starts with its own header — re-inspect"
src = src.replace(inc, inc + "#include <cstdlib>\n", 1)
loop = "    for (int y = 0; y < refined_size.height; y += hparams.image_size) {\n"
assert src.count(loop) == 1, "idefics3 tile loop anchor not unique — re-inspect"
patch = (
    "    // CLEOPHIS (vision spike, 2026-11-01): HF's idefics3 processor with\n"
    "    // do_image_splitting=false emits ONLY the global image; upstream mtmd always\n"
    "    // adds the tile grid, so an image that fits one tile still costs two encoder\n"
    "    // passes. With MTMD_IDEFICS3_NO_TILE=1 and a 1x1 grid, emit the overview only.\n"
    "    // Opt-in by environment for the spike; a product build would make this a\n"
    "    // context parameter. Default behaviour is unchanged.\n"
    "    const bool no_tile_single = instructions.grid_size.width == 1 && instructions.grid_size.height == 1\n"
    "        && std::getenv(\"MTMD_IDEFICS3_NO_TILE\") != nullptr;\n"
    "    for (int y = 0; y < refined_size.height && !no_tile_single; y += hparams.image_size) {\n"
)
open(path, "w").write(src.replace(loop, patch, 1))
print("patched mtmd-image.cpp (MTMD_IDEFICS3_NO_TILE opt-in, idefics3 overview-only)")
PY

cat <<EOF

Now add to the consuming crate's Cargo.toml (kpack-engine for the spike; the
workspace root at gate time):

  [patch.crates-io]
  llama-cpp-sys-2 = { path = "$DEST" }

Verify after rebuild:
  find \$CARGO_TARGET_DIR/aarch64-linux-android -path '*ggml-cpu*' -name flags.make \\
    | xargs grep -o 'march=[^ ]*'      # expect: march=armv8-a
  docs/superpowers/mobile-tools/audit-arch-floor.sh <the built binary>   # expect: PASS
EOF
