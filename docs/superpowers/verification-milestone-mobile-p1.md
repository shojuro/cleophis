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
- **Windows desktop suite — WALL (surfaced to steering):** Windows-side cargo is
  unreachable from this WSL session — `binfmt_misc` is enabled but the
  `WSLInterop` handler is not registered (`systemd=true` in `/etc/wsl.conf`
  drops it), so `cmd.exe`/`powershell.exe` fail with "Exec format error".
  Windows cargo exists at `C:\Users\JM505 Computers\.cargo\bin\cargo.exe` but
  cannot be invoked. The full Windows-target desktop suite needs a founder-side
  run or an interop fix. The H15-critical part (real embedder revalidation)
  runs on Linux (golden-pack, above) and is what actually de-risks the patch.

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

---

## Phase 0 founder items surfaced (⚑0.4)

1. Android APK signing-key ceremony (crown-jewel #2; same regime as curator key,
   two offline backups verified restorable). Blocks the first signed APK (Phase 4).
2. Google Play developer account ($25) + H8 developer-verification registration
   (Thailand in the outside-Play verification pilot — our market). Register early.
3. Two more real test devices for the 3-device install/update gate (spec §3: one
   8 GB mid-ranger, one Huawei/Honor no-GMS). A22 is device #1.
