# Vision input, Stage 2 (capture, confirm, handoff on Android): verification record

Branch `mobile/vision-p2`, head at the time of the gates `010fd60`. Plan:
`docs/superpowers/plans/2026-11-13-vision-stage2-capture-confirm.md`. Stage 1's record is
`docs/superpowers/verification-vision-stage1.md`; Stage 0's is
`docs/superpowers/verification-vision-spike.md`. Every number below comes from a command run
in Task 6 (2026-11-13, the WSL2 box and the founder's Windows toolchain), or is quoted from
the file named beside it. Nothing is estimated unless marked. No device step has run: the
checklist below is handed to the founder unrun.

## What each task built

| Task | Commit | What |
|---|---|---|
| 1 | `662dff4` | `VisionConfig.single_formula` (default true): generation ends on granite-docling's `</formula>` token (id 100276) with `StopReason::Eos`. Alpha is composited over white instead of being dropped. `vision_bench --multi-formula` keeps the page regime. |
| 2 | `a49ea99`, `2f2edd0` | `messages.attachments`, a nullable JSON column added by migration; `MessageInfo.attachments`; `append_message(.., attachments)` writes it on insert and on the partial-row upgrade. |
| 3 | `e59b96b`, `8fa691f`, `55fb6fa` | Three pure modules. `vision-crop.js`: crop geometry, display to image pixels, the 512 px target size. `vision-confirm.js`: verdict to UI mode, amber flags, `canUse` / `canUseAnyway`, `composeHandoff`, `attachmentFor`, the pinned copy. `vision-download.js`: availability from `vision_state`, the two-file download plan. The gate fails closed. |
| 4 | `4d0e285`, `5dce8a5`, `0c21bd2`, `010fd60` | The WebView flow: the composer camera button, capture and pick inputs, the crop frame, "Reading the image…", the confirmation modal, the handoff as a user turn with its attachment, the vision download. Fix round: one decode at a time, only maths can be amber, the shared download slot survives a tier switch, rotation mid-drag re-lays out, Use waits for the composer, a single open per tap, the crop preview's memory freed, a tier-switch error stays in its own drawer. |
| 5 | `04707df` | The manifest `<queries>` entry for `android.media.action.IMAGE_CAPTURE`; `CaptureHygiene.sweep`, called from `MainActivity.onCreate` and over JNI after every transcription; its keep rule. Backups were checked, not changed: they already exclude the external domain. |
| 6 | this commit | Checklist steps V1-V7, the pin test's step list and module map, the debug APK, this record. |

The catalog block (`20bf148`, every tutor tier declares the granite-docling transcriber) was
merged in at `a808f48` before Task 4. The plan is `661d18f`.

## Rulings

From the SDD ledger, one line each.

| Ruling | What was decided |
|---|---|
| S2-P1 | `defaultCrop`'s proportions (88% by 34%, centred) are a UX starting point, not a measurement; the founder's device run decides. |
| S2-P2 | The chat title comes from the first line, `Problem from photo:`, for v0.1. A title from the first latex block is a Stage 3 nicety. |
| S2-P3 | Implementers may run in parallel in separate worktrees with no shared files; they share `~/cleophis-mobile-target` (cargo serialises) and only one task at a time uses the Windows worktree. |
| S2-P4 | Task 3 ran in its own worktree from `a49ea99` while Task 2's fix round held this one, then was rebased and fast-forwarded. |
| S2-P5 | Task 5 ran in a third worktree from `2f2edd0`, in parallel with Tasks 3 and 4, and was the only Windows-suite user until it finished. |
| S2-P6 | A result with no latex block, whatever its verdict, shows as `refuse` with `REFUSE_NO_MATHS`, so the UI never offers a handoff that would throw. |
| S2-P7 | In review, `canUse` needs at least one latex block edited to non-blank content and no latex block still flagged; text and blank edits never unlock. |
| S2-P8 | Three Task 3 minors adopted: an unknown verdict reads as review, `attachmentFor` tolerates a missing model, `visionDownloadPlan` throws on a non-numeric size. |
| S2-P9 | Task 4 ran beside Task 3's fix round because it reads the gate only through `mode`, `canUse` and `canUseAnyway`, whose signatures did not change. |
| S2-P10 | `canUseAnyway` also needs at least one non-blank latex block. The mode never flips on an edit; Retake and Type it are always on the pane. |
| S2-P11 | Cancel mid-reading closes the modal (as built), so V5 reads "Cancel mid-reading closes the modal and the foreground notification disappears". |
| S2-P12 | Task 4's fix round covered I1 and M1 to M6; the copy items (M7) were deferred and the device notes (M8) went to this checklist. |
| S2-P13 | Only latex blocks can be amber. Text blocks render grey and read-only. `REVIEW_NOTE` is present exactly while a latex block is flagged, recomputed on every edit. |
| S2-P14 | `downloadArtifact` rejects at once with the engine's own "A download is already in progress." while the shared slot is held, without overwriting it. |
| S2-P15 | `confirmState` applies the same notes rule as `applyEdit`; Use and Use anyway render disabled while the composer is disabled. |

## Gates

All on `010fd60`. `npm test` also ran over this commit's checklist and pin-test changes,
which touch no product code.

| Where | Command | Last line |
|---|---|---|
| WSL | `npm test` | `# tests 1277`, `# pass 1277`, `# fail 0`, exit 0 |
| WSL | `cargo test -p kpack-engine` | `test result: ok. 166 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.39s` (lib), `test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s` (`qwen3_render_parity`), 0 doc tests, exit 0 |
| WSL | `RUSTFLAGS='-D warnings' cargo ndk -t arm64-v8a -P 24 check -p cleophis --all-targets --release` (from `src-tauri/`, after `source ../.superpowers/ndk-env.sh`) | `Finished \`release\` profile [optimized] target(s) in 8m 10s`, exit 0, no warning lines in the output |
| Windows | `cargo test --locked -p cleophis -- --test-threads=1` on the Windows worktree detached at `010fd60` | `test result: ok. 459 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 112.07s`, then three more `ok` result lines (bin 0, `engine_smoke` 0 passed 1 ignored, doc tests 0), `== exit 0 ==` |

- **The desktop suite ran on Windows.** `cargo test -p cleophis` does not build on the WSL box
  (verification ruling V1). The Windows log shows the Stage 2 tests executing, not only
  compiling: `an_old_database_gains_the_attachments_column`,
  `attachments_round_trip_and_are_omitted_when_absent`,
  `partial_row_upgrade_writes_attachments` and the three `vision_cmds` tests all print `ok`.
  The 10 ignored tests are the pre-existing device and sidecar ones.
- **The checklist went RED first.** With V1-V7 added and V1 pinned to `src/app.js`, as the
  task brief said, `npm test` gave `# pass 1276`, `# fail 1`:
  `src/app.js does not contain "Add a photo of a problem"`. The aria-label is markup, in
  `src/index.html:117`; the pin now names that file. The nine constant pins passed in the RED run
  itself. The source-pin test stops at its first failure, so the other eight new
  source pins were first checked after the fix, and passed.
- **The step count did not change.** V1-V7 extend an existing test's step list, so the suite
  still has 1277 tests.

## The debug APK

Built with `docs/superpowers/mobile-tools/build-android-apk.sh`, no flags: a debug APK,
aarch64, the default `tutor` variant. The APK of record is the controller's build at `010fd60`
(03:04:21 to 03:05:25). This task's re-run (03:06:58 to 03:07:10), started before the
controller's hold reached it, produced the same bytes.

| | |
|---|---|
| Commit | `010fd60246cb588d3896f1cf25be07535d883b2d`, tree clean before and after in both builds (provenance `porcelain <empty: tree clean>` at PRE and POST) |
| Log, provenance | `~/cleophis-mobile-logs/apk-debug-20261113-030421.log` and `.provenance` beside it (`tauri_exit 0`, `variant tutor`); the identical re-run is `apk-debug-20261113-030658.log` and `.provenance` |
| Build output | `src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk` |
| Archived copy (the one to install) | `~/cleophis-artifacts/cleophis-vision-s2-debug-010fd60.apk` |
| sha256 | `c4ad4c06b46b66e4644eeae0512df174cf95645d3aaf5de1c88e10b2d88ab865` |
| Size | 380,325,886 bytes (362.71 MiB) |
| Signing | the machine's debug key (`~/.android/debug.keystore`; `apksigner` prints `CN=Android Debug`, certificate SHA-256 `552c5cdf9d2136e92e6e9d17b4859163fe414d1d794a55e9cf428d6649d76a3e`). Nothing was missing for a debug build. No release APK was built. |
| Package | `com.cleophis.app`, versionCode 1000, versionName 0.1.0, minSdk 24, targetSdk 36 |

`verify-apk.py` (with build-tools 35.0.0's `aapt2`) on the build output:

- 970 entries, 178,405 bytes unaccounted (0.05%), so no stranded orphan.
- `assets/` is exactly `assets/tauri.conf.json`. The only ABI is `arm64-v8a`.
- The largest entries are `lib/arm64-v8a/libcleophis_lib.so` (370,697,336),
  `classes.dex` (10,260,852) and `classes9.dex` (4,164,264).
- The package, `REQUEST_INSTALL_PACKAGES`, `FOREGROUND_SERVICE_SPECIAL_USE`,
  `allowBackup=false` and `windowSoftInputMode` checks all print `ok`.
- Every Kotlin class it knows is present in `classes8.dex`: NativeBridge, SecureStore,
  NetworkPolicy, ShareSheet, InferenceService and its companion, ScreenPrivacy.
- `VERDICT: PASS`.

Two checks the script does not make, run by hand on the same APK:

- `Lcom/cleophis/app/CaptureHygiene;` and `Lcom/cleophis/app/MainActivity;` are both in
  `classes8.dex`, and so is the string `sweep`. A debug build does not run R8, so this proves
  the class compiled and was packaged. It does not exercise the keep rule, which only a
  release build would.
- The packaged manifest (`aapt2 dump xmltree`) has `queries > intent > action`
  `android.media.action.IMAGE_CAPTURE`.

### The library in the APK

| | |
|---|---|
| Artifact | `~/cleophis-mobile-target/aarch64-linux-android/debug/libcleophis_lib.so` (debug profile, with debug info, not stripped) |
| sha256 | `05fd110c8661d2f1069e8a9c65bed49fb07adfb9fd6c1de4e4bcb3c8049ee1d3`, equal to the APK's `lib/arm64-v8a/libcleophis_lib.so` entry (stored uncompressed) |
| Size | 370,697,336 bytes (353.52 MiB) |
| Arch audit | `PASS: …/libcleophis_lib.so — nothing above the floor outside 8 outline-atomics helper(s)` |

- **8 helpers here, 5 in Stage 1, from different profiles.** Stage 1 audited a release
  library; this is the debug one the APK carries. `llvm-nm` names the eight:
  `__aarch64_ldadd4_acq_rel`, `__aarch64_ldadd4_relax`, `__aarch64_ldadd8_acq_rel`,
  `__aarch64_ldadd8_relax`, `__aarch64_swp4_acq`, `__aarch64_swp4_acq_rel`,
  `__aarch64_swp4_rel` and `__aarch64_swp4_relax`. Every one is runtime-gated on
  `__aarch64_have_lse_atomics`, which is what the audit allows.
- **Size delta is still not available.** Stage 1's 40,041,448 bytes is a release library and
  this one is a debug library. No release library was built in this task.

### Gradle compiled the new Kotlin for the first time

`CaptureHygiene.kt` and `MainActivity.kt` (Task 5) had never been through Gradle before this
task. Their first compile is part of this record:

1. **A pre-warm build of `0c21bd2` failed** (`apk-debug-20261113-025127.log`, 02:51 to 03:02).
   It failed at `:app:compileUniversalDebugKotlin` with five errors, all in `MainActivity.kt`
   and all from one cause, `Unresolved reference: TauriActivity`. Without its superclass,
   `MainActivity` is not a `Context`, so `CaptureHygiene.sweep(this)` reported a type
   mismatch too. No error was in `CaptureHygiene.kt`.
2. **The cause is a known build trap, not the new code.** The Phase 2 guardrails plan records
   it (2026-09-10): the script pins a shared `CARGO_TARGET_DIR`, and tauri 2.11.5's build
   script writes `generated/TauriActivity.kt` only when it re-runs. It declares no
   `rerun-if-env-changed` on the Kotlin out-dir, so the first build from a new worktree has
   no `TauriActivity.kt`. The file is git-ignored.
3. **The controller restored the file.** It copied `generated/TauriActivity.kt` (03:04:19) from
   the built sibling worktree `~/cleophis-wt/p2-final`, which has the same tauri 2.11.5. The
   copy equals that worktree's file byte for byte, and differs from the registry template
   `tauri-2.11.5/mobile/android-codegen/TauriActivity.kt` only in its
   `package com.cleophis.app` line. wry's seven generated files had regenerated on their own.
   Nothing was committed: the file is git-ignored.
4. **The controller's build then compiled the Kotlin.** It ran at `010fd60` from 03:04:21 to
   03:05:25 with `tauri_exit 0` (`apk-debug-20261113-030421.log`). The compiled
   `CaptureHygiene.class` and `MainActivity.class` are stamped 03:05:18, inside that build.
5. **This task's re-run reused that compile.** It ran 03:06:58 to 03:07:10 and produced a
   byte-identical APK: the same sha256, `c4ad4c06…`.

`app/proguard-tauri.pro`, the other file the same trap leaves out, is still absent from this
worktree. A debug build does not read it; a release build from this worktree would need it
rendered first.

## Device check

Not run. The checklist is handed to the founder unrun:
`docs/superpowers/mobile-tools/phone-journey.md`, section "Photo input (Stage 2)", on the
APK above. Each step has PASS and FAIL boxes and names its evidence, and every quoted screen
string is pinned by `probes/phone-journey.test.mjs`.

| Step | What | Pins |
|---|---|---|
| V1 | The camera button shows on the tutor only, once `vision_state` says declared; the download, or the sideload while the signed catalog lacks the files | `src/index.html` aria-label, `src/vision-download.js`, `src/dist-pick.js`, `TAKE_PHOTO_LABEL`, `CHOOSE_IMAGE_LABEL` |
| V2 | "Take a photo" opens the system camera, not the file picker | the manifest's `android.media.action.IMAGE_CAPTURE`, `src/app.js` "Use selection", `RETAKE_LABEL` |
| V3 | "Choose an image" picks a screenshot | none of its own (V1's labels) |
| V4 | A picture stored sideways with EXIF orientation 6 shows and reads upright | `READING_TEXT` |
| V5 | Cancel mid-reading closes the modal and the foreground notification disappears | `InferenceService.kt` "Writing a reply", `src/app.js` "Cancel" |
| V6 | The confirmation gate: amber latex only, text blocks grey and read-only; "Use this" always shown and disabled until the gate allows; "Use anyway" only in review; after the amber block is edited, "Use this" enables and "Use anyway" disappears; the handoff bubble | `REVIEW_NOTE`, `USE_LABEL`, `USE_ANYWAY_LABEL`, `TYPE_IT_LABEL`, `HANDOFF_PREFIX` |
| V7 | No `JPEG_*.jpg` is left in the app's external pictures folder, after a reading and after a cold start, with a positive listing first | `CaptureHygiene.kt` `JPEG_`, `mobile_native.rs` `[vision] swept {n} capture file(s)` |

V4's test picture is made on the laptop from Stage 0's `p03-quadratic.png`: rotated 90°
counter-clockwise and tagged orientation 6. Checked here with Pillow 12.3.0: stored
160 by 512 with tag 6, it transposes to 512 by 160, and its mean absolute difference from the
original is 0.06 grey levels upright against 10.19 turned 180°.

The device notes from the Task 4 review (its M8) and the implementer's concerns sit under
the steps as "Probe" lines, which are observations and not pass criteria:

- Android back in each pane (menu V1, crop V2, reading V5, confirm V6). On API 31 and later
  the task goes to the background and the flow survives. On API 24 to 30 the activity
  finishes and the flow is lost, with nothing persisted.
- HEIC from "Choose an image" (V3): a decode error shows the decoder's message in the
  refuse pane.
- Decode memory on the largest camera mode with the tutor loaded (V2): `dumpsys meminfo` for
  the app and the WebView renderer, and lmkd kills in logcat.
- Process death while the camera app is in front (V7): the capture is lost, and the start-up
  sweep removes the orphan JPEG.
- One-thumb resizing with the 28 px handles, and a rotation mid-drag, where the crop must
  re-lay out when the finger lifts (V2).
- A Cancel that beats the engine's registration of the request id (V5): the transcription
  finishes unseen and keeps the button disabled until then. The window is about the base64
  decode of a ~100 KB crop.
- The export (V6): the photo turn's JSON row carries `attachments`, with counts and no text.

V5 quotes one latency, with its source: Stage 0's A22 prefill, `prefill_ms` 7,609 to 7,653 over
the 15 rows of `docs/superpowers/vision-spike/a22-floor-formula-1pass.jsonl`. No Stage 2
latency was measured.

## Security inheritances honoured

The plan's Global Constraints and Security Assessment, each with where it lives at `010fd60`.

- **H1: only confirmed maths reaches the tutor, as a user turn.** `composeHandoff`
  (`src/vision-confirm.js:116`) keeps only `latex` blocks, trimmed and non-blank, and
  prefixes `HANDOFF_PREFIX` (`:14`, `Problem from photo:`). It throws when none is left, and
  the gate (`gate`, `:58`) never offers a button that would make it throw.
  `visionHandoff` (`src/app.js:3436`) pushes the text as `{ role: 'user' }` (`:3458`) and
  sends it through the existing `sendCompletion`; the system prompt is untouched. Tested in
  `src/vision-confirm.test.mjs` ("composeHandoff uses only trimmed latex blocks…", "…throws
  when no latex remains").
- **H1: model text reaches the DOM only as text.** `showVisionConfirm`
  (`src/app.js:3378`) creates each block as a `<textarea>` and sets `ta.value` (`:3392`);
  refusals and errors use `textContent` (`:3375`). The section's own header states the rule.
  The check: `grep -c innerHTML src/app.js` is 21 at `480c05e` and 21 at `010fd60`, and
  `git diff 480c05e 010fd60 -- src/app.js | grep '^+' | grep -c innerHTML` prints `0`. Task 4's
  smoke run rendered a block of `<img src=x onerror=alert(1)>` as inert text.
- **H1: provenance is counts, never text.** `attachmentFor` (`src/vision-confirm.js:129`)
  stores kind, model id and family, verdict, `edited` and the block counts.
  `sendCompletion` takes the pending attachment and clears it before anything else
  (`src/app.js:2136`), so a typed turn cannot inherit it.
- **M1: the crop sent is small, and EXIF is gone.** `targetSize` caps the longest edge at
  `MAX_SIDE` 512 and never upscales (`src/vision-crop.js:11`, `:71`). `useCrop`
  (`src/app.js:3309`) draws the crop from the full-resolution bitmap onto a white-filled
  canvas and re-encodes it with `toDataURL('image/jpeg', 0.92)` (`:3323`). The canvas
  re-encode carries no EXIF block, GPS included. The decode applies the orientation first
  (`createImageBitmap(file, { imageOrientation: 'from-image' })`, `:3236`). The bitmap is
  closed once the crop is drawn, and only one decoded photo is ever held.
- **The thumbnail is a `data:` URL.** `src/app.js:3384`; the CSP in
  `src-tauri/tauri.conf.json` allows `img-src … data:`.
- **`prewarm_vision` only where the transcriber can stay beside the tutor.** The menu calls
  it only when `vision_state` says `coresident` (`src/app.js:3146`), and the command gates
  itself on installed and co-resident (`src-tauri/src/vision_cmds.rs:299`, Stage 1).
- **One transcription in flight.** The camera button is disabled while one runs, while the
  tutor streams, and while the composer is off (`src/app.js:3088`). The engine refuses a
  second call with "A transcription is already in progress."
  (`src-tauri/src/vision_cmds.rs:220`).
- **The camera intent is declared.** `<queries>` with `android.media.action.IMAGE_CAPTURE`,
  `src-tauri/gen/android/app/src/main/AndroidManifest.xml:44-48`. It is present in the
  packaged manifest.
- **M2: captured photos are swept.** `CaptureHygiene.sweep`
  (`src-tauri/gen/android/app/src/main/java/com/cleophis/app/CaptureHygiene.kt`) deletes
  `JPEG_*.jpg` from `getExternalFilesDir(Pictures)`. It runs from `MainActivity.onCreate`
  (`MainActivity.kt:10`) and over JNI (`src-tauri/src/mobile_native.rs:471`) after the
  base64 refusal (`vision_cmds.rs:231`) and in the single teardown after every transcription
  (`vision_cmds.rs:263`). Its keep rule is `src-tauri/gen/android/app/proguard-cleophis.pro:81`.
- **M2: backups exclude the external files.** `android:allowBackup="false"`
  (`AndroidManifest.xml:55`), plus `<exclude domain="external" />` in
  `res/xml/backup_rules.xml:51` and in both sections of `res/xml/data_extraction_rules.xml`
  (`:28` cloud backup, `:36` device transfer). Read, not device-tested.
- **L2: download trust is unchanged.** The vision files arrive only through
  `download_artifact`, which refuses shas the bundled catalog does not pin (Stage 1).

## What Stage 3 inherits

- **The 20-image probe set, the scorer and `vision-regression.yml` (CI).** Only five
  rendered items have been measured (Stage 0), and no photographed item.
- **Verdict calibration.** `CONFIDENCE_FLAG` 0.9 and the private `OPERATOR_FLAG` 0.97 in
  `src/vision-confirm.js`, and the engine's own constants, are plan values, uncalibrated.
  V6 may need several tries to produce a review result for the same reason.
- **The page regime needs `single_formula: false`.** Task 1 measured that the default stop
  ends a page after its first formula: Stage 0's `p05-system` page lost its second equation.
  The crop flow is the only caller today, and it wants the default.
- **WebView memory beside the engine is unmeasured.** V2's decode-memory probe is the first
  reading.
- **The device numbers are still Stage 0's.** No device run of Stage 1 or Stage 2 code exists.

## What did not run

- **Every device step.** V1-V7 are the founder's. The camera, the picker, the sweep on a real
  external folder, the foreground notification and the backup exclusion are all unproven on
  a phone.
- **A release build.** So the keep rules, including `CaptureHygiene`'s, are unexercised by R8,
  and `proguard-tauri.pro` is missing from this worktree.
- **`check-mobile-build.sh` and `vision_bench`.** No engine code changed in Task 6.
- **`cargo test -p cleophis` on WSL.** It was replaced by the Windows run.

### Traps

- **`TauriActivity.kt` in a fresh worktree.** With the shared `CARGO_TARGET_DIR`, tauri
  2.11.5's build script writes the git-ignored
  `src-tauri/gen/android/app/src/main/java/com/cleophis/app/generated/TauriActivity.kt`
  from `TAURI_ANDROID_PROJECT_PATH`, with no `rerun-if-env-changed` for it. In a fresh
  worktree it stays cached and never runs. Gradle then fails with
  `Unresolved reference: TauriActivity` at `MainActivity.kt:6`, and every other
  `MainActivity` error cascades from it. wry's seven generated files do regenerate. The fix
  is to copy the file from a built sibling worktree on the same tauri version, or to render
  the registry template with the package line. `app/proguard-tauri.pro` comes from the same
  build script and is still missing here; only a release build reads it.

## Deferred minors

One line per task, from the SDD ledger, plus Task 6's.

- **Task 1:** alpha is composited before the resize (Triangle resampling over transparent
  neighbours can fringe light ink); a stale `vision.rs` module-doc bullet ("the closer is the
  only guard"); `FORMULA_END_TAG` sits between `use` lines and the two tags live in
  different modules; no unit test for the stop itself (native-checked only).
- **Task 2:** a malformed stored `attachments` string reads as `None`; `export_chat` now
  carries `attachments` on photo turns (see below).
- **Task 3:** none deferred. The review minors were adopted (S2-P8), and the "dead end"
  after blanking the only maths was ruled not one (S2-P10).
- **Task 4 (the M7 copy pass):** `Downloading…` is a literal, not a constant; the
  engine-state row says "Downloading your model" during a vision download; the busy crop
  pane has no copy; Type it focuses a disabled input while the composer is off; the metered
  question comes before the tier-switch refusal (pre-existing); a worker panic leaves the
  JS download slot held until restart (pre-existing for the vision path).
- **Task 5:** the sweep has no age guard, so a JPEG mid-capture could be deleted. That is
  reachable only when the activity is recreated under the camera, where the chooser's
  callback is lost anyway, and Task 4 disables the camera button while a transcription
  runs, so no new capture overlaps the teardown sweep.
- **Task 6:** `verify-apk.py` has no `CaptureHygiene` entry (checked by hand above); the
  foreground notification says "Writing a reply" while the device reads an image (for the
  copy pass); the build script still has no self-heal for the `TauriActivity.kt` trap
  (deferred since Phase 2).

## Follow-ups outside this plan

- **`.lookupbar[hidden]{display:none}`.** `.lookupbar{display:flex}` defeats the `hidden`
  attribute, so "Look up a drug or condition" shows in the tutor chat on every platform.
  Pre-existing.
- **The M7 copy pass.** The Task 4 copy items above, and the notification title.
- **The metered-question order.** A tier switch during a vision download asks the metered
  question before it is refused.
- **The sweep's missing age guard.** Skipping files younger than a minute or two would
  close it without weakening the stale-photo goal.
- **`export_chat` carries `attachments` on photo turns.** The export contract widened in
  Task 2: counts, model and verdict, never text. A reader of exports should expect the key.
- **From the plan:** the share-sheet target (ACTION_SEND) and a Tauri mobile plugin for
  capture (v0.2); a LaTeX renderer in the confirm modal (v0.1 shows monospace source);
  desktop drag-and-drop and paste, after the phone.
