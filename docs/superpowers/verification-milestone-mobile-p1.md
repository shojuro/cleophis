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

#### Build-blocker list — Phase 0 lessons (gate-report material)

Two of the three are worth carrying beyond this repo:

- **A config merge that silently does nothing looks exactly like a config merge
  that works.** `bundle.resources = {}` read as correct in review, in the config
  file, and in the diff — the only place it was observably wrong was the built
  artifact. Nothing warns you; the APK just quietly grows by 239 MB of the wrong
  platform's binaries. The general rule for RFC 7386 merge: **`{}` is a no-op,
  `null` deletes.** Any instruction anywhere in the P1 plan of the form "set X to
  `{}` in the platform config" is wrong the same way. Corollary adopted here:
  bundle-content claims are verified by unzipping the artifact, never by reading
  the config that was supposed to produce it.
- **A failure's location in the build graph is not where it comes from.** The
  gradle launcher bug (`node` + bare `"tauri"` arg → `Cannot find module
  '.../src-tauri/tauri'`) surfaced as `Execution failed for task
  ':app:rustBuildArm64Debug'` — a *Rust* task — and only after the Rust
  cross-compile had genuinely succeeded and symlinked `libcleophis_lib.so` into
  `jniLibs`. Reading the task name and re-attacking the Rust build (which was
  never broken) is the natural wrong move, and is the most likely thing that
  consumed the previous agent's context. The lesson that generalizes: when a
  build task fails, find the last thing it *did* before it failed, not the thing
  the task is named after.
- Third blocker (`NDK_HOME`) was ordinary toolchain drift, now fixed at the
  source in `docs/superpowers/mobile-dev-setup.md`. Its only interesting
  property is that the error text blamed the SDK for an NDK problem.

**Flaky desktop family: `cloud::rest`/`session` mock-server tests (desktop
scope, not P1's).** The Phase 1.1 Windows gate needed three runs to produce
evidence-grade output:

| run | conditions | result |
|---|---|---|
| 1 | parallel, machine loaded (an Android build running) | 263 passed / **3 failed** |
| 2 | parallel, quiet machine | 265 passed / **1 failed** |
| 3 | `--test-threads=1` | **266 passed / 0 failed, exit 0** |

The failures *rotate*: run 2's single failure (`create_checkout_request_shape`)
was not among run 1's three, and run 1's three all passed in run 2. Every one is
in the `cloud::rest`/`session` mock-server family, every one passes in
isolation, and none is in a file this branch touches.

That rotation is the tell, and it is the transferable part: **a failure set that
changes between runs is evidence about the harness, not the code.** A fixed set
of failures would have implicated 1.1; a rotating set under load points at
contention between mock servers racing for ports. Two consequences adopted —
future Windows gate runs on this machine use `--test-threads=1` by default, and
the family is a candidate for a port-allocation fix upstream (desktop scope,
surfaced not owned).

#### Carried into later phases

- `usesCleartextTraffic=true` is injected into the **debug** manifest by Tauri
  (dev-server support). The 5.2 release-config audit must confirm it is absent
  from release, alongside `debuggable=false` and the `abiFilters` check.
- The Tauri CLI warns that an identifier ending in `.app` collides with the
  macOS bundle extension. Harmless for Android and macOS is not a target, but
  it is recorded here because the identifier is otherwise locked.

---

## Phase 1 — Engine swap-in

### 1.1 `inference.rs` cfg seam + Android resource materializer — DONE (commit 2a52b44)

**The seam.** Desktop's sidecar machinery (`spawn_server`, `build_server_args`,
the health poll, `child_exited`/`kill_child`, `sweep_stray_servers`, the
watchdog `start`, `restart`, `shutdown`, `start_if_no_model`, and the sidecar
unit tests) is now `#[cfg(desktop)]`; `#[cfg(mobile)]` re-exports the same four
entry points from the new `engine_inproc.rs`. `Engine.child` is desktop-only.
Every caller — `lib.rs` setup, `load_model`, the window `Destroyed` handler,
`cloud::download`'s completion path — names `inference::start` and friends on
both platforms and compiles unchanged; `tier_select.rs` and `cloud/download.rs`
were not touched, as the brief required.

**Two extractions that make shared code actually shared:**
- `verify_launch()` — the entire load-time integrity gate (base model, then
  each declared LoRA) behind one `pub(crate)` fn that *both* platforms call.
  This matters beyond tidiness: "tampered file → engine-failed" is now one
  implementation with two callers rather than two implementations that agree
  today. Desktop keeps the identical `and_then` chain, relocated.
- `Engine::clear_verify_caches()` replaces `restart`'s three inline resets.

**The materializer.** Android bundles no resources (Phase 0 saw to that), so
`resources_embed.rs` compiles `catalog.json` + the 11 covers (~250 KB) into the
binary with `include_bytes!` and writes them to `<app_data>/resources` on first
launch. Keyed by a sha256 stamp over the whole payload — names included — so an
app update rewrites the tree and an unchanged one skips. **The stamp is written
last**, so an interrupted materialization leaves no stamp and simply redoes
itself rather than pairing a valid stamp with a truncated catalog.
`resources_root()` gained a mobile branch returning that tree; all consumers
(catalog reads, `get_catalog`'s `coverAbs`, `tier_select`) are unchanged.
Materialization runs first in `setup`, before `model_path` reads through
`resources_root`; failure is logged and non-fatal, degrading to the same
empty-catalog/NoModel path an absent catalog already produces.

A macro declares the cover list once and derives both the name array and the
byte array from it. This is a correctness guard, not style: a hand-maintained
second array could pair one cover's *name* with another's *pixels* — silent,
untestable by any assertion on counts, and visible to every user.

**1.1 is the seam, not the engine.** `engine_inproc`'s bodies are fail-closed
placeholders that report `Failed` rather than pretending to be `Ready`, so a
checkpoint build says "no engine" instead of hanging on a chat that can never
stream. One deliberate exception: **the integrity gate is already live**, ahead
of the engine it guards, so the tampered-file probe is exercisable on-device at
CP1 whether or not generation works, and 1.2 inherits a gate that has already
run on real hardware rather than one written blind.

#### Verification

- `cargo ndk -t arm64-v8a -P 24 check -p cleophis --all-targets` — **clean,
  zero warnings** (`cleophis-mobile-logs/p11-android-tests-check.log`).
  `--all-targets` also type-checks the new tests.
- The two cover/catalog assertions were validated against the real data
  independently (11 embedded == 11 on disk; every catalog `cover` embedded and
  under `covers/`), since they cannot be *run* on this host — see below.
- An earlier iteration of this work produced 11 dead-code warnings on aarch64.
  Rather than blanket-suppress them, they were resolved on the merits: most
  vanished once the placeholder called the shared integrity gate for real, two
  were genuinely desktop-only (`LaunchPaths::loras`, the `Arc` import) and got
  cfg gates, and only the three that belong to 1.2's inference thread
  (`thread_alive`, `restart_lock`, the unconstructed `EngineStatus` variants)
  carry a narrow `cfg_attr(mobile, allow(dead_code))` with a note to remove it
  when 1.2 lands. Warnings that are merely silenced come back as blind spots
  when Phase 5.3 wires the `mobile-check` CI job.

#### Desktop gate — PASS (Windows suite, 266 passed / 0 failed, exit 0)

Run by steering on this worktree at commit `c0e2505`. **The decisive run is
single-threaded (`--test-threads=1`): 266/0, exit 0.** Both new 1.1 tests
(`embedded_cover_list_matches_the_resources_directory`,
`every_catalog_cover_is_embedded`) executed and passed. Logs:
`scratchpad/win-test-p1-c0e2505{,-run2,-st}.log` (steering side).

The two parallel runs that preceded it are recorded below as a flake story, not
as failures of this branch.

#### ⚠ Desktop regression cannot run on this Linux host

`cargo test -p cleophis` fails here before compiling any of our code:
`keyring`'s `linux-native` feature pulls `libdbus-sys`, whose build script
requires system dbus development headers, and the toolchain is userspace-only
by founder decision (no sudo). This is environmental and pre-existing — it is
also why Phase 0.2's Linux evidence was the golden-pack (`kpack-embed`) plus
the pure crates, never the app crate. **The desktop gate for Phase 1 is
therefore the steering-side Windows suite, requested at each phase boundary.**
Every desktop-visible change in 1.1 is a cfg gate, a visibility widen, or a
semantics-identical extraction.

#### Packaging finding: rebuilding on top of an APK silently doubles it

The first 1.1 APK came out at **690,099,887 bytes** — roughly double Phase 0's,
from a change that added 37 KB of embedded resources. The `.so` was unchanged
(340,173,744 vs 340,136,376, i.e. exactly the catalog + covers). The archive's
own central directory summed to ~350 MB against a 690 MB file: **~340 MB of the
file was an orphaned copy of the previous `libcleophis_lib.so` that no entry
pointed at**, left behind by AGP's incremental zip (zipflinger) rewriting a
large entry in place.

The APK installs and runs perfectly in that state — the central directory is
authoritative — which is exactly why it would have gone unnoticed, and the
founder would have sideloaded 658 MiB for no reason. Deleting the previous
output before packaging restores it: **349,947,125 bytes**, the Phase 0 size
plus precisely the 37,368 bytes of embedded resources.
`build-android-apk.sh` now always removes the prior APK first.

Generalization for the release work in 5.2: **APK size must be judged against
the sum of its zip entries, not the file length** — the two can differ by a
factor of two with nothing wrong in the build.

Worth noting for Phase 3.1: moving `keyring` to
`[target.'cfg(not(target_os = "android"))'.dependencies]` (already required
because keyring v3 silently mocks in-memory on unsupported targets) will *not*
fix this — Linux is exactly where `linux-native` applies.

### 1.2 In-process engine lifecycle — DONE (commit 4a1bc6f)

`engine_inproc` now owns one long-lived inference thread and drives
`kpack-engine`'s `EngineBackend` on it.

**Why a thread rather than a call.** `EngineHandle` is deliberately `!Send`:
`llama-cpp-2`'s `LlamaLoraAdapter` holds a raw `NonNull` and `LlamaContext` is
thread-bound, so a loaded stack must live entirely on the thread that created
it. The handle never leaves the inference thread; callers speak to it by mpsc
`Command`. Desktop gets the same isolation for free from the sidecar being a
separate process.

**Lifecycle mapping.** `start` is idempotent with respect to the thread — an
already-running thread is re-tasked with a fresh `Load` rather than a second
being spawned, and `thread_alive` is set before the spawn (desktop's handshake)
so a concurrent restart cannot race a second thread into existence. `Load`
releases the previous handle **before** loading the new one; loading first would
momentarily need both models resident, which the 8 GB reference device cannot
afford. `restart` serializes on the shared `restart_lock` and joins the thread
before starting another — desktop's version of that race is two llama-servers
fighting over a port, but here the contended resource is RAM, where a doubled 4B
stack is fatal rather than merely wasteful. `shutdown` joins so the native model
is released before teardown.

**Panics → Failed, never hung.** An `ExitGuard` moves the engine to `Failed` and
emits `engine-failed` on any thread exit that was not a deliberate shutdown, so
a panic cannot strand the UI on a `Starting` spinner. It is poison-tolerant,
since panicking a second time inside `Drop` aborts the process.

**Integrity.** The gate is the backend's own `prepare_stack`, fed the catalog's
pinned hashes through `ModelSpec`/`AdapterSpec` via the new
`inference::launch_hashes`; nothing native is touched until every artifact
passes. This **replaces** 1.1's placeholder call to `inference::verify_launch`
— running both would hash a multi-gigabyte model twice on the device least able
to afford it. `verify_launch` and the entire per-slot cache it drives
(`verify_*_once`, `verify_hash_once`, `model_sha256`, the three `verified_*`
fields, `clear_verify_caches`) are consequently `#[cfg(desktop)]` now. Mobile
needs no cache clearing on a tier switch: `kpack-engine`'s `VerifyCache` is
keyed by *path*, and a switch resolves different files, so the new stack
re-verifies on its own.

CPU-first is policy on Android, not a fallback, so `gpu_offload` is pinned false
and there is no GPU→CPU retry ladder. `EngineStatus::Restarting` is therefore
desktop-only (it reports the sidecar's fallback and watchdog respawn) and stays
in the shared enum only because `EngineInfo` is one serialized shape for both.

#### Verification

- `cargo ndk -t arm64-v8a -P 24 check -p cleophis --all-targets` — **clean, zero
  warnings** (`cleophis-mobile-logs/p12-android-check.log`). The five dead-code
  warnings the cfg change first produced were resolved by gating the
  genuinely-desktop-only hash chain, not by an allow attribute.
- **It links.** A `check` does not link, and 1.2 is the first code to reference
  `LlamaEngine` for real, so a full APK build was run to prove the native
  symbols resolve: `tauri` exit 0, **352,809,813 bytes**, sha256
  `5cd5dad1…f5d526`, debug-signed, arm64-v8a only, assets still just
  `tauri.conf.json`. The +2.9 MB over 1.1 is the kpack-engine wrapper alone —
  llama.cpp's core was already linked in via the bundled BGE embedder.

Generation (`chat_stream` / `chat_complete` / `chat_cancel`, the calc tool-loop,
the partial-turn flush) is 1.3–1.4 and will be served by this same thread,
opening a session from the handle it already owns.

---

## Phase 0 founder items surfaced (⚑0.4)

1. Android APK signing-key ceremony (crown-jewel #2; same regime as curator key,
   two offline backups verified restorable). Blocks the first signed APK (Phase 4).
2. Google Play developer account ($25) + H8 developer-verification registration
   (Thailand in the outside-Play verification pilot — our market). Register early.
3. Two more real test devices for the 3-device install/update gate (spec §3: one
   8 GB mid-ranger, one Huawei/Honor no-GMS). A22 is device #1.
