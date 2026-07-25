# Cleophis Mobile — P1 Android Alpha Gate Report (accumulating)

**Status: IN PROGRESS.** Evidence is recorded here per phase as it is produced
(brief operating protocol: "Gate evidence accumulates ... not retro-written at
the end"). Branch `mobile/p1-alpha` off `origin/main @ 0073d5c` (P0 gate merge).

Reference device (all on-device evidence): Samsung Galaxy A22 5G (SM-A226B),
MediaTek Dimensity 700 (2×A76 + 6×A55), 8 GB RAM, Android 13 — Cortex-A55-class
floor silicon: **dotprod yes, i8mm no**. On-device runs are founder-serialized
checkpoints (📱).

---

## Phase 0 — Foundation (identity, workspace, hardening)

### 0.1 Android identity split + gen/android regen — DONE (commit 427ffce)

- New `src-tauri/tauri.android.conf.json` overrides `identifier` →
  `com.cleophis.app`; desktop `tauri.conf.json` unchanged (`com.cleophis.desktop`).
  Mobile CSP drops `http://127.0.0.1:*` (no sidecar on mobile).
- `gen/android` regenerated via `tauri android init` (CLI 2.11.4) after deleting
  the pristine P0 scaffold (git history is the diff baseline — "diff before
  deleting" satisfied). CLI help confirms `tauri.android.conf.json` is
  auto-merged for android commands.
- **Verification (git diff vs committed P0 scaffold):** the regen changed ONLY
  identifier-derived paths —
  - `app/build.gradle.kts`: `namespace` + `applicationId` → `com.cleophis.app`
    (2-line diff, nothing else).
  - java package dirs renamed `com/cleophis/desktop` → `com/cleophis/app`
    (`MainActivity` package + `enableEdgeToEdge()` onCreate preserved; buildSrc
    kotlin is default-package, dir rename cosmetic).
  - All 36 other scaffold files byte-identical (manifest, resources, gradle
    wrappers) → P0's touches were 2.11.4 defaults, regenerated as-is.
  - No `com.cleophis.desktop` remnants under `gen/android`.

### 0.2 Workspace integration + vendored dotprod patch — IN PROGRESS

- **Vendored patch (`third_party/llama-cpp-sys-2`, 0.1.152):** produced by
  `docs/superpowers/mobile-tools/vendor-llama-sys-dotprod.sh`. The patch is a
  single aarch64-android-CONDITIONAL `build.rs` insert forcing
  `GGML_CPU_ARM_ARCH=armv8.2-a+dotprod` (spec H3; i8mm excluded for A55 safety).
  `build.rs` is **byte-identical** to P0's on-device-validated (DOTPROD=1)
  artifact (`diff` exit 0).
  - **Script bug found + fixed:** the vendor script's idempotency guard keyed on
    the bare `GGML_CPU_ARM_ARCH` symbol, which 0.1.152's *stock* build.rs already
    contains (for its `TargetOs::Linux`+aarch64 docker path), so it false-positived
    and silently skipped patching — leaving aarch64-android at baseline armv8-a.
    Re-keyed the guard on the injected value `armv8.2-a+dotprod`. Patch comment
    aligned to P0's proven wording.
- **Workspace:** `crates/kpack-engine`'s standalone `[workspace]` table deleted;
  added to root `members`. Root `[patch.crates-io]` points llama-cpp-sys-2 at
  `third_party/llama-cpp-sys-2`.
- **src-tauri:** `[target.'cfg(target_os = "android")'.dependencies]` adds
  `kpack-engine {features=["real"]}` + `jni` — Android-only, so desktop never
  pulls the second native build.
- **Lock resolution verified:** `cargo update -p llama-cpp-sys-2 --precise
  0.1.152` collapsed the graph to a single llama-cpp-sys-2 0.1.152 from the
  vendored **path** (no registry source). `cargo tree` confirms the desktop
  embedder path `kpack-embed → llama-cpp-2 0.1.151 → llama-cpp-sys-2 0.1.152
  (path)`. The "patch not used" warning is gone → the patch is live workspace-wide.
  For every non-android target the vendored build.rs is byte-identical to stock
  0.1.152, so desktop is behaviorally unchanged (H15 gate proves it).

#### H15 embedder-revalidation gate (0.1.151 → 0.1.152 bump) — PASS

- **Golden-pack (Linux host build), GREEN on patched 0.1.152:**
  `cargo test -p kpack-embed --features real -- --ignored`
  - `bge_smoke`: 2/2 (dims=768, semantic sanity, query-prefix drift guard).
  - `build_determinism`: 2/2 — stored int8 vectors **BYTE-IDENTICAL** across two
    independent real-embedder builds (the strong claim).
  - `rag_retrieval`: 3/3 (after the t3 fix below).
- **Definitive H15 verdict — the sys-2 0.1.151→0.1.152 bump is byte-neutral to
  the embedder.** Proven by a controlled Linux baseline: stock 0.1.151 and
  patched 0.1.152 produce the *identical* calibration cosine (0.8061) and the
  *identical* t3 outcome. The dotprod patch is aarch64-android-conditional, so it
  is inactive on the host build — desktop embeddings are unchanged.
- **Pre-existing test bug found + fixed (NOT a P1 regression):**
  `rag_retrieval::t3_multi_pack_delta1_strict_gate_never_bypassed_by_loose_pack`
  failed on Linux for BOTH 0.1.151 and 0.1.152. Root cause: `assemble()` applies
  `PERSONAL_RUNTIME_GATE_ABS_FLOOR` (0.30, a deliberate "gentler personal-pack
  gate" from commit 6b5811c) which overrides the extreme floor the test bakes
  into its Personal-tier strict pack — so the strict pack's sub-threshold chunk
  passes and the Δ1 assertion trips. This is the *same* regression review-fix
  81dbb01 fixed for t22 by pinning it to Curated; t3 was missed in that pass.
  Fix (test-only, no product code — desktop runtime behavior byte-identical):
  override the mounted strict manifest's `pack_tier` to Curated (mirrors t22;
  a Curated built pack can't mount without a curator signature, so we mutate the
  in-memory manifest, exactly as t22 does). t3 was green on the original Windows
  runs (milestone-retrieval: strict chunk 0.8064); the override postdates that.
- **Windows-target desktop suite — GREEN (run by steering).** WSLInterop is
  unregistered in this agent's WSL session (`systemd=true` in `/etc/wsl.conf`
  drops the binfmt handler → `cmd.exe`/`powershell.exe` give "Exec format
  error"), a machine quirk specific to this session's environment; interop works
  from the steering session. Steering ran the full Windows-target `cargo test` on
  this worktree (commit e5fb016, vendored patch active via `[patch.crates-io]`):
  **exit 0 — 4 suites, 264 passed, 0 failed, 1 ignored** (engine_smoke,
  pre-existing ignore). So the 0.1.151→0.1.152 patch is byte-neutral to desktop
  on BOTH axes — the embedder (H15 golden-pack, Linux) and the full app suite
  (Windows). Windows-suite runs are a steering-side service, requested at each
  phase boundary. Log: `scratchpad/win-test-p1.log` (steering side).
- **Workspace-member regression (Linux host, mock features), GREEN:**
  `cargo test -p kpack-core -p kpack-calc -p kpack-engine` — 190/0 (kpack-calc 6,
  kpack-core 156, kpack-engine 28 mock). Confirms folding kpack-engine into the
  workspace + the patch didn't break the pure-Rust crates.

**Phase 0.2 desktop-regression gate: CLOSED** (Linux golden-pack + Windows app
suite + pure-crate regression all green).

### 0.3 Manifest hardening batch — DONE (pending CP0 build validation)

Edits to `src-tauri/gen/android` (hand-editable post-regen; identifier is locked
so no further `tauri android init`):
- `AndroidManifest.xml`: `allowBackup="false"` + `fullBackupContent` /
  `dataExtractionRules` referencing new exclusion XMLs (§6 privacy invariant #1 —
  keeps the conversation DB out of Google Drive / device-transfer);
  `REQUEST_INSTALL_PACKAGES` (self-update), `POST_NOTIFICATIONS` (download-done),
  `FOREGROUND_SERVICE` + `FOREGROUND_SERVICE_SPECIAL_USE`; the `specialUse`
  inference `<service>` with `PROPERTY_SPECIAL_USE_FGS_SUBTYPE` justification
  ("local AI text generation at explicit user request; no network").
- New `res/xml/backup_rules.xml` + `res/xml/data_extraction_rules.xml` (exclude
  file/database/sharedpref/external from cloud-backup + device-transfer).
- New `InferenceService.kt` stub (real lifecycle in Phase 5.1; declared now so the
  manifest reference resolves and the build stays clean).
- `res/xml/file_paths.xml`: `<files-path name="app_updates" path="updates/">` for
  the self-update FileProvider (§4.1).
- `app/build.gradle.kts` release buildType: `abiFilters` → arm64-v8a only
  (release-only; the CP0 debug build keeps all flavors). debuggable defaults
  false for release; minify stays on (scaffold defaults, unchanged).
- Deferred to their phases with in-file notes: FGS start/stop around chat_stream
  + notification + thermal notice (5.1); FLAG_SECURE toggle command (5.1);
  release-config audit incl. abiFilters validation (5.2).

### 📱 CP0 debug APK — BUILT, ready for the founder's A22

`docs/superpowers/mobile-tools/build-android-apk.sh` (new) wraps
`tauri android build` with the provisioned env block and tees everything to
`/home/$USER/cleophis-mobile-logs/apk-<mode>-<ts>.log` — the failure trail now
survives the session that produced it (P0's silent-stall lesson, extended: the
first P1 agent died mid-build and its diagnosis was lost with it).

**Artifact:** `src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk`
— 349,909,757 bytes, `tauri` exit 0.

Verified from the APK itself (`aapt2 dump badging` / `apksigner verify` /
zip listing), not from build logs:
- `package: name='com.cleophis.app' versionCode='1000' versionName='0.1.0'`,
  `minSdkVersion:'24'`, `targetSdkVersion:'36'` — the 0.1 identity split is live
  in the shipped artifact, and desktop keeps `com.cleophis.desktop`.
- Debug-signed (`CN=Android Debug`, SHA-256
  `552c5cdf…76a3e`) → installs via `adb install` without a signing ceremony.
- 0.3 hardening present in the *packaged* manifest: `allowBackup=false`,
  `REQUEST_INSTALL_PACKAGES`, `POST_NOTIFICATIONS`, `FOREGROUND_SERVICE`,
  `FOREGROUND_SERVICE_SPECIAL_USE`, and `service com.cleophis.app.InferenceService`
  with `foregroundServiceType=0x40000000` (specialUse).
- `lib/` contains **only** `arm64-v8a` (`libcleophis_lib.so`, `libc++_shared.so`).
- `assets/` contains **only** `tauri.conf.json` (2,365 bytes).

**Size note (expected, not a defect):** 340 MB of the 334 MiB APK is the single
unstripped debug `libcleophis_lib.so` — llama.cpp compiled `-O0` with full
debuginfo. Release builds strip and optimize this to a small fraction. Nothing
else in the APK is above 11 MB.

#### Three build blockers found and fixed

1. **`bundle.resources` leaked the entire desktop resource tree into the APK
   (correctness + supply-chain hygiene).** The Android build was packing 239 MB
   of *Windows* binaries — `resources/llama/*.dll`, `llama-server.exe`,
   `resources/ocr/*.dll` + `tesseract.exe`, `pdfium.dll`, the x86 embedder
   GGUF — into `assets/`, all of it dead weight on ARM Android and none of it
   reviewable as mobile code. Brief §4 requires the Android bundle ship no
   desktop binaries.
   **The documented fix does not work:** the brief says set
   `bundle.resources = {}` in `tauri.android.conf.json`. Tauri merges the
   platform config with `json_patch::merge` (RFC 7386 JSON Merge Patch,
   `tauri-utils-2.9.3/src/config/parse.rs:185`), where an **object patch merges
   recursively — `{}` is a no-op** and only `null` removes a key. Confirmed
   empirically before the fix: the generated `assets/tauri.conf.json` still
   listed all six desktop resource dirs. Now `"bundle": {"resources": null}`;
   post-fix the APK's `assets/` is just `tauri.conf.json`.
   `app/.gitignore` also gained `/src/main/assets/resources/` so a generated
   239 MB tree can never be committed.
2. **`NDK_HOME` is required and is not in the P0 env block.** `tauri android`
   (cargo-mobile2) resolves the NDK from `NDK_HOME` *only* — not the
   `ANDROID_NDK_ROOT`/`NDK_ROOT`/`ANDROID_NDK` trio that `llama-cpp-sys-2`
   reads. Without it the CLI aborts before doing any work with a misleading
   message about the *SDK*: "failed to ensure Android environment: Skipping
   Android Studio command line tools installation". Exported in the build
   script; `docs/superpowers/mobile-dev-setup.md` should gain it too.
3. **The scaffolded gradle `BuildTask` cannot invoke the Tauri CLI.**
   `tauri android init` generated `buildSrc/.../BuildTask.kt` with
   `executable = "node"` and a bare `"tauri"` first argument, run with
   `workingDir` = `src-tauri`. Node therefore resolved `tauri` as a *module
   path* and died with `Cannot find module '…/src-tauri/tauri'`, failing
   `:app:rustBuildArm64Debug` — even though the Rust cross-compile itself had
   already succeeded and symlinked `libcleophis_lib.so` into `jniLibs`. Changed
   to `npm` + `run --silent tauri -- android android-studio-script`; npm locates
   the workspace `package.json` by walking up from `src-tauri`, and the existing
   Windows fallback in that task then resolves `npm.cmd` correctly. `BuildTask.kt`
   is tracked, carries no "autogenerated" header, and the identifier is locked,
   so no future `tauri android init` will clobber the edit.

#### Carried into later phases

- `usesCleartextTraffic=true` is injected into the **debug** manifest by Tauri
  (dev-server support). The 5.2 release-config audit must confirm it is absent
  from release, alongside `debuggable=false` and the `abiFilters` check.
- The Tauri CLI warns that an identifier ending in `.app` collides with the
  macOS bundle extension. Harmless for Android and macOS is not a target, but
  it is recorded here because the identifier is otherwise locked.

---

## Phase 0 founder items surfaced (⚑0.4)

1. Android APK signing-key ceremony (crown-jewel #2; same regime as curator key,
   two offline backups verified restorable). Blocks the first signed APK (Phase 4).
2. Google Play developer account ($25) + H8 developer-verification registration
   (Thailand in the outside-Play verification pilot — our market). Register early.
3. Two more real test devices for the 3-device install/update gate (spec §3: one
   8 GB mid-ranger, one Huawei/Honor no-GMS). A22 is device #1.
