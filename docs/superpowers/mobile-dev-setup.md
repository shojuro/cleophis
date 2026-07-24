# Mobile P0 — dev environment setup (WSL2, userspace)

Reproducible, **userspace-only** toolchain for the mobile P0 spike. Plan of
record (founder decision): everything lives in `$HOME`, nothing is installed
system-wide, `sudo` is not used. Rationale: the mobile-track dependencies stay
quarantined from the system the desktop demo depends on, and cleanup is a plain
`rm -rf`. This mirrors the `CARGO_TARGET_DIR` note — document, don't bake
machine-specific paths into committed config.

## Why any of this is needed

The WSL host came with: rustc/cargo, node/npm, cmake, make, gcc/g++, python3.
It did **not** have: a Rust android target, `cargo-ndk`, an Android **NDK**, a
JDK, or `libclang`. And `~/Android-Sdk` is a **symlink to the Windows-side SDK**
(`/mnt/c/.../AppData/Local/Android/Sdk`) whose `platform-tools` are Windows
`.exe`s — unusable for compiling from Linux. So the Android build chain is set
up fresh, Linux-native, under `$HOME`.

## What was installed (all in `$HOME`, all reversible)

| Piece | Location | How |
|---|---|---|
| Rust android targets | rustup | `rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android i686-linux-android` |
| cargo-ndk 4.1.2 | `~/.cargo/bin` | `cargo install cargo-ndk` |
| **NDK r27c** (27.2.12479018) | `~/android-ndk-r27c` | direct zip from dl.google.com (see note on extraction) |
| libclang (for bindgen) | `~/.local/.../clang/native/libclang.so` | `pip install --user libclang` |
| **JDK** Temurin 17.0.19 | `~/jdk/jdk-17.0.19+10` | Adoptium tarball, untarred |
| **Android SDK** (Linux) | `~/android-sdk-linux` | cmdline-tools + `sdkmanager` install |

SDK packages: `platform-tools`, `platforms;android-34`, `build-tools;34.0.0`,
licenses accepted via `yes | sdkmanager --licenses`.

### NDK extraction gotcha (WSL, no `unzip`)

`unzip` is absent and `sudo` is unavailable. `python3 -m zipfile -e` **corrupts
the NDK**: it drops unix exec bits and, worse, extracts **symlinks as text
files** (the NDK's `bin/clang -> clang-18` becomes a one-line text file, so the
toolchain won't run). Use the perm+symlink-preserving extractor at
`docs/superpowers/mobile-tools/zextract.py` (reads each entry's unix mode from
`external_attr`, recreates symlinks, restores `chmod`). Same tool is used for
the SDK cmdline-tools zip.

## Env block for mobile builds

Source this (or set per-invocation) for any cargo/gradle mobile work:

```bash
# WSL-native target dir (drvfs is 5-10x slower; keeps 10-20 GB off the Windows drive)
export CARGO_TARGET_DIR=/home/$USER/cleophis-mobile-target

# NDK — note llama-cpp-sys-2 reads ANDROID_NDK_ROOT / NDK_ROOT / ANDROID_NDK,
# NOT the ANDROID_NDK_HOME that cargo-ndk sets. Set all of them.
export ANDROID_NDK_HOME=/home/$USER/android-ndk-r27c
export ANDROID_NDK_ROOT=/home/$USER/android-ndk-r27c
export NDK_ROOT=/home/$USER/android-ndk-r27c
export ANDROID_NDK=/home/$USER/android-ndk-r27c

# bindgen (llama-cpp-sys-2 build.rs) needs a host libclang
export LIBCLANG_PATH=/home/$USER/.local/lib/python3.10/site-packages/clang/native

# JDK + Android SDK (Linux, NOT the Windows-side ~/Android-Sdk symlink)
export JAVA_HOME=/home/$USER/jdk/jdk-17.0.19+10
export ANDROID_HOME=/home/$USER/android-sdk-linux
export ANDROID_SDK_ROOT=/home/$USER/android-sdk-linux
export PATH="$JAVA_HOME/bin:$ANDROID_HOME/platform-tools:$ANDROID_HOME/cmdline-tools/latest/bin:$PATH"
```

## Cross-compiling the engine (aarch64)

```bash
cd crates/kpack-engine
cargo ndk -t arm64-v8a -P 24 build --features real   # aarch64 + llama.cpp
```

(`-P` is the platform/API flag in cargo-ndk 4.x; `-p` is cargo's package flag.)

## Building the engine for the HOST (x86_64) — for host-side validation

Unlike the NDK build (whose sysroot supplies the C headers), a host build with
the pip libclang fails bindgen with `'stdbool.h' file not found` — the pip
`libclang` ships the shared lib but not clang's builtin-header resource dir.
Borrow the NDK's clang-18 headers:

```bash
cd crates/kpack-engine
unset ANDROID_NDK_ROOT NDK_ROOT ANDROID_NDK ANDROID_NDK_HOME   # host, not cross
export LIBCLANG_PATH=/home/$USER/.local/lib/python3.10/site-packages/clang/native
export BINDGEN_EXTRA_CLANG_ARGS="-isystem /home/$USER/android-ndk-r27c/toolchains/llvm/prebuilt/linux-x86_64/lib/clang/18/include"
cargo build --features real --example probe   # gcc/g++ builds llama.cpp for host
```

## Fetching hero artifacts (host validation / device runs)

`docs/superpowers/mobile-tools/fetch-artifacts.sh [base_model]` pulls a tier's
base + adapters from the signed dist catalog (public GET from `cleophis-dist`),
each sha256-verified. Note: the engine's own load-time sha256 gate is slow in a
**debug** build (unoptimized sha2 — ~1-2 s per 100 MB); it is negligible in
`--release`. Generation tok/s is unaffected (that runs in optimized llama.cpp C).

## Teardown

```bash
rm -rf ~/android-ndk-r27c ~/jdk ~/android-sdk-linux ~/cleophis-mobile-target \
       ~/android-ndk-r27c-linux.zip ~/cmdline-tools.zip ~/temurin17.tar.gz
pip uninstall -y libclang
```
