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

**Prefix-KV mechanism gate: GREEN, and the first clean-provenance run in the
track's history** (run 14, at `734df86`, protocol v2 in force): **302 passed / 0
failed / 0 warnings**, seventh consecutive zero-flake run.

| checkpoint | HEAD | `git status --porcelain` |
|---|---|---|
| PRE | `734df86` | empty (clean) |
| POST | `734df86` | empty (clean) |

All four facts agree, so this run is attributable **to the commit** rather than
to "live worktree near `734df86`" — the distinction protocol v2 was written to
create, now exercised for the first time. The log's first compile line is
`Compiling cleophis v0.1.0`, so the app crate was rebuilt in this run rather
than served from cache; that is what makes "0 warnings" a measurement of
`734df86`'s source and not a stale artifact. Steering held the freeze for the
run's duration and this agent wrote its handoff **outside** the worktree to
respect it, which is why the tree was clean at both ends.

Log: `scratchpad/win-test-p1-734df86-frozen.log` (steering side).

**Phase 1 closing gate: GREEN** (run 15, at `b2075f4`, protocol v2): **313 passed
/ 0 failed, zero warnings**, `cleophis` freshly compiled. PRE `b2075f4`/clean →
POST `b2075f4`/clean, all four provenance facts agreeing — the second run in the
track attributable to a commit, and the one that closes the phase.

The count was **predicted before the run**: 302 + 11 serve-loop tests = 313,
and 313 is what came back. That is worth recording for a reason beyond
tidiness. A test count that lands exactly where the change says it should is
evidence that the new tests actually *executed on the target* rather than being
silently cfg'd out — which is the specific way a `#[path]`-declared,
`cfg_attr(desktop, allow(dead_code))` module could have passed a gate while
asserting nothing. An unpredicted 313 would have been the same number with none
of that meaning.

Test-count trajectory across the phase, all green: **266 → 277 → 286 → 297 →
302 → 313**, with **zero failures attributable to new code at any point**.

**2.1 + 2.3 gate: GREEN** (run 16, at `c4fb4c9`, protocol v2): **321 passed / 0
failed, zero warnings**, `cleophis` freshly compiled, PRE `c4fb4c9`/clean →
POST `c4fb4c9`/clean.

**Predicted 313 → 321 before the run; 321 came back — the second consecutive
exact prediction.** Two in a row is the point at which the rule stops being a
nice habit and starts being an instrument: the prediction is a claim about
*which* tests will execute on the target, and a suite that keeps landing on it
is a suite whose new assertions are demonstrably not being cfg'd into silence.
A miss in either direction would have been the interesting result — high means
something unexpected got compiled in, low means something intended did not.

Running total across both phases: **266 → 277 → 286 → 297 → 302 → 313 → 321**.

**2.2 layout + engine-state gate: GREEN** (run 17, at `55673cf`, protocol v2):
**321 passed / 0 failed, zero warnings**, `cleophis` freshly compiled, PRE
`55673cf` → POST `55673cf`, porcelain identical at both ends (one untracked
`docs/ops/android-signing-ceremony.md`, staged by steering, named in advance
and inert — the frontend is embedded from `src/` and docs are not a build
input). **Predicted 321 unchanged, and 321 came back.** Third consecutive exact
prediction, and the first where the informative outcome was a number that
*did not move*: every change in that range is frontend or Android-manifest with
zero Rust touched, so a movement would have meant something compiled that
should not have.

**D-4 + interrupted-download gate: GREEN** (run 18, at `0ede91b`, protocol v2):
control run **322 passed / 0 failed**, single-threaded, 291 s. Predicted
321 → 322 for the one new desktop test, and 322 is what came back.

That gate took two runs, and the reason is worth keeping:

- **Run 1: 321 passed / 1 FAILED, total 322 — and the failing test's name was
  never recorded**, because the capture filter matched only the tally line and
  not the `FAILED`/`panicked` lines. A steering-side legibility flaw, fixed at
  the source (the gate template now captures those lines permanently).
- **Run 2, same commit, same command: 322 / 0.** Per gate protocol v1 step 2, a
  same-commit control is what a suspicious result calls for, and the failure
  moved-or-vanished — the signature of the documented `cloud::rest`/`session`
  family rather than of the branch.

**A prediction was registered before run 2 and was upheld without being
needed.** The new test performs no I/O, binds no port, touches no filesystem
and shares no state — `Engine::new(8080)` stores the integer, it does not bind
it — so it is *incapable* of flaking, and the two outcomes were stated in
advance: the same test failing again would be deterministic and real, while a
failure that moved or vanished would exonerate it. The second branch fired.
Registering the discriminator first is what made a one-run ambiguity resolvable
in one further run instead of an argument.

**Provenance and legibility are separate properties, and this phase has now
been bitten by each independently.** The contaminated runs were a correct
recording of the wrong artifact; run 1 was a correct measurement of the right
artifact whose recorder dropped the part that mattered. A gate needs both, and
fixing one has never fixed the other.

Running total: **266 → 277 → 286 → 297 → 302 → 313 → 321 → 321 → 322**.

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

## Phase 1 — Engine swap-in — ✅ COMPLETE (gate run 15, `b2075f4`, 313/0)

1.1 through 1.5 all landed and gated. The engine runs in-process on Android
behind the same `inference::start/restart/shutdown` surface desktop uses, with
tool calling, streaming suppression, the calc loop, partial-turn recovery, and
prefix-KV session reuse. Desktop behaviour is unchanged throughout: every
divergence is behind `cfg(mobile)`, and `kpack-engine` never enters the desktop
dependency graph at all (`[target.'cfg(target_os = "android")'.dependencies]`).

**What Phase 1 still cannot demonstrate on device** is unchanged and is not a
defect — see the section immediately below. Chat needs 2.1's transport swap.

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

### 1.4 Chat commands + partial-turn flush — DONE (commits `d0b647d`, `22f0aa8`, `a959717`)

`chat_stream` / `chat_complete` / `chat_cancel` on the shared invoke surface
(decision **D-1** below), the calc tool-loop bridged onto a real session
(`4aadb8e`), and the partial-turn checkpoint writing draft-marked rows
(decision **D-2**). Gate: run 13 at `a959717`, 301/1 with the one failure a
documented cloud flake passing in isolation. The dead-code episode that ran
through `0fa893a` → `1850a76` → `4b3feed` is written up in full under the Phase
0 gate section, because its lesson is about *evidence provenance* rather than
about 1.4.

### 1.5 Prefix-KV session reuse — DONE (`734df86` + `f5fb7f6` `24d6c79` `88167e5` `8a5da5a`)

Full design and rationale: `specs/2026-07-25-mobile-p1-handoff-1.5.md` (a
predecessor instance's continuation handoff, committed verbatim). Recorded here
because one finding in it has precedent value well beyond this task.

**The finding: the obvious implementation of 1.5 is silently wrong.** The brief
says "keep `EngineSession` alive per chat", which is necessary and **not
sufficient**. Before `734df86`, `LlamaSession::stream` re-rendered the full
history, decoded from absolute position 0 every call, tracked no `n_past`, and
never cleared the cache. Holding a session open across turns would therefore
have left **stale KV entries from the previous turn sitting past the end of the
new prompt**, where the model attends to tokens that are no longer in the
conversation. That is a wrong answer, not a slow one, and nothing in the system
would have reported it. It was masked only because `engine_inproc` opened a
fresh session per turn, and `EngineHandle::session()` builds a fresh
`LlamaContext` with an empty cache — so the pre-1.5 state is **correct and
slow**, and that pair is what 1.5 must break by halves without breaking the
first half.

**Two invariants now carry the safety, and neither may be removed:**

1. **Trim before extending** — anything in the cache beyond the shared prefix is
   stale and is cleared (`clear_kv_cache_seq`) before the suffix is decoded. Its
   absence *is* the bug above.
2. **Always decode at least one token** — `reuse` is capped at
   `prompt_tokens - 1`, so the sampler reads fresh logits. A fully-reused prefix
   would leave the final logits belonging to the previous turn; also silent.

Only tokens that were actually **decoded** are mirrored in `LlamaSession.cached`.
The token that ends a turn (EOG, cancel, or the max-tokens cap) is sampled but
never fed back, so recording it would desynchronise the mirror from the real
cache. The mirror exists at all because **the KV cache is invisible from Rust**:
without a record of its contents there is no way to know which prefix is valid,
and reusing a cache you cannot describe is how you get corruption instead of
speed.

**Generalisable form of the lesson.** The two earlier phase lessons were about
checks that could not fail and about a measurement labelled with the wrong
artifact. This one is a third kind: **an optimisation whose failure mode is
indistinguishable from success at the point of measurement.** A reused-prefix
turn is fast either way; only the *content* of the answer distinguishes correct
reuse from corruption, so the acceptance criterion has to be coherence, not
latency. Any future caching work in this codebase (prompt caches, adapter
caches, resource caches) inherits the same requirement: describe what is cached,
or do not reuse it.

#### The wiring — DONE (commits `f5fb7f6`, `24d6c79`, `88167e5`, `8a5da5a`)

The obstacle was never the cache; it was the borrow. `EngineHandle::session()`
returns `Box<dyn EngineSession + '_>`, so a session cannot be stored beside the
handle (self-referential) and cannot outlive a scope. What fits is **two loops
on one thread**: an outer loop owning the handle, and an inner loop holding one
session for as long as turns keep arriving for the same chat. `Load` must
`unload()` the handle, which is impossible while a session borrows it, so the
inner loop hands such commands back instead of acting on them.

- `SessionTurns` borrows instead of owning: `&'s mut (dyn EngineSession + 'a)`.
  Both lifetimes are load-bearing — `&mut` is **invariant** in its referent, so
  `'a` cannot be quietly collapsed into `'s`.
- `Command::Chat` carries `chat_key`. `chat_complete` passes `None` on purpose:
  auto-title and analysis are not continuations of the chat they are about.
- The outer loop keeps a `pending` slot, because a yielded command that is
  dropped is either a message the user sent that never gets an answer, or a
  `Shutdown` that hangs `stop_thread`'s join forever.

**Where the routing lives, and why it is not a style question.** The failure
modes here are deadlocks and missing turns, not compile errors, and
`engine_inproc` is `cfg(mobile)` — the desktop suite cannot see it. So the
routing moved to `engine_inproc/serve.rs`, declared in `lib.rs` via `#[path]`
on every platform, exactly as `tools.rs` and `tool_loop.rs` were for the same
reason. `serve_loop` is closure-generic over "run a turn" and "get the next
command"; `engine_inproc` supplies the two effects it cannot have. **Every exit
condition is enumerated in `SessionEnd` and has a test**: chat switch,
transient turn, `Load`, `Shutdown`, closed channel, idle deadline — plus one
asserting every accepted turn was answered.

#### Verification — 11/11 + 35/35, and a clean aarch64 check

| what | result |
|---|---|
| serve-loop routing (scratch crate, `--cfg desktop`) | **11 passed / 0 failed** |
| `cargo test -p kpack-engine` | **35 passed / 0 failed** (was 28) |
| `cargo ndk -t arm64-v8a -P 24 check -p cleophis --all-targets` | **exit 0, zero warnings** |

The warning counts are from an **unanchored** `grep -ci warning` over the whole
log, not `^warning` — the distinction that cost three rounds during 1.4. Logs
in `cleophis-mobile-logs/aarch64-check-1.5-*.log`.

The scratch crate compiles the *real* `serve.rs` with `pub(crate)` widened to
`pub`, against verbatim copies of the three type definitions it names. The
logic under test is byte-identical; the Windows gate is what turns it from
measured into confirmed.

#### Two holes found while writing the comments, not while writing the code

Both were harmless before the wiring and live after it, which is the pattern
worth noticing: **this task converted two latent defects into real ones**, and
neither would have been reported by anything.

1. **The trim was guarded by `if self.cached.len() > reuse`.** That guard trusts
   the mirror to be right about the cache being *empty* — and the mirror exists
   precisely because the cache is invisible from Rust. If the two ever
   disagreed, the guard skipped the trim in exactly the case where the trim was
   load-bearing. Now unconditional; the removal is a no-op when there is
   nothing to remove.
2. **`clear_kv_cache_seq` returns a `bool`, and it was discarded.** llama.cpp
   reports a partial removal it *cannot perform* by returning false (removing a
   whole sequence never fails). A false meant the stale span survived while the
   code proceeded as though it had not. Now a false escalates to clearing the
   whole sequence and re-decoding.

Plus the general form: `EngineSession::stream` is now a thin wrapper that
invalidates the prefix — native cache and mirror together — whenever a turn
returns an error, because there are **nine `?`s** in the turn body and
remembering at each one is not a plan. Before the wiring this cost nothing (the
session was discarded after every turn); it now costs a full re-decode, which
is the correct direction to fail in.

**Both were found by writing down the justification for code that already
existed.** That is the second time in this phase that stating a reason out loud
disproved it — the first being the dead-code warning prediction. It is cheap
enough to be worth doing deliberately rather than accidentally.

#### The one steady-state cost, and its bound

A live `LlamaContext` holds its KV cache — order 60 MB at the floor tier's
2048-token window — and **before 1.5 no context outlived a turn**. Keeping one
alive between turns is the mechanism; keeping it alive forever is a leak, on
the device with the least RAM to spare, where an LMK kill would be attributed
to anything but this. An open session therefore waits at most
`IDLE_SESSION_TIMEOUT` (5 minutes) for the conversation's next turn, then
releases: `Idle`, not `Closed` — the thread stays live and still wants commands,
only the context goes.

The deadline is a **proxy for a signal we do not have yet**. The right one is
the Android lifecycle; release-on-pause should replace it in the §8
backgrounding work (5.1) and demote the timer to a backstop. It is also the one
piece of 1.5 not in the predecessor's handoff design, landed as its own commit
so it reverts cleanly.

#### Known limits, surfaced not fixed

- **No context-window management.** A conversation whose rendered prompt exceeds
  `n_ctx` (2048 on the floor tier) will fail its decode and surface an engine
  error rather than truncating. This is **pre-existing, not caused by 1.5** —
  a fresh session per turn overflowed at the same conversation length — but it
  becomes visible the moment anyone holds a long chat on device, and the
  sidecar hides it on desktop by shifting context itself. Truncate-oldest vs.
  summarise vs. refuse is a product decision, so it is surfaced here rather
  than chosen. Expect it at CP1 with a long conversation.
- **Only *consecutive* same-chat turns benefit.** Switching to another chat and
  back re-prefills, because one handle yields one session at a time. Inherent
  to the borrow, not a tuning parameter.
- **Auto-title evicts the cache once per chat.** It is a transient turn, so it
  closes the session; `app.js:1417` fires it only on the first exchange, so the
  cost is one re-prefill on turn 2 and never again.
- **A tool-using turn reuses less across turns.** Within a turn the loop appends
  the model's *raw* text, so rounds share a near-total prefix; across turns the
  frontend sends back the *displayed* text, which diverges from what was decoded
  wherever the suppressor removed tool syntax. Correct either way — the mirror
  compares token ids, so a divergence costs reuse and never correctness.

**Acceptance still outstanding: coherence on device.** The number 1.5 exists for
is TTFT-with-history, but the criterion that matters is that a **second
same-chat turn is coherent, not merely fast** — that is the check for invariant
1 having survived. Nothing off-device can produce it: the mock backend has no KV
cache, so only CP1 on the A22 can close this.

---

## Phase 2 — Transport swap + mobile UI

### 2.1 `transport.js` seam + the H1 rendering audit — IN PROGRESS

**The seam.** `src/transport.js` describes a chat turn in the terms the UI
already has — `streamTurn(req) → {content, calculations}` and
`complete(req) → string` — and each platform implements it its own way. Desktop
delegates to the existing `streamWithTools` with the same arguments and the
same `fetch` body; the implementations are the previous inline call sites
**moved, not rewritten**, because a seam that also changes behaviour is two
changes wearing one commit. Mobile invokes `chat_stream` / `chat_complete` and
maps the `Channel` events onto the same return shape.

Nothing in the module reads a global — dependencies are injected, the way
`calc-loop.js` already takes `fetchImpl`. That is what makes the *choice*
testable, including the property decision D-1 exists to guarantee.

**The brief says three call sites; there are two.** `sendCompletion`'s streamed
turn and `maybeAutoTitle`'s non-streaming completion are the only places in
`app.js` that talk to the engine — `grep -n "fetch(" src/*.js` returns exactly
one line, and `127.0.0.1` now appears nowhere in `src/` outside `transport.js`
and its test. The brief's "~1648/1785/2006" predates some edits; the third site
does not exist in this revision. Recorded because "we changed 2 of 3 sites" and
"there were only 2" look identical in a diff.

#### 🔒 H1 rendering-hygiene audit — NO VIOLATION FOUND

The security review's H1 finding required that model and tool output reach the
DOM only via `textContent` or a sanitizer, on the grounds that in a Tauri
WebView a DOM XSS is the whole `invoke()` surface. Every path audited:

| path | sink | verdict |
|---|---|---|
| streaming answer (`sendCompletion`) | `bubble.textContent` | safe |
| replayed messages (`appendBubble`, from `rebuildChatDom`) | `el.textContent` | safe |
| citations (`renderCitations`) | `createElement` + `textContent`, `insertAdjacentElement` | safe |
| calculations (`renderCalculations`) | `createElement` + `textContent` | safe |
| **chat titles — model-generated by auto-title** | `chatRowHtml` → `innerHTML`, via `escapeHtml` | safe |

The last row is the one that mattered: it is the only place model output
reaches an `innerHTML` template, and it was already escaped. `escapeHtml`
covers `& < > " '`, so it is correct in attribute position as well as text
position — which matters because the same helper is used for `data-` attributes
elsewhere in that template. Catalog and pack fields interpolated into the other
`innerHTML` templates (library grid, pack lists, move menus) are escaped the
same way.

**No fix was required, and that is the finding** — recorded explicitly because
an audit that changes nothing and an audit that was never run leave identical
diffs. What the audit *did* produce is the invariant worth pinning in 2.2 as
the UI grows: **model output has exactly one escaped path into markup (the chat
title) and every other path is `textContent`.** Any new renderer that breaks
that symmetry is the regression to look for.

**Mobile CSP** already omits `http://127.0.0.1:*` (`tauri.android.conf.json`),
so the desktop transport is not merely unused on Android but forbidden. Note
that `csp` is a *string*, so RFC 7386 replaces it wholesale — the opposite of
the `{}`-merges-silently trap that cost 239 MB in Phase 0. Per this document's
convention, that belongs verified against a built APK, not only against the
config; it is queued with the next build.

#### Known limit surfaced in 2.1

**A Stop pressed in the sub-millisecond window between `invoke('chat_stream')`
being dispatched and the Rust side registering its cancel flag is lost.**
`chat_cancel` looks the request id up in `ChatCancels` and does nothing when it
is absent, and `begin()` runs on the Rust side after the IPC hop. The window is
sub-millisecond and closes before any human could press a button, so this is
recorded rather than fixed: the available fixes (a pre-cancelled set consulted
by `begin`) add a map keyed by frontend-supplied strings, which is a worse
trade against a race a user cannot reach. Revisit if 2.2's Stop button or a
programmatic abort-on-chat-switch ever makes it reachable.

### 2.3 SoC-aware tiering — DONE (commit `f78424d`)

`tier_for_mobile`: **mid (4B) only if RAM ≥ 8 GB AND (i8mm OR a core ≥ 2.75
GHz)**; `mobile_supported`: ≥ 4 GB, else `HardwareInfo.supported = false`.

The rule exists because of a measurement, not a preference. P0 ran a 4B stack on
the A22's Dimensity 700 at **1.0–1.8 tok/s** — it loads, and nobody would use
it. That SoC pairs workhorse-class RAM with floor-class CPU, so a RAM-only rule
would have shipped a technically-working, practically-unusable model to the
reference device. `i8mm` is the proxy for "a generation above the floor":
present on A78/X1-class cores, absent on the A55-class cores that define it.

Detection reads `AT_HWCAP`/`AT_HWCAP2` out of `/proc/self/auxv` rather than
calling `getauxval` — no new dependency, and the parse becomes a pure function.
**Every failure path degrades to "no capability" and therefore to the floor
tier**, which is the chosen direction: guessing low costs a smaller model,
guessing high costs a device that swaps or is OOM-killed. A truncated auxv read
loses a capability rather than inventing one.

**Two things checked rather than assumed.** `HardwareInfo` gained a field and
`HardwareInfo` is sent to the device-registration API — but `upsert_device`
builds its body field-by-field with an explicit `json!`, so the outbound request
is byte-identical and only `detect_hardware`'s return to the frontend changed,
additively. And **rounding is load-bearing at the support floor**: `ram_gb` is
rounded `MemTotal`, and a nominally-4 GB phone reports ~3.6–3.7 GiB. Rounding
keeps it at 4 and supported; flooring would put it at 3 and lock out the A22
along with every other 4 GB Android phone. A test exists whose only job is to
fail if that changes.

The aarch64 check earned its keep: `tier_for` became dead on Android once
`detect()` stopped calling it there. Resolved on the merits with a narrow
`cfg_attr(target_os = "android", allow(dead_code))` — the mirror image of
convstore's partial-row methods.

Evidence: **8/8** mobile-tier tests (scratch crate over the real module,
extracted byte-for-byte); aarch64 `check --all-targets` exit 0, zero warnings
unanchored. **Expect the Windows suite at 313 → 321.**

Not done here: the "not yet" onboarding screen `supported: false` should drive.
That is UI, and belongs with 2.2.

### 📱 CP1 APK — BUILT and verified against the artifact (at `f56e825`)

```
sha256  7b068bf307a9549fe3fff4c27c7aef7407c01b9ba0aa94f7250e1012c7bc58c0
size    355,280,098 bytes
built   from f56e825 (2.1 + 2.3 + the [kernels] line), debug-signed, arm64-v8a
copy    ~/cleophis-artifacts/cleophis-cp1-debug-f56e825.apk
log     ~/cleophis-mobile-logs/apk-debug-20260726-025529.log
```

Digest produced by `sha256sum` and never retyped; re-hashed at the artifacts
copy and identical, so the copy is intact too. **This APK supersedes every
earlier one** — it is the first that can chat.

Verified **against the built APK**, per this document's convention, not against
the config that was meant to produce it:

| check | result |
|---|---|
| zip-entry accounting (the zipflinger orphan trap) | 968 entries, Σ compressed **355,090,078** vs file **355,280,098** — **0.05 % unaccounted**, i.e. no stranded copy |
| `assets/` contents (the `{}`-vs-`null` resource trap) | **`assets/tauri.conf.json` only** |
| `lib/` ABIs | **`arm64-v8a` only** |
| package identity | **`com.cleophis.app`**, versionCode 1000, versionName 0.1.0 |
| `allowBackup` | **false** (spec-critical, 0.3 batch) |
| permissions | INTERNET, REQUEST_INSTALL_PACKAGES, POST_NOTIFICATIONS, FOREGROUND_SERVICE, FOREGROUND_SERVICE_SPECIAL_USE |

The size check is the one worth reading twice. **A stranded orphan is invisible
in the file length** — Phase 0's 334 MiB APK became 658 MiB with half of it an
unreferenced `libcleophis_lib.so` that no central-directory entry pointed at,
and it installed and ran perfectly. Only the entry sum can tell the difference,
so that is what is recorded: the payload is one 345,655,072-byte
`libcleophis_lib.so`, and everything else is 9 MB of dex, resources and
`libc++_shared.so`.

`debuggable=true` and `usesCleartextTraffic=true` are present and correct for a
debug build; both are already on the carried-into-later-phases list for the 5.2
release-config audit to confirm absent from release.

### 📱 CP1 — the founder session this APK is for

**Sequenced deliberately before 2.2.** The 2.2 field requirements were derived
from screenshots of an app that *could not chat*; an hour of someone actually
chatting will produce better ones. Steering accepted the argument, and this is
the record of it.

**The one result that cannot be obtained any other way:**

> **Send a message. Read the answer. Then send a SECOND message in the SAME
> chat, and check that the answer addresses what was actually asked.**

That is the acceptance criterion for task 1.5, and nothing off-device can
produce it. Turn 2 is the first turn that reuses a KV prefix. If invariant 1
(trim-before-extend) were broken, turn 2 would come back **fluent, confident,
and about the wrong thing** — the model attending to tokens from turn 1 that are
no longer in the prompt. **A fast wrong answer is the failure mode, so speed is
not the check.** The mock backend has no KV cache, so this is the one thing the
whole test suite cannot say anything about.

Then, in rough priority order:

1. **`[kernels]` in logcat must show `DOTPROD = 1`** (new in `f56e825`). Per the
   brief, a build without it invalidates every timing number below. This is the
   first APK that prints its own proof.
2. **TTFT on turn 2 vs turn 1.** The number 1.5 exists for. What matters is the
   *shape* — first-token latency should stop growing with conversation length,
   where pre-1.5 it reached 25–40 s mid-chat.
3. tok/s and peak RSS; engine reaches `engine-ready` at low tier.
4. Tampered model file → integrity failure (fail-closed gate).
5. Stage-5 probes 4/4.

**Expected and NOT defects, so they do not get reported as regressions:**

- **The UI will be rough.** Sidebar takes ~2/3 of the width, the view transition
  misbehaves, the status pill clips. That is exactly the 2.2 backlog, and this
  session is partly to sharpen it — notes on what was most *in the way* are the
  most valuable thing to bring back after the coherence result.
- **A long conversation will fail with an engine error** rather than truncating,
  once the prompt exceeds `n_ctx` (2048 at the floor tier). Surfaced above as a
  product decision, not a bug.
- **A force-stop logs the user out** — keyring is a silent in-memory mock until
  3.1.
- Timing figures are **indicative, not definitive**: this is a debug APK. That
  matters less than it sounds (see the note below), but the definitive numbers
  still come from release harness binaries.

#### Why a debug APK can still produce usable numbers

Worth measuring rather than assuming, and it was: llama.cpp is built through the
`cmake` crate with **`CMAKE_BUILD_TYPE=Release` and `-O3 -DNDEBUG` even inside a
cargo *debug* build** (confirmed in the generated `ggml-cpu` `flags.make`). The
kernels where essentially all inference time goes are fully optimized; only the
Rust glue is unoptimized, and its per-token work is negligible beside a forward
pass. A release APK is not an option anyway — signing is founder-serialized and
the ceremony has not happened.

**A provenance caution for whoever checks the dotprod flag next.** The target
directory holds five `llama-cpp-sys-2` build directories and **the oldest
predates the vendored patch**, carrying plain `-march=armv8-a`. A
`find … | head -1` samples exactly that one and looks precisely like "the patch
is not applied" — it was, briefly, mistaken for that here. The current
directories carry both `-march=armv8-a` *and* a later
`-march=armv8.2-a+dotprod` on the same command line, where the later flag wins.
Same trap as the contaminated gate runs: a correct measurement of the wrong
artifact. The runtime `[kernels]` line now settles it without archaeology.

### 2.2 Mobile UI — layout + engine state DONE (commit `95d1e98`)

Design note and the reason it was left to a fresh instance:
`specs/2026-07-26-mobile-p1-handoff-2.2.md`.

#### 🔬 The ONE-BUG hypothesis: right conclusion, wrong mechanism

The handoff predicted that `.views` grows wider than `2 × 100vw` when the chat
pane's contents overflow, so `translateX(-50%)` overshoots and the pane lands
short — making the "slides ¼ and stops" transition a *consequence* of the
2/3-width sidebar rather than a second bug.

**It is one bug. It is not that one.** Measured in a real browser at 360×800,
against the real `src/` tree served over HTTP with a Tauri stub:

| fact | measured |
|---|---|
| `.views` width | **exactly 720px** = 2 × 360 |
| `.views` transform with `in-chat` | **exactly `-360px`** |
| `#chatView` landing position | **exactly x=0**, right=360 |

And the falsification run — the kind this ledger keeps demanding, one that
*could* have come back positive: forcing `#chatSidebar` to **900px** inside the
360px pane (a 2.5× overflow, far worse than the real 260px) left `.views` at
**720px** and `#chatView` at **x=0**. Neither number moved.

Why it cannot move: `.views` has a *definite* percentage width, so overflowing
content never grows it; `.view` children are `flex:none;width:50%`, so they
neither shrink nor stretch; and `.chatview{overflow:hidden}` clips rather than
propagates. The transform model is arithmetically correct at every viewport
width, so **it was left alone** — and steering's "disable or replace the
transition" ruling was made on the belief that it was broken, which it is not.

**The actual root cause is one declaration:** `#chatSidebar{width:260px;
flex:none}`, which took **72.2%** of a 360px viewport and left `.chatmain`
**100px (27.8%)**. Every field report is downstream of it:

| field report | measured cause |
|---|---|
| "sidebar takes ~2/3" | 72.2% |
| "slides ~¼ and stops" | the transition **completes**; the chat column is 27.8% of the screen, so the correct destination is visually indistinguishable from a stalled slide |
| "status pill clipped" | pill right edge at **458.8px** in a 360px viewport — 98.8px past it |
| — | `#attachPacksBtn` **192.1px** past, `#costCounter` **292.2px** past |
| — | **`#sendBtn` 230.6px past the edge — Send was not on screen in portrait at all** |

`.chatbar` needed 392px in a 100px box and is `overflow:visible`, so the spill
was clipped by `.chatview{overflow:hidden}` and could not even be scrolled to.
That last row is new and reconciles CP1: the founder chatted successfully in
airplane mode, which means they used landscape or the keyboard's Go key.

**Second, independent defect found the same way:** `#catalogView` carried
**86px of real horizontal overflow** at 360px (`.bar` needs 446px) — on the
first screen a new user sees.

**The generalisable form:** the founder's *observation* was accurate and the
*mechanism they inferred* was not, and the two are easy to conflate because the
observation is vivid. Acting on the inference would have rewritten a correct
view model and left the screen looking identical, because the sidebar would
still have been 260px. **A field report is evidence about what a person saw,
never about why.**

#### The engine state had no element to fix

`#chatStatusPill` appeared **exactly once in the entire codebase** — in
`index.html`, as the hard-coded string `"Local · offline"`, permanently teal.
Nothing read it and nothing wrote it. So CP0's "🔴 most significant UX finding"
resolves more simply than it was written up: the founder could not identify the
engine-state element **because there was no engine-state element**. The only
thing that ever reported engine state was `#engineBanner`, which fires on
`engine-restarting` and `engine-failed` and nothing else — so the ordinary
`Starting → Ready` path rendered *nothing at all*. The ledger's line that "the
app spends those moments silently" was literally true: no code existed that
could have spent them otherwise.

It also explains the copy defect recorded under the download retraction. A
permanently green "Local · offline" beside a model that is not on disk reads as
"running fine, locally" — which is how someone concludes a download happened
when none did. 2.2 therefore **built** engine state rather than restyling it.

**The causal chain, in order**, because each link was recorded separately and
read at the time as a different problem:

1. **The pill was hard-coded.** One occurrence, in `index.html`, never read or
   written.
2. **So the founder's CP0 report was 100 % accurate.** "Could not identify the
   engine-state element" was not a complaint about a small or badly-placed
   affordance — there was no such element, and taking "the pill" to mean the
   download button was the *correct* reading of what was on screen.
3. **So the product note was literal.** "Every mechanism that would make the
   claim tangible either flashes past unreadably or sits in a corner the user
   never looks at — the app spends those moments silently" describes a codebase
   in which no code existed that could have spent them otherwise.
4. **So 2.2 built the presentation layer rather than restyling it.** There was
   nothing to restyle. That is why this item's shape differs from every other
   entry on the 2.2 list, which were all genuine fixes to things that existed.

**Named invariant — the dwell governs DISPLAY, never CAPABILITY.** The 700 ms
floor paces the narration so a state can be read; it never delays the composer,
which enables the moment the engine is genuinely ready. Pacing the story the
product tells about itself is a product choice. Pacing the product would be a
regression wearing the same clothes, and the two are one careless line apart.

`src/engine-state.js`, pure and tested (**decision D-3** — logic whose failure
mode is silent goes where the tests run; a status line that says the wrong
thing throws nothing and looks exactly like one that says the right thing):

- `describeEngineState` maps `EngineStatus` + `download-progress` phase +
  install state + turn timing onto one presentation. Every branch derives from
  something the backend actually reports; nothing guesses.
- `createReadableSequence` gives each state a **700 ms floor** so transitions
  can be read — the milestone's own "fast is the wrong optimization for the one
  transition worth showing". Three rules, each because the obvious version is
  wrong: same-kind updates bypass the dwell (a download percentage must not
  freeze), error states preempt the queue (readability must not outrank
  honesty), and a backlog that would put the display more than 2.5 s behind
  reality collapses to the newest.
- **The dwell governs display only, never capability** — the composer enables
  the moment the engine is genuinely ready. Pacing the narration is a product
  choice; pacing the product would not be.
- The slow first turn is explained from a **real signal** — a request in flight
  with no delta *is* prefill — rather than from a guess about the engine.

#### Verification — measured, then looked at

`npm test`: **14 → 43, zero failures.** (Prediction was 24 new; 29 landed. The
miss was a miscount of my own file when predicting, not tests failing to
execute — 14 + 29 reconciles exactly against the file, which is the property
the convention exists to check.)

Layout facts, before → after at 360×800:

| fact | before | after |
|---|---|---|
| chat column | 100px (27.8%) | **360px (100%)** |
| `.chatbar` overflow | 292px | **0** |
| `.composer` overflow | 231px | **0** |
| `#sendBtn` vs right edge | **+230.6px (off-screen)** | −12px (inside) |
| `#chatStatusPill` vs right edge | +98.8px | −173px (inside) |
| `#catalogView` horizontal overflow | 86px | **0** |

**Desktop byte-identity, checked rather than asserted.** All mobile CSS is
scoped under `.is-mobile`, set from the same `isAndroid(navigator.userAgent)`
predicate that chooses the transport — deliberately *not* a media query, which
would also have changed narrow desktop windows. Verified in-browser at **both
1280px and 360px** with a desktop UA: static 260px sidebar, `display:none` on
all five new elements, `--app-h` unset, and the pill still reading
`"Local · offline"` **after `engine-ready` and `download-progress` events were
pushed through the machinery** — so the `if (!IS_MOBILE) return` guard
demonstrably *stops* it rather than merely existing. The 360px desktop run is
the one that matters: it is the case a media query would have silently broken.

#### 🔬 Numbers right, pixels wrong — the third instance, twice in one session

Both defects below passed their assertions and were caught by **looking at a
screenshot**. The ledger already records this failure mode for the adaptive
icon (bounding box correct, contents wrong); it recurred here immediately.

1. **The pill rendered `"Ready — r"`.** The assertion checked that the pill's
   `getBoundingClientRect()` was inside the viewport, and it *was* — the text
   inside the box was still cut by `text-overflow:ellipsis`. Same shape as the
   icon: the box was right, the pixels in it were not. Fixed by giving every
   presentation a separate **`short`** field, because the row has the width of
   the screen and the pill has ~170px beside a model name; one string cannot
   serve both. A test now caps `short` at 16 characters, since the real
   constraint is character count and CSS cannot enforce it.
2. **`.grow` is `flex:1` and so is `.chattitle`**, so the spacer *split* the
   free width with the title and the model name got 81px of the 168px
   available, ellipsizing to `"Socratic…"`. On desktop `.grow`'s job is to push
   `#costCounter` right; mobile hides the counter, leaving it nothing to push.

A third, of the same family: `pill.hidden = true` left the pill on screen,
because `.pill{display:inline-flex}` is an author rule and beats the UA
stylesheet's `[hidden]{display:none}`. The code's comment said "exactly one
engine-state element is visible at a time" and that was simply false until a
screenshot showed both. Now asserted in-browser across all three states.

#### Also landed

- Drawer + scrim + hamburger, `aria-expanded` maintained; picking a chat closes
  it. The rail costs the chat nothing when closed (measured off-canvas at
  x=−306).
- The honest **"not yet" screen** for 2.3's `supported:false`, shown at boot
  and **before** the account/payment funnel rather than at sign-in the way
  desktop does — a phone that cannot run a model should learn that before it is
  asked to pay. Not a hard block: the library and existing chats still work,
  which is the truth and the whole truth.
- **IME**: `windowSoftInputMode="adjustResize"` *and* a `visualViewport`
  listener driving `--app-h`. Belt and braces on purpose — `100vh` is the
  *initial* viewport and never shrinks for the keyboard (which would leave the
  composer below the fold), and an edge-to-edge activity on newer Android may
  ignore `adjustResize` entirely, while `visualViewport` reports the truth
  either way.
- Safe-area insets on every edge that touches one; `font-size:16px` on the
  composer input to stop WebView zoom-on-focus; font scaling respected (no px
  heights on text containers, and the two clippable spots ellipsize).
- **H1 invariant preserved**: every new render site writes via `textContent`.
  The one escaped path into markup is still the chat title.

#### 📱 2.2b APK — the superseding build (D-4 + interrupted downloads)

```
sha256  7c22d7c72d20613a8b76e38b4e9b565a307cc8fe6f43c58ea494bf9dd3c69571
size    355,315,918 bytes
built   from 53780ca, debug-signed, arm64-v8a
copy    ~/cleophis-artifacts/cleophis-2.2b-debug-53780ca.apk
log     ~/cleophis-mobile-logs/apk-debug-20260726-094411.log
```

**Supersedes the `402e5e6` APK below.** Adds D-4's real context window (a long
chat on the A22 now trims instead of failing its decode) and interrupted-
download visibility with a Resume control in the chat view. Everything the 2.2
visual-acceptance session judges — drawer, on-screen Send, the engine states —
is unchanged from `402e5e6`, so acceptance findings on those carry over.

| checkpoint | HEAD | `git status --porcelain` |
|---|---|---|
| PRE (compile start, 09:44:11) | `53780ca`, reflog-attested | **agent-attested clean; not durably recorded** |
| POST (steering's verification) | `53780ca` | clean apart from this entry |

**Attributed as "live worktree at `53780ca`", not "at `53780ca`."** Protocol v2
wants four facts and this build has three and a half.

The PRE checkpoint **was taken** — gen-4 ran it in its own command immediately
before launching the build, deliberately separated from the launch after a
trailing `&` had swallowed an earlier `&&` chain and truncated exactly this
kind of record. It read `53780ca` and an empty porcelain. But it survives only
in a session transcript, and **an agent's report that it checked is precisely
the class of assurance protocol v2 was written to stop accepting.** So it is
recorded here as an attestation and ranked *below* the reflog, and the
attribution stays weak.

That distinction is worth more than the checkpoint would have been. The
interesting case is not "nobody looked" — it is **"somebody looked, and the
looking left no trace anyone else can audit."** A gate discipline that only
catches the first kind is half a discipline: it is satisfied by an agent
sincerely remembering, which is how the contaminated runs passed review in the
first place. The fix is not to trust the transcript more; it is to make the
check write something down where a stranger can find it.

Two things *are* attestable by durable records rather than by an agent's
memory:

- **HEAD did not move across the build.** The reflog puts `53780ca` at 09:38:47
  with no entry after it; the build log opens at 09:44:11 and the artifact
  lands at 10:21. That is better evidence than a remembered checkpoint and
  still not the same fact — the reflog watches HEAD, never the tree.
- **The tree is clean at `53780ca` now**, its only deviation being this ledger
  entry, which is docs.

So the compiled content is `0ede91b`'s gated tree unless something was edited
and reverted without trace inside a 37-minute window, while the *attribution*
is a live worktree. Recording the weaker of the two available claims is the
whole point: this document already softened every pre-v2 run for exactly this
reason, and a new entry that quietly awarded itself the stronger form would
spend that credibility rather than add to it.

**A precision worth keeping rather than rounding off.** This is loosely "the
`0ede91b` APK", since `0ede91b` is the commit the 322/0 gate blessed, but it
was built from `53780ca`. `git diff --stat 0ede91b..53780ca` is **one file,
+67 lines, the ledger markdown** — measured, not remembered. Docs are not a
build input and the frontend is embedded from `src/` into the binary, so the
artifact carries exactly `0ede91b`'s gated code while its *attribution* belongs
to `53780ca`. Both statements are true, and neither substitutes for the other:
saying only the first overstates what was checked out, saying only the second
hides that the compiled content is the gated tree.

`verify-apk.py` PASS: 968 entries, Σ compressed **355,125,898** vs file
**355,315,918** = **0.05 % unaccounted**; `assets/` is `tauri.conf.json` only;
`arm64-v8a` only; `com.cleophis.app`; `allowBackup=false`;
`windowSoftInputMode=0x10` in the packaged manifest.

**Verified from the artifact twice, by two parties** — by steering when it
blessed the build, and independently by gen-5 against the archived copy, both
using the committed `verify-apk.py`. Identical digest, entry count and
unaccounted fraction. The second run is not ceremony: steering verified the
*build output*, and what the founder installs is the **archive**, so re-hashing
`~/cleophis-artifacts/cleophis-2.2b-debug-53780ca.apk` is what closes the gap
between the thing that was checked and the thing that was shipped.

Unusually, the build output at
`gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk`
**also still exists** — gen-4 stalled before starting another build, so nothing
overwrote it, and it hashes identical to the archive. Every previous entry in
this document had to note that its build output was gone; this is the one time
both copies can be compared, and they agree.

*(A claim in this paragraph's first draft — that the output had been overwritten
"by every later build" — was written from the pattern of previous entries rather
than from `ls`, and was false. Corrected before commit. It is a small instance
of the thing this document keeps recording: the plausible generalisation and the
checked fact diverge exactly where nobody looks.)*

**What it is for:** the founder holds it as `cleophis-2.2b.apk` for the 2.2
visual-acceptance session. Results arrive via steering.

#### 🔬 The third way good work becomes worthless: delivery

This APK was built, correct, and sat **unreported for a day.** The build
finished with `tauri exit status: 0`, the tree never moved, and the artifact
was on disk the whole time — but the agent's verify-and-report step never ran,
so nothing downstream knew it existed. Steering noticed only because no digest
ever arrived, and opened a status check on the assumption of a silent death.

That completes a trio this phase has now suffered in three distinct forms:

| failure | shape |
|---|---|
| **provenance** | a correct recording of the *wrong* artifact (gate runs labelled with a commit while compiling a live worktree) |
| **legibility** | a correct measurement of the *right* artifact whose recorder dropped the part that mattered (gate run 1's lost test name) |
| **delivery** | a correct, correctly-measured artifact that **nobody was told about** |

**An artifact nobody is told about is indistinguishable from one that was never
built** — and from the outside it looks exactly like an agent that died. The
countermeasure is the same shape as the other two: **the report is part of the
deliverable, not a follow-up to it.** A build step that ends without emitting
its digest has not finished, however green its exit status.

Mechanically: background waiters were armed on the build and they fired, but
the turn that should have collected them never resumed. Arming a watcher is not
the same as having reported — the watcher only creates the *opportunity* to
report.

#### 📱 2.2 APK — BUILT, verified against the artifact, clean provenance

```
sha256  13a26f8a054b5462606b058554c9a040e495fb9f2d0a1070d6966b76a9d4f870
size    355,300,838 bytes
built   from 402e5e6 (2.2 layout + engine state), debug-signed, arm64-v8a
copy    ~/cleophis-artifacts/cleophis-2.2-debug-402e5e6.apk
log     ~/cleophis-mobile-logs/apk-debug-20260726-062018.log
```

**Provenance — the first APK in this track built under protocol v2.**

| checkpoint | HEAD | `git status --porcelain` |
|---|---|---|
| PRE | `402e5e6` | `?? docs/ops/android-signing-ceremony.md` |
| POST | `402e5e6` | `?? docs/ops/android-signing-ceremony.md` |

All four facts agree. The single untracked file is another agent's
work-in-progress under `docs/`, and it **cannot** enter the artifact: the
frontend is embedded from `src/` into `libcleophis_lib.so` at Rust compile
time (`assets/` holds only `tauri.conf.json`), and docs are not a build input.
Recorded rather than hidden — "clean except X, and here is why X is inert" is
worth more than an unqualified claim of pristine.

**Why this APK is a rebuild.** The first build of this session was started
before the last commit landed and was therefore attributable to no commit at
all — the frontend snapshot is taken when `cleophis` compiles, so a later
`src/` edit silently produces an artifact whose contents match nothing. That is
the same failure as the contaminated gate runs, in a new place. It was
discarded and rebuilt from a frozen tree rather than shipped with a caveat.

Digest produced by `sha256sum`, never retyped, and re-hashed at the artifacts
copy: **identical**, so the copy is intact.

Verified from the APK itself by the new
`docs/superpowers/mobile-tools/verify-apk.py`, which encodes the checks this
document has been performing by hand:

| check | result |
|---|---|
| zip-entry accounting (zipflinger orphan trap) | 968 entries, Σ compressed **355,110,818** vs file **355,300,838** — **0.05 % unaccounted**, no stranded copy |
| `assets/` contents (`{}`-vs-`null` merge trap) | **`assets/tauri.conf.json` only** |
| `lib/` ABIs | **`arm64-v8a` only** |
| package identity | **`com.cleophis.app`**, versionCode 1000, targetSdk 36 |
| `allowBackup` | **false** |
| permissions | REQUEST_INSTALL_PACKAGES, FOREGROUND_SERVICE_SPECIAL_USE |
| **`windowSoftInputMode` (new in 2.2)** | **present, `0x00000010` = `adjustResize`** |

That last row is the one worth reading twice. The IME fix was a change to
`gen/android/AndroidManifest.xml`, and this repo's standing rule is that config
claims are checked against the built artifact and never against the config that
was meant to produce them — so it is confirmed by dumping the *packaged*
manifest, where the attribute resolves to the numeric constant rather than the
source text.

**What this APK is for:** the 2.2 visual-acceptance session. It is also the
first build carrying the CP1 items that were still open — `[kernels]`
DOTPROD=1, quantified TTFT/tok-s/RSS, the tampered-file probe and a formal
Stage-5 4/4 all remain uncaptured and fold naturally into the same device
session.

#### Surfaced, not fixed

- **Desktop's pill has the same defect** — static `"Local · offline"`, no
  engine state. Fixing it is a desktop *behaviour* change, which this task's
  hard constraint forbids. Desktop-scope backlog, owned by nobody yet.
- **The Stop-button race is unchanged and not newly reachable.**
  `openChat`/`newChat` have *always* called `state.chat.aborter?.abort()`, so
  the programmatic abort the handoff worried 2.2 would introduce already
  existed. Reaching the race still needs a chat switch within the
  sub-millisecond window between `invoke('chat_stream')` and Rust's `begin()`,
  and the drawer adds a tap, not a shortcut.
- **BRIDGE-FIRST (ratified by steering) — the JNI bridge is built and
  smoke-tested as its own commit, with its own device checkpoint, before any
  feature rides on it.** The sequencing changed because of a read-only audit
  done during a gate freeze, and the facts are worth recording since they are
  cheap to re-derive and easy to assume away:
  - **The bridge has zero prior art in this repo.** `jni = "0.21"` is declared
    under the Android target deps and **nothing** in `src-tauri/src/`
    references `jni::`, `JNIEnv`, or `ndk_context`.
  - **`ndk_context` is not declared at all** — and it is what yields the
    `JavaVM`/`Context` from the Android runtime. So the first native task
    starts one dependency short of what the plan assumes.
  - `cloud/secure_store.rs` does not exist (3.1 unstarted), and **`keyring` is
    still ungated** — unconditional with `windows-native`/`linux-native`,
    neither of which applies on Android, so v3's silent in-memory mock is still
    live and **a force-stop still logs the user out on device.**

  Three separate Kotlin shim surfaces (`ConnectivityManager`, `ACTION_SEND`,
  and SAF/content-URIs if the founder's pack-upload feature lands) all sit
  behind that one unbuilt bridge. Discovering its failure modes *inside*
  whichever feature happens to go first is compound debugging — two unknowns,
  one symptom — which is the specific trap this track keeps paying for
  (a gradle launcher bug that surfaced as a Rust task failure being the
  clearest prior example).

- **Download-manager network policy and share-sheet export are NOT done.**
  Both need native work — `ConnectivityManager` via the Kotlin shim (with the
  JNI bridge smoke-tested first, per the brief) and an `ACTION_SEND` shim. The
  frontend half is deliberately not written ahead of them: a policy layer
  invoking a command that does not exist would be a seam with nothing behind
  it, and `navigator.connection` reports wifi-vs-cellular, which is **not** the
  same fact as metered.

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
- **Warning counts come from an unanchored `grep -i warning`**, never
  `^warning`. An anchored grep kept "the report filtered it" alive for a whole
  round during the 1.4 dead-code episode.
- **Predict the test count before a gate run, and record the prediction.** A
  suite that lands exactly where the change says it should is evidence the new
  tests *executed on the target*; the same number arriving unpredicted proves
  nothing. This is how a cfg-stripped test module gets caught passing a gate
  while asserting nothing (Phase 1 closing gate: 302 + 11 = 313, predicted).

### 🔬 Write the justification for code you are about to build on

**Both defects fixed in `24d6c79` were found by writing the comment explaining
why the existing code was correct — and discovering it was not.** Neither was
found by testing, reading for bugs, or the aarch64 check, all of which were run.

That is the **second** time in this phase. The first was the dead-code warning:
"the wiring will resolve it" collapsed the moment steering asked what,
specifically, would make it disappear. Twice is a pattern worth making
deliberate rather than lucky, so it is a convention now: **before extending a
piece of code, write down why the existing version is correct.** Not what it
does — why it is *safe*. The two are easy to confuse and only one of them
finds anything.

Why it works here specifically: this codebase's characteristic failure is the
silent one — a config `{}` that merges instead of clearing, a gate run labelled
with the wrong commit, a cache reused without being described. None of those
announce themselves, so the only cheap detector is a claim stated plainly
enough to be checked against the code sitting in front of you. Both `24d6c79`
findings were one sentence away from invisible:

- "trim only when the mirror says there is something there" — *the mirror is
  the thing that might be wrong.*
- "clear the stale span" — *the call returns a bool saying whether it did, and
  we throw it away.*

Cost: minutes. It is the cheapest verification technique in this document, and
the only one that needs neither Windows nor a device.

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

### D-3 (RATIFIED by steering) — Logic whose failure mode is silent goes where the tests run, even at the cost of a seam

**Decision:** when a piece of logic is (a) platform-neutral and (b) fails in a
way that produces no error, it is extracted to a module compiled on every
platform, and only the genuinely platform-bound remainder stays behind the
`cfg`. Applied twice in 1.5: the serve loop's routing (`engine_inproc/serve.rs`,
`#[path]`-declared in `lib.rs`) and the prefix arithmetic
(`kpack-engine/src/prefix.rs`, outside the `real` feature gate).

**Rejected alternative:** leave both where they naturally belong — inside the
`cfg(mobile)` module and inside the `real`-gated backend — and verify by
inspection plus a cross-compile `check`.

**Rationale.** A cross-compile `check` proves a thing compiles, not that it is
right, and on this branch **nothing runs on Android until a founder device
checkpoint**. So "behind the cfg" is the same as "untested" for as long as it
takes to get to a device. That is tolerable for FFI, where the alternative is
mocking llama.cpp; it is not tolerable for a loop whose failure modes are a
dropped turn and a hung join, or for an arithmetic cap whose off-by-one yields
a fluent answer to the wrong question. The cost is one extra module and a
closure-generic signature. The benefit is 18 executing tests instead of an
argument in a commit message: the **11** serve-loop tests join the Windows
suite (302 → 313 expected), and the **7** prefix tests run in `kpack-engine`'s
own suite, which needs no Windows and no device.

**The tell that this is the right split:** everything left behind the `cfg` in
`serve_one_session` is *borrow plumbing* — open a session, hand it to a
closure, drop it. There is no decision left in it to get wrong.

**Precedent it extends:** `tools.rs` and `tool_loop.rs` were placed this way in
1.3–1.4 for the same reason. D-3 states the rule those two were following.

### D-4 (FOUNDER) — Truncate-oldest for the model's window; storage is never truncated

**Decision:** when the rendered prompt would exceed `n_ctx`, the oldest turns
are dropped from the *request*. The on-screen transcript and the convstore
rows are **never** truncated. No user-facing notice by default (the founder
chose plain truncation over the notice variant).

The storage half is already true by architecture, and it is stated here as a
decision precisely so that nobody later "optimises" storage to match the
window. It is also load-bearing for a P3 feature — see the long-term-memory
note under future work — because a transcript that was trimmed to fit a phone's
context can never be re-ingested for retrieval later. **The full transcript is
the enabler; protect it.**

**What implementing it actually required, which was not what the ledger
predicted.** This document previously recorded "**No context-window
management** — a prompt exceeding `n_ctx` fails its decode rather than
truncating." That diagnosis was wrong. Management existed the whole time:

- `windowMessages` (`src/app.js`) already returned the most-recent suffix that
  fits a budget, already reported `droppedCount`, and is pure and testable;
- `updateContextDivider()` already drew the boundary in the transcript;
- nothing ever truncated the DB.

What was wrong was one number:

| where | value |
|---|---|
| `src/app.js` | `const N_CTX = 4096;  // must match inference.rs \`-c\`` |
| `src-tauri/src/inference.rs` (desktop sidecar) | `-c 4096` |
| `src-tauri/src/engine_inproc.rs:504` | `n_ctx: if tier == "low" { 2048 } else { 4096 }` |

On the A22 — a **low**-tier device, and the reference device — the frontend
budgeted roughly **3456 tokens of history against an engine holding 2048 in
total**. It truncated correctly and far too late, so the decode failed and the
user saw an engine error.

**Why it drifted is the transferable part.** The comment "must match
`inference.rs` `-c`" was *true when written*: desktop had exactly one context
size. Phase 1.2 introduced a **per-tier** mobile `n_ctx` and nothing propagated
it to the frontend constant. Two numbers that must agree, and a comment naming
only one of the two places they live. The frontend could not even discover the
truth: `EngineInfo` carried `port`, `status`, `gpu_offload` and no context
size, so 4096 was not a lazy default — it was the only number available.

**Fix: delete the second copy rather than correct it.** `n_ctx` is reported on
`EngineInfo` and the frontend reads it, with 4096 as the fallback for an older
backend. Deriving the window from the tier on the frontend was rejected: it
would duplicate the tier→`n_ctx` rule in a second language, which is exactly
how this drifted.

**Interaction with 1.5 (prefix-KV reuse), by design:** dropping the head of the
history invalidates the cached prefix. That is correct and expected, and it is
handled by the existing trim-before-extend path rather than a special case —
the mirror compares token ids, so a changed prefix costs reuse and never
correctness (invariant 1 in the 1.5 section). A turn immediately after a
truncation is coherence-tested in the serve-loop's style, because "fast and
about the wrong thing" is the failure mode that would otherwise be silent.

**The family this belongs to.** A function that is present, correct-looking,
commented, pure, and silently mis-parameterised sits alongside
`bundle.resources = {}` (a merge that reads as correct and does nothing) and
the gate run labelled with the wrong commit. In all three the code and its
documentation agreed with each other and disagreed with reality. That is the
family's cleanest definition yet: **present, correct-looking, commented, and
silently mis-parameterised.**

#### 🔬 The drift etiology — a comment is a coupling declaration, not a reminder

Worth separating from D-4, because the mechanism outlives this particular
number.

`// must match inference.rs \`-c\`` is a *good* comment by every normal
standard. It is specific, it names the other side, it explains why the constant
has the value it has, and **it was true when written.** It became a trap
without ever becoming false: Phase 1.2 introduced a per-tier `n_ctx` on mobile,
so the set of places that must agree grew from two to three, and the comment —
which can only ever name the places that existed when someone typed it — kept
pointing at the one it knew about. Nothing was edited incorrectly. Nothing
warned. The comment aged into a lie by addition rather than by change.

**The rule: a cross-reference in a comment is a declaration that a coupling
exists, and a coupling wants a single source, not a reminder.** When you find
yourself writing "must match X", you have discovered a duplicated fact, and the
comment is a note that you chose to keep the duplicate. Sometimes that is the
right trade — but it should be a decision, and it carries an obligation that
the *next* person to add a third copy will not know they have inherited.

Concretely, that is why D-4's fix reports `n_ctx` on `EngineInfo` instead of
correcting `4096` to a tier-aware expression: correcting the value would have
left three places that must agree and a comment naming one of them, which is
the same trap reset with a fresher number. Removing the second copy is the only
version that cannot recur.

The corollary for reviewers: **"must match" in a comment is a smell with a
half-life.** It is harmless the day it is written and dangerous the day someone
adds a platform, a tier, or a mode — which, on this branch, is most days.

**The control case is on the very next line, which is what makes this
provable rather than merely plausible:**

```js
const N_CTX = 4096;         // must match inference.rs `-c`
const REPLY_RESERVE = 512;  // must match the request's max_tokens
```

Two adjacent constants, the same "must match" phrasing, opposite outcomes.
`REPLY_RESERVE` is **safe**, because the value it must match is *passed from
this constant* — `baseBody: { max_tokens: REPLY_RESERVE }`, with its own
comment saying "bound to the windowing reserve so the two can't drift". The
coupling was collapsed to one source and the comment merely describes it.
`N_CTX` is **unsafe**, because the value it must match lives in another
language, in a branch on tier, and was never plumbed back — so the comment is
all that holds the coupling together, and a comment holds nothing.

Same file, same author, same idiom, one line apart: the difference is not
discipline or care, it is **whether the fact has one home or two.** That is the
whole rule, and it is why "be careful to update both places" is not a fix.

**An audit of the same pattern across the tree** (`must match` / `keep in sync`
/ `mirrors`) found the rest to be descriptive rather than duplicated-fact
couplings — shape and rationale comments, which are fine — with one genuine
cross-language duplicate left standing and surfaced rather than fixed here:
`src/calc-tool.js` declares the calc grammar and says "Keep in sync with
`crates/kpack-calc`". That one is a real second home for a real fact. It is out
of 2.2's scope, it is not currently wrong, and it is now written down.

### D-5 — Mobile divergence keys on a platform CLASS, never on viewport width

**Decision:** every mobile CSS rule is scoped under `.is-mobile`, a class set
on `<html>` from the same `isAndroid(navigator.userAgent)` predicate that
chooses the transport. No `@media` query gates mobile behaviour.

**Rejected alternative:** a width breakpoint (`@media (max-width: 700px)`),
which is the conventional answer and reads as more idiomatic CSS.

**Rationale.** The standing constraint is that **desktop behaviour is
byte-identical**, and a width-keyed rule silently breaks it: a desktop user who
narrows their window would get the drawer, the hidden cost counter and the
rebuilt chat bar — a behaviour change nobody asked for, invisible in review,
and uncatchable by the desktop suite (which cannot see CSS). Keying on the
platform makes the invariant hold **at every width**, which is a property you
can check by reading one class name instead of reasoning about breakpoints.

**Evidence:** verified in a real browser with a desktop UA at **1280px and at
360px** — static 260px sidebar, `display:none` on all five new elements,
`--app-h` unset, and the pill still reading `"Local · offline"` *after*
`engine-ready` and `download-progress` were pushed through the machinery. The
360px run is the one that matters: it is the case a media query would have
broken, and it is the reason the check was run at two widths rather than one.

**Precedent it extends:** the same reasoning as `transport.js` choosing its
platform once at startup rather than per call. One predicate, one place.

---

#### 🔬 Numbers right, pixels wrong — the exhibit, now four entries

**Three in one session, none caught by an assertion.** Alongside the adaptive
icon (bounding box correct, contents wrong), 2.2 produced three more, and the
tally is the point: **the screenshot is the check-that-can-fail for a visual
claim, the way a same-commit control run is for a flaky suite.**

| # | defect | why the assertion passed |
|---|---|---|
| 1 | pill rendered `"Ready — r"` | the assertion checked the pill's *rect* was inside the viewport, and it was — the text inside the rect was clipped |
| 2 | model name ellipsised to `"Socratic…"` | `.grow` is `flex:1` and so is `.chattitle`, so the spacer split the free width; nothing measured the *title's* share |
| 3 | pill and prominent row both visible | `pill.hidden = true` did nothing, because `.pill{display:inline-flex}` is an author rule and beats the UA `[hidden]{display:none}`. The code's comment claimed exactly one element was visible and was simply false |

Defect 3 is the sharpest: **the comment and the code disagreed, and only a
picture could say which was right.**

This also previews the limitation flagged when 2.2 was assigned. The harness
converts geometry into fact; it cannot say whether a screen reads well. The
founder's device session remains the visual gate, and no amount of assertion
count substitutes for it.


---

## ⚑ Surfaced for Phase 2 scoping (founder decision, do not solve unilaterally)

**Context-window policy — RESOLVED as decision D-4 (truncate-oldest).** The
paragraph that stood here described the overflow as unmanaged. It was not: the
windowing existed and was handed the wrong number. See D-4 above for the
policy, the root cause, and the fix.

## Future work (founder-originated; logged, not scheduled)

### Long-term memory via RAG at link time (P3+)

When §6 device-to-device sync lands, conversation history syncs to the desktop
and becomes RAG-ingestible, so the tutor regains old context by **retrieval**
while the phone keeps only its working window. **Phone = working memory,
library = long-term memory.**

This is why D-4's never-truncate-storage half is an invariant rather than an
implementation detail: a transcript trimmed to fit a phone's context can never
be re-ingested later. The cheap "optimisation" of matching storage to the
window would quietly destroy this feature's input years before anyone tried to
build it.

### Mobile pack creation + query (post-P1-gate or alongside P3 — founder to decide)

Upload files from the phone, build a personal pack, query it in chat.

**This supersedes spec v1.1 §0's "pack building" non-goal by founder decision,
so it needs a spec-amendment line and a deliberate scoping decision at a phase
boundary — it is not a backlog item that can be quietly picked up.**

Technical shape, known from the plan phase:
- **Feasible now:** text/MD ingest + embedder + `rag_query` — `kpack-core` and
  `kpack-embed` already compile for aarch64. The BGE GGUF is ~118 MB, so tier
  and storage accounting are needed (it is a meaningful fraction of a floor
  device's budget next to the model itself).
- **Blockers:** pdfium (PDF) and tesseract (OCR) are desktop-native with no
  mobile port — either replaced or excluded from v1 of the feature.
- **Another Kotlin-shim surface:** file access is SAF/content-URIs through the
  dialog plugin, joining `ConnectivityManager`, `ACTION_SEND` and the
  AndroidKeyStore bridge.

It composes with the long-term-memory note above into one story: **the phone
gains its own library.**

## Phase 0 founder items surfaced (⚑0.4)

1. Android APK signing-key ceremony (crown-jewel #2; same regime as curator key,
   two offline backups verified restorable). Blocks the first signed APK (Phase 4).
2. Google Play developer account ($25) + H8 developer-verification registration
   (Thailand in the outside-Play verification pilot — our market). Register early.
3. Two more real test devices for the 3-device install/update gate (spec §3: one
   8 GB mid-ranger, one Huawei/Honor no-GMS). A22 is device #1.
