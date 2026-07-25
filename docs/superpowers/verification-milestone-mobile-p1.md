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
  `552c5cdf9d2136e92e6e9d17b4859163fe414d1d794a55e9cf428d6649d76a3e`)
  → installs via `adb install` without a signing ceremony.
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

#### 📱 CP0 FOUNDER VERDICT: **PASS** (Galaxy A22, on hardware)

- Installs under `com.cleophis.app` ✓
- Launches to a working window ✓
- **The library screen populates with the packs** — the `resources_embed`
  materializer is confirmed working on real hardware, not just in theory.

Two observations from the founder, one a defect and one not:

1. **Cover images do not render.** This is the plan's D3 asset-protocol-scope
   risk, confirmed on device. Root cause and fix below.
2. **Sign-in fails.** *Expected at CP0, not a defect* — cloud auth is Phase 3
   (`secure_store` seam + Kotlin AndroidKeyStore bridge) and nothing implements
   it yet. Recorded here so a later reader does not mistake it for a regression.

#### Unscheduled on-device verification: sign-up, payment, entitlement, download

The founder exercised, spontaneously and in one session on the A22, an entire
band of functionality that was not scheduled for verification until CP2/CP3:

- **Account sign-UP succeeded on-device** — which means `ureq` + rustls TLS
  works against our backend from Android, the portability question Phase 3 was
  going to have to answer.
- **Stripe checkout round-tripped** — left the app, paid, returned, and the
  **entitlement was recognized**, so the external-browser hand-off and the
  entitlement gate both work under Android's app-switching.
- **The 1B model downloaded to 100 %** — the first completed model download on
  Android, exercising catalog resolve → CDN fetch → the full
  `run_ranged_download` path.
- The app then transitioned toward chat.

**Recorded as pre-verified *with caveats*, not as gate evidence** — these paths
have not been run against their actual acceptance criteria (airplane-mode
suite, force-stop survival, sign-out purge), and one session is not a gate.

**⚠ Caveat that materially qualifies the sign-in result — known-until-3.1.**
`keyring` v3 **silently mocks in-memory on unsupported targets**, which is
precisely why the brief requires moving it to
`[target.'cfg(not(target_os = "android"))'.dependencies]` in Phase 3.1. Until
the Kotlin AndroidKeyStore `SecureStore` lands, an Android session lives in
process memory only: **a force-stop logs the user out**, and nothing is
persisted to a keystore. So "sign-up worked" is true and "auth works on Android"
is not — the credential *storage* half is still entirely unbuilt. This is also
the likely explanation for the earlier "sign-in fails" report (a lost in-memory
session, or expectations set by a different flow), and it is explicitly **not**
being chased as a bug until 3.1 makes a real claim possible.

**Engine load remains unclaimed.** The pill's state transitions "went by too
fast to read", so no terminal state was observed. Per the rule established by
the retraction below: **no terminal state, no claim.** Whether the pinned-hash
gate → `LlamaEngine::load` → `engine-ready` path succeeded is still unknown,
and it stays unknown in this document until someone reads the locked-in state.

**2.2 UX requirement, from that same unreadable transition:** post-download
state changes flash past too quickly to follow. A visible
verified → loading → ready progression is not decoration here — this product's
pitch is that the model is *yours and on your device*, and watching it be
verified and loaded is exactly the moment that claim becomes credible. Fast is
the wrong optimization for the one transition worth showing.

#### ~~Pre-CP1 evidence — the download chain works in-app~~ **RETRACTED**

**This evidence line was wrong and is withdrawn. No model download has ever run
on Android; the catalog→CDN download flow remains UNVERIFIED on device.**

What happened: a reported figure of "673,926 KB" alongside the word
"downloading" was taken as a model download in progress. It was the **app's
installed size**. It reconciles exactly — the 1.2 APK is 352,809,813 B
(344,541 KB) and Android additionally stores the extracted debug
`libcleophis_lib.so` at 340,173,744 B (332,201 KB), totalling 676,742 KB against
the 673,926 KB observed, a 0.4 % gap that is ordinary Android size accounting.
A 334 MiB APK carrying a 324 MiB unstripped debug library is *why* the installed
footprint looks like a model download; a release build would not.

**Why the sanity-check failed to catch it, which is the part worth keeping.**
The check compared 673,926 KB against the low tier's 807,694,112 B model, found
it was ~85 % of it, and concluded "a progress figure, plausible, right file."
Every step is true and the conclusion is false. The error was **testing only the
hypothesis I was handed**: any number between zero and the total is ~x % of it,
so "it's a plausible fraction" is not evidence — it cannot fail. The question
never asked was *what else could produce this number*, and one substitution of
the app's own size answers it exactly. A confirming check that had no way to
come back negative is not a check. Two habits adopted: state what a proposed
figure would have to be *inconsistent with* before calling it confirmation, and
treat "in progress" numbers as unverified until a terminal state is observed.

The sharper form of the same lesson: **one number fit two stories, and the
user's direct observation outranks arithmetic plausibility.** The banner in the
founder's own screenshot reads "Model not downloaded yet." — ground truth that
was available the whole time and settles it without arithmetic. When a
calculation and an observation disagree, the calculation is the thing on trial.

**What survives from that check, and is still useful:** the low tier does
resolve to `models/Llama-3.2-1B-Instruct-Q4_K_M.gguf` at `fileBytes`
**807,694,112** (versus mid's 4B at 2,497,280,384 and high's 8B at
5,027,783,616). So the *right file would be pulled* — that resolution is
documented and correct. What is withdrawn is only the claim that anything was
pulled.

Consequences: nothing was killed by app suspend, because nothing started, so the
suspend-kill theory is withdrawn too. The resume-from-`.part` behaviour is
simply untested rather than suspect, and stays where it was planned — CP1/CP2.

Caveat still standing for CP1: the A22 lands on `low` via the RAM-only
`tier_for`, which is **lucky-correct on this device, not correct in general** —
an 8 GB phone with floor-class silicon is precisely the case the SoC-aware
`tier_for_mobile` in 2.3 exists to catch.

*(Attribution: the misreading originated in relay, not with the founder, and
the correction came from steering. Recorded because a gate report that silently
rewrites its own evidence is worth less than one that shows where it was
wrong.)*

#### CP0 addendum — field evidence from the A22 screenshot

Read directly from the founder's screenshot (`Screenshot_20260725_151829_
Cleophis.jpg`), so these are observations rather than relay.

**✅ VERIFIED WORKING ON-DEVICE — the §7 conversation store.** The founder
created a chat, the hero's `greeting` rendered, they sent "Hello", it persisted
and displays, and the chat is listed in the sidebar with its action row. That is
SQLite + the per-account conversation directory + message append + chat listing,
all working on Android, unprompted, with **zero mobile-specific work done on
convstore** — it compiled and ran as-is. A P1-gate line item confirmed early.
It also confirms catalog data (the `greeting` field) reaching the UI.

**✅ Not a bug — "Open chat" alongside "Model not downloaded yet."** The button
is keyed off pack availability, not model presence, and the banner states the
model situation separately. Coherent by design; the earlier button-state
question is closed.

**❌ Expected pre-2.2 — layout.** Field requirements for 2.2, captured from the
image rather than described in the abstract:
- The desktop sidebar takes **~2/3 of the phone's width**, crushing the chat
  column into the remaining third; message text wraps at one or two words per
  line. The sidebar must become a drawer, not a narrowed column.
- The view transition **slides ~1/4 and stops**, leaving the layout distorted
  and chat unusable. Per steering: the transition must be **disabled or
  replaced** on mobile, not merely restyled.
- The status pill is **truncated off the right edge** ("● Lo…"), and the hero
  title clips mid-word. See the finding below — this turned out to be worse
  than a clipping bug.
- The hero cover renders as a **broken-image icon** — independent confirmation
  of the asset-scope defect fixed below.

**✅ Expected pre-2.1, and a baseline to PRESERVE — graceful send failure.** The
founder sent "5+5=19?" and got a *"That didn't go through — tap to retry"* chip.
That is exactly right for this build: the desktop transport still posts to
`127.0.0.1`, which on mobile has no server behind it and is blocked by the CSP
besides. What matters is how it fails — no crash, no lost input, and the message
still renders in the transcript because convstore already persisted it.

Recorded as a **requirement for 2.1, not just an observation**: when the
invoke+Channel transport replaces the fetch path, this retry chip must behave
identically on failure. It is easy to lose a graceful degradation while
replacing the thing that was degrading, and the current behaviour is the
reference for what "failed to send" should look like.

#### 🔴 The most significant UX finding so far: the engine state is invisible

**The founder could not identify the status pill at all.** Asked to read it,
they took "the pill" to mean the Open-chat / download button. This is not a
styling complaint — *the person who commissioned the product could not locate
the element that reports whether the model is loaded.*

Two compounding causes, and the second is the serious one:

1. It is clipped to about two characters ("● Lo…") by the portrait layout bug,
   so even when found it cannot be read. The founder has had to rotate to
   landscape to attempt it.
2. **The onboarding flow railroads account → payment → chat with no moment that
   presents engine state at all.** Nothing in the path a first-time user walks
   ever draws attention to it, so there is nowhere to learn that the indicator
   exists or what it means.

**Combined 2.2 requirement: engine state must be an element a first-time user
can find and read unprompted** — never clipped, and given a visible moment in
the flow rather than left as ambient chrome.

This is the same family as the too-fast post-download transition recorded
above, and together they point at one product problem rather than two UI bugs.
The pitch is that the model is *yours, on your device*. Every mechanism that
would make that tangible — verification, loading, readiness — currently either
flashes past unreadably or sits in a corner the user never looks at. **The
trust this product sells is built exactly at those moments, and right now the
app spends them silently.**

**UX note for 2.2 (from the retraction above):** something in the pill/banner
area read convincingly as "downloading model" to the founder when no download
existed. Whatever produced that impression, model-state copy should make
"no model" / "downloading" / "ready" unmistakable — a user believing a download
was running when none was is a copy defect regardless of which element caused it.

#### The cover-rendering defect — root cause and fix

`convertFileSrc(coverAbs)` in `app.js:134` routes cover images through Tauri's
asset protocol, which is gated by `assetProtocol.scope`. Desktop's scope is
`["$RESOURCE/**"]` and the covers live under the install's resource dir, so it
matches. On Android the covers are materialized to
`<app_data>/resources/covers/` — **not** a `$RESOURCE` path — so every cover URL
falls outside the scope and is refused. The catalog still renders, which is why
the screen populates with blank art rather than failing outright.

Fixed in `tauri.android.conf.json` by scoping the asset protocol to
`$APPDATA/resources/covers/**` on Android only.

**Why this is provably right rather than a plausible guess** — the concern with
a path-variable fix is that the variable might resolve somewhere other than
where the code actually writes. It cannot here:
`tauri-2.11.5/src/path/mod.rs:332` resolves `BaseDirectory::AppData` as
`resolver.app_data_dir()`, which is the *same call* `resources_embed::
materialized_root` uses to choose the destination. One function, two callers —
they cannot disagree, on any platform. That is what made the scope fix
preferable to the pre-designed base64-command fallback: same guarantee, one code
path instead of two, and no image bytes crossing the IPC boundary.

Two details that make the merge behave:
- `scope` is an **array**, and RFC 7386 replaces non-object values rather than
  merging them, so the Android array *replaces* `["$RESOURCE/**"]` outright
  instead of appending to it. That is the desired outcome and it is also
  tighter: an Android build has no `$RESOURCE` tree to grant access to, and the
  scope narrows further to `covers/**` rather than all of `resources/`.
- `assetProtocol` is an object, so `enable: true` is inherited from the base
  config and only `scope` is overridden.

The mobile CSP already permits `asset:` and `http://asset.localhost` in
`img-src`, so no CSP change was needed.

**Verified in the built artifact** (this repo's standing rule: config claims are
checked against the APK, never the config source). Reading
`assets/tauri.conf.json` out of the rebuilt APK:

```json
"assetProtocol": { "scope": ["$APPDATA/resources/covers/**"], "enable": true }
```

Both merge predictions hold: the array was **replaced** (not appended), and
`enable: true` was **inherited** from the base object. `src-tauri/tauri.conf.json`
still reads `["$RESOURCE/**"]`, so desktop is untouched.

**APK carrying the fix — 352,809,173 bytes, sha256**
`7616f10057904c4a0eb9b5f771de658102ed3cdda739310a5f99bf9159777c4f`
(independently hashed from the artifact by both this agent and steering).

**✅ VERIFIED ON HARDWARE — covers render on the A22.** The loop is fully
closed: the path resolution was *proven* by reading Tauri's source, and the
visual outcome is now *observed* on device. Both halves were necessary — the
proof said the scope would match, but only the device could say the images
appear.

#### App icon — "Facet", wired Android-only

Founder-chosen mark (from three steering-drafted candidates): ink-navy
(`#141829`) rounded field, four-facet magenta diamond, the brand system from the
pitch deck and website. Master committed at
`docs/superpowers/mobile-tools/brand/icon-master-1024.png`, sha256
`d65fb117de418337352a0b730cf48c81ac5ca3896f002586e838d612eb55cd4a`, verified
against the staged file before use.

Generated by `docs/superpowers/mobile-tools/gen-android-icons.py` (committed, so
the set is reproducible rather than a one-off). **Android-only by construction:
the script never touches `src-tauri/icons/`** — a desktop rebrand is a
main-branch founder decision, and a blanket `tauri icon` run would have
clobbered it.

Three things the scaffold got wrong that this corrects:

1. **Adaptive icons were inert.** `tauri android init` emitted
   `ic_launcher_foreground.png` at every density but **no
   `mipmap-anydpi-v26/`**, so nothing referenced them and Android fell back to
   the legacy raster — which launchers then mask and letterbox themselves,
   usually onto white. Added `mipmap-anydpi-v26/ic_launcher{,_round}.xml` plus
   a `ic_launcher_background` colour resource, so the adaptive path is live.
2. **`android:roundIcon` was never declared**, so the round variant could not be
   used no matter what was in the mipmaps. Added to the manifest.
3. **hdpi was 49 px**, which is not an Android bucket size (mdpi 48 → hdpi
   **72**). Corrected across the set: 48 / 72 / 96 / 144 / 192.

Also deleted `drawable/ic_launcher_background.xml` and
`drawable-v24/ic_launcher_foreground.xml` — the stock Android Studio **green
droid** template vectors, unreferenced by anything (verified by grep before
removal) and shipping in every APK we had built.

**A defect caught by looking at the output.** The first foreground render was
the diamond *plus a navy square*: the diamond is a rotated square, so cropping
to its bounding box keeps field colour in the box's corners, and that would have
pasted a navy square onto the adaptive background layer. The geometry assertions
all passed — 60 % of canvas, inside the safe zone, centred — because they
measured the bounding box, which was correct. Only rendering the PNG showed it.
Fixed by ramping alpha with distance from the field colour (soft, so the
master's antialiased edge survives) rather than cropping. Recorded because
"the numbers were right and the image was wrong" is the whole argument for
looking at visual output rather than asserting about it.

Safe zone honoured: the mark spans **60 % of the 108dp canvas** against a
72/108 = 66.7 % guaranteed-visible area, so it clears every mask shape rather
than grazing the edge.

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

**A check that measures what you already believe cannot falsify it — the visual
case.** The download retraction gave this lesson its arithmetic form ("~85 % of
the total" is a fraction that can never come back negative). The icon work gave
it a second, independent form, which is why the pair is worth keeping together:

The first adaptive foreground rendered as the diamond **plus a navy square**,
because the diamond is a rotated square and cropping to its bounding box keeps
field colour in the box's corners. Every assertion passed — 60 % of canvas,
inside the 72/108 safe zone, centred to sub-pixel — and every one was *true*.
They measured the bounding box, and the bounding box was correct. The defect
lived in the pixels *inside* that box, which nothing was looking at. Only
rendering the PNG and viewing it revealed a bug that would have shipped a navy
square onto the adaptive background layer.

The generalization across both: **numeric assertions confirm the property you
thought to measure, never the one you didn't.** For anything with a visual or
externally-observable output, render it and look — the artifact is the evidence,
the measurements are only a proxy for it. This is the same instinct as the
project's other standing rule (verify bundle contents against the built APK, not
the config that was meant to produce it).

**And the third domain: provenance.** A check can be capable of failing, run
correctly, and still measure *a different artifact than the one it is labelled
with* — the case recorded under the dead-code episode, where suite runs pinned
to commit hashes had actually compiled an in-flight working tree. That failure
is nastier than the first two, because a correct measurement of the wrong thing
looks exactly like a correct measurement of the right thing. The countermeasure
is not a better assertion but a **recorded identity for what was measured**:
tree state alongside the commit, before and after. Numbers, pixels, provenance —
each needed its own lesson, and none of the three generalized from the others
until it had bitten.

**Flaky desktop family: `cloud::rest`/`session` mock-server tests (desktop
scope, not P1's).** Establishing that Phases 1.1 and 1.2 were non-regressive
took six full-suite runs across two commits, because the suite is
nondeterministic at fixed code:

| # | commit | conditions | result |
|---|---|---|---|
| 1 | c0e2505 | parallel, machine loaded (Android build running) | 263 / **3 failed** |
| 2 | c0e2505 | parallel, quiet machine | 265 / **1 failed** (a *different* test) |
| 3 | c0e2505 | `--test-threads=1` | **266 / 0, exit 0** |
| 4 | da7f8d1 | `--test-threads=1` | 264 / **2 failed** |
| 5 | da7f8d1 | those 2 in isolation | **5 / 5 ok** |
| 6 | da7f8d1 | `--test-threads=1` control, same commit | **266 / 0, exit 0** |

**Runs 4 and 6 are the decisive pair: identical commit, identical command,
different results.** That is nondeterminism proven at fixed code, which is the
only thing that can definitively exonerate a branch — no amount of green runs
alone could. Verdict: **1.1 + 1.2 cfg-gating is verified non-regressive on
desktop.**

Every failure across all six runs was in the `cloud::rest`/`session` mock-server
family; all passed in isolation; none was in a file this branch touches; and the
behaviour is independent of threading (run 4 was single-threaded). Likely
mechanism: **stale pooled HTTP connections to recycled ephemeral mock-server
ports** — which fits all four observations, including why it only appears in a
full suite.

The transferable lesson: **a failure set that changes between runs is evidence
about the harness, not the code.** A *fixed* set would have implicated the
branch; a rotating one under load points at the harness. The corollary is that
the right response to a suspicious green is a same-commit control run, not more
green runs.

### ⚑ Gate-run protocol v2 — provenance (supersedes commit-hash-only attribution)

Adopted after the contamination episode above. **A commit hash records what was
checked out, not what was compiled**, so a run is only attributable to a commit
if the tree was clean throughout it.

1. Every desktop gate run records `git status --porcelain` **and** HEAD, both
   **before and after** the run. **All four must agree** — clean tree, same
   commit, both times.
2. If they do not agree, the run is attributed to **"live worktree near
   `<commit>`"**, never to the commit itself.
3. **Freeze.** When steering launches a gate run it says *"freeze for gate
   run"*; this agent holds all edits until *"thawed"*. The porcelain check
   catches contamination after the fact; the freeze prevents it.
4. Prior runs keep their pass/fail verdicts — a passing superset-in-motion still
   shows desktop unbroken at that moment — but their **commit attributions are
   softened** to "live worktree near `<commit>`" wherever a clean tree was not
   established. One honest sweep, applied below.

**Honest sweep over prior runs (one blanket correction, applied here rather than
rewriting each line).** No gate run in this document before protocol v2 recorded
tree cleanliness. Their **verdicts stand** — a suite that passes against a
superset-in-motion still demonstrates desktop was unbroken at that moment, and
several ran while nothing was being edited — but **every "at `<commit>`" in the
gate records below should be read as "live worktree near `<commit>`"** unless
stated otherwise. Two are known-contaminated and identified as such in the
episode above (`22f0aa8`, `a959717`); the rest are simply unverified in this
respect rather than suspect. Restating the whole ledger's precision downward is
cheaper and more honest than defending each entry individually.

**Gate protocol v1 (steering, from run 6) — flake handling, still in force:**
1. Gate runs execute the **full suite**.
2. A failure in the `cloud::rest`/`session` family triggers an isolation rerun
   **plus a same-commit control run**.
3. Only non-cloud failures, or cloud failures reproducible in isolation,
   **block**.
4. The pooled-connection fix (per-test agent, or `Connection: close` in the mock
   tests) is **desktop-scope backlog, not P1** — surfaced, not owned.

**Phase 1.2 + 1.3 desktop gate: GREEN** (run 7, at `bbb1dd2`, single-threaded):
**275 passed / 2 failed**, both failures the documented cloud flakes
(`create_checkout_request_shape`, `create_portal_session_request_shape`), both
passing in isolation immediately after — protocol step 2 satisfied, so per step
3 they do not block. **All 11 tool tests executed and passed on the real Windows
target**, which is what turns the scratch-crate run from evidence into
confirmation. Suite total grew 266 → 277 across the day with zero failures
attributable to new code.

**Phase 1.3 + cover-fix desktop gate: GREEN** (run 8, at `09899fa`,
single-threaded): **277 passed / 0 failed, exit 0** — zero flakes, the cleanest
of the eight runs, requiring no protocol steps at all. Suite total grew
266 → 277 across the day with **zero failures attributable to new code** at any
point.

**Suppressor desktop gate: GREEN** (run 9, at `f7419ea`, single-threaded):
**286 passed / 0 failed, exit 0**, zero flakes. All 9 suppressor tests executed
on the real Windows target, which is what upgrades them from locally-believed to
verified — the scratch-crate run proved the logic, this proved it on the
platform that ships.

**Tool-loop desktop gate: GREEN** (run 10, at `1c1de2d` covering `8d8360f`,
single-threaded): **297 passed / 0 failed, exit 0**, zero flakes — the fourth
consecutive clean run. All 20 new tests (9 suppressor + 11 loop) executed on the
real Windows target.

**Shared-invoke-surface gate: GREEN** (run 11, at `d0b647d`, single-threaded):
**297 passed / 0 failed, exit 0**, fifth consecutive zero-flake run — **and
zero compiler warnings on the desktop build**, which is the result that
mattered. It confirms both that the pre-empted stub warnings were correctly
predicted and that decision D-1 (mobile-only commands on the shared surface) is
clean end to end. First change this phase to touch desktop's invoke surface.

**D-2 convstore gate: GREEN** (run 12, at `22f0aa8`, single-threaded): **302
passed / 0 failed, exit 0**, sixth consecutive zero-flake run. The five
partial-row tests executed on the real target alongside all 297 prior — the
number that matters being the *prior* ones, since a schema change and migration
are only safe if everything that already worked still does.

That run also carried **one dead-code warning**, the phase's only deviation from
the zero-warning standard: `checkpoint_partial` / `finalize_partial` /
`discard_partial` never used. Transient from commit sequencing — but steering
correctly noted they would stay dead on desktop *after* wiring too, since only
`chat_stream`'s `cfg(mobile)` path checkpoints and desktop's transport streams
through the sidecar without ever writing an in-flight row. Resolved with a
narrow `cfg_attr(desktop, allow(dead_code))` and a stated reason, per this
phase's standing rule: resolve on the merits, never blanket-suppress, and never
let the 5.3 `mobile-check` CI job inherit a blind spot. The allow states a true
platform fact rather than hiding an unknown.

**Phase 1.4 gate: GREEN** (run 13, at `a959717`): **301 passed / 1 failed**,
the failure being a *new* member of the documented cloud family
(`cloud::download::tests::public_resume_sends_range_header_and_completes`,
the first download-family flake, same local-listener mechanism), passing in
isolation in 1.74 s — protocol step 2 satisfied, so it does not block.
**Zero compiler warnings.**

*Environmental signal worth more than it looks:* that run took **293 s against
the usual ~91 s** (machine load), and slow runs correlating with flakes is
evidence about the *mechanism* rather than noise about the machine — local
listeners racing for ports lose that race under load. It makes the family
predictable rather than merely catalogued: expect flakes when the run is slow.

#### 🔬 The dead-code warning: an evidence-provenance failure, resolved

The most instructive episode of the phase, because the *evidence itself* was
wrong and it took three rounds to find that out. Recorded in full, since the
outcome alone would teach nothing.

**Round 1 — a wrong prediction.** A dead-code warning for convstore's three
partial-row methods appeared, and was dismissed as transient: "the wiring will
resolve it." Steering correctly rejected that: only `chat_stream`'s
`cfg(mobile)` path checkpoints, so the methods stay dead on desktop *after*
wiring too. A narrow `cfg_attr(desktop, allow(dead_code))` was added on that
reasoning. **The failure: predicting a warning would disappear without checking
what would make it disappear.**

**Round 2 — evidence apparently contradicted the fix.** The `a959717` run, one
commit *before* the allow existed, came back clean; the `22f0aa8` run had
warned. Steering nailed that down properly — first log line `Compiling cleophis
v0.1.0`, and an *unanchored* `warning` grep over the whole log returning nothing
(the earlier check used anchored `^warning`, so "the report filtered it" stayed
live until the unanchored re-check excluded it). On that basis the allow
suppressed nothing and was **deleted**, which was the right call *given that
evidence*. Source was also checked rather than assumed, which **disproved** the
proposed mechanism: every call site (`chat_cmds.rs` 262, 320, 332, 335) sits
inside `#[cfg(mobile)] mod imp` (82–357), so those calls really are cfg-stripped
on desktop. That left an explicit, recorded contradiction between the model and
the measurement.

**Round 3 — the measurements were contaminated.** Commit times: `22f0aa8` at
18:20:54, `a959717` at 18:36:10, `0fa893a` at 19:02:44. The "22f0aa8" suite run
executed inside 18:20→18:36 and the "a959717" run inside 18:36→19:02 — both
while this agent was **actively editing the same live worktree**. The first
compiled a tree with the allows stripped mid-flight (so it warned); the second
compiled a tree with the allow already re-added (so it was clean). **The gate
runs verified HEAD's hash and never the tree's cleanliness, so they compiled
work-in-progress and attributed it to a commit.**

**Resolution: both anomalous data points are invalid. The original model was
correct, and the allow is restored** with its rationale unchanged — desktop has
no caller for these and never will, so the attribute states a true permanent
platform fact, narrowly scoped to three methods.

**Why this is the phase's sharpest lesson.** The previous two lessons were about
checks that couldn't fail — a percentage that fit any number, assertions that
measured a bounding box while the defect sat inside it. This one is worse and
subtler: **the check was capable of failing, ran correctly, and measured a
different artifact than the one it was labelled with.** A run pinned to a commit
hash proves only which commit was *checked out*, not what was *compiled*. Three
domains now — numbers, pixels, and **provenance** — and provenance is the one
where a correct measurement of the wrong thing is indistinguishable from a
correct measurement of the right thing, unless you record what you compiled.

Contributing factor on this side, not only steering's: **editing a shared
worktree while a gate run is in flight.** The freeze protocol below fixes the
collection flaw; not editing during a run is the other half.

Test-count trajectory across the phase, all green: **266 → 277 → 286 → 297 →
302**, with **zero failures attributable to new code at any point in the
phase**.

Logs: `scratchpad/win-test-p1-*.log` (steering side).

#### A note on evidence hygiene: digests

A reported APK digest in one of this milestone's status messages was 65 hex
characters — a duplicated `0` introduced while *retyping* a hash that the tool
output had given correctly. Caught by steering on sight, since a 65-character
sha256 cannot exist. No artifact or claim was affected, but the habit it broke
is worth naming: **never retype a digest, and never record an abbreviated one**
in a ledger. `7616f100…77c4f` looks tidy and is unverifiable; the full 64
characters can be re-hashed by anyone. All digests in this document are now
full-length, and the one above was independently produced by two parties from
the same artifact.

#### Carried into later phases

- `usesCleartextTraffic=true` is injected into the **debug** manifest by Tauri
  (dev-server support). The 5.2 release-config audit must confirm it is absent
  from release, alongside `debuggable=false` and the `abiFilters` check.
- The Tauri CLI warns that an identifier ending in `.app` collides with the
  macOS bundle extension. Harmless for Android and macOS is not a target, but
  it is recorded here because the identifier is otherwise locked.

---

## Phase 1 — Engine swap-in

### ⚠ What a Phase-1 build can and cannot demonstrate on device

Read this before interpreting any founder install of a Phase-1 APK.

**Chat does not work yet, and that is not a defect.** `chat_stream`,
`chat_complete` and `chat_cancel` exist and are registered, the tool loop and
suppressor are complete and tested, and the engine loads — but **nothing invokes
them.** The frontend still posts to `127.0.0.1`, which on Android has no server
behind it and is CSP-blocked besides. The transport swap is **2.1**, and until
it lands, sending a message correctly produces the "That didn't go through — tap
to retry" chip.

So a Phase-1 install can show: the app installs and launches under
`com.cleophis.app`, the library populates with covers, the conversation store
persists chats and messages, sign-up/payment/download work, and the engine
reaches its loaded state. It **cannot** show a model answering anything. An
install that reaches `engine-ready` and still fails to chat is the expected
outcome, not a regression.

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
  `5cd5dad1d4df4f7fe304714a29831e602a3e0b4d51f39f9b6c6bb210c3f5d526`,
  debug-signed, arm64-v8a only, assets still just `tauri.conf.json`. (That
  artifact has since been overwritten by later builds, so unlike the digests
  above it can no longer be re-derived — superseded artifacts take their
  verifiability with them.) The +2.9 MB over 1.1 is the kpack-engine wrapper alone —
  llama.cpp's core was already linked in via the bundled BGE embedder.

Generation (`chat_stream` / `chat_complete` / `chat_cancel`, the calc tool-loop,
the partial-turn flush) is 1.3–1.4 and will be served by this same thread,
opening a session from the handle it already owns.

### 1.3 `Role::Tool`, per-family tool formats, closed registry — DONE (commit 6278410)

Desktop gets tool calling from `llama-server`: it passes an OpenAI-style `tools`
array, the server renders it through `--jinja`, and streamed deltas arrive with
structured `tool_calls`. In-process there is no server and
`llama_chat_apply_template` takes no tools argument, so **both halves are ours**
— the contract is injected into the prompt as a preamble, and calls are
recovered by parsing the model's generated *text* (Qwen3/ChatML
`<tool_call>{…}</tool_call>`; Llama-3.2 bare JSON with a `parameters` key).

`kpack-engine` gains `Role::Tool`, mapped to the `"tool"` role both shipping
families use.

**The registry is closed (security review M2).** `Tool` is an enum, not a
name→handler map, and `dispatch` matches on it: an invented name fails
`Tool::from_name` and returns a tool *error result*, fed back like a calc error
so the model self-corrects. No registration path a prompt can reach, no string
that becomes callable by accident. That matters more here than on desktop — a
tool name from a model is attacker-influenceable whenever the conversation
contains text the user did not write (a pack, a pasted document), and in a Tauri
WebView a dispatch-by-name table is one step from the whole `invoke()` surface.

**Parsing is permissive, dispatch is strict.** Both wire shapes are accepted
regardless of declared family, because a fine-tune may answer in the other and a
missed call is a wrong answer shown to the user — which is safe *because* a
parsed call still has to name a registry tool to run.

**Testability drove the module layout.** `tools.rs` lives under
`engine_inproc/` but is declared in `lib.rs` via `#[path]`, so it compiles on
every platform; it defines its own `ToolFamily` rather than using
`ChatTemplate`, since kpack-engine is Android-only and tying the module to it
would mean the closed-registry property is asserted **nowhere** — the desktop
suite is the only place tests run.

#### Verification — 11/11 passing, run locally

Executed in a scratch crate (the app crate cannot build on this Linux host), so
this is measured rather than deferred: both wire formats, multi-call ordering,
prose/markdown-fence embedding, brace-inside-string, the double-encoded OpenAI
`arguments` shape, malformed arguments, calc errors becoming tool results, and
the closed registry against 7 invented names including case and whitespace
variants. A round-trip test asserts each family's *preamble example* parses back
into a valid call, so prompt and parser cannot silently drift apart.
`kpack-engine` 28/0 and `kpack-calc` 6/0 still green after the `Role` change;
aarch64 `check --all-targets` clean.

#### ⚠ Open design question for 1.4: suppressing tool syntax from the stream

Desktop never had this problem — `llama-server` delivered `tool_calls`
*structurally*, separate from `content`, so the UI only ever saw prose. Parsing
calls out of generated text means the tool syntax is **in the same token stream
the user is watching**, and a naive forward would render
`<tool_call>{"name":"calc"…}</tool_call>` into the chat.

The two obvious answers are both wrong: buffering each round until its calls are
known costs token-by-token streaming on the final answer (the one round the user
is actually waiting on), and forwarding everything leaks syntax. The shape that
fits is a **streaming suppressor** in the mould of the existing
`ThinkStripper` — it already solves the identical problem for ChatML's
start-of-turn `<think>` block, including boundaries falling mid-token. Bare
JSON is the harder half, since it has no opening sentinel; a leading-`{`
heuristic with a bounded lookahead is the likely approach. Flagged now because
it is a genuine design decision rather than transcription, and `calc-loop.js`
offers no guidance — it never faced it.

---

## Conventions

- **A `docs(` prefix can hide a code change.** Commit `6299dc3` is prefixed
  `docs(` but also carries the `DESKTOP_REFUSAL` change; the body says so, but
  a `git log --grep` filtering for code commits would miss it. Noted here so a
  future archaeologist searching this phase knows the prefix is not a reliable
  filter, and as a reminder to split or re-prefix when a "docs" commit acquires
  code.
- **Digests are recorded full-length and never retyped** — see the evidence
  hygiene note under the Phase 0 gate.
- **Bundle and resource claims are verified against the built APK**, never the
  config or source that was meant to produce them.

## Decisions with precedent value

### D-1 — Mobile-only commands live on the *shared* invoke surface

**Decision:** `chat_stream` / `chat_complete` / `chat_cancel` are compiled on
every platform and registered in the single `generate_handler!` list; their
bodies are cfg-gated and the desktop ones refuse immediately.

**Rejected alternative:** a second `generate_handler!` list under `cfg`.

**Rationale.** Two parallel fifty-entry lists are a *silent-drift failure
class* — one gains a command the other doesn't, nothing fails, and a feature is
quietly missing on one platform until someone notices in the field. That is the
same class of bug this phase has repeatedly hunted (the `{}` config no-op, the
inert adaptive icons): a difference that produces no error. Three inert
fail-fast names on the desktop invoke surface are the opposite — auditable,
greppable, and behaviourally void, since they return `Err` before touching any
state.

**Tightenings adopted (steering):**
1. The desktop refusal names the platform and the command family explicitly
   (`DESKTOP_REFUSAL`), so a misrouted invoke is diagnosable from one line of a
   bug report rather than a mystery about which half of the seam fired.
2. **For 2.1:** the transport picks its path once at startup, and a desktop
   build must never invoke these even accidentally. One frontend test should
   pin that — the refusal is a backstop, not the mechanism.

**Evidence:** desktop suite at `d0b647d` — 297 passed / 0 failed **and zero
compiler warnings** on the desktop build, so the pre-empted stub warnings held
and the shared-surface change is verified clean end to end.

That run also covers `dc082bf` (the icon commit): steering verified the
`d0b647d → dc082bf` delta contains **zero Rust/Cargo changes** — Android
resources and scripts only — so there is no desktop surface in it for a new run
to exercise. Worth stating explicitly, because "we didn't re-run the suite" and
"the suite cannot say anything about this change" look identical in a ledger
unless the reason is written down.

### D-2 — Partial-turn rows carry a draft marker, not an in-place update

**Decision (design, lands with the flush):** the partial-turn checkpoint writes
an assistant row with an explicit **partial/draft marker**, cleared on
finalize — rather than `UPDATE`-ing a normal message row in place.

**Rationale.** Kill-restore recovery has to distinguish *"truncated because the
process died"* from *"completed"*, and an in-place update erases exactly that
distinction: a truncated row and a short-but-finished row become
indistinguishable. The §11 kill-restore test needs to assert
**truncated-but-uncorrupted**, which is only assertable if truncation is
recorded as a state rather than inferred from content. Lands as its own commit
with desktop-suite evidence, per the brief's shared-change rule.

---

## Phase 0 founder items surfaced (⚑0.4)

1. Android APK signing-key ceremony (crown-jewel #2; same regime as curator key,
   two offline backups verified restorable). Blocks the first signed APK (Phase 4).
2. Google Play developer account ($25) + H8 developer-verification registration
   (Thailand in the outside-Play verification pilot — our market). Register early.
3. Two more real test devices for the 3-device install/update gate (spec §3: one
   8 GB mid-ranger, one Huawei/Honor no-GMS). A22 is device #1.
