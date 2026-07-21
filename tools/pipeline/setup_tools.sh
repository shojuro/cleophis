#!/usr/bin/env bash
# Sets up everything the Cleophis distribution pipeline (tools/pipeline/)
# needs to run: a pinned llama.cpp checkout (for convert_hf_to_gguf.py +
# convert_lora_to_gguf.py), a llama-quantize binary, and a Python venv with
# both this pipeline's own deps and the convert scripts' deps installed.
#
# Runs in WSL2 / plain Linux bash — NOT the Windows PWSH toolchain the Rust
# side uses. Needs: python3 (3.9+), git, curl, tar. No credentials needed —
# everything this script does is public downloads.
#
# Idempotent: safe to re-run. Each step checks whether its output already
# exists and is usable before doing any work.
set -euo pipefail

# ---------------------------------------------------------------------
# Pinned llama.cpp version. `b10042` matches the llama-server release the
# app itself bundles (see ../fetch-llama-server.mjs's release-asset fetch;
# that script tracks "latest" for the Windows Vulkan build, this pin keeps
# the pipeline's convert/quantize tooling on the exact same tag so
# converted/quantized artifacts match the runtime llama.cpp build byte-for-
# byte in behavior). LLAMA_COMMIT and ASSET_SHA256 were resolved once
# against the GitHub API/release when this pin was set — see README.md's
# "Pinned llama.cpp version" section, which is the durable record of this
# pin; re-running this script re-verifies against these same constants
# rather than re-resolving them, so a compromised/rotated release asset is
# detected as a checksum mismatch, not silently accepted.
# ---------------------------------------------------------------------
LLAMA_TAG="b10042"
LLAMA_COMMIT="3f08ef2c519710831cb68c8dc2c2693e6bb5bf81"
ASSET_NAME="llama-${LLAMA_TAG}-bin-ubuntu-x64.tar.gz"
ASSET_SHA256="132d5c09e1d8087bb68ddc7876e69e7e82ae503493933f5706163b10b7036eee"
ASSET_URL="https://github.com/ggml-org/llama.cpp/releases/download/${LLAMA_TAG}/${ASSET_NAME}"
LLAMA_REPO_URL="https://github.com/ggml-org/llama.cpp.git"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

LLAMA_DIR="$SCRIPT_DIR/llama.cpp"
BIN_DIR="$LLAMA_DIR/bin"
VENV_DIR="$SCRIPT_DIR/.venv"
WORK_DIR="$SCRIPT_DIR/work"

log() { printf '>> %s\n' "$*"; }
die() { printf 'setup_tools.sh: FATAL: %s\n' "$*" >&2; exit 1; }

# ---------------------------------------------------------------------
# Preflight
# ---------------------------------------------------------------------
command -v python3 >/dev/null 2>&1 || die "python3 not found on PATH. Install python3 (3.9+) and re-run. BLOCKED: needs user/sudo action."
command -v git >/dev/null 2>&1 || die "git not found on PATH. BLOCKED: needs user/sudo action."
command -v curl >/dev/null 2>&1 || die "curl not found on PATH. BLOCKED: needs user/sudo action."
command -v tar >/dev/null 2>&1 || die "tar not found on PATH. BLOCKED: needs user/sudo action."
command -v sha256sum >/dev/null 2>&1 || die "sha256sum not found on PATH. BLOCKED: needs user/sudo action."

mkdir -p "$WORK_DIR"

# ---------------------------------------------------------------------
# Step 1: pinned llama.cpp checkout (convert_hf_to_gguf.py,
# convert_lora_to_gguf.py, and their requirements/*.txt live here).
# ---------------------------------------------------------------------
if [ -d "$LLAMA_DIR/.git" ]; then
  current_commit="$(git -C "$LLAMA_DIR" rev-parse HEAD 2>/dev/null || echo "")"
  if [ "$current_commit" = "$LLAMA_COMMIT" ]; then
    log "llama.cpp already checked out at pinned commit $LLAMA_COMMIT (tag $LLAMA_TAG) — skipping clone."
  else
    log "llama.cpp checkout exists but HEAD ($current_commit) != pinned commit ($LLAMA_COMMIT); re-pinning."
    git -C "$LLAMA_DIR" fetch --depth 1 origin "tag" "$LLAMA_TAG"
    git -C "$LLAMA_DIR" checkout --detach "$LLAMA_TAG"
    current_commit="$(git -C "$LLAMA_DIR" rev-parse HEAD)"
    [ "$current_commit" = "$LLAMA_COMMIT" ] || die "llama.cpp tag $LLAMA_TAG resolved to $current_commit, expected pinned $LLAMA_COMMIT (upstream tag moved?). Refusing to continue."
  fi
else
  log "Cloning llama.cpp @ tag $LLAMA_TAG (shallow) into $LLAMA_DIR ..."
  git clone --branch "$LLAMA_TAG" --depth 1 "$LLAMA_REPO_URL" "$LLAMA_DIR"
  current_commit="$(git -C "$LLAMA_DIR" rev-parse HEAD)"
  [ "$current_commit" = "$LLAMA_COMMIT" ] || die "llama.cpp tag $LLAMA_TAG resolved to $current_commit, expected pinned $LLAMA_COMMIT (upstream tag moved?). Refusing to continue."
fi

[ -f "$LLAMA_DIR/convert_hf_to_gguf.py" ] || die "convert_hf_to_gguf.py missing from llama.cpp checkout."
[ -f "$LLAMA_DIR/convert_lora_to_gguf.py" ] || die "convert_lora_to_gguf.py missing from llama.cpp checkout."
[ -f "$LLAMA_DIR/requirements/requirements-convert_lora_to_gguf.txt" ] || die "llama.cpp requirements/requirements-convert_lora_to_gguf.txt missing."

# Record the resolved pin for this checkout (informational; llama.cpp/ is
# gitignored — README.md is the committed, durable pinned-versions note).
cat > "$LLAMA_DIR/.pinned-commit" <<EOF
tag=$LLAMA_TAG
commit=$current_commit
resolved_by=setup_tools.sh
resolved_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)
EOF
log "llama.cpp pinned at tag $LLAMA_TAG, commit $current_commit."

# ---------------------------------------------------------------------
# Step 2: llama-quantize binary. Prefer the prebuilt ubuntu-x64 release
# asset for the SAME tag (verified against a pinned sha256); only fall
# back to a cmake source build if no usable Linux binary can be obtained
# or it doesn't actually run in this environment.
# ---------------------------------------------------------------------
quantize_runs() {
  # llama-quantize exits nonzero on --help (it's asking for a real
  # invocation), so "runs" means it executes and prints its usage text,
  # not a successful exit code.
  [ -x "$BIN_DIR/llama-quantize" ] || return 1
  "$BIN_DIR/llama-quantize" --help >"$WORK_DIR/.quantize-help-check.log" 2>&1 || true
  grep -q "^usage:" "$WORK_DIR/.quantize-help-check.log"
}

# Downloads + verifies + extracts the prebuilt release asset into BIN_DIR.
# Returns 1 (does NOT die) on download/extract failure so the caller can
# fall back to a cmake source build; dies on a sha256 mismatch, since that's
# not a "try the fallback" situation — it means the pinned checksum in this
# script no longer matches what the tag serves, which needs a human to
# re-check the pin, not a silent fallback to building an unverified tree.
install_prebuilt_quantize() {
  local tmp_tar tmp_extract inner actual_sha256
  tmp_tar="$(mktemp "$WORK_DIR/llama-release-XXXXXX.tar.gz")"
  tmp_extract="$(mktemp -d "$WORK_DIR/llama-release-extract-XXXXXX")"
  trap 'rm -rf "$tmp_tar" "$tmp_extract"' RETURN

  curl -fSL --retry 3 "$ASSET_URL" -o "$tmp_tar" || return 1

  actual_sha256="$(sha256sum "$tmp_tar" | awk '{print $1}')"
  [ "$actual_sha256" = "$ASSET_SHA256" ] || die "downloaded $ASSET_NAME sha256 ($actual_sha256) != pinned ($ASSET_SHA256). Refusing to use it — re-check the pin in this script."

  tar xzf "$tmp_tar" -C "$tmp_extract" || return 1
  inner="$tmp_extract/llama-${LLAMA_TAG}"
  [ -f "$inner/llama-quantize" ] || die "llama-quantize not found inside $ASSET_NAME (release layout changed?)."
  mkdir -p "$BIN_DIR"
  # Copy the whole flat release dir (binary + its sibling .so libs it needs
  # at runtime via RUNPATH=\$ORIGIN — see README.md) into bin/.
  cp -f "$inner"/* "$BIN_DIR"/
  chmod +x "$BIN_DIR/llama-quantize"
}

build_quantize_from_source() {
  command -v cmake >/dev/null 2>&1 || die "No prebuilt llama-quantize available and cmake is not installed, so a source build isn't possible either. BLOCKED: install cmake + a C/C++ toolchain (needs sudo), or make ${ASSET_URL} reachable, then re-run."
  local build_dir found_bin
  build_dir="$LLAMA_DIR/build"
  cmake -S "$LLAMA_DIR" -B "$build_dir" -DCMAKE_BUILD_TYPE=Release -DLLAMA_BUILD_TESTS=OFF -DLLAMA_BUILD_EXAMPLES=OFF -DLLAMA_BUILD_SERVER=OFF
  cmake --build "$build_dir" --target llama-quantize -j"$(nproc)"
  mkdir -p "$BIN_DIR"
  found_bin="$(find "$build_dir" -maxdepth 3 -type f -name "llama-quantize" | head -n1)"
  [ -n "$found_bin" ] || die "cmake build finished but no llama-quantize binary was produced."
  cp -f "$found_bin" "$BIN_DIR/llama-quantize"
  chmod +x "$BIN_DIR/llama-quantize"
}

if quantize_runs; then
  log "llama-quantize already present at $BIN_DIR/llama-quantize and runs — skipping download."
else
  log "Fetching prebuilt llama-quantize: $ASSET_NAME (tag $LLAMA_TAG) ..."
  if install_prebuilt_quantize; then
    log "Installed prebuilt llama-quantize into $BIN_DIR (verified sha256)."
  else
    log "Prebuilt release asset fetch/extract failed; falling back to a cmake source build."
    build_quantize_from_source
    log "Built llama-quantize from source into $BIN_DIR."
  fi

  quantize_runs || die "llama-quantize was installed at $BIN_DIR but does not run in this environment (checked: exec + --help prints usage). Check 'ldd $BIN_DIR/llama-quantize' for missing shared libraries. This is the 'won't run in WSL2' case the brief calls out for falling back to a cmake source build — if install_prebuilt_quantize succeeded but the binary still doesn't run, re-run after removing $BIN_DIR to force the cmake fallback."
fi

# ---------------------------------------------------------------------
# Step 3: Python venv with this pipeline's own deps + the convert
# scripts' deps (from llama.cpp's own requirements files).
# ---------------------------------------------------------------------
if [ -x "$VENV_DIR/bin/python3" ] && [ -x "$VENV_DIR/bin/pip" ]; then
  log "Python venv already present at $VENV_DIR — skipping creation."
else
  rm -rf "$VENV_DIR"
  log "Creating Python venv at $VENV_DIR ..."
  set +e
  python3 -m venv "$VENV_DIR" 2>"$WORK_DIR/.venv-create.log"
  venv_rc=$?
  set -e
  if [ $venv_rc -ne 0 ] || [ ! -x "$VENV_DIR/bin/pip" ]; then
    # Common on Debian/Ubuntu: ensurepip's data isn't installed unless the
    # python3-venv apt package is present, and we may not have sudo to
    # install it. Work around it without sudo: create the venv without
    # pip, then bootstrap pip via the standalone get-pip.py installer
    # (pure stdlib zipapp, no ensurepip involved).
    log "python3 -m venv didn't produce a usable pip (see $WORK_DIR/.venv-create.log); bootstrapping pip manually (no sudo/apt needed)."
    python3 -m venv --without-pip "$VENV_DIR"
    get_pip="$WORK_DIR/get-pip.py"
    curl -fsSL --retry 3 https://bootstrap.pypa.io/get-pip.py -o "$get_pip"
    "$VENV_DIR/bin/python3" "$get_pip" --quiet
    rm -f "$get_pip"
  fi
fi

[ -x "$VENV_DIR/bin/pip" ] || die "venv at $VENV_DIR has no usable pip after setup."

log "Installing this pipeline's own dependencies (requirements.txt) ..."
"$VENV_DIR/bin/pip" install --upgrade pip --quiet
"$VENV_DIR/bin/pip" install -r "$SCRIPT_DIR/requirements.txt"

log "Installing convert-script dependencies from llama.cpp's own requirements (torch CPU + friends — this is a multi-GB download, be patient) ..."
"$VENV_DIR/bin/pip" install -r "$LLAMA_DIR/requirements/requirements-convert_lora_to_gguf.txt"

# ---------------------------------------------------------------------
# Verification (mirrors task A1 step 2's acceptance check)
# ---------------------------------------------------------------------
log "Verifying convert_hf_to_gguf.py --help ..."
"$VENV_DIR/bin/python3" "$LLAMA_DIR/convert_hf_to_gguf.py" --help >/dev/null

log "Verifying convert_lora_to_gguf.py --help ..."
"$VENV_DIR/bin/python3" "$LLAMA_DIR/convert_lora_to_gguf.py" --help >/dev/null

log "Verifying llama-quantize --help ..."
quantize_runs || die "llama-quantize verification failed after setup."

cat <<EOF

=======================================================================
setup_tools.sh: all green.

  llama.cpp checkout : $LLAMA_DIR
    tag               : $LLAMA_TAG
    commit            : $current_commit
  llama-quantize      : $BIN_DIR/llama-quantize
  convert_hf_to_gguf  : $LLAMA_DIR/convert_hf_to_gguf.py
  convert_lora_to_gguf: $LLAMA_DIR/convert_lora_to_gguf.py
  Python venv         : $VENV_DIR
  scratch/work dir    : $WORK_DIR

Later pipeline scripts should invoke tools via "$VENV_DIR/bin/python3"
and "$BIN_DIR/llama-quantize" directly (both are self-contained; no PATH
or LD_LIBRARY_PATH changes needed — llama-quantize resolves its sibling
.so libs via its own RUNPATH).
=======================================================================
EOF
