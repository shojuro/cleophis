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
*(Sharpened in 3.1: the mock does not hold the token in process memory either
— `CredentialPersistence::EntryOnly`, a fresh credential per `Entry::new`, so
the save is discarded on the next line while returning `Ok`. The session
survives in `Session`'s own field, not in keyring. See the 3.1 section.)*
`keyring` v3 **silently mocks on unsupported targets**, which is
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
- **⚑ The 5.2 release-config audit now has a checklist:
  `docs/ops/release-config-audit.md`.** Written cold, ahead of the work, because
  every item on it is verifiable *only* on a signed release build — and the
  first signed release build happens in the same session as the founder's
  signing ceremony. The earliest possible moment to verify these is also the
  worst possible moment to be designing their verification, so the checklist
  exists before that session rather than being improvised during it. Its
  highest-risk item is the R8 one below.
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
`libdbus-sys`'s build script requires system dbus development headers, and the
toolchain is userspace-only by founder decision (no sudo). This is
environmental and pre-existing — it is
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
because keyring v3 falls back to a non-persisting mock on unsupported targets)
will *not* fix the Linux-host build.

> **CORRECTION (amended in place, like the `{}`-vs-`null` line in the brief).
> This paragraph originally attributed `libdbus-sys` to `keyring`'s
> `linux-native` feature and concluded "Linux is exactly where `linux-native`
> applies." That reasoning is void — `keyring`'s Linux subtree is
> `linux-keyutils` + `log` and it declares no default features. `libdbus-sys`
> comes from `tauri → tauri-runtime-wry → tao → dbus`, which is
> unconditional.** The conclusion survives and gets stronger: **no `keyring`
> change of any kind can unblock the Linux host**, because the blocker is
> Tauri itself. Measured in 3.1; full write-up in that section.

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

**Verified from the artifact THREE times, by three independent parties, all
agreeing** — gen-4 (which built it), steering (which blessed it), and gen-5
(which re-verified the archive), each running the committed `verify-apk.py`.
Identical digest, identical entry count, identical unaccounted fraction from all
three. **This is the strongest artifact evidence the track has produced**, and
the strength is in the independence rather than the repetition: three observers
who did not share a measurement.

The third run is not ceremony. Steering and gen-4 verified the *build output*;
what the founder installs is the **archive**. Re-hashing
`~/cleophis-artifacts/cleophis-2.2b-debug-53780ca.apk` is what closes the gap
between the thing that was checked and the thing that was shipped — the same
distinction that makes "correct measurement of the wrong artifact" the hardest
failure in this document to catch.

*(An earlier revision of this paragraph said "twice, by two parties", counting
steering and gen-5 and omitting gen-4's own run. Corrected on steering's roster
rather than left to stand: undercounting independent verification is a smaller
error than overcounting it, but it is still a wrong number in an evidence
record.)*

Unusually, the build output at
`gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk`
**also still exists** — gen-4 stalled before starting another build, so nothing
overwrote it, and it hashes identical to the archive. Every previous entry in
this document had to note that its build output was gone; this is the one time
both copies can be compared, and they agree.

*(Later the same day that window closed: the next APK build ran, and
`build-android-apk.sh` deletes the prior output before packaging — the fix for
the zipflinger orphan trap. So 2.2b again exists only as the archive. The
comparison above was made while both copies were present and is recorded as a
measurement taken then, not as something a reader can now re-derive. Noted
because a future reader following the path above would otherwise find an empty
directory and wonder which claim was wrong.)*

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

---

## Native chunk — N.1 The JNI bridge, built and proven on its own

Steering ratified bridge-first sequencing (rationale under "Surfaced, not
fixed" above). This is the deliverable: **one commit whose only job is a round
trip — Rust → our Kotlin → Rust — that fails loudly if the bridge does not
work.** Not a feature. `ConnectivityManager`, `ACTION_SEND` and (if the
pack-upload feature lands) SAF all sit behind it.

`src-tauri/src/android_bridge.rs` + `gen/android/.../NativeBridge.kt`, called
once from `lib.rs` setup, printing one `[bridge]` line in the mould of
`[kernels]`.

#### Process breach, recorded by its author

**I set task 1's `owner` to `mobile-p1` believing that was my own name. It is
gen-2, retired after Phase 1**, and the task system dispatched the assignment to
it. It declined — correctly, and on better grounds than my request was made on:
it could not author a provenance entry for events it never witnessed, which is
precisely the failure protocol v2 exists to prevent. Steering separately
recorded ledger edits staged during a hold I had not yet received.

Two things worth keeping, and the second is not the obvious one:

1. Check the roster before addressing anything. A name that looks like yours may
   be an earlier holder of it, and generation numbers are not visible from
   inside a generation.
2. **"I had not received the hold yet" and "I ignored the hold" are
   indistinguishable downstream** — which is the same shape as every other
   lesson in this document, arriving this time about my own conduct rather than
   about an artifact. I cannot prove the timing from my side, and that is the
   point: the remedy is never a better recollection, it is a record that exists
   without one. The provenance-sidecar policy below is the structural version of
   this paragraph, and it is not a coincidence that both landed in the same
   chunk.

### 🔬 The brief's `ndk_context` is wrong, and it would have failed on device

The brief (§5) and the native handoff both specify `ndk_context` as the source
of the `JavaVM` and `Context`, and steering's assignment repeated it. **It is
the wrong mechanism for this app**, and the way it is wrong is this milestone's
signature failure shape: it compiles, it cross-compiles, and it hands you null
pointers at runtime.

Measured, not assumed:

| claim | check |
|---|---|
| `ndk-context` is undeclared by us | true, and **stronger than that** — it is absent from `Cargo.lock` entirely, so *nothing* in the graph depends on it |
| `Cargo.lock` would show Android-only deps | yes — `jni 0.21.1` and `ndk 0.9.0` are both in it via `tao`, so absence is meaningful rather than an artifact of target filtering |
| something initializes it | **no.** Zero calls to `initialize_android_context` in `tao 0.35.3`, `wry 0.55.1` or `tauri 2.11.5`. The single occurrence of the name across all three is a commented-out `// TODO: use ndk-context instead` in tao's Android event loop |

`ndk_context`'s accessor reads process-global statics that some *other* crate is
expected to have filled in. Adding the dependency would have produced a bridge
that failed on device presenting as **"JNI is broken"** rather than as "this
global was never initialized" — and it would have failed inside whichever
feature went first, which is the exact compound-debugging trap bridge-first
sequencing exists to prevent. The sequencing earned its keep before a single
feature was written.

**What Tauri actually offers is better.** `tao` keeps its own `AndroidContext`
(the `JavaVM` pointer plus the activity's global ref) and the accessor is public
the whole way down: `tauri::tao` is a re-export (`tauri/src/lib.rs`:
`pub use tauri_runtime_wry::{tao, wry}`), and
`tao::platform::android::prelude` re-exports the `ndk_glue` module holding
`main_android_context()`. **No new dependency at all** — the bridge adds zero
lines to `Cargo.toml`, and `jni = "0.21"`, declared since Phase 0.2 and
referenced by nothing, finally has a caller.

**Ordering is proven rather than hoped for.** tao inserts the context into its
map and only *then* calls the setup path that reaches our `run()` — the insert
precedes the `setup(...)` call in the same function in tao's activity-create
handler. So `main_android_context()` is populated by the time Tauri's `setup`
hook runs, which is where the probe is called. That is a source-level proof, in
the mould of the cover-scope fix: two callers of one function cannot disagree.

**Version unification checked**, because two semver-incompatible `jni` crates
would mean `JObject` from tao and `JObject` from us are different types with an
identical name — a genuinely baffling error. `tao` declares `jni = "0.21"` under
its Android target and our lock resolves exactly one `jni 0.21.1`. One crate,
one set of types.

### The class-loader trap, avoided by reading tao rather than by debugging

`env.find_class("com/cleophis/app/NativeBridge")` is the obvious call and it is
**wrong on Android**. On a thread attached through JNI, `FindClass` resolves
against the *system* class loader, which cannot see application classes. It is
the most common way an Android JNI bridge fails, and it fails with a bare
`ClassNotFoundException` that says nothing about class loaders.

The bridge instead calls `WryActivity.getAppClass(name)` on the activity, which
is a one-line `Class.forName(name)` — but executed *inside an app class*, so it
resolves in the app's loader. tao routes its own lookups through the same
method, which is what made this findable by reading rather than by a device
round trip. Recorded because the correct code and the broken code differ by one
call and look equally reasonable.

Note the consequence for `Desc`: passing a `&str` where `jni` wants a class
silently selects the `find_class` path. The bridge passes a `&JClass` obtained
from `getAppClass`, so the wrong path is not merely avoided but unreachable.

### 🔬 The R8 trap: the smoke test proves the bridge in debug and says NOTHING about release

The most valuable thing found while writing this commit, and it was found by
asking what the smoke test *could not* tell us.

`app/build.gradle.kts` sets `isMinifyEnabled = false` for debug and **`true` for
release**. R8 shrinks on reachability from Java/Kotlin, and `NativeBridge`'s
only caller is Rust across JNI, which R8 cannot see. From R8's point of view
`describeDevice` is dead code.

The existing wry rule is **not** sufficient, and reading it carelessly says it
is:

```
-keep class com.cleophis.app.* { native <methods>; }
```

That matches our class and so preserves the class *name* — but its member spec
keeps only `native` methods. `describeDevice` is a plain static with no Java
caller and would be removed. The symptom would be **`NoSuchMethodError`, not
`ClassNotFoundException`** — a class that exists with the method missing, which
points an investigator at the signature rather than at minification.

Fixed with `app/proguard-cleophis.pro` (hand-written; the other two `.pro` files
are autogenerated and marked do-not-edit). The release buildType globs
`**/*.pro` under `app/`, so a new file is picked up without a gradle change.

**This rule is reasoned, not yet verified**, and the distinction is the point:
no release APK exists, because signing is founder-serialized. It is now item 1
of **`docs/ops/release-config-audit.md`** — filed as an explicit checklist line
with its verification method, rather than left to live only as a lesson in this
ledger, because a lesson is not a task and nothing would have executed it.

**The scheduling consequence, which is the part that needed writing down:** the
earliest possible verification is the first signed release build, and that
happens in the same session as the founder's signing wiring. So the checklist
had to exist *before* that session — improvised during a key ceremony is the one
way it does not get done properly. Every future Kotlin entry point reached only
from Rust inherits this item and needs its own keep rule and its own line.

The generalisable form, which is new to this document: **a debug-only proof and
a release-only failure mode are the same "check that cannot fail" pattern, split
across build types instead of across artifacts.** The device checkpoint below
will legitimately pass while the release path stays broken, and nothing in the
green result would hint at it. Every previous instance of this family was one
artifact measured wrongly; this one is the right measurement of an artifact that
is not the one that ships.

### 🔬 The diagnostic that would have crashed instead of reporting

Found after the bridge was committed (`580b2bc`) and while its APK was
building, by the ledger's own habit — writing down why the existing code was
correct and discovering it was not. **The in-flight build was killed**, because
the defect lands squarely on the one path the bridge exists to protect, and
shipping a checkpoint APK whose failure mode is a crash would have burned a
founder-serialized device session.

Landed as its own `fix(mobile-p1)` commit rather than an amend, deliberately:
`580b2bc` had already been reported to steering by hash, and rewriting a hash
someone else has been told about trades a small tidiness gain for exactly the
kind of provenance confusion this document spends most of its length
preventing. The history showing the defect and its fix is also the more useful
artifact.

**`jni` does not clear a pending Java exception on error.** Its own docs say so
(`jnienv.rs`: the exception "will be thrown in java unless `exception_clear` is
called"). So a failing `getAppClass` or `describeDevice` returns `Err` *and*
leaves the exception pending on the thread.

That thread is the **UI thread**, which Tauri and wry drive with constant JNI
traffic, and ART aborts the process on the next JNI call made with an exception
pending. So the first version of the probe would have **crashed the app instead
of printing `[bridge] FAILED …`** — and only when the bridge was broken.

Why this is worth a section rather than a line in a diff:

- **The failure mode was inverted.** A working bridge prints its line and a
  broken one takes the app down. The checkpoint's entire value is the
  *distinction* between six enumerated failure strings, and a crash collapses
  all of them into one uninformative outcome — on a founder-serialized device
  session that cannot cheaply be re-run.
- **Nothing off-device could have caught it.** It compiles, cross-compiles
  clean, and passes every check in this document. There is no test, because
  there is no JVM here.
- **It is a genuinely new member of the "check that cannot fail" family, and
  the distinction has a name.** Every prior member was a check that could not
  **fail** — a percentage that fit any number, assertions measuring a bounding
  box while the defect sat inside it, a run labelled with the wrong commit.
  This is a check that could not **fail-report**: the detection worked and the
  *reporting channel* did not. The probe would have found a broken bridge and
  destroyed the evidence on its way out.

  That is worse than a silent check, because the two outcomes are not read
  equally: **a green run reads as verification, and a crash reads as
  environment trouble or founder error.** The defect would not merely have
  hidden the answer, it would have pointed the next investigation away from the
  code.

- **The cost asymmetry that justified killing a running build.** On a
  founder-serialized checkpoint, **a wrong artifact is more expensive than a
  late one**: a re-run costs a human session that has to be scheduled, a delay
  costs machine minutes that cost nobody anything. Those are not close. It is
  worth writing down because the pressure at the moment of decision runs the
  other way — the build was 25 minutes in, and finishing it felt like the
  thrifty choice.

Fixed by splitting the JNI calls into `round_trip` so the caller can, on any
error, `exception_describe` (which dumps the Java stack trace to logcat — the
part that names the missing class or method) and then `exception_clear`,
once, rather than repeating cleanup at each `?`.

**The generalisable form: error paths in FFI are the code most likely to be
wrong and least likely to be exercised.** Every `?` here was written for the
happy path; the failure path had never been reasoned about at all, and it was
the only path that mattered for this commit's purpose.

#### Operational trap found while killing that build: `pkill` does not kill the build

Worth recording because it has a provenance consequence, not merely a tidiness
one. Killing the build meant `pkill -f build-android-apk.sh`, `pkill -f "tauri
android build"` and the gradle daemon — and **the compile survived all three.**
The actual worker was

```
cargo build --package cleophis --manifest-path .../src-tauri/Cargo.toml --target aarch64-linux-android ...
```

spawned several layers down, matching none of those patterns. It kept running
for six more minutes, and it did two harmful things at once: it **held the
cargo build-directory lock**, so the follow-up `check` sat on "Blocking waiting
for file lock on build directory" with no live holder that `pgrep` on the
obvious patterns would reveal; and it was **compiling a tree that was being
edited underneath it** — precisely the contamination protocol v2 exists to
prevent, arrived at from a new direction.

The rule this adds to the freeze discipline: **a build is not stopped until the
compiler is stopped.** Kill by walking the process table for `cargo`/`rustc`
against this manifest, not by killing the script that launched them. And a
`check` blocked on a build-directory lock is evidence that some earlier build is
*still alive*, not that a lock leaked.

### One safety property worth stating, because the code cannot show it

The probe calls `vm.attach_current_thread()` from the UI thread, which the JVM
has already attached. `jni` tries `get_env()` first and, on success, returns a
**nested** guard with `should_detach: false`, so dropping it is a no-op; only a
guard for a thread the call actually attached detaches on drop. Verified in
`jni`'s source rather than assumed, because the failure it rules out —
detaching the UI thread from the JVM on the way out of a smoke probe — would
take the whole app down and would look nothing like a bridge bug.

### Why none of this went where the tests run (decision D-3 applied, not ignored)

D-3 moves logic out from behind a `cfg` when it is platform-neutral **and**
fails silently. Neither half holds. Every line is FFI against a live JVM — the
case D-3 explicitly exempts — and the failure mode is the opposite of silent:
each step returns a described `Err` that the startup line prints. There is no
decision left in it for a test to check; the class name and the method
signature are the only claims, and the round trip is what checks them. D-3's
own tell applies: what remains behind the `cfg` is plumbing.

### Verification

| what | result |
|---|---|
| `cargo ndk -t arm64-v8a -P 24 check -p cleophis --all-targets` | **exit 0, zero warnings** (unanchored `grep -ci warning` = 0) |
| desktop surface touched | **none** — see below |

Logs: `cleophis-mobile-logs/bridge-aarch64-check-*.log`.

**Desktop prediction: 322 unchanged, zero warnings.** Every file in this commit
is Android-only by construction — `android_bridge.rs` is declared under
`#[cfg(target_os = "android")]` (not `mobile`, which would include iOS), the
setup call carries the same gate, and the Kotlin and ProGuard files are not
desktop build inputs. A movement in either direction would be the interesting
result: it would mean something compiled on desktop that should not have.

### 📱 Bridge APK — BUILT, verified against the artifact, clean provenance both ends

```
sha256  fa85ab6b8536aa3938963715c34c235e5001c7cde33c6379c90d1bdd7226f0d1
size    355,402,510 bytes
built   from 7ec85ae (bridge + the exception fix), debug-signed, arm64-v8a
copy    ~/cleophis-artifacts/cleophis-bridge-debug-7ec85ae.apk
log     ~/cleophis-mobile-logs/apk-debug-20260727-084037.log
```

**Provenance — both checkpoints recorded live, each in its own command.**

| checkpoint | HEAD | `git status --porcelain` | recorded |
|---|---|---|---|
| PRE | `7ec85ae` | empty, exit 0 | 2026-07-27T08:38:35+07:00 |
| POST | `7ec85ae` | empty, exit 0 | 2026-07-27T09:19:41+07:00 |

All four facts agree and **this is attributable to the commit**, not to a live
worktree — the standard protocol v2 was written to produce, and the first APK in
this track to meet it with both checkpoints taken by a living witness rather
than reconstructed afterwards. The 2.2b entry above is the contrast case, and
the difference is entirely that someone was present to run two commands.

Digest produced by `sha256sum` and never retyped. **Both copies hash
identically**, so the archive is intact — and unlike 2.2b, that comparison was
made and recorded in the same breath as the build rather than a day later.

`verify-apk.py` **PASS**: 968 entries, Σ compressed **355,212,779** vs file
**355,402,510** = **0.05 % unaccounted** (no zipflinger orphan); `assets/` is
`tauri.conf.json` only; `arm64-v8a` only.

**One check this commit needed that the standard script does not do.** Every
other artifact claim in this document is about Rust or config; this is the first
commit whose payload is *Kotlin*, and the script has nothing to say about
whether a `.kt` file compiled and got packaged. If it had not, the on-device
symptom would be `ClassNotFoundException` — indistinguishable from the
class-loader bug the bridge deliberately avoids, which is precisely the
two-causes-one-symptom confusion the whole bridge-first sequencing exists to
prevent. So it was read out of the APK:

| check | result |
|---|---|
| `Lcom/cleophis/app/NativeBridge;` class descriptor | present, `classes8.dex` |
| `describeDevice` method name | present, `classes8.dex` |

Read from the shipped archive's dex, not from a gradle log. It confirms the
Kotlin compiled, survived dexing, and is in the artifact the founder will
install — which narrows what a failure on device could mean before the device
is even touched.

### 📱 Device checkpoint — and the value predicted BEFORE it runs

The bridge cannot be proven off-device: an aarch64 `check` proves it compiles,
and nothing on this host can run Kotlin. So this needs the founder's A22.

**Predicted output, recorded before the run** — per the convention that an
unpredicted result proves less than a predicted one:

```
[bridge] ok round-trip via com.cleophis.app.NativeBridge device=samsung/SM-A226B/api33
```

`samsung` is `Build.MANUFACTURER`, `SM-A226B` is the A22 5G's `Build.MODEL`, and
`33` is `Build.VERSION.SDK_INT` for its Android 13. The probe returns real
`android.os.Build` values rather than a constant on purpose: a constant would
prove the call mechanism and nothing else, while this proves the Kotlin ran with
genuine framework access, which is what every queued shim actually needs.

**A mismatch is informative rather than merely disappointing:**

| observed | means |
|---|---|
| the line, values as predicted | bridge works end to end |
| the line, *different* values | bridge works; the prediction about the device was wrong (harmless, and it still proves real framework access) |
| `[bridge] FAILED getAppClass(...)` | the class-loader or minification path — R8 is off in debug, so suspect packaging |
| `[bridge] FAILED describeDevice: ...` | class found, method not — the `@JvmStatic` contract |
| `[bridge] FAILED main_android_context() is None` | the ordering proof above is wrong |
| **no `[bridge]` line at all** | the probe never ran — worse than a failure, and the one outcome that would otherwise be silent |

That last row is why both branches print. Reaching logcat: Rust's stderr is
redirected by tao to the tag **`RustStdoutStderr`**, so
`adb logcat -s RustStdoutStderr` (or `adb logcat -d | grep -F "[bridge]"`)
carries it, the same channel the `[kernels]` line uses.

**A seventh outcome, now excluded by construction: the app must not crash.**
Before the exception fix it would have, on every failure branch. If a crash
happens anyway, that is information rather than noise — it would mean a JNI
misuse the `exception_clear` path does not cover, and the logcat ART abort
message names it directly.

**Everything else about this build is unchanged from 2.2b**, so the visual
acceptance findings from that session carry over — the bridge probe adds one
log line and touches no UI. A founder holding both APKs should use this one.

### ⚑ The provenance sidecar — the delivery lesson closed structurally

Ratified policy, landed with this chunk's build-script touch:
**`build-android-apk.sh` now writes `<log>.provenance` beside its teed log**,
containing HEAD, `git status --porcelain`, and a timestamp at **both** PRE and
POST, plus the size and sha256 of every APK found.

Two design choices carry the whole value:

1. **Every field records the exit status of the command that produced it.**
   `porcelain_exit 0` is the only reading that licenses "clean". This is not
   defensive coding — it is the fix for a mistake already made on this track,
   and it was verified by deliberately forcing the failure rather than reasoning
   about it: a `git status --porcelain` that times out returns **exit 124 with
   empty output**, byte-identical to a clean tree. The sidecar's comment says so
   in the file, where the next reader is.
2. **The build computes its own digest.** The delivery failure recorded earlier
   in this document was an agent that never reached its verify-and-report step;
   the artifact was correct and nobody knew it existed. A build that emits its
   digest cannot finish silently, which converts "the report is part of the
   deliverable" from a discipline someone has to remember into a property of
   the tool.

Verified across all three branches before commit, since a provenance recorder
that records the wrong thing is worse than none:

| branch | result |
|---|---|
| clean tree | `porcelain <empty: tree clean>`, `porcelain_exit 0` |
| dirty tree | verbatim porcelain, including an `??` untracked scratch file |
| timed-out `git status` | `porcelain_exit 124`, `output_len 0` — the trap, reproduced on purpose |

**The bridge APK above predates this policy and is deliberately not
back-filled.** Its provenance is agent-recorded — the weaker form — and writing
a sidecar for it now would manufacture exactly the auditable-looking artifact
this policy exists to make real. The first genuine sidecar will belong to the
next build.

### Prerequisite already found for N.2 (`ConnectivityManager`)

Surfaced now because it is cheap to find and expensive to hit mid-feature:
**`ACCESS_NETWORK_STATE` is not declared in the manifest.** The permission list
is currently `INTERNET`, `REQUEST_INSTALL_PACKAGES`, `POST_NOTIFICATIONS`,
`FOREGROUND_SERVICE`, `FOREGROUND_SERVICE_SPECIAL_USE` — and
`ConnectivityManager.isActiveNetworkMetered()` / `getNetworkCapabilities()` both
require `ACCESS_NETWORK_STATE`, throwing `SecurityException` without it. It is a
normal (install-time) permission, so no runtime prompt, but it must be in the
manifest before the metered check can run at all.

Noted here rather than added now: this commit is the bridge and nothing else,
and a permission with no caller is the same kind of dead surface as a frontend
policy invoking a command that does not exist.

---

## 2.2 native completions — download network policy + share sheet — DONE

The two items 2.2 surfaced but could not build without a proven bridge. Both
ride `android_bridge::with_app_class`, so neither is the first thing over that
bridge — which is the whole return on bridge-first sequencing: a failure here
has one candidate cause instead of two.

### N.2 `ConnectivityManager` metered policy

**`isActiveNetworkMetered` is the only correct source, and the two obvious
substitutes are both wrong in the case that matters.** A transport check
(`TRANSPORT_WIFI`) and `navigator.connection` both report
Wi-Fi-versus-cellular — and a **metered Wi-Fi hotspot** (a tethered phone, a
hotel plan, a capped home line) is precisely where that distinction inverts.
The feature exists to respect someone's data plan, so a false negative costs
them money. The brief said this; it is restated here because the cheap
implementation is the wrong one and looks fine.

**`ACCESS_NETWORK_STATE` landed WITH its caller**, not before. A permission
with no caller is the same dead surface as a frontend policy invoking a
command that does not exist: it appears in every store listing and every
privacy review and nobody can say what it is for.

#### The split, and why it is three pieces rather than one

| piece | where | why there |
|---|---|---|
| `NetworkPolicy.describe(context)` → `metered=1 charging=0 battery=57` | Kotlin | Only the platform can answer it. |
| `net_state::parse` | Rust, compiled everywhere, **8 tests** | Pure, and silently expensive when wrong. |
| `decideDownload` / `chargeAdvice` | `src/download-policy.js`, **18 tests** | A user-facing rule with an override, whose prompt the frontend already owns. |

**One Kotlin string, not three booleans.** Each JNI method is a signature that
can be wrong, failing with a `NoSuchMethodError` that names the signature
rather than the cause; three accessors would be three chances at that, three
R8 keep surfaces, and — the real reason — three round trips for facts that
must describe *the same instant*. A connection that changes between two calls
yields a report that was never simultaneously true. One call, one snapshot,
and the parse becomes testable.

#### The asymmetry, stated so it cannot be "simplified" away

**Every unknown resolves to metered.** Absence, malformity, an empty string, a
missing key, a bridge failure — all of it lands on the conservative answer,
asserted by a test whose only job is to enumerate the failure modes.

The reason is that the two mistakes are not comparable: guessing *unmetered*
when the truth is metered spends the user's money and cannot be undone;
guessing *metered* when the truth is unmetered costs one tap. Same shape as
`tier_for_mobile` degrading to the floor tier on every detection failure, and
the general rule is worth naming: **when a probe can fail, fail toward the
cheaper mistake — and write down which one that is**, because the next person
to read the code will otherwise "clean up" the default to the common case.

Two smaller decisions with the same character:

- **`charging` is `Option<bool>`, not `bool`.** "We do not know" and "it is not
  charging" must stay distinct, because a charge *recommendation* issued on a
  guess is noise. `chargeAdvice` returns nothing on the unknown, and a test
  pins it.
- **A small download never prompts, even on metered.** A prompt that fires for
  a 2 MB catalog trains people to dismiss prompts, which is how the one that
  matters gets dismissed. The threshold is asserted as a boundary.
- **The charge notice is advice and never blocks.** A download refused for
  battery is one the user cannot start while their phone sits at 38 %, and
  they may have a charger in their bag. Telling them is useful; deciding for
  them is not.

The gate runs **once per install**, totalled over base + adapter + contract
adapter, rather than per artifact: those are one user action, and the figure
someone is asked to approve should be what will actually be transferred.

**Checked rather than assumed:** `window.confirm` is the app's existing
confirmation idiom, and in an Android WebView an unimplemented `onJsConfirm`
returns *false* silently — which would have meant a metered download could
never proceed, a policy that fails closed and invisibly. wry's generated
`RustWebChromeClient` implements it (line 161), so the idiom holds.

### N.3 `ACTION_SEND` share-sheet export

`exportChat` branches on `IS_MOBILE`; **the desktop `dialog.save` path is
untouched below the branch**. Both platforms format through the same
`convstore::export_chat`, so the bytes are identical and only the destination
differs — the handoff's point that mobile needs a destination, not a
formatter.

Three details in `ShareSheet.kt` are load-bearing, each a silent or misleading
failure: `FileProvider.getUriForFile` rather than `Uri.fromFile` (which raises
`FileUriExposedException` on everything ≥ N, and our minSdk is 24);
`FLAG_GRANT_READ_URI_PERMISSION` on the **chooser**, since that is the intent
the system delivers and granting only the inner one surfaces as a permission
denial inside the *other* app; and `FLAG_ACTIVITY_NEW_TASK`, because this is
called from a JNI-attached thread with no activity stack.

**No new FileProvider wiring was needed** — the scaffold's `cache-path` root
already covers the `exports/` directory, found by reading `file_paths.xml`
rather than assuming a new entry was required.

### 🔬 A D-3 violation caught in my own draft, and then a real bug behind it

`safe_file_stem` — the sanitiser that turns a user-typed chat title into a
filename — was **first written inside the `cfg(target_os = "android")`
module, with its tests beside it.** Those tests would never have executed:
not in the desktop suite, not in the aarch64 `check`, not anywhere, because
nothing on Android runs until a founder device checkpoint. A sanitiser whose
failure mode is a path separator surviving into a filename is D-3's trigger
almost by definition, and the first draft put it on the wrong side of the line
while the module's own doc comment argued for D-3.

Moving it to `mobile_native/pure.rs` (the `#[path]` pattern `engine_tools` and
`engine_serve` established) made it runnable — **and it immediately failed.**

`safe_file_stem("...")` returned `"___"` rather than falling back. Two defects
in one line:

1. **A `trim_matches('.')` that could never fire.** The character map has
   already replaced every dot with `_` by the time the trim runs. A defensive
   step guarding against something that cannot reach it — the "check that
   cannot fail" family, in miniature, and invisible on inspection because it
   reads as prudent.
2. **The fallback tested `is_empty()`, which was the wrong question.** `"___"`
   is non-empty and perfectly safe, and a file called `___.md` tells the user
   nothing about what they just shared.

Fixed by asking the question that was actually meant: fall back when the stem
**carries no information** — no alphanumeric character — which covers `""`,
`"   "`, `"..."`, `"///"` and `"!@#$%"` with one rule and no unreachable
defence.

**The chain is the lesson.** Putting the code where tests run is what made the
test run; the test running is what exposed the bug; the bug's shape was a
guard that could not fire. D-3 has been ratified since 1.5 and it still had to
be applied against a draft that had just finished explaining it — which is the
same finding as the absent-result lesson above: **a rule you have written down
is not a rule you automatically apply.**

### Why `safe_file_stem` sanitises where `account_dir_segment` rejects

Two sanitisers, opposite policies, in one codebase — worth stating so neither
gets "unified" into the other:

- A `user_id` is **machine-issued**. Anything malformed is a bug or an attack,
  and rejecting costs a legitimate user nothing. Reject, don't strip.
- A chat title is **typed by a human**. A chat honestly named `3/4 of what?` is
  not an attack, and refusing to share it would be a defect. Sanitise and keep.

Safety does not depend on the difference: the character map removes every
separator, the result is one component joined onto a directory we choose, and
the length is capped.

### Verification

| what | result |
|---|---|
| `net_state.rs` + `mobile_native/pure.rs`, scratch crate over the **real files** via `#[path]` | **14 passed / 0 failed** |
| `net_state.rs` sha256 | `d343a25a8568931a0679b5f1ef07503e5bcb52306b003061d8cfaef7c3ee1e95` |
| `mobile_native/pure.rs` sha256 | `8d904dda2953dea6c637ad0c739cacb14715456d0210012f2c72df99a4bd4cc6` |
| `npm test` | **58 → 76**, 0 failed |
| `cargo ndk -t arm64-v8a -P 24 check -p cleophis --all-targets` | **exit 0, zero warnings** (unanchored) |

Log: `cleophis-mobile-logs/item3-aarch64-final-20260729-185951.log`.

#### Item 3 gate: **346/0 exact, all 14 named — and ONE warning**

Run 21. **346 passed / 0 failed**, 14 new tests observed by name (6
`mobile_native::pure`, 8 `net_state`), so both the 338 and 340 discriminator
branches are excluded **by observation**. The count prediction hit exactly.

| checkpoint | HEAD | porcelain |
|---|---|---|
| PRE 19:11:16 | `b1f7b89` | clean |
| POST 19:18:06 | `e203ec3` | clean |

**Attributed as "live worktree `b1f7b89` → `e203ec3`, Rust-identical across the
delta."** The tree was never dirty; only the tip moved, because a commit landed
mid-run. The delta is two docs files plus one **comment-only** change to
`download-policy.js` (verified with a non-comment-line filter: the expression
`net?.metered !== false` is byte-identical, a trailing comment moved above the
line), so no Rust changed and the result means the same thing at either commit.

**This was recoverable rather than a permanent asterisk for one structural
reason: the harness reads HEAD live at both ends rather than accepting a value
from a message.** That is the same principle as the provenance sidecar,
applied to the gate runner — the run records what it observed, not what it was
told. Compare 2.2b, where a correct PRE checkpoint survived only in a
transcript and the attribution had to be softened forever.

**The race itself was structural, not a discipline failure.** The freeze
message and the commit crossed: steering wrote "freeze" while this agent was
committing work steering had approved. **A message in flight is not a freeze in
effect** — the protocol assumed the agent would be idle between the gate
request and the freeze arriving, and it had explicitly been told to keep
working. Proposed structural close, deferred to Phase 4: the gate script
refuses to start without a **freeze-marker file on disk**, and the requester
**pins the commit** rather than inheriting the tip. Same shape as
sidecar-over-watcher, applied to the coordination layer.

##### The warning, and why the prediction was half right in the interesting half

`warning: function `parse` is never used` — one warning, `cleophis` lib. The
count was predicted and hit; **"zero warnings" was predicted and missed.**

Diagnosed from source: nothing gates `parse`, nothing gates its tests, and
**desktop never calls it** — its only production caller is
`mobile_native.rs:68`, inside `#[cfg(target_os = "android")]`. `NetState` does
*not* warn, being `network_state`'s return type outside any cfg, which is why
there was exactly one. So this is **correct by design, not a wiring gap**: the
tests reach `parse` while nothing in the desktop production path does, which is
the state D-3 deliberately creates. Fixed with the narrow cfg'd `allow` and a
stated platform fact, and D-3 amended above so the next extraction carries it
by construction.

#### ⚑ A CHECK'S SILENCE IS ONLY EVIDENCE WITHIN ITS COMPETENCE

The zero-warnings half of the prediction rested on
`cargo ndk … check --all-targets` reporting clean. **It could not have reported
anything else.** On Android `parse` *is* used, so the cross-compile is clean by
construction — the check was structurally incapable of detecting a function
that is dead on desktop.

This is the check-that-cannot-fail family's **cleanest self-inflicted
instance**. Every prior member was inherited: a percentage that fit any number,
assertions measuring a bounding box, a run labelled with the wrong commit.
Here the check was built by the same agent that then trusted it *outside its
competence* — not fooled by someone else's instrument, but by the range of
one's own.

**The general form:** a green result is evidence only about what the check is
capable of seeing. Before citing a clean run, state what that run **could have
come back negative about** — the same discipline the download retraction
demanded for numbers ("say what a figure would have to be inconsistent with"),
now applied to tooling.

**The specific corollary, in both directions:** `cargo ndk check` **cannot
substitute for the desktop gate on dead-code questions**, because each platform
is blind to code dead on the other. A D-3 extraction is dead on desktop and
live on Android; a `cfg(desktop)`-only helper is the mirror image. Neither
check sees both, and their silences are not additive.

#### Desktop prediction: **332 → 346**

**8** `net_state` tests + **6** `pure` tests, counted from the files. `npm test`
is already measured at 76 (58 + 18) and needs no prediction.

| observed | means |
|---|---|
| **346** | both modules' tests executed on the Windows target |
| **338** | `pure.rs` compiled but did not run — the `#[cfg_attr(not(android), allow(dead_code))]` misapplied, i.e. the exact D-3 failure this chunk already made once |
| **340** | `net_state`'s tests did not run |
| anything else | something compiled that should not have |

---

## Phase 3 — Secure credential storage

### 3.1 `secure_store` seam + `keyring` off Android — DONE

Six functions — `save`/`load`/`delete` × (refresh token, offline-auth
verifier) — moved out of `cloud/store.rs` into `cloud/secure_store.rs`, which
is a seam with two bodies: `secure_store/desktop.rs` (the old bodies verbatim)
and `secure_store/android.rs` (a legible platform error until 3.2).
`keyring` moved from `[dependencies]` to
`[target.'cfg(not(target_os = "android"))'.dependencies]`.

#### 🔬 The thing being fixed is worse than this document recorded

The ledger has said throughout that "keyring v3 **silently mocks in-memory** on
unsupported targets", so an Android session "lives in process memory only".
Checked in `keyring-3.6.3`'s own source before building on it — per the
convention that you write down why the existing claim is correct — and **it is
not in-memory at all**:

| claim | source |
|---|---|
| Android resolves to the mock | `lib.rs`'s platform ladder ends `#[cfg(not(any(target_os = "linux", "freebsd", "openbsd", "macos", "ios", "windows")))] pub use mock as default;`. `target_os = "android"` matches none of those, and `linux-native` is gated on `target_os = "linux"`, which Android is not |
| the mock does not persist across `Entry`s | `MockCredentialBuilder::persistence()` returns `CredentialPersistence::EntryOnly`, and its `build()` constructs a **fresh** `MockCredential` per `Entry::new` |

Both of our functions build their own `Entry`. So on Android
`save_refresh_token` wrote the token into a value **dropped on the next line**
and returned `Ok`, and `load_refresh_token` — constructing its own fresh entry
— could only ever return `None`. Not a store with the wrong lifetime: **a write
that reports success and keeps nothing.**

The correction matters beyond pedantry, because the two versions predict
different things. "In-memory for the process" says a sign-in would survive
until the process dies; the truth is the token was never readable at all, even
one line later. It is the purest specimen yet of this milestone's signature
family — *present, correct-looking, and silently void* — sitting alongside
`bundle.resources = {}` and the gate run labelled with the wrong commit.

#### What changes on Android is nothing, and that is the intended result

Stated up front so a founder session does not read it as a regression, and
because "we changed the storage layer and nothing changed" is the kind of claim
that needs its reason written down:

- A **live** session already survived on `Session`'s own in-memory
  `refresh_token` — `refresh_via_gate` reads the token from the session
  struct (`session.rs:902-910`), never from the keyring — so mid-session
  rotation is untouched.
- Boot-time `restore()` already got `None`, so a force-stop already logged the
  user out.

What changes is that it now does so **for a reason that is written down**. The
three functions that return `Result` return `Err`; the three that return
`Option`/`()` cannot carry a reason in their return value, so they emit a
`[secure-store]` line — the same tag convention as `[bridge]` and `[kernels]`,
where a device transcript carries its own explanation instead of requiring
someone to have believed a build log. **3.2 changes the behaviour; 3.1 makes
the gap honest and takes a dependency that cannot work out of the Android
graph.**

That split follows the "check that cannot **fail-report**" lesson from the
bridge's exception bug: a `None` that means "nothing stored" and a `None` that
means "this platform has no store" are indistinguishable at the call site, and
the log line is the only channel that exists to tell them apart.

#### Why the gate is `not(target_os = "android")` and not `desktop`

`mobile` includes iOS, where `apple-native` is off and `keyring` lands on the
same mock. The property that actually matters is "does this target have a
working `keyring` backend compiled in", which is exactly the set the
`Cargo.toml` target section names — so the code gate and the dependency gate
are written in the same vocabulary and **cannot disagree about who gets it**.
One fact, one home; the D-4 coupling etiology applied prospectively rather than
after it bit.

#### Verification

| what | result |
|---|---|
| `cargo ndk -t arm64-v8a -P 24 check -p cleophis --all-targets` | **exit 0, zero warnings** (unanchored `grep -ci warning` = 0) |
| `keyring` in the **aarch64-linux-android** graph | **absent** — `cargo tree -i keyring --target aarch64-linux-android` → "nothing to print" |
| `keyring` in the **windows-msvc** graph | `keyring v3.6.3 └── cleophis v0.1.0` |
| `keyring` in the **linux-gnu** graph | `keyring v3.6.3 └── cleophis v0.1.0` |

Read from the **resolved dependency graph** rather than from `Cargo.toml`,
which is the dependency-level form of this document's standing rule that
bundle claims are checked against the built APK and never against the config
meant to produce them. A target section that is subtly mis-spelled reads as
correct in the manifest and changes nothing in the graph — the same shape as
`bundle.resources = {}`.

**The aarch64 check is a check that can fail, and that is why it counts here.**
`--all-targets` compiles the `#[cfg(test)]` modules, so if the
`cfg(not(target_os = "android"))` gate on `secure_store/desktop.rs` were wrong
in the permissive direction, that file's `use keyring::Entry` would have hit a
crate that is no longer in the Android graph and the check would have failed to
compile. Its passing *is* the proof of exclusion, not a separate assurance.

Log: `cleophis-mobile-logs/p31-aarch64-check-20260729-105611.log`.

#### Desktop prediction: **322 unchanged**, zero warnings

322 is the standing baseline (run 18 at `0ede91b`; the bridge chunk predicted
and expected no movement). 3.1 adds no tests and removes none — the two keyring
round-trips move module, not existence — and every desktop body is a
relocation.

The prediction has a discriminator registered in advance, because an unmoving
number proves nothing on its own:

| observed | means |
|---|---|
| **322**, with `cloud::secure_store::desktop::tests::keyring_round_trip` and `…::verifier_keyring_roundtrip` in the log under their **new** paths | the move landed and both tests executed on the target |
| **320** | the `cfg(not(target_os = "android"))` gate excluded `desktop.rs` on Windows too — the specific way this change could pass a gate while silently deleting two tests |
| **324** | both bodies compiled, which the `pub use` collision should have made impossible |

The middle row is the one this prediction exists for. Two tests that vanish
into a cfg leave a green suite and a smaller number, and 320 read without a
prediction beside it looks like an ordinary pass.

#### 3.1 gate: **GREEN** (run 19, at `ecc785a`, protocol v2) — top row, by name

**322 passed / 0 failed / 10 ignored, zero warnings**, 202 s. PRE `ecc785a`
/empty → POST `ecc785a`/empty, all four provenance facts agreeing, so this is
attributable **to the commit**.

```
test cloud::secure_store::desktop::tests::keyring_round_trip ... ok
test cloud::secure_store::desktop::tests::verifier_keyring_roundtrip ... ok
test result: ok. 322 passed; 0 failed; 10 ignored; finished in 202.00s
```

Both tests observed **under their new module paths**, which is the part the
prediction was actually about. The 320 branch — the cfg silently excluding
`desktop.rs` on Windows too — is excluded **by observation** rather than by
assumption.

**What this run adds to the predict-the-count convention.** Every prior use
predicted a *number*. This one predicted a **mapping from each possible number
to a distinct cause**, registered before the run — so the result was not "the
number matched" but "one specific causal branch fired and the other two are
ruled out". A number alone could not have done that here, because the
informative outcome was a number that *did not move*: 322 is what a correct
move produces and also what "nothing compiled at all" would produce, and only
the test **names** separate them. Steering captured the names because the
discriminator asked for them. Running total: **266 → … → 321 → 321 → 322 →
322**.

#### ⚑ Convention added: gate evidence should be emitted by the thing under test

Logged beside this run because the run exposed it. Steering's tee target
pointed at a scratchpad path that no longer existed, so the wrapper reported a
nonzero exit and **wrote no log file** — the evidence survived only because the
task runner happened to capture stdout.

That is the **legibility** failure again (the run was correct; the recorder
was not), and it is the third distinct place it has appeared: the lossy filter
that dropped a failing test's name, the 2.2b PRE checkpoint that existed only
in a transcript, and now a gate log that was never written. The committed
provenance-sidecar policy already answers this for *builds* —
`build-android-apk.sh` writes its own record, so a build cannot finish
silently. **Steering-side gate runs have no equivalent and still depend on
ad-hoc redirection by the hand that invokes them.**

The rule this generalizes to, and the reason the sidecar policy was worth
having: **gate evidence should be emitted by the thing under test, not by the
hand that invokes it.** Same instinct as putting the `[kernels]` and
`[bridge]` lines inside the app rather than in a build log. A wrapper that can
lose its own output is a wrapper whose green result means less than it looks.

### 3.2 Kotlin `SecureStore` — AndroidKeyStore AES-256-GCM — DONE (pending 📱 CP3)

`gen/android/.../SecureStore.kt` does crypto and nothing else;
`cloud/secure_store/android.rs` and `cloud/secure_store/blob.rs` own the file
layout, slot naming, AAD derivation and the on-disk envelope. Design reviewed
by steering before implementation, on the standing habit that the justification
gets written before the code it justifies.

**Why the division is this way round, and not the conventional way.** The
common Android answer is a Kotlin store that owns its own persistence
(EncryptedSharedPreferences-shaped). That would put the traversal rejection,
the slot naming and the AAD derivation on the far side of a JNI boundary — in
the one language this project cannot run a test in — and every one of those
rules **fails silently**: a path that escapes its directory throws nothing, and
a `save`/`load` pair deriving different AAD produces a decrypt failure whose
symptom is "you keep getting signed out" and whose apparent cause is the
keystore. That is decision **D-3** exactly, so the pure half is `blob.rs`,
compiled on every platform, tested by the desktop suite. What stays behind the
`cfg` is the JVM call, which D-3 exempts.

#### The choices, each with the reason it was made

| choice | reason |
|---|---|
| AndroidKeyStore provider | Non-exportability is a property of the provider, not a flag: key material never enters our address space. A key we could read would be no better than a file, which is the entire point of the phase. |
| AES-256-GCM, `ENCRYPTION_PADDING_NONE` | Brief + security review L1. AEAD, so integrity ships with confidentiality; GCM is a stream mode and AndroidKeyStore rejects padding with it. |
| `setRandomizedEncryptionRequired(true)` | The requirement "random 96-bit IV per encryption" **enforced by the platform instead of by our discipline** — with it set, AndroidKeyStore *refuses* a caller-supplied IV and generates one from its own CSPRNG. Set explicitly although it is the default, because a security property that holds only by default is one line from not holding. |
| tag pinned at 128 bits | Specified on decrypt, so a stored blob cannot negotiate its own tag length. |
| **`setUserAuthenticationRequired` deliberately OFF** | See the warning below — this is the footgun of the file. |
| StrongBox not requested | `StrongBoxUnavailableException` on devices without a discrete secure element (much of our market), so adopting it means a new fallback branch **on the boot path** in exchange for a property we cannot confirm the A22 offers. |
| one key, N blobs | GCM under one key with distinct random 96-bit IVs is safe far past our volume (collision bound ~2⁴⁸ encryptions; we do single digits per session). Per-slot keys would add lifecycle without adding a property. |
| `@Synchronized` key creation | `generateKey()` on an existing alias **replaces** the key, which would leave the losing writer's blobs undecryptable. Two saves genuinely can overlap — `restore()` runs on a `spawn_blocking` thread while a sign-in runs on another — so check-then-create has to be atomic rather than usually-fine. |

**GCM nonce reuse is catastrophic, not merely weak**, which is why the IV
source is the row that matters most. Two messages under one key sharing a
nonce leak their XOR *and* leak the authentication subkey, which lets an
attacker forge tags for that key. It is the one mistake in this file that could
not be recovered from — so it is arranged to be unmakeable rather than merely
avoided.

#### AAD: one line beyond L1, and what it buys

Each ciphertext is bound to its slot (`cleophis-secure-v1/refresh-token`,
`…/verifier/<user_id>`). Without it every blob under one key is
interchangeable, so an attacker who could write to the app's private directory
could copy `verifier-<A>.blob` over `verifier-<B>.blob`; it decrypts perfectly,
and **account B's offline sign-in then accepts account A's password.** That is
a privilege transfer, not corruption. The threat needs root, so it is not the
top risk — but L1 asked that the tag be verified, and AAD is what makes
verifying it say *which* secret was verified.

#### ⚠ The footgun, written down because only a written warning stops it

**Do not add `setUserAuthenticationRequired(true)`.** It reads as a free
security upgrade. It is not, and the causal chain is worth stating because a
future agent will meet the flag before they meet this paragraph:

1. **It breaks the feature.** Every decrypt would demand a device credential
   or biometric, and the decrypt that matters runs during boot-time
   `Cloud::restore()` — no UI, no user present. "Stays signed in" becomes
   "prompt on every launch", the exact opposite of what 3.2 delivers.
2. **It arms `KeyPermanentlyInvalidatedException`.** With user-auth **off**, a
   new fingerprint enrolment or a lock-screen PIN change does *not* invalidate
   this key, so the exception essentially cannot arise and its residue is
   already covered by the generic "key missing or unusable → no credential"
   path. Turn user-auth on and a routine PIN change **silently destroys the
   key**: every stored blob becomes permanently undecryptable, and the user is
   signed out with no explanation and no route back but signing in again.

The absence of that flag is therefore a decision with a stated reason, not an
oversight awaiting tidying. Recorded in `SecureStore.kt` beside the builder —
where someone would add it — as well as here.

#### `getNoBackupFilesDir()`, and why it is not merely defence in depth

Blobs live in `<noBackupFilesDir>/secure`. Steering asked for the argument
either way; it turns out not to be a trade-off at all.

**The key is non-exportable and device-bound, so it cannot be backed up or
transferred by anything.** A blob restored onto another device — or onto this
one after a wipe — is therefore *permanently undecryptable*. Backing up the
ciphertext could only ever produce a confusing decrypt failure, never a working
session. There is no convenience being given up.

The second reason is structural: `BackupAgent.getExtraExcludeDirsIfAny`
unconditionally adds this directory, and the framework ignores **even an app's
own explicit `fullBackupFile()` call** on a file inside it (`BackupAgent.java`,
the `fullBackupFile` doc). Compare the alternative — exclusion by a rule in
`backup_rules.xml`, a mechanism this project demonstrated *in the previous
commit* can be wrong for four phases without anyone noticing. **A guarantee the
framework enforces against our own mistakes beats one we have to keep writing
correctly.** The directory is also under the data-dir root, which those rules
now exclude, so the blobs are covered twice — once structurally. That is what
belt-and-braces is supposed to look like, as against what it looked like
yesterday.

#### The bridge's exception discipline now has exactly one home

`android_bridge::with_app_class` generalises what was `describe_device`'s
private `round_trip`: acquire the JVM and activity, resolve the class through
`WryActivity.getAppClass` (never `find_class`), run a closure, and
`exception_describe` + `exception_clear` on any error. `describe_device` and
all three `SecureStore` calls go through it.

**Not tidying.** That cleanup is the code most likely to be wrong and least
likely to be exercised — it runs only after something else has failed, there is
no JVM on the build host to test it against, and its first version was wrong in
a way that *crashed the process instead of reporting*. A second hand-written
copy is the duplicated-fact trap sitting in precisely the code that could never
catch the divergence.

#### ⚠ A JNI path the bridge checkpoint did NOT prove

The probe ran on the **UI thread**, where `attach_current_thread` finds the JVM
already attached and returns a nested guard whose drop is a no-op.
`SecureStore`'s callers arrive from a different direction: `Cloud::restore`
reaches us via `tauri::async_runtime::spawn_blocking` (`cloud/commands.rs:75`),
i.e. a Tokio blocking-pool thread the JVM has **not** attached, and potentially
a different one per call. There the guard genuinely attaches and genuinely
detaches on drop.

That is correct, and it is **unclaimed** rather than verified: the bridge
result does not cover it, and CP3's force-stop → relaunch cycle is exactly the
exercise that does — `restore()` on a fresh process, on a blocking thread, is
the first thing that runs.

#### A checkpoint flaw found by asking what the founder would actually see

The first version of `android.rs` logged only failures. The happy path would
therefore have been **silent** — and this document already records, from the
bridge probe, that "no line at all" is the one outcome worse than a failure,
because it cannot be told apart from "the code never ran". CP3's whole content
would have reduced to an inference from app behaviour.

Fixed: **every read and write emits exactly one `[secure-store]` line, success
included**, so a transcript distinguishes four outcomes — `ok`, `no blob
stored`, a named failure, and (by absence) never called. One line per launch is
what `[kernels]` already costs and it carries no secret. Found by writing the
checkpoint ask before the code was final, which is the same habit that has now
caught defects in five separate places.

#### Log hygiene (M3)

No token, no verifier, no `user_id`, no ciphertext reaches a log line. The
Java-side detail that identifies a failure — `AEADBadTagException` (tampered,
truncated, or wrong slot label) versus a keystore error — reaches logcat
through the bridge's `exception_describe`, which prints a stack trace and not
our data.

**A failed decrypt does not delete the blob**, deliberately: a destructive read
path would turn transient keystore unavailability into permanent credential
loss. Leaving it costs one failed read per launch until the next successful
`save_*` overwrites it — self-healing in the safe direction.

#### Verification

| what | result |
|---|---|
| `blob.rs` logic, run in a scratch crate over the **real file** via `#[path]` (not a copy) | **10 passed / 0 failed** |
| file compiled by that run, sha256 | `c15aa379fe1810ecc9e0b71cde4707212b652c3268c687f827e4150952441f52` |
| `cargo ndk -t arm64-v8a -P 24 check -p cleophis --all-targets` | **exit 0, zero warnings** (unanchored) |

The scratch crate compiles the shipping `blob.rs` **byte-identically** — the
digest above is of the file `#[path]` pointed at — against a verbatim copy of
the one function it names from elsewhere (`store::account_dir_segment`). Same
pattern as `tools.rs` (1.3), `serve.rs` (1.5) and `tier_for_mobile` (2.3), and
for the same reason: `cargo test -p cleophis` cannot build on this host.

Log: `cleophis-mobile-logs/p32-aarch64-check-final-20260729-162621.log`.

#### Desktop prediction: **322 → 332**, zero warnings

10 new tests, **counted from the file** rather than from memory — the
correction from 2.2, where a predicted 24 came back 29 because the count was
made from recollection.

| observed | means |
|---|---|
| **332** | all 10 executed on the Windows target |
| **322** | `blob.rs` compiled but its tests did not run — the module is `#[cfg_attr(not(android), allow(dead_code))]`, and a mistake there is exactly how D-3 logic passes a gate while asserting nothing |
| anything else | something compiled that should not have |

##### Gate deliberately SKIPPED for `c2c8105..bb01769` — with the delta stated

`git diff --name-only c2c8105..bb01769` is **five files, all `docs/`** (the
acceptance-coverage design, the guard, `check-mobile-build.sh`, the spec's
A1–A7 amendment, this ledger). **Zero `.rs`, `.js`, `.kt`, `.xml`** — no
compiled input, so run 21's 346/0/zero-warnings at `2305fc1` covers this tip
and the next code chunk's gate absorbs it.

Recorded with the delta attached, per the `dc082bf` precedent: **"we didn't
re-run the suite" and "the suite cannot see this change" leave identical
evidence unless the reason is written down.** Verified by steering
independently rather than asserted by the agent proposing the skip.

### 3.2 gate: **GREEN** (run 20, at `0354f6c`, protocol v2) — 332/0, exact

**332 passed / 0 failed, zero warnings.** PRE `0354f6c`/clean → POST
`0354f6c`/clean, all four provenance facts agreeing, so this is attributable
**to the commit**. All ten `blob::tests` observed **by name**, so the 322 branch
— compiled-but-not-run — is excluded by observation rather than by assumption.

Running total: **266 → … → 321 → 321 → 322 → 322 → 332.**

**A confounder pre-registered, and that is the new thing here.** A window of
machine load was disclosed to steering *before* the verdict landed (two Android
builds briefly overlapped the suite — see the operational note below). This
document already records that slow runs correlate with the
`cloud::rest`/`session` flake family, so the disclosure gave steering a
known cause to weigh a cloud-family failure against, rather than one invented
after the fact. No flake appeared, so it cost nothing — **which is the point**.
The predict-the-count convention pre-registers what we expect to see; this
pre-registers what could *contaminate* what we see, and the two are the same
discipline pointed at different things. Cheap when unnecessary, decisive when
not.

### ⚑ The general form: AN ABSENT RESULT IS NOT A NEGATIVE FINDING

Promoted to a heading because this proposition has now bitten twice in two
different instruments, and the second time it bit an agent **who had read the
first instance**. That is the evidence that lessons do not transfer across
instruments by being written down as incidents — the general form has to be
stated, with the incidents filed under it.

**The proposition:** a command that has not answered tells you nothing. Silence,
emptiness and absence are not evidence of a negative; they are evidence of
nothing at all. Before reading any empty result, establish that the producing
command *finished*.

| instrument | incident |
|---|---|
| `git status --porcelain` | A run that **times out** returns exit 124 with empty output, byte-identical to a clean tree. Demonstrated deliberately while testing the provenance sidecar. Only `porcelain_exit 0` licenses the word "clean". |
| a backgrounded pipeline | A long build piped through `tail -40` writes **nothing** until the pipe closes, so an empty output file means *still running*, not *produced nothing*. Read as a dead build; a second build was launched on that basis (below). |
| `grep … \| head` | A caller search for `ToolOutcome::is_err` was piped through `head`, which **cut the list at ten**. The three real callers sat below the cut, the visible portion was read as exhaustive, and the method was deleted as "zero callers anywhere". The compiler caught it in 80 seconds. |

All three are the same sentence with a different subject. The countermeasure is
also the same: **record the exit status of every command whose output you
interpret**, and treat "no output yet" — or "no output *here*" — as a question
rather than an answer.

**The third instance is the one that proves the heading was needed**, because
it happened *in the same session as the first two, while writing them up*. The
specific defeater is worth naming since it recurs: **`head` and `tail` are
truncation, and truncation is indistinguishable from absence.** For any query
whose answer licenses a *deletion*, count first (`wc -l`) or don't truncate —
a partial list read as complete is how correct-looking code gets removed.

#### The incident, recorded by its author

An empty task-output file plus a `pgrep` whose pattern did not match at that
instant led to the conclusion that the APK build had died. A second build was
started. It immediately printed:

```
Blocking waiting for file lock on build directory
```

— **exactly the signal this document already documents**: *a check blocked on a
cargo build-directory lock means an earlier build is still alive, not that a
lock leaked.* The entry earned its keep by being read from the inside.

Resolved by the ledger's own kill discipline — *a build is not stopped until
the COMPILER is stopped* — walking the process table to the actual `cargo` and
`rustc` workers rather than killing the wrapper, then verifying afterwards that
exactly one build script remained, that both compiler processes were parented
to it, and that the killed tree was fully reaped. Disclosed to steering as a
confounder before the gate verdict, per the paragraph above.

### ⚑ STRUCTURAL COUNTERMEASURES SURVIVE FAILURES THAT PROCEDURAL ONES DO NOT

The heading this track has been circling for three phases, now with the
evidence to state it: **a watcher creates the opportunity to report; a file
IS the report.** Two independent failures of the procedural form, one
resolution by the structural form.

| # | the procedural mechanism | how it failed |
|---|---|---|
| 1 (gen-4) | background waiters armed on a finished build | they **fired**, into a turn that never resumed. The APK was correct, measured and on disk, and sat unreported for a day; steering opened a status check assuming a silent death. |
| 2 (gen-6) | a watcher armed on the CP3 build | it **fired correctly**, capturing the tail and provenance with exit 0 — and its notification reached the agent only *after* steering had independently verified and ferried the artifact. |

Instance 2 is the sharper one precisely because **nothing malfunctioned**. The
watch was perfect; the gap sat in the resume. So "arming a watcher is not the
same as having reported" is not a claim about unreliable watchers — it holds
when the watcher does its job exactly as designed. Any countermeasure whose
final step is *an agent being present to speak* inherits that gap, and no
amount of care removes it.

**What held was neither the watcher nor the agent: the sidecar.**
`build-android-apk.sh` had already written HEAD, porcelain, both exit statuses
and the sha256 into a durable file before either party looked. So steering's
verification was a **check against the build's own record** rather than a
substitute for a missing report — and the two digests matched. That is the
first live payoff of the ratified policy that *scripts emit provenance, agents
do not report it*, arriving in exactly the scenario it was written for: the
agent unavailable, the evidence not.

The distinction to carry forward: a **procedural** countermeasure asks someone
to do the right thing at the right moment (report the digest, check the
porcelain, arm a watcher). A **structural** one makes the artifact exist
whether or not anyone shows up. This document's three standing mechanisms are
all structural for that reason — the provenance sidecar, the `[kernels]` and
`[bridge]` lines printed by the app itself, and the build computing its own
digest — and each replaced a procedure that had already failed once.

**Corollary, applied the same day:** the release-config audit's R8 items were a
Python snippet pasted into a checklist — procedural, and due to be retyped
correctly during a signing ceremony. They are now a step inside
`verify-apk.py`. Same conversion, made before the failure rather than after
it.

#### 📱 CP3 — what only the device can say

Predicted logcat, recorded before the run (`adb logcat -s RustStdoutStderr`, the
same channel as `[bridge]` and `[kernels]`):

| step | expected line |
|---|---|
| first launch, never signed in | `[secure-store] load_refresh_token: no blob stored` |
| sign in with **remember** | `[secure-store] save_refresh_token: ok (encrypted, written)` |
| **force-stop → relaunch** | `[secure-store] load_refresh_token: ok (blob decrypted, tag verified)` **and the app is still signed in** |
| sign out | `[secure-store] delete_refresh_token: ok (blob removed)` |
| relaunch after sign-out | `[secure-store] load_refresh_token: no blob stored` |

Acceptance, in priority order:

1. **Sign in → force-stop → relaunch → still signed in.** The point of the
   phase, and the first exercise of the `spawn_blocking` attach path above.
2. **Sign out → blobs purged.** `adb shell run-as com.cleophis.app ls
   no_backup/secure/` is empty.
3. Airplane mode → offline sign-in works against a stored verifier.
4. No `[bridge] FAILED` and no `[secure-store]` failure line on the happy path.

A mismatch is informative rather than disappointing: a `getAppClass` failure
means packaging (R8 is off in debug); a `NoSuchMethodError` means the
`@JvmStatic` contract; `no SecureStore key in AndroidKeyStore` on a *load* that
should have succeeded means the key was replaced between write and read.

**What CP3 still cannot say:** whether R8 leaves `SecureStore` intact in a
release build. Debug sets `isMinifyEnabled = false`, so a green checkpoint here
is compatible with a shipped build that signs the user out on every launch.
That is item **1b** of `docs/ops/release-config-audit.md` — the same
debug-proof/release-failure split as the bridge, now carrying a security
feature instead of a diagnostic.

### 📱 CP3 APK — built, verified, clean provenance both ends

```
sha256  a3fd92dddf1f0f61a19d715b37d310f566539ea271a50754d7e61458690efd24
size    355,151,042 bytes
built   from 0354f6c (3.2 + the §6 backup fix), debug-signed, arm64-v8a
copy    ~/cleophis-artifacts/cleophis-cp3-debug-0354f6c.apk
log     ~/cleophis-mobile-logs/apk-debug-20260729-163405.log
```

**Provenance, written by the build itself** (the sidecar policy, not an agent's
report):

| checkpoint | HEAD | porcelain | exit |
|---|---|---|---|
| PRE 16:34:05 | `0354f6c` | `<empty: tree clean>` | `porcelain_exit 0` |
| POST 17:16:35 | `0354f6c` | `<empty: tree clean>` | `porcelain_exit 0` |

All four facts agree → **attributable to the commit**. Worth noting *how* that
was achieved: the build was deliberately started while steering's gate freeze
was in force and all worktree edits were held until POST was written. Editing
docs mid-build would have dropped this to "live worktree near `0354f6c`", the
weaker form the 2.2b entry had to record and defend. The cost of the strong
form was latency and nothing else.

**Digest independently derived three times, all identical** — the build's own
sidecar, steering's verification, and this agent's re-hash of the **archive**.
Produced by `sha256sum` and never retyped.

**The third derivation is not ceremony, and the reasoning is the point:
steering verified the BUILD OUTPUT, the sidecar recorded the BUILD OUTPUT, and
what the founder installs is the ARCHIVE. Re-hashing the archive closes the gap
between the thing checked and the thing shipped.** That distinction was paid
for by the 2.2b entry — where the archive comparison happened a day late and
the window to make it had closed — and this is the first time it was applied
*prospectively* rather than reconstructed. Steering additionally re-hashed the
ferry copy in the founder's Downloads, so the chain **build output → archive →
founder copy** is unbroken end to end.

`verify-apk.py` **PASS**: 968 entries, Σ compressed **354,963,874** vs file
**355,151,042** = **0.05 % unaccounted** (no zipflinger orphan); `assets/` is
`tauri.conf.json` only; `arm64-v8a` only.

#### The check the standard script cannot do, on the commit that needed it most

`verify-apk.py` has nothing to say about whether Kotlin compiled and got
packaged. The bridge commit established this gap; **this is the first commit
where the answer is a security property**, since a missing `SecureStore` means
no credential can be read at all. Read out of the shipped archive's dex:

| symbol | result |
|---|---|
| `Lcom/cleophis/app/SecureStore;` | present, `classes8.dex` |
| `blobDir`, `encrypt`, `decrypt` | all present, `classes8.dex` |
| `Lcom/cleophis/app/NativeBridge;`, `describeDevice` | present — regression check that the `with_app_class` refactor did not drop the bridge |

This narrows what a CP3 failure can mean **before the device is touched**: a
`getAppClass` failure can no longer be "the class is not in the APK". It is
also a *debug* answer only — R8 is off here — so it says nothing about release,
which remains audit item 1b.

**Second time reading the dex has pre-emptively narrowed a device failure**
(the bridge APK was the first), which is what promoted it from a habit to a
script step. And the argument for scripting it is visible in this very run:
the `NativeBridge` + `describeDevice` regression check **rode along free**. A
human performing the habit checks what they are thinking about — here,
`SecureStore` — while a script checks everything it knows about, including the
class the current commit's refactor might have broken without anyone
suspecting it. That is the difference between a check that scales with
attention and one that scales with the codebase.

### 🔴 §6 privacy: the backup exclusions covered the empty set — FIXED

Found while deciding where 3.2's credential blobs should live, which is the
only reason anyone looked at these files again. **Not a live leak** —
`allowBackup="false"` is decisive and nothing has ever left the device — but
the second layer, whose entire purpose is to be a second layer, protected
nothing.

Both XMLs excluded `file`, `database`, `sharedpref`, `external`. **None of
those is where our data lives.** Read out of AOSP rather than from a doc page:

| fact | source |
|---|---|
| `"root"` → `ROOT_DIR`, and `ROOT_DIR = ceContext.getDataDir()` | `FullBackup.java`, `getDirectoryForCriteriaDomain` + the `*_DIR` assignments |
| `"file"` → `FILES_DIR = ceContext.getFilesDir()` — a **child** of the data dir, not the same directory | same |
| Tauri's `app_data_dir()` on Android **is** `activity.dataDir` | `tauri-2.11.5/src/path/android.rs:137` calls `getDataDir`; `mobile/android/.../PathPlugin.kt:64` resolves it to `activity.dataDir` for `SDK_INT >= N`. Our `minSdkVersion` is 24, so the `applicationInfo.dataDir` branch is unreachable |

So `cloud-cache.json`, `auth-cache/`, the conversation DB, `models/` and
`resources/` all sit **directly in the data-dir root** — the one domain the
rules did not name. The files read as correct in review, in the diff, and in
their own comments ("exclude ALL app-managed data"), and covered four
directories the app does not use.

**Fixed** by adding `<exclude domain="root" />` and `<exclude
domain="device_root" />` to `backup_rules.xml` and to both blocks of
`data_extraction_rules.xml`.

#### The fix was checked for the defect it fixes, before shipping

Steering's review raised exactly the right objection: Android's docs show
`path` on `<exclude>`, so a bare `<exclude domain="root" />` might be ignored
or rejected as malformed — **a rule that reads correct, reviews correct, diffs
correct, and covers nothing, in the very commit written to escape that
family.** Settled by reading the parser rather than a doc example:

- **`path` is optional.** `FullBackup.extractCanonicalFile` substitutes `""`
  for a null path, with the source comment *"Allow things like `<include
  domain="sharedpref"/>`"*, and `validateInnerTagContents` permits **up to 2**
  attributes on `<exclude>`. The bare form is well-formed by design.

  **The defect was TARGETING, never SYNTAX** — worth stating plainly, because
  the four pre-existing lines were *also* bare, so anyone who later greps for
  this fix and sees bare `<exclude>` on both sides of the diff could easily
  conclude the bare form was the bug and "fix" it by adding `path` attributes
  everywhere. The old rules parsed perfectly. They named four directories the
  app does not use. Nothing about their form was wrong and nothing about their
  form changed.
- **Excluding a directory prunes its whole subtree.**
  `BackupAgent.fullBackupFileTree` matches excludes by **exact canonical path**
  and, on a match, `continue`s *before* enqueueing that directory's children.
  So the first entry of the root-domain walk is the data dir itself, and
  nothing beneath it is ever scanned. Exact-match plus prune-on-match is what
  makes one line cover a tree — a fact that is invisible if you only read the
  matcher, since exact matching alone would suggest it covers one directory
  entry.
- The Android 12+ `dataExtractionRules` path shares the same `parseRules` and
  the same domain map, so the tokens mean the same thing in both files.

**Effectiveness remains unverified and is recorded as such.** Schema validity
is proved from source; whether the DB is actually absent from a backup set is
only observable with `bmgr` on a device. That is now a checklist item in
`docs/ops/release-config-audit.md`, phrased **"the conversation DB is ABSENT
FROM THE BACKUP SET"** rather than "the exclusion rules are present" — because
an item that checks for the rule's presence would inherit the exact bug it
exists to catch. Packaged-artifact confirmation rides with the next APK build,
per the standing rule that config claims are read from the artifact.

Landed as its own `fix(mobile-p1)` commit ahead of 3.2 (steering's call): a §6
privacy fix has nothing to do with crypto and deserves to be findable in the
log on its own.

#### 🔬 Ledger correction: the Linux host is blocked by Tauri, not by `keyring`

This document has recorded since Phase 1.1 that "`cargo test -p cleophis` fails
here before compiling any of our code: `keyring`'s `linux-native` feature pulls
`libdbus-sys`, whose build script requires system dbus development headers",
and drew the consequence that "moving `keyring` to
`[target.'cfg(not(target_os = "android"))'.dependencies]` will **not** fix this
— Linux is exactly where `linux-native` applies."

**The conclusion is right and the mechanism is wrong.** Re-run deliberately
after the move, to find out whether the desktop suite had become locally
runnable:

```
libdbus-sys v0.2.7
└── dbus v0.9.12
    └── tao v0.35.3
        └── tauri-runtime-wry v2.11.4
            └── tauri v2.11.5
                └── cleophis v0.1.0
```

`keyring`'s entire Linux subtree is `linux-keyutils` + `log` — no dbus anywhere
— and the crate declares **no default features** at all, so nothing it offers
could pull one. `libdbus-sys` comes from **Tauri's own Linux backend**, which is
unconditional. The host check still fails (`libdbus-sys` build script: "The
pkg-config command could not be found", explicit panic; the toolchain is
userspace-only by founder decision), so the Windows suite remains the desktop
gate.

The corrected statement is *stronger* than the one it replaces: **no change to
`keyring` could ever have unblocked local desktop testing**, because the
blocker was never `keyring`. The old wording quietly invited a future agent to
go after the keyring feature set — which is a **desktop-behaviour change**,
forbidden by this branch's hard constraint — in pursuit of a capability keyring
does not control.

**A new member of the collection, and steering named it better than I did: a
TRUE CLAIM WITH A FALSE MECHANISM.** Every failure shape recorded above is a
statement that was *wrong* — a percentage that fit any number, a run labelled
with the wrong commit, a config that merged instead of clearing. This one is a
statement that was **right**, and stayed right, for four phases. That is what
makes it more dangerous rather than less: **the conclusion keeps validating
it.** Every agent that tried `cargo test -p cleophis` on this host got the
predicted failure, and each success confirmed the sentence without ever
touching the half of it that was false. Nothing in the normal course of work
could have caught it — only asking "why, specifically?" of a claim that had
never once misled anyone.

The keyring correction earlier in this section is the same shape one level
down: "silent in-memory mock" and "a write that keeps nothing and reports
success" both explain every symptom anyone had observed, and only the second
predicts that `load` can **never** return a token. Two explanations, identical
track record, different futures — which is exactly the situation where a
ledger's stated *mechanism* starts doing real work, and where a wrong one is
invisible until someone builds on it.

The practical rule: **a conclusion that keeps being confirmed is not evidence
for the reason attached to it.** When a documented cause has never been
exercised — because nobody ever needed the conclusion to be false — it has
never been tested at all.

## Phase 5 — hardening, harnesses, CI

### 5.1 Thermal-throttle detection — DONE (commit `7550f23`, spec §8 / H6 / A7)

**Attribution, because this project records who produced evidence.** Gen-7
authored `engine_inproc/thermal.rs` and its 13 tests, then died to an
infrastructure failure with the work uncommitted. Gen-8 audited it, found two
gaps, closed both, and committed. The module doc is preserved verbatim — its
reasoning is the expensive part.

**The design.** The *decision* half lives where the tests run (D-3); the cfg
keeps only a clock and an `emit`. Inter-token gaps are measured at the **raw
sampler sink, not at `on_delta`** — `on_delta` is downstream of `ToolStream`,
which withholds possible tool syntax and releases it in bursts, so timing there
measures the suppressor's release schedule and reads a held-back stretch as a
stall and its flush as a speed-up. The baseline is per-*watch*, not per-turn,
and that is the whole design: a per-turn baseline re-measures against the
already-throttled rate, reports a healthy device forever, compiles cleanly and
passes every single-turn test. `the_baseline_survives_a_turn_boundary` pins it.
Prefill, idle and stalls are excluded structurally rather than by threshold.
Constants are biased toward false negatives on purpose — a notice that fires
wrongly teaches the user to ignore the one that fires rightly.

**Surfaced, not attempted:** `PowerManager.getCurrentThermalStatus()` would
corroborate the signal, but needs API 29+, a new Kotlin class over the JNI
bridge and a device checkpoint — and on mid-range hardware, precisely the floor
devices this matters on, OEMs frequently leave the thermal HAL unwired so it
returns `NONE` on a phone that is visibly throttling. It could confirm this
signal; it could not replace it.

#### 🔬 A predicted-zero-warnings gate that came back with one, and a real bug behind it

Gen-8 predicted **exit 0, zero warnings** for the inherited tree and got exit 0
with **one**:

```
warning: method `throttled` is never used
   --> src-tauri/src/engine_inproc/thermal.rs:221:19
```

**The D-3 amendment caught it, and the novelty is that it caught it
prospectively.** 1.3/1.4 found a dead method and a dead constructor
*retrospectively*, when bare allows were tightened to cfg'd ones. Here a module
authored *with* the canonical placement caught its own dead method at the moment
of authoring, because the allow is `cfg_attr(desktop, …)` and the dead-code
check therefore stayed live on the platform the code actually ships to. A bare
allow would have hidden this in exactly the way the `check-mobile-build.sh` D-3
audit exists to prevent.

And the warning was not cosmetic. `throttled()`'s own doc promised a caller —
"lets the caller re-assert state to a webview that reloaded" — that nobody
built, which is the asserted-but-unbuilt disease in miniature, inside the very
feature D-6 was built to police. Behind it sat a genuine silent bug: the watch
emits **only on transitions** and lives on the inference thread, which
**outlives the webview**. If the WebView renderer is killed under memory
pressure, the frontend loses `state.thermal` while the watch keeps
`notified = true`, and the onset branch's `!self.notified` guard means **no
further onset can ever be emitted** — the notice is gone for the rest of a hot
session. Rotation is already covered (`configChanges` includes
`orientation|screenSize`), so the live path is renderer death under memory
pressure: ordinary on a 4 GB floor device running a local model, and
*correlated with the very thermal load the feature detects*. It would also
silently void the A7 soak run — a founder device session, the scarcest resource
on this track — by recording a false negative.

Closed with `chat_thermal_state`, a boot-time re-assert mirroring the
`ChatCancels` managed-state precedent (D-1: shared invoke surface, desktop
refuses). `throttled()` is now `cfg(test)`: the re-assert publishes the whole
`ThermalNotice`, because the rates are what a bug report and the soak run need,
and a bare bit would throw them away. A re-assert deliberately arrives
**non-prominent** — backdated by `THERMAL_PROMINENT_MS` so it lands in the pill
rather than re-raising the full-width row for a fact the user already read.

**Verification** (gen-8, all re-run *after* both completions):

| check | result |
|---|---|
| `cargo ndk -t arm64-v8a -P 24 check -p cleophis --all-targets` | exit 0, **zero** warnings (unanchored `grep -i warning`) |
| `npm test` | **81/81**, 0 fail (76 baseline + 5 thermal) |
| `thermal.rs` | 13 `#[test]`, which run on the **desktop** target by design |

Desktop-suite prediction for the next gate: **346 → 359**. Discriminating: 359
means the 13 tests executed on the target; **346 means the module was cfg'd out
of the desktop build and D-3's entire purpose was defeated** — the cfg-stripped
test-module trap this ledger already names once; any other number is
unaccounted for.

**A tooling instance of "an absent result is not a negative finding."**
Gen-8's first `cargo ndk` invocation returned **exit 1, "Could not find any
NDK"** — an environment failure that looks nothing like a code verdict but sits
in the same slot in a transcript. The documented env block from
`mobile-dev-setup.md` is now saved at
`/home/penguinzyue/cleophis-mobile-logs/mobile-env.sh` so the trap costs one
round-trip rather than one per generation.

### ⚑ 5.1 — TWO NOTIFICATION GAPS, recorded as distinct items (steering ruling)

The brief said `POST_NOTIFICATIONS` was "already in the manifest". True, and
**insufficient**: `targetSdk` is 36, and *nothing in this app requests the
permission at runtime* — there is no `requestPermissions` call anywhere. On
Android 13+ it is therefore never granted. Steering's ruling was to skip both
the download notification and the permission request this phase, and to record
the consequence as **two items, because they have different owners and
different urgency.**

**(a) The download-complete notification is blocked on a runtime grant.**
Optional per spec, genuinely blocked, cheap to note. It ships as **one unit
with the permission ask**, and deliberately not before: steering rejected the
tempting split (ask at first download now, deliver the notification later) on
the grounds that **a permission request is a promise** — asking and then not
redeeming spends the user's trust on a prompt whose payoff never arrives.
*Ask and redeem together, or do not ask.*

**(b) 🔴 The foreground service's notification is invisible without the grant,
and that is a PRODUCT finding, not a plumbing note.** This app's whole thesis
is that you can see what it is doing. Android's persistent foreground-service
notification is the platform's own mechanism for exactly that — and on every
device we ship to it is suppressed by default until someone asks. **So today
the app runs a foreground service the user cannot observe.**

Not urgent: it runs only during generation, and the service works regardless
(the notification is how the platform makes the service *visible*, not why it
*works* — the survivable-degradation note in `InferenceService.kt` is correct
and stays). But *"a privacy-first product with an invisible background
service"* is a sentence that should never become true by accident. When the
permission work lands, **(b) is the stronger justification for the ask, not the
download toast.**

### ⚑ A7 THERMAL SOAK — the founder ask, with BOTH READINGS PRE-REGISTERED

The problem, and the reason this needs a design rather than a caution: **the
detector is deliberately biased toward false negatives**, so a soak that
produces no notice is a *possible correct result*. Run naively, the founder
spends twenty minutes on a hot phone and we cannot distinguish "working, and
correctly quiet" from "broken, and silent" — the outcome is uninterpretable and
the scarcest resource on this track is spent for nothing.

Steering's split, which fixes it by making the two unknowns **fail
differently**:

**Question 1 — does the notice PIPELINE work? (cold phone, five seconds)**

A debug-only affordance injects a synthetic slowdown into the detector's input.
If the notice appears, then detection → event → UI copy is proven end to end,
on any device, with no heat required. This converts one of the two unknowns
into a check that can be run before the soak even starts, and it is worth more
than any amount of soak transcript because its outcome is unambiguous in both
directions.

⚠ **Scope of that evidence, stated so it is not over-read:** a synthetic
injection proves the *decision and presentation* path. It does **not** prove
that `on_token` is genuinely called once per token at the raw sampler sink —
that wiring is only exercised by real generation. Question 2 is what covers it.

**Question 2 — does REAL throttling get detected? (hot phone, ~20 minutes)**

Only a hot phone answers this, and it is interpretable **only if the transcript
carries the input signal alongside the decision.** So: log raw tokens/sec
continuously through the soak. The readings, registered *before* the run:

| observation | verdict |
|---|---|
| tok/s visibly collapses **and** no notice | **BROKEN**, provably |
| tok/s stays flat **and** no notice | **correctly quiet**, provably |
| notice fires, rates in the payload match the logged collapse | **working** |
| notice fires while logged tok/s is flat | **false positive** — worse than silence, since it teaches the user to ignore the real one |
| silence with **no** trace | the uninterpretable outcome — *this is what the logging exists to eliminate* |

The general rule this is an instance of, and the reason it belongs in the
ledger rather than only in the founder ask: **a test whose negative result is
also its expected result must carry a trace of its input, or it cannot be read
at all.** Pre-registering both readings is what converts a soak from "let us
see what happens" into a measurement.

### ⚑ THE D-6 GUARD'S CONTROL PAIR IS COMPLETE — and completing it exposed a fourth bug

The pair steering asked for is the point of the exercise: a guard demonstrated
failing *and* passing is a proven instrument; one demonstrated only failing is
a plausible one. Completing it found the guard's **fourth self-inflicted bug**,
in the same family as the first three.

**What happened, in order:**

| # | state | A7 | note |
|---|---|---|---|
| 1 | gen-6, original guard, no thermal code | **FAIL** | the banked negative control |
| 2 | gen-8, original guard, thermal work **uncommitted** | **ok** | ⚠ the finding |
| 3 | gen-8 splits A7's regex into two required patterns | — | guard tightened |
| 4 | gen-8, tightened guard, detector still untracked | **FAIL** | *fresh* negative control, HEAD `5b17981` |
| 5 | gen-8 commits `7550f23` | — | tree clean, `porcelain_exit 0` |
| 6 | gen-8, **byte-identical guard** to step 4 | **ok** | positive control, HEAD `7550f23` |

**Step 2 is the bug.** A7's entry was `[r'fn +(detect_)?thermal_|"thermal-notice"']`
— *one* pattern with a top-level alternation, where A1, A4 and A5 all use
two-element lists that the guard requires **all** of. So either half satisfied
A7 alone. Measured with the guard's own file-selection logic rather than a
re-implementation:

```
detector definition   fn +(detect_)?thermal_    matches: NONE
event name            "thermal-notice"          matches: ['src-tauri/src/engine_inproc.rs']
thermal.rs tracked?   NO -- untracked, invisible to the guard
```

**A bare `emit("thermal-notice", …)` with no detector at all turned A7 green.**

**Why this one is the nastiest for the instrument, though not for the code.**
Runs 1–3 produced *wrong verdicts* — prose, a TODO, and the manifest satisfying
themselves. This one produces a **right verdict for the wrong reason**: A7 is
genuinely implemented, so nothing looks amiss, and the pair would have been
banked as "A7 flipped when thermal detection landed" when the flip was **not
attributable to the detector and would have happened with it deleted**. A
contaminated positive control is worse than a missing one, because it retires
the question.

**How it survived: the two documents disagreed.**
`docs/ops/acceptance-coverage.md:71` banks the failure as "nothing **emits** a
throttle notice" — an OR reading. This ledger's own second-order finding states
the contract as `fn detect_thermal_*` **and** `"thermal-notice"`. The manifest
implemented the weaker of the two, and nobody compared them.

**The fix and why the pair is now clean.** Split into
`[r'fn +(detect_)?thermal_', r'"thermal-notice"']` — a strict tightening that
cannot turn anything green that was red. Step 4's failure names **only** the
detector pattern as missing while `"thermal-notice"` still matches, which is the
direct proof the AND is load-bearing: the old OR would have passed that exact
state. Steps 4 and 6 differ by **one `git add`** and nothing else — the guard
was byte-identical, confirmed by `git diff` returning empty.

**The general form, which outlives this script:** *a check can be satisfiable
by less than the requirement it names, and the symptom is not a red that should
be green — it is a green that is right for the wrong reason.* The only way to
see it is to ask **which** clause carried the verdict, not whether the verdict
was correct. Run 3's lesson was that the moment a check becomes self-satisfying
may be a `git add` rather than an edit; run 4's is that **a passing check still
owes you an attribution.**

## Conventions

- **⚑ A PASSING CHECK STILL OWES YOU AN ATTRIBUTION.** Ask *which clause
  carried the verdict*, not merely whether the verdict was right. A green can
  be correct for the wrong reason, and that is strictly harder to catch than a
  wrong verdict, because nothing looks amiss — the item really is implemented,
  the check really does pass, and the causal link between them is the only
  thing missing.

  This applies to **every** green on this branch, gate runs included. The
  instance that produced it: A7 flipped to `ok` on the strength of an `emit`
  call while the entire detector was invisible to the guard, and the pair would
  have been banked as "A7 flipped when thermal detection landed" when it would
  have flipped identically with the detector deleted. The countermeasure is
  cheap — run the check against a state where only the *suspected* cause is
  absent, and confirm it names that cause specifically.

- **⚑ A CHECK'S SILENCE IS ONLY EVIDENCE WITHIN ITS COMPETENCE — including
  across languages.** `cargo ndk check` returned exit 0 with zero warnings on a
  tree whose Android resources would not compile at all (`--` inside an XML
  comment; aapt2 rejects the whole file). Rust silence says nothing about
  Kotlin, resources, or the manifest. **Any commit touching `gen/android`
  needs a gradle compile — `./gradlew :app:compileUniversalDebugKotlin` — not
  just a cargo check.**

  Same episode, second lesson: gradle was run through `| tail -40`, so the
  harness reported the task as **exit 0**, which was `tail`'s status while the
  build had failed. Only the explicitly teed `EXIT=${PIPESTATUS[0]}` showed
  `1`. **Piping a build through anything launders its exit status.**

- **⚑ USE `mobile-tools/run-logged.sh` FOR LONG COMMANDS — this one is a TOOL,
  not a rule, and the reason is the point.** The laundered-exit-status family
  reached **six instances in about a day**: a lost gate log; a buffering `tail`
  whose empty output read as a stalled build; a truncating `head`; `git status
  --porcelain` returning exit 124 with empty output, byte-identical to a clean
  tree; a swallowed guard exit; and gradle through `tail` reporting **exit 0 on
  a failed build**.

  Every one was diagnosed correctly, written up well, and generalised into a
  rule — **and the next one still arrived.** That is this project's own
  repeatedly-proven finding turned on itself: *a rule that must be remembered
  at the moment of use will not be*, because the moment of use is exactly when
  attention is on something else. Six write-ups did not prevent a seventh; a
  wrapper does, by making the correct behaviour the default rather than a thing
  to recall.

  `run-logged.sh <label> -- <command...>` always tees to
  `cleophis-mobile-logs/`, always exits with `${PIPESTATUS[0]}` rather than the
  pipeline's, emits PRE/POST provenance in `build-android-apk.sh`'s proven
  shape (including that only `porcelain_exit 0` licenses "clean"), merges
  stderr so a failed build's errors are actually in the log, and prints an
  unanchored warning count as a prompt to look. **Demonstrated capable of
  failing before being believed:** a command exiting 7 propagates 7 and prints
  `FAILED`; a command exiting 0 propagates 0. Self-testing it also caught a
  real bug in itself — the phase argument leaking into the recorded command
  line, fixed with a `shift`.

- **⚑ A TOOL BUILT TO CATCH A FAILURE CLASS IS A PRIME CANDIDATE FOR THAT
  FAILURE CLASS — and the only thing that has ever found it is running it
  against a KNOWN-BAD INPUT.** Three distinct instruments on this branch have
  now been defeated by the class they were built to catch:

  | instrument | how it failed at its own job |
  |---|---|
  | `acceptance-coverage.py` | four times — satisfied by prose, by a TODO, by its own source, and by an OR where a stub sufficed |
  | `verify-apk.py` | reported `ok .describe` directly beneath `FAIL … class MISSING` — a green under a red |
  | `run-logged.sh` | a tool built to stop mis-attributed evidence **mis-attributed evidence**, recording `command PRE bash -c …` because the phase argument was never shifted |

  **Four negative controls, four finds, zero found by review.** Every one of
  these was invisible to reading the source — including reading it carefully,
  including by the person who had just written it — and every one appeared the
  moment the tool was pointed at input whose correct answer was already known.
  Treat "I have built a checker" as the *start* of the verification, never the
  end of it.

- **⚑ A HANDOFF THAT HANDS OVER A RULE HANDS OVER PRECISELY THE THING THAT
  ALREADY FAILED.** Recorded verbatim because it generalises past its incident
  to every handoff this project will write. The gen-8 handoff originally warned
  "never pipe a build through `tail`" — which is the identical instruction that
  had already failed **six times** in the hands of people who knew it. Replaced
  with a pointer to `run-logged.sh`. When writing a handoff, for each warning
  ask: *is there a tool that makes this unnecessary?* If yes, hand over the
  tool. If no, that absence is the more useful thing to report.

- **The cheap instances of a failure family are the evidence the discipline has
  become reflexive.** Seventh instance of absent-result, and the first caught
  *before* it became a claim rather than after: while self-testing
  `run-logged.sh`, a `grep` for its warning-count line returned nothing and the
  obvious conclusion was that the line never printed. The raw file showed it
  present — the grep pattern had an apostrophe mismatch. Cost: one command.
  Recorded *because* it cost nothing; a family whose instances are getting
  cheaper and earlier is being managed rather than merely survived.

- **⚑ FREEZE PROTOCOL — now THREE rules, because the third incident had a
  different cause from the first two.** Recorded by gen-8, in its own words, at
  steering's request.

  **The two established halves.** (1) *The party under freeze holds
  unconditionally* — an approval or an obviously-good idea arriving mid-freeze
  takes effect at **thaw**, never on arrival. (2) **Steering's, added here:**
  *a freeze message carries the freeze and nothing actionable.* Rulings wait
  for the thaw message. Both prior breaches happened because work arrived
  inside the message that ordered the hold, so the second rule removes the
  temptation the first rule requires resisting.

  **What actually happened this time, stated precisely, because an inaccurate
  incident record is worse than none.** The gate ran at `616ee1b` and its POST
  provenance was **not clean**: `M verification-milestone-mobile-p1.md`,
  `?? run-logged.sh`. Those edits are real and they are mine. But **the freeze
  message and the thaw message were delivered to me in the same batch, at the
  start of the turn *after* the edits** — commits at 12:34:04 and 12:35:37
  against a gate that began at 616ee1b (12:29:04). No freeze was in effect from
  my side at any point while I was editing; I did not receive one and hold it,
  and the previous freeze I did receive I held correctly and reported holding.

  **Resolved, and it was not even a race — steering supplied the dispositive
  evidence from its own side.** The gate was launched *first*, and the "FREEZE
  FOR GATE RUN" message was sent *afterwards, in the same turn*: the tool call
  starting `cargo test` preceded the tool call sending the freeze. So the
  freeze provably post-dates the gate start, and there was necessarily a window
  in which the run was live and **no freeze existed anywhere**. Not a message
  in flight — a message not yet written.

  ⚑ **How this was settled is the part worth keeping.** The agent's account was
  not accepted on trust, and it was not overridden on rank either. Steering
  went looking in its *own* transcript for evidence that could disconfirm its
  own claim, found it, and produced it. **A disagreement about what happened is
  settled by whichever side holds the records, and that side has to be willing
  to look.** The cheap failure here would have been the agent conceding to be
  agreeable — the record would then have been wrong, permanently, in the
  direction that flatters whoever spoke last.

  It exposes a gap neither existing rule covers: *a freeze cannot bind work
  that started before it existed, and **a broadcast is not a barrier**.*

  **(3) THE FIX — a freeze needs a HANDSHAKE, not an announcement.** Steering
  must not start a gate run until the frozen party has **acknowledged** the
  freeze. Then "was the tree frozen?" is answerable from the record instead of
  inferred from timing, an unacknowledged freeze is visibly unfrozen rather
  than silently raced, and the POST-provenance surprise cannot recur. This is
  the same move as every other fix on this branch: convert a rule that depends
  on timing nobody controls into a mechanism that fails loudly.

  **Why the verdict still stood, and why that is luck rather than method.** The
  delta was a ledger edit and an untracked script — no compiled input — so
  359/0 remains attributable to `616ee1b`'s code. **"The delta happened to be
  inert" is not a property anyone could have known in advance**, which is
  precisely why the handshake is worth more than the care of either party.

  **⚑ THE COMPLETED SET, and the meta-lesson in how long it took to find.**
  The three rules govern three different intervals, and *each was discovered by
  being violated*:

  | rule | governs | found by |
  |---|---|---|
  | 1 — hold unconditionally | the party who **has** the freeze | gen-4's breach |
  | 2 — a freeze message carries nothing actionable | what the freezer **puts in** it | the second breach |
  | 3 — no gate without acknowledgement | the interval **before it arrives** | this incident |

  Each hole was invisible while the other two held, and each looked like the
  whole problem at the time it was found. Hence:

  > **A protocol's gaps are found one incident at a time, and each looks like
  > the whole problem until the next one.**

  The practical consequence is not "write better protocols up front" — that has
  been tried three times here. It is to expect a fourth gap, and to treat any
  protocol that has never been violated as **untested rather than sound.**

- **⚑ Before writing tests in a test-writing phase, take the ASSERTION
  INVENTORY** (decision D-6, part 3). List every assertion the phase will make
  and name the `file:line` that satisfies it, or mark it **MISSING**. One page,
  into this ledger, *before* the first test is written.

  This is the manual pass that caught all four "asserted but unbuilt"
  instances, and it is **not** made redundant by the
  `acceptance-coverage.py` guard. The guard catches **absence**; the inventory
  catches **inert presence** — a symbol that exists and is wired to nothing.
  `#chatStatusPill` is the standing example: it existed for the entire project,
  hard-coded in `index.html`, never written by any code, and any
  symbol-existence check would have passed it green while the founder could not
  locate the engine state at all. A grep cannot tell "defined" from "reachable
  and doing something"; a person reading the call chain can.

  **Phase 5 inventory (taken 2026-07-29, before 5.1/5.2/5.3):**

  | acceptance | satisfied by | state |
  |---|---|---|
  | A1 determinism CI | *nothing yet* — 5.3's workflow must invoke `cargo test -p kpack-embed --features real -- --ignored` | **MISSING** |
  | A2 airplane suite | *nothing yet* — 5.2 harness | **MISSING** |
  | A3 Stage-5 probes in-app | *prose only* (spec §11, `adapter-distribution-design.md` §8) — 5.2 must make them runnable | **MISSING** |
  | A4 kill-restore | `convstore.rs` `checkpoint_partial` / `finalize_partial` / `discard_partial`; `chat_cmds.rs:262,320,332,335` | present |
  | A5 backup-leak | `backup_rules.xml` + `data_extraction_rules.xml` `root`/`device_root`; test procedure in `release-config-audit.md` §4 | rule present, **test MISSING** |
  | A6 update path | Phase-4 gated | **not checkable** |
  | A7 thermal soak | *nothing* — 5.1 must create `fn detect_thermal_*` and emit `"thermal-notice"` | **MISSING** |

  **Successor row (2026-07-30, after 5.1).** The snapshot above is deliberately
  left as taken — an inventory that silently updates itself is a snapshot that
  lies about when it was made, which is this ledger's own perishable-evidence
  rule applied to its own tables. What changed since:

  | acceptance | satisfied by | state |
  |---|---|---|
  | A7 thermal soak | `engine_inproc/thermal.rs` `detect_thermal_collapse`/`thermal_recovered`; `"thermal-notice"` emitted in `engine_inproc.rs`; notice copy in `engine-state.js` | **built** at `7550f23`, desktop-verified 359/0 |
  | A3 Stage-5 probes | still *nothing in the app*. `crates/kpack-engine/examples/probe.rs` defines the four probes but is a CLI, is not in-app, has **no pass/fail logic**, and is outside the guard's SEARCH roots | **MISSING** — and the guard now asks for `fn probe_verdict`, not the prompt text |

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
- **⚑ Scripts emit provenance; agents do not report it.** Ratified policy. Any
  script producing gate or build evidence writes a `.provenance` sidecar beside
  its teed log — HEAD and porcelain at start *and* end, timestamps, and the
  output digest. Implemented in `build-android-apk.sh`. The case it closes is
  not "nobody looked" but **"somebody looked, and the looking left no trace a
  stranger can audit"**: the 2.2b PRE checkpoint was taken, was correct, and
  survived only in a session transcript. An agent's recollection is the class of
  assurance protocol v2 stopped accepting, and the fix is not to trust the
  transcript more — it is to make the check write something down. Same instinct
  as putting the `[kernels]` line inside the app rather than in a build log.
- **Record the exit status of every command whose output you interpret**, and
  never read an empty result as a negative finding without it. `git status
  --porcelain` on this drvfs worktree can take minutes; a run that times out
  returns **exit 124 with empty output**, which is byte-identical to a clean
  tree. Demonstrated deliberately while testing the sidecar rather than
  asserted: `porcelain_exit=124 output_len=0`. Only `porcelain_exit 0` licenses
  the word "clean". This mistake has already been made once on this track and
  had to be walked back.
- **A build is not stopped until the COMPILER is stopped**, and *a check
  blocked on a cargo build-directory lock means an earlier build is still
  alive, not that a lock leaked.* Killing the wrapper script, the `tauri
  android build` node process and the gradle daemon left `cargo build --package
  cleophis …` running for six further minutes, holding the lock and **compiling
  a tree that was being edited underneath it**. That is the contaminated-gate-run
  failure (see the dead-code episode under Phase 0) arriving from a direction
  protocol v2 did not cover: there, a gate run was mislabelled; here, a *killed*
  build silently kept going. Kill by walking the process table for
  `cargo`/`rustc` against this manifest, not by killing what launched them.
- **Some evidence is perishable; record when it was taken, not as though it
  still holds.** The 2.2b entry compares the archived APK against its build
  output and finds them identical — a comparison that became impossible hours
  later, because the next build pre-deletes the prior output (the zipflinger
  orphan fix). The claim is true and was verified; it is simply no longer
  re-derivable. A ledger's job for such facts is to timestamp them and say so
  plainly, so a later reader who finds an empty directory concludes "that window
  closed" rather than "this document is wrong."

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

#### ⚑ AMENDMENT (ratified, item 3 chunk) — the `allow` is PART of the extraction

**Every D-3 extraction leaves a function whose production caller lives on the
other platform.** That is not an occasional side effect; it is what the rule
*does* — it moves logic to where the tests run, which by construction is not
where the caller is. So the narrow `cfg_attr(..., allow(dead_code))` **with a
stated platform fact is a required step of the extraction, not an afterthought
to it.**

**The mechanical form, answerable by grep at the moment the code is written:**

> **Who calls this on the platform where the tests run? If the answer is "only
> the tests", it needs the allow.**

**Why the rule needed amending rather than restating.** `net_state::parse`
reached a gate with a dead-code warning after this same pattern had been
applied *twice in the same session* (`blob.rs`, `pure.rs`). The author's
account: *"I didn't fail to know it, I failed to notice it applied again."*
Knowing the rule was never the missing ingredient — which is why the fix is a
derived checklist item that fires at the point of application, and not a more
emphatic statement of the rule.

**And the audit that explains how it was missable.** The same rule turned out
to be encoded **three different ways** across six modules:

| module | form |
|---|---|
| `engine_inproc/serve.rs` | `#![cfg_attr(desktop, allow(dead_code))]` — inner, cfg'd |
| `engine_inproc/tools.rs` | `#![allow(dead_code)]` — inner, **bare** |
| `engine_inproc/tool_loop.rs` | `#![allow(dead_code)]` — inner, **bare** |
| `cloud/secure_store/blob.rs` | outer, on the `mod` declaration, cfg'd |
| `mobile_native/pure.rs` | outer, on the `mod` declaration, cfg'd |
| `net_state.rs` | **nothing** |

The earlier extractions did not get lucky — each solved this, in a *different
place*. **A rule with three encodings cannot be checked by looking**: answering
"does every D-3 module carry it?" means inspecting two locations per module and
knowing that bare and cfg'd variants both count. Each new extraction re-derives
the placement, and one eventually re-derives *nowhere*.

**Canonical form, going forward: on the `mod` declaration, cfg'd, beside the
D-3 rationale comment that already lives there.**

One nuance that must not be flattened: **the canonical thing is the placement
and the principle, not a single literal cfg.** The allow must *mirror its
module's production caller* — `engine_serve`/`engine_tools`/`engine_tool_loop`
are consumed by `engine_inproc`, which is `cfg(mobile)`, so they take
`cfg_attr(desktop, …)`; `blob`/`pure`/`net_state` are consumed from
`cfg(target_os = "android")` blocks, so they take
`cfg_attr(not(target_os = "android"), …)`. Forcing one literal cfg on all six
would put the suppression on the wrong platform for half of them.

#### 🔴 The bare allows were hiding real dead code on the platform that SHIPS

Tightening `tools.rs` and `tool_loop.rs` from bare `#![allow(dead_code)]` to
the cfg'd form was expected to be a tidy-up. It was not.

**A bare allow switches the check off on *both* platforms — including Android,
where those modules actually run, and which is exactly what the 5.3
`mobile-check` CI job exists to police.** So two of the six D-3 modules had
been running with dead-code checking disabled on the platform that matters,
since they were written in 1.3–1.4. The first aarch64 run after tightening
surfaced two genuinely dead items that had been invisible for the whole phase:

| item | verdict on the merits |
|---|---|
| `ToolOutcome::is_err` (`tools.rs`) | **test-only** — its three callers are all closed-registry assertions in this module's own tests. Production never asks: `tool_loop` feeds `as_content()` back to the model either way, which *is* the design — a tool error is a message the model self-corrects from, not a branch Rust takes. Now `#[cfg(test)]`. |
| `LoopMessage::user` (`tool_loop.rs`) | **test-only** — production converts each `WireMessage` with a struct literal (`chat_cmds.rs:162,167`) because the role comes from the wire, not the call site. Its siblings `assistant` and `tool` have production callers, which is why only this one was dead. Now `#[cfg(test)]`. |

Both **gated, not suppressed** — the standing rule. And note the shape of what
was hidden: not bugs, but *two items whose real scope was narrower than their
declaration claimed*. That is the same family as the dead `trim_matches('.')`
found hours earlier: code that reads as legitimate API and is reachable by
nothing that ships.

This is the blind spot 5.3 would have **inherited silently**. A CI job that
runs `cargo ndk check` against a tree containing bare allows reports clean and
means nothing for those modules.

#### ⚑ The audit, in `check-mobile-build.sh` — convention needs a mechanism

Canonical placement is the convention; this is the mechanism, and **both were
required** — the placement alone would have been re-derived away again, and the
check alone would not have said what the right shape was.

```sh
grep -rn '^#!\[allow(dead_code)\]' src-tauri/src   # must return nothing
```

Step 0 of `check-mobile-build.sh`, the script a CI job will call. **Demonstrated
capable of failing**, per the standing requirement for believing a green: run
against the tree at `2305fc1` it finds both bare allows.

It deliberately checks for the **bare** form rather than for the presence of an
allow. Absence-of-a-bare-allow is mechanically checkable; "every D-3 module has
a *correctly cfg'd* allow" is not, since the correct cfg differs per module. So
the check catches the failure that is uniform and leaves the judgement that is
not — which is the honest division between what a grep can assert and what a
reviewer must.

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

### D-6 (FOUNDER-APPROVED) — Acceptance criteria get IDs, and a guard checks that something implements them

**Decision:** §11's acceptance criteria are named **A1–A7** in the spec, and
`check-mobile-build.sh` fails the build when an acceptance item names no
implementation. Design: `docs/ops/acceptance-coverage.md`. Before writing tests
in a test-writing phase, a **phase-entry assertion inventory** lists every
assertion with the `file:line` that satisfies it, or MISSING.

**Rejected alternative:** keep relying on plan review, which had caught all
four instances.

**Rationale.** It caught them, and it is the mechanism this project has
otherwise stopped trusting. The ledger records rules failing three times in one
day *on rules the agent could recite* — knowing a rule was never the missing
ingredient. Every other recurring failure here has been converted into a check;
this was the last one enforced by attention.

The class is **D-4**: a requirement lives in the spec, the code, and a test —
three homes, no canonical link. What made *this* class uncatchable is that
**§11's bullets were the only requirement set in the spec with no identifiers**
(hazards are H1–H15, decisions are D-n and cited from code). A mapping cannot
be checked mechanically when one side has no name, so the IDs are the enabling
change and everything else follows.

**Evidence.** Demonstrated capable of failing on its first run, before 5.1
exists: A7 **ASSERTED-BUT-UNBUILT**, exit 1, with A1/A2/A3 also correctly red
and A6 reported **NOT-CHECKED** under its own token rather than as a pass.

**And it caught two of its own bugs while being built**, which is the sharpest
evidence for the rule that a check must be seen to fail. Run 1 reported A7
*implemented* because it searched markdown and matched the ledger's own prose
about the requirement — the spec satisfying itself. Run 2 still passed A7,
matching unrelated rate-limiting (`throttl`) and comments saying the notice
"lands in Phase 5.1" — **a TODO counting as the feature.** Run 3 — worst of the three — the guard's own
`ACCEPTANCE` dict lists every pattern as a literal, so **the manifest satisfied
itself**, reporting A1/A2/A7 implemented on the strength of its own source; and
it did so **only once the file was committed**, since `git ls-files` skips
untracked files. The demonstration banked and reported before that commit was
taken in a state the guard would never be in again.

All fixed: markdown is never evidence, every pattern is definition-shaped
(`fn name(`, `"event-name"`), and the guard excludes its own resolved path. The
run-2 fix turned the manifest into a **contract**: each pattern names the symbol
the implementing phase must create. The run-3 lesson generalises past this
script — **a check whose own source lives inside its search space is
self-satisfying, and the moment that becomes true may be a `git add` rather
than an edit.**

**The general form of the fix, which outlives this script: EVIDENCE MUST BE
DEFINITION-SHAPED.** A pattern a *promise* can satisfy is not a check. Both
false positives produced the word `ok` beside the item and both would have
passed review — and had the guard been run once, seen green, and shipped, **a
guard against unbuilt requirements would itself have been satisfied by unbuilt
requirements.** Nothing else on this branch demonstrates the
demonstrated-capable-of-failing rule as completely.

**Second-order finding — the manifest became a CONTRACT, and that inverts the
usual failure.** Definition-shaped patterns do not merely resist false
positives; they state each requirement's **acceptance signature in advance**.
5.1 knows before writing a line that `fn detect_thermal_*` and
`"thermal-notice"` are what turn A7 green. The ordinary order is: build, write
a test, discover an absence afterwards. Here the requirement declares what
would satisfy it *first*, so the implementing phase is handed a target rather
than a verdict. That was not designed in — it fell out of fixing run 2 — and it
is the most reusable thing the guard produced.

**The blind spot, stated so it is designed around rather than into.** A symbol
can exist and be wired to nothing — `#chatStatusPill` existed for the whole
project, hard-coded and never written by any code, and a symbol-existence check
would have passed it green while the founder could not find the engine state at
all. **The guard catches absence; only the inventory catches inert presence.**
Part 3 is not optional, and this script's existence must not be allowed to
argue it away.

**The same blind spot, second confirmed instance — now in the test
infrastructure layer.** The guard checks that a runner **exists**, and
**existence is not execution**. A1's suite (`crates/kpack-embed/tests/
build_determinism.rs`) is already written, `#[ignore]`d, and skips with a
message when the GGUF is absent — so a CI step that invokes it would satisfy
A1's patterns while never running a single assertion. Stated at full strength
because it is this instrument turned on itself:

> **The acceptance item that warns about unexecuted suites can itself be
> satisfied by an unexecuted suite.**

This is `#chatStatusPill` (defined vs. reachable-and-doing-something) arriving
one layer up, which makes it a **boundary of the instrument** rather than a new
surprise each time it appears. The consequence for 5.2: A1's runner must **fail
loudly when the model is absent, never skip**, so a determinism gate that has
never run is a red build rather than a green one. Steering's ruling is to let
A1 sit *red-because-unexecuted* rather than *green-because-a-runner-exists* —
a red that accurately says "this gate has never run" is worth more than either
alternative, and it keeps the CI-model decision live rather than foreclosed.

**And the fourth bug's sibling finding: A3 never complied with the banner the
manifest states in capitals.** Run 2's fix — *patterns must be definition-
shaped, not topic words* — was written into the file and then not applied to an
entry three lines below it. `fake[- ]entity` and `medical` are topic words. It
survived only because the one file containing them sits outside the SEARCH
roots, so a correct red was being produced by accident. That is gen-6's own
heading — **a rule you have written down is not a rule you automatically
apply** — recurring *inside the document that states the rule*, which is the
strongest available argument that the fix has to be structural. Hence the
schema note now at the top of the manifest, distinguishing a LIST (AND, across
distinct evidence) from ALTERNATION (OR, across spellings of the same
evidence).

**A near miss worth recording because the reasoning generalises.** Steering's
first ruling was that the missing SEARCH root made A3 a *false red* — the
guard's first failure in the opposite direction. Measured before implementing:
`probe.rs` matches both topic words, is a CLI binary with `fn main()`, is not
in-app (§11's actual requirement), and has no pass/fail logic. Widening the
roots would therefore have turned a **correct red into a false green**, and
banked a fifth bug into the record that was not real. Ruling reversed on the
evidence. The rule extracted, which outlives this entry:

> **The fix for a coverage gap is not to widen coverage under patterns too
> loose to survive it.** Tighten first, widen second, or not at all.

So the record stands at **four failures, all in the same direction —
satisfiable by less than the requirement — with one root cause.** That is more
coherent, and more actionable, than a symmetry that was not there.

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

## 📱 BRIDGE CHECKPOINT — PASSED (founder, Galaxy A22, captured by steering)

APK `fa85ab6b8536aa3938963715c34c235e5001c7cde33c6379c90d1bdd7226f0d1` (built from `7ec85ae`, commit-attributable provenance).

Founder ran `adb logcat -d | grep -F "[bridge]"`. Captured output:

```
07-29 09:22:41.458 26146 26207 I RustStdoutStderr: [bridge] ok round-trip via com.cleophis.app.NativeBridge device=samsung/SM-A226B/api33
```

**Exact match to the prediction recorded before the run**, character for character — device string included. The JNI bridge acquires the JVM/activity context via tao's `main_android_context()`, resolves the app class through `WryActivity.getAppClass`, and reads genuine `android.os.Build` values. Not a constant: real framework access, which is what the three queued shims require.

Unblocked by this result: Phase 3.2 (AndroidKeyStore `SecureStore`), 2.2 native completion task 3 (`ConnectivityManager` metered state — note `ACCESS_NETWORK_STATE` is still absent from the manifest, deliberately, until it has a caller) and task 4 (`ACTION_SEND` share sheet).

Note: this entry was appended by steering because the gen-5 agent went offline (spend limit) between starting 3.1 and this capture — the delivery lesson applied by the other party. Whoever resumes should commit it under their own authorship with this provenance note intact.

## 📱 CP3 — PASSED (founder, Galaxy A22, 2026-07-29; captured by steering)

APK `a3fd92dddf1f0f61a19d715b37d310f566539ea271a50754d7e61458690efd24` (built from `0354f6c`, three independent derivations agreeing: build sidecar, steering verification, agent re-hash of the archive).

**Founder result: signed in, force-stopped the app, relaunched — STILL SIGNED IN.**

That closes Phase 3.2 on the Android axis and retires the caveat carried since CP0: *"keyring v3 silently mocks in-memory on unsupported targets, so a force-stop logs the user out."* It is no longer true. The credential is encrypted under a non-exportable AndroidKeyStore AES-256-GCM key, stored in `noBackupFilesDir` (excluded from backup by construction, not by rule), AAD-bound to its slot, and its tag verified on read.

**What this result also proves, which nothing off-device could:** `Cloud::restore` reaches the bridge via `spawn_blocking` — a Tokio blocking-pool thread the JVM has *not* attached — so `attach_current_thread` genuinely attaches and detaches there. The bridge checkpoint ran on the UI thread and got a nested no-op guard; gen-5 flagged that gap explicitly as unclaimed. **A successful decrypt after relaunch is the first exercise of that path.** The gap is now closed by observation rather than by assumption.

**Still uncaptured from this session** (fold into the next founder device run, not a new session): the remaining CP1 items — `[kernels] DOTPROD=1` from logcat, quantified TTFT/tok-s/RSS, the tampered-file integrity probe, and formal Stage-5 4/4. Also unverified by construction: R8/release behaviour (audit item 1b — a green debug checkpoint is fully compatible with a release build that signs the user out on every launch), and the sign-out purge (`no_backup/secure/` empty).

Recorded by steering because no agent was running; whoever resumes should commit it under their own authorship with this provenance note intact.
