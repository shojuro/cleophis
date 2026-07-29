#!/usr/bin/env bash
# Mobile P0 cross-compile + 16 KB page-alignment check (spec H2).
#
# Cross-compiles the in-process engine (kpack-engine --features real ->
# llama.cpp via llama-cpp-sys-2) for aarch64-linux-android and asserts the
# linked native library's LOAD segments are 16 KB aligned, so it will not crash
# on the newest Android (15+/16) devices. Verified passing 2026-07-24 with
# NDK r27c, which gives 16 KB alignment by default.
#
# NOT yet wired into CI: this repo has no CI system, and adding the first
# workflow is a founder-gated "root CI config" change. This is the runnable
# check a CI job would call once that decision is made.
#
# Env (see docs/superpowers/mobile-dev-setup.md for how these were provisioned):
set -euo pipefail

: "${ANDROID_NDK_ROOT:?set ANDROID_NDK_ROOT to a Linux NDK r27+}"
export ANDROID_NDK_HOME="$ANDROID_NDK_ROOT" NDK_ROOT="$ANDROID_NDK_ROOT" ANDROID_NDK="$ANDROID_NDK_ROOT"
: "${LIBCLANG_PATH:?set LIBCLANG_PATH to a host libclang dir (bindgen)}"
: "${CARGO_TARGET_DIR:?set CARGO_TARGET_DIR to a WSL-native path}"

HERE="$(cd "$(dirname "$0")" && pwd)"
ENGINE_DIR="$(cd "$HERE/../../../crates/kpack-engine" && pwd)"
PROBE_DIR="$HERE/aarch64-align-probe"
READELF="$ANDROID_NDK_ROOT/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-readelf"

SRC="$(cd "$HERE/../../../src-tauri/src" && pwd)"

echo "== [0/3] D-3 audit: no BARE dead-code allows in cross-platform modules =="
# Why this is a check and not a convention: the D-3 dead-code allow was, at one
# point, encoded three different ways across six modules (inner bare, inner
# cfg'd, outer cfg'd, and once not at all). With three encodings the question
# "does every D-3 module carry it, correctly?" cannot be answered by looking --
# so it was never enforceable by attention.
#
# A BARE `#![allow(dead_code)]` is the failure this catches. It switches the
# check off on BOTH platforms, including the one where the module actually
# ships, which is exactly the blind spot the mobile-check job exists to close.
# Two such allows hid a genuinely dead method and a genuinely dead constructor
# from 1.3/1.4 until they were tightened. The allow must be cfg'd to mirror its
# module's production caller, and must live on the `mod` declaration.
BARE="$(grep -rn '^#!\[allow(dead_code)\]' "$SRC" || true)"
if [ -n "$BARE" ]; then
  echo "FAIL (D-3): bare inner dead-code allow(s) -- cfg them to mirror the caller:"
  echo "$BARE"
  exit 1
fi
echo "PASS (D-3): no bare inner dead-code allows"

echo "== [1/3] cross-compiling kpack-engine --features real for arm64-v8a =="
( cd "$ENGINE_DIR" && cargo ndk -t arm64-v8a -P 24 build --features real )

echo "== [2/3] linking the alignment probe .so =="
( cd "$PROBE_DIR" && cargo ndk -t arm64-v8a -P 24 build )
SO="$CARGO_TARGET_DIR/aarch64-linux-android/debug/libaarch64_align_probe.so"
[ -f "$SO" ] || { echo "FAIL: probe .so not found at $SO"; exit 1; }

echo "== [3/3] asserting 16 KB (0x4000) LOAD alignment =="
ALIGNS="$("$READELF" -l "$SO" | awk '/LOAD/{print $NF}' | sort -u)"
echo "LOAD segment alignments: $ALIGNS"
if [ "$ALIGNS" != "0x4000" ]; then
  echo "FAIL (H2): not all LOAD segments are 16 KB (0x4000) aligned"
  exit 1
fi
echo "PASS (H2): all LOAD segments 16 KB aligned; $(basename "$SO") is AArch64."
