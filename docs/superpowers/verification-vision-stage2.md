# Vision input, Stage 2 (capture, confirm, handoff on Android): verification record

Branch `mobile/vision-p2`. Task 6's gates ran on `010fd60`. The final-review fix wave's gates ran on `22652bc` (code tree identical to `3cd142d`)
and its debug APK on `3cd142d`, its code commit; the commit that brings this record and the
checklist up to date follows it and changes no product code. Plan:
`docs/superpowers/plans/2026-11-13-vision-stage2-capture-confirm.md`. Stage 1's record is
`docs/superpowers/verification-vision-stage1.md`; Stage 0's is
`docs/superpowers/verification-vision-spike.md`. Every number below comes from a command run
in Task 6 or in the fix wave (2026-11-13, the WSL2 box and the founder's Windows toolchain),
or is quoted from the file named beside it. Nothing is estimated unless marked. The device run below
was made by the founder on the Galaxy A22 on 2026-11-13 with the fix wave's APK (`3cd142d`); every
step passed.

## What each task built

| Task | Commit | What |
|---|---|---|
| 1 | `662dff4` | `VisionConfig.single_formula` (default true): generation ends on granite-docling's `</formula>` token (id 100276) with `StopReason::Eos`. Alpha is composited over white instead of being dropped. `vision_bench --multi-formula` keeps the page regime. |
| 2 | `a49ea99`, `2f2edd0` | `messages.attachments`, a nullable JSON column added by migration; `MessageInfo.attachments`; `append_message(.., attachments)` writes it on insert and on the partial-row upgrade. |
| 3 | `e59b96b`, `8fa691f`, `55fb6fa` | Three pure modules. `vision-crop.js`: crop geometry, display to image pixels, the 512 px target size. `vision-confirm.js`: verdict to UI mode, amber flags, `canUse` / `canUseAnyway`, `composeHandoff`, `attachmentFor`, the pinned copy. `vision-download.js`: availability from `vision_state`, the two-file download plan. The gate fails closed. |
| 4 | `4d0e285`, `5dce8a5`, `0c21bd2`, `010fd60` | The WebView flow: the composer camera button, capture and pick inputs, the crop frame, "Reading the image…", the confirmation modal, the handoff as a user turn with its attachment, the vision download. Fix round: one decode at a time, only maths can be amber, the shared download slot survives a tier switch, rotation mid-drag re-lays out, Use waits for the composer, a single open per tap, the crop preview's memory freed, a tier-switch error stays in its own drawer. |
| 5 | `04707df` | The manifest `<queries>` entry for `android.media.action.IMAGE_CAPTURE`; `CaptureHygiene.sweep`, called from `MainActivity.onCreate` and over JNI after every transcription; its keep rule. Backups were checked, not changed: they already exclude the external domain. |
| 6 | `9a988b4` | Checklist steps V1-V7, the pin test's step list and module map, the debug APK, this record. |
| Fix wave | `3cd142d`, then this commit | The final review's I1, M1-M5 and M7 (ruling S2-P16). The `sweep_captures` command, which the page calls once a capture is decoded and in `resetVision`. An accept with an amber latex block opens as review, and a node test pins the gate's thresholds to the engine's. The attachment gains `override`. The record and the checklist corrected, V7 rebuilt around the earlier sweep, and the APK rebuilt. |

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
| S2-P16 | One fix wave after the final review: I1, M1-M4, M5 (fail closed, plus a node test tying the gate's thresholds to `vision_parse.rs`) and M7 (`override` in the attachment, while the schema is new). M6 goes to Stage 3 and the device watchlist, M8 to the copy pass, M9 to a follow-up. |

## Gates

> **Commit rewrite (trailer only).** The fix-wave commits were first recorded as `22652bc` (code) and `ed4a067` (docs) with a `Co-Authored-By: Claude Opus 5.5` trailer; both were rewritten to carry the repository's `Claude Fable 5.1` trailer. The code commit's tree is byte-identical to `22652bc`'s (same git tree hash), so the Android `-D warnings` check and the Windows desktop run made on `22652bc` hold for `3cd142d`; the docs commit's tree differs from `ed4a067`'s only by this record's and the checklist's own corrections. The APK was rebuilt from `3cd142d` because `build-android-apk.sh` stamps `CLEOPHIS_APP_BUILD` with the commit; the `22652bc` build (sha256 b2f99ce0…, 380,312,110 bytes) is superseded.


### The fix wave, on `22652bc` (the code tree is identical to `3cd142d`; see the rewrite note)

`3cd142d` is the code commit. The docs commit after it changes only this record and the
checklist, so `npm test` ran again on it, because the pin test reads the checklist.

| Where | Command | Last line |
|---|---|---|
| WSL | `npm test` (on the docs commit) | `# tests 1283`, `# pass 1283`, `# fail 0`, exit 0 |
| WSL | `node --check src/app.js`, then `git diff 9a988b4 -- src/app.js \| grep -c '^+.*innerHTML'` | no output and exit 0, then `0` |
| WSL | `RUSTFLAGS='-D warnings' cargo ndk -t arm64-v8a -P 24 check -p cleophis --all-targets --release` (from `src-tauri/`, after `source ../.superpowers/ndk-env.sh`) | `Finished \`release\` profile [optimized] target(s) in 13.01s`, exit 0, no warning lines in the output |
| Windows | `cargo test --locked -p cleophis -- --test-threads=1` on the Windows worktree detached at `22652bc` | `test result: ok. 459 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 106.02s` in `desktop-test-22652bc.log`, then three more `ok` result lines (bin 0, `engine_smoke` 0 passed 1 ignored, doc tests 0), and no line in the log or on the console contains "warning". The `== exit 0 ==` line is on the PowerShell console, not in the log: the command writes it after `Tee-Object` has closed the file. `git rev-parse HEAD` in that worktree after the run printed `22652bc8a3efde65ebed1a3a7a2a5305c4fd7ef5`. This is the second run on `22652bc`. The first gave the same counts (`finished in 108.46s`), but its exit line was lost: the WSL shell expanded `$LASTEXITCODE` to nothing before PowerShell saw it, and the last statement failed after cargo had finished. |

- **RED first, for M5 and M7.** With the new tests written and nothing else changed, the confirm
  test file did not load: `The requested module './vision-confirm.js' does not provide an export
  named 'OPERATOR_FLAG'`. With the constant exported, 35 of 40 passed. The five failures were the
  re-pinned attachment shape (no `override` key), both "an accept with amber opens as review"
  tests (`actual: 'accept'`, `expected: 'review'`), and both `override` tests (`undefined`). The
  threshold-pin test passed at once, because the two sides already agree.
- **Two fixtures were re-pinned.** The notes test (`vision-confirm.test.mjs`, "notes follow wire
  order…") and "an edit in accept mode never adds the review note" both built an accept with
  amber blocks. Such a state now opens as review, so both use confident blocks for their accept
  side.
- **Six new tests.** `npm test` went from 1277 to 1283.
- **Not re-run in the fix wave:** `cargo test -p kpack-engine`, because no engine file changed
  (the new node test only reads `vision_parse.rs`), and the headless page smoke.

### Task 6, on `010fd60`

`npm test` also ran over Task 6's checklist and pin-test changes, which touch no product code.

| Where | Command | Last line |
|---|---|---|
| WSL | `npm test` | `# tests 1277`, `# pass 1277`, `# fail 0`, exit 0 |
| WSL | `cargo test -p kpack-engine` | `test result: ok. 166 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.39s` (lib), `test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s` (`qwen3_render_parity`), 0 doc tests, exit 0 |
| WSL | `RUSTFLAGS='-D warnings' cargo ndk -t arm64-v8a -P 24 check -p cleophis --all-targets --release` (from `src-tauri/`, after `source ../.superpowers/ndk-env.sh`) | `Finished \`release\` profile [optimized] target(s) in 8m 10s`, exit 0, no warning lines in the output |
| Windows | `cargo test --locked -p cleophis -- --test-threads=1` on the Windows worktree, which the implementer reported detached at `010fd60` | `test result: ok. 459 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 112.07s`, then three more `ok` result lines (bin 0, `engine_smoke` 0 passed 1 ignored, doc tests 0). Those `test result:` lines are the log's last lines. |

- **What the 010fd60 log shows, and what it does not.** `desktop-test-010fd60.log` is UTF-16.
  No line in it contains `exit`: `grep -a -c 'exit'` prints `0` on the raw file, and so does
  `grep -c` after decoding. It also names no commit. So the run's exit status is inferred from
  its `test result:` lines, and the commit it ran on rests on the implementer's report.
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
aarch64, the default `tutor` variant. The fix wave rebuilt it from `3cd142d`, its code commit,
before the docs commit, so the APK carries the I1 sweep that V7 now checks.

| Build | Commit | sha256 | Size | Status |
|---|---|---|---|---|
| Fix wave | `3cd142d` | `cc4f6386ba91187394811d161e1c9d827ba83e5a0b42e7d8fa4882126e124f6d` | 380,312,110 bytes (362.69 MiB) | **current: the one to install** |
| Task 6 | `010fd60` | `c4ad4c06b46b66e4644eeae0512df174cf95645d3aaf5de1c88e10b2d88ab865` | 380,325,886 bytes (362.71 MiB) | superseded: it lacks the I1 sweep |

### The fix wave's build (current)

| | |
|---|---|
| Commit | `3cd142d230f1731958a09deef7be32b60f31d046`, tree clean before and after (provenance `porcelain <empty: tree clean>` at PRE and POST) |
| Time | 04:28:11 to 04:28:48 |
| Log, provenance | `~/cleophis-mobile-logs/apk-debug-20261113-042811.log` and `.provenance` beside it (`tauri_exit 0`, `variant tutor`) |
| Build output | `src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk` |
| Archived copy (the one to install) | `~/cleophis-artifacts/cleophis-vision-s2-debug-3cd142d.apk`, sha256 re-checked after the copy |
| sha256 | `cc4f6386ba91187394811d161e1c9d827ba83e5a0b42e7d8fa4882126e124f6d` |
| Size | 380,312,110 bytes (362.69 MiB) |
| Signing | the machine's debug key, the same as Task 6's (`apksigner` prints `CN=Android Debug`, certificate SHA-256 `552c5cdf9d2136e92e6e9d17b4859163fe414d1d794a55e9cf428d6649d76a3e`). No release APK was built. |
| Package | `com.cleophis.app`, versionCode 1000, versionName 0.1.0, minSdk 24, targetSdk 36 |

`verify-apk.py` (with build-tools 35.0.0's `aapt2`) on the build output:

- 970 entries, 178,406 bytes unaccounted (0.05%), so no stranded orphan.
- `assets/` is exactly `assets/tauri.conf.json`. The only ABI is `arm64-v8a`.
- The largest entries are `lib/arm64-v8a/libcleophis_lib.so` (370,683,560),
  `classes.dex` (10,260,852) and `classes9.dex` (4,164,264).
- The package, `REQUEST_INSTALL_PACKAGES`, `FOREGROUND_SERVICE_SPECIAL_USE`,
  `allowBackup=false` and `windowSoftInputMode` checks all print `ok`.
- Every Kotlin class it knows is present in `classes8.dex`: NativeBridge, SecureStore,
  NetworkPolicy, ShareSheet, InferenceService and its companion, ScreenPrivacy.
- `VERDICT: PASS`.

Checks the script does not make, run by hand on the same APK:

- `Lcom/cleophis/app/CaptureHygiene;`, `Lcom/cleophis/app/MainActivity;` and the string
  `sweep` are in `classes8.dex`, as in the Task 6 build. The fix wave changed only
  `CaptureHygiene.kt`'s doc comment.
- The string `sweep_captures` is in the APK's `libcleophis_lib.so`, so the new command is
  compiled in.
- The packaged manifest (`aapt2 dump xmltree`) has `queries > intent > action`
  `android.media.action.IMAGE_CAPTURE`.

| Library | |
|---|---|
| Artifact | `~/cleophis-mobile-target/aarch64-linux-android/debug/libcleophis_lib.so` (debug profile) |
| sha256 | `133721d976d28a5d5b1e4da98252221005f12e69881131ca110c68ff115bf3c3`, the APK's `lib/arm64-v8a/libcleophis_lib.so` entry unzipped from the archived `3cd142d` APK and hashed (the superseded `22652bc` build's library was `f3edd427…`) |
| Size | 370,683,560 bytes (353.51 MiB) |
| Arch audit | re-run on the `3cd142d` APK's library: `PASS: …/libcleophis_lib.so — nothing above the floor outside 8 outline-atomics helper(s)`, exit 0; `verify-apk.py` re-run on the same APK: `VERDICT: PASS`, exit 0, 970 entries, unaccounted 178,406 (0.05%), largest `libcleophis_lib.so` 370,683,560 |

### The Task 6 build at `010fd60` (superseded)

The APK of record was then the controller's build at `010fd60` (03:04:21 to 03:05:25). Task
6's re-run (03:06:58 to 03:07:10), started before the controller's hold reached it, produced
the same bytes.

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

#### The library in that APK

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

#### Gradle compiled the new Kotlin for the first time

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

**Run by the founder on the Galaxy A22 (the floor tier) on 2026-11-13, on the fix wave's APK
(`3cd142d`, sha256 cc4f6386…): V1–V7 all PASS.** The results and the phone-log evidence follow the
step table. The checklist text:
`docs/superpowers/mobile-tools/phone-journey.md`, section "Photo input (Stage 2)", on the
fix wave's APK above. Each step has PASS and FAIL boxes and names its evidence, and every quoted screen
string is pinned by `probes/phone-journey.test.mjs`.

| Step | What | Pins |
|---|---|---|
| V1 | The camera button shows on the tutor only, once `vision_state` says declared. The menu then shows either the size line and Download, or, while the signed catalog lacks the files, an "is not in the signed catalog" message with no button, and the founder sideloads | `src/index.html` aria-label, `src/vision-download.js`, `src/dist-pick.js`, `TAKE_PHOTO_LABEL`, `CHOOSE_IMAGE_LABEL` |
| V2 | "Take a photo" opens the system camera, not the file picker | the manifest's `android.media.action.IMAGE_CAPTURE`, `src/app.js` "Use selection", `RETAKE_LABEL` |
| V3 | "Choose an image" picks a screenshot | none of its own (V1's labels) |
| V4 | A picture stored sideways with EXIF orientation 6 shows and reads upright | `READING_TEXT` |
| V5 | Cancel mid-reading closes the modal and the foreground notification disappears | `InferenceService.kt` "Writing a reply", `src/app.js` "Cancel" |
| V6 | The confirmation gate: amber latex only, text blocks grey and read-only; "Use this" always shown and disabled until the gate allows; "Use anyway" only in review; after the amber block is edited, "Use this" enables and "Use anyway" disappears; the handoff bubble | `REVIEW_NOTE`, `USE_LABEL`, `USE_ANYWAY_LABEL`, `TYPE_IT_LABEL`, `HANDOFF_PREFIX` |
| V7 | No `JPEG_*.jpg` is left in the app's external pictures folder. A positive listing comes first, taken while the camera app is in front, because the file now lives only until the crop pane draws it. Then empty listings once the crop pane shows the photo, after Retake, after a capture abandoned in the camera app and the menu closed, after a reading, and after a cold start | `CaptureHygiene.kt` `JPEG_`, `mobile_native.rs` `[vision] swept {n} capture file(s)` |

### Results (founder, A22, 2026-11-13)

Evidence is the founder's report per step plus the phone log (`adb logcat`, tag `RustStdoutStderr`,
the `[vision]` lines) and `adb shell ls` of the app's external Pictures folder, read from the
laptop during the run.

| Step | Result | Evidence |
|---|---|---|
| V1 | PASS | The camera button on the tutor; the menu offered the download against the live v13 catalog; the download completed; the menu turned into "Take a photo" / "Choose an image". |
| V2 | PASS | The system camera opened. First reading 11:04:20: `verdict Accept warnings [] blocks 1`, `transcribe ok total 8991 ms (coresident=true, reload=None)`. |
| V3 | PASS | A screenshot picked and cropped. |
| V4 | PASS | The sideways picture read upright with "Reading the image…" showing (founder). |
| V5 | PASS | Cancel closed the modal and the notification went away (founder). Engine side: the cancel at 11:08:21 settled after 20,417 ms because the encoder pass is not interruptible; nothing of that reached the UI. |
| V6 | PASS | Amber maths, "Use this" disabled until edited, "Use anyway" present, the `Problem from photo:` bubble (founder). Review readings in the log: 11:06:31 `[LowConfidence, OperatorUncertain]` 9,403 ms; 15:34:30 `[OperatorUncertain]` 9,258 ms. |
| V7 | PASS | Pictures folder empty after a full reading and after Take a photo → Retake; log `swept 1 capture file(s)` at 15:38:44 (the decode sweep) then `swept 0` at 15:38:45 (the reset sweep). |

Readings observed during the run: four totals, 8,991 / 9,403 / 9,373 / 9,258 ms, all
`coresident=true`, `reload=None`, inside Stage 0's 8–12 s floor gate. The 15:33:38 reading was a
`Refuse [NoMathFound]` on a non-maths frame. Transcriber loads: model 393–943 ms, projector
226–488 ms. The 120 s idle release fired twice as designed.

Process lifecycle, observed and diagnosed from the log (not Stage 2 defects): at 11:09:07 the
system reclaimed the app as a cached process (`cch CRE`) about 45 s after a cancelled reading
with the app in the background; at 11:13:30 the foreground process exited through
`std::process::exit` in tao's Android event loop 0.4 s after a launcher relaunch of
`LaunchGateActivity` onto the backgrounded task, and Android restarted it 1.6 s later. No panic,
no signal, the crash buffer empty. An in-flight photo flow does not survive either; the
transcriber reloads in under a second. Follow-up outside this plan: the Tauri/tao Android
relaunch lifecycle.

Found during the run: the triage "Look up a drug or condition" bar showed on the tutor, the
pre-existing `.lookupbar` rule flagged by the Task 4 review; fixed in PR #43.

Not probed in this run and still on the watchlist: HEIC input, decode memory at the largest
camera mode, one-thumb handle drags and a rotation mid-drag, Android back in each pane, a
two-line problem in one crop, and the backup exclusion on a real backup.

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
  decode of a crop at most 512 px on its longest edge.
- The export (V6): the photo turn's JSON row carries `attachments`, with counts and no text,
  and `override` false after a handoff by Use this.

V5 quotes one latency, with its source: Stage 0's A22 prefill, `prefill_ms` 7,609 to 7,653 over
the 15 rows of `docs/superpowers/vision-spike/a22-floor-formula-1pass.jsonl`. Stage 2's device run then measured four readings on the A22
at 8,991, 9,403, 9,373 and 9,258 ms total (phone log; co-resident, no reload), inside that gate.

## Security inheritances honoured

The plan's Global Constraints and Security Assessment, each with where it lives at `3cd142d`.

- **H1: only confirmed maths reaches the tutor, as a user turn.** `composeHandoff`
  (`src/vision-confirm.js:122`) keeps only `latex` blocks, trimmed and non-blank, and
  prefixes `HANDOFF_PREFIX` (`:14`, `Problem from photo:`). It throws when none is left, and
  the gate (`gate`, `:60`) never offers a button that would make it throw.
  `visionHandoff` (`src/app.js:3451`) pushes the text as `{ role: 'user' }` (`:3473`) and
  sends it through the existing `sendCompletion`; the system prompt is untouched. Tested in
  `src/vision-confirm.test.mjs` ("composeHandoff uses only trimmed latex blocks…", "…throws
  when no latex remains").
- **H1: model text reaches the DOM only as text.** `showVisionConfirm`
  (`src/app.js:3393`) creates each block as a `<textarea>` and sets `ta.value` (`:3407`);
  refusals and errors use `textContent` (`:3390`). The section's own header states the rule.
  The check: `grep -c innerHTML src/app.js` is 21 at `480c05e`, at `010fd60` and at `3cd142d`,
  `git diff 480c05e 010fd60 -- src/app.js | grep '^+' | grep -c innerHTML` prints `0`, and so
  does `git diff 9a988b4 -- src/app.js | grep -c '^+.*innerHTML'` for the fix wave. Task 4's
  smoke run rendered a block of `<img src=x onerror=alert(1)>` as inert text.
- **H1: provenance is counts, never text.** `attachmentFor` (`src/vision-confirm.js:138`)
  stores kind, model id and family, verdict, `edited`, `override` and the block counts.
  `override` is true only when the user pressed "Use anyway" (`visionHandoff` passes the
  button), so a correction and an override stay apart for Stage 3. The verdict is the
  engine's, even when the gate opened it as review.
  `sendCompletion` takes the pending attachment and clears it before anything else
  (`src/app.js:2136`), so a typed turn cannot inherit it.
- **M1: the crop sent is small, and EXIF is gone.** `targetSize` caps the longest edge at
  `MAX_SIDE` 512 and never upscales (`src/vision-crop.js:11`, `:71`). `useCrop`
  (`src/app.js:3324`) draws the crop from the full-resolution bitmap onto a white-filled
  canvas and re-encodes it with `toDataURL('image/jpeg', 0.92)` (`:3338`). The canvas
  re-encode carries no EXIF block, GPS included. The decode applies the orientation first
  (`createImageBitmap(file, { imageOrientation: 'from-image' })`, `:3246`). The bitmap is
  closed once the crop is drawn, and only one decoded photo is ever held.
- **The thumbnail is a `data:` URL.** `src/app.js:3399`; the CSP in
  `src-tauri/tauri.conf.json` allows `img-src … data:`.
- **`prewarm_vision` only where the transcriber can stay beside the tutor.** The menu calls
  it only when `vision_state` says `coresident` (`src/app.js:3156`), and the command gates
  itself on installed and co-resident (`src-tauri/src/vision_cmds.rs:300`, Stage 1).
- **One transcription in flight.** The camera button is disabled while one runs, while the
  tutor streams, and while the composer is off (`src/app.js:3088`). The engine refuses a
  second call with "A transcription is already in progress."
  (`src-tauri/src/vision_cmds.rs:221`).
- **The camera intent is declared.** `<queries>` with `android.media.action.IMAGE_CAPTURE`,
  `src-tauri/gen/android/app/src/main/AndroidManifest.xml:44-48`. It is present in the
  packaged manifest.
- **M2: captured photos are swept, and the folder is not private.** wry's capture writes
  `JPEG_<stamp>_*.jpg` into `getExternalFilesDir(Pictures)`, which is app-scoped external
  storage. On Android 9 and lower any app holding READ_EXTERNAL_STORAGE can read it (minSdk is
  24). On Android 10 an app running with legacy storage can too. From Android 11 only USB/MTP
  or root reaches it. `CaptureHygiene.sweep`
  (`src-tauri/gen/android/app/src/main/java/com/cleophis/app/CaptureHygiene.kt`) deletes those
  files, and runs at each of these points:
  - on every activity start, from `MainActivity.onCreate` (`MainActivity.kt:10`);
  - over JNI (`src-tauri/src/mobile_native.rs:473`) after the base64 refusal
    (`vision_cmds.rs:232`) and in the single teardown after every transcription
    (`vision_cmds.rs:264`);
  - through the `sweep_captures` command (`vision_cmds.rs:381`, final review I1), which the
    page calls on mobile once `createImageBitmap` has settled, decoded or not, in `startCrop`
    (`src/app.js:3254`), and in `resetVision` (`src/app.js:3115`). `resetVision` runs on
    Retake, on every dismissal of the modal (the × close, Escape, the handoff, Type it) and as
    the modal opens. So a photo abandoned at Retake, at the close or at a decode failure is gone
    as soon as the page has it decoded or leaves the flow. A capture abandoned inside the camera
    app is gone when the menu closes.

  The file now exists only while the camera app is in front and until the crop pane has
  decoded it. Its keep rule is `src-tauri/gen/android/app/proguard-cleophis.pro:81`.
- **M2: backups exclude the external files.** `android:allowBackup="false"`
  (`AndroidManifest.xml:55`), plus `<exclude domain="external" />` in
  `res/xml/backup_rules.xml:51` and in both sections of `res/xml/data_extraction_rules.xml`
  (`:28` cloud backup, `:36` device transfer). Read, not device-tested.
- **M5: the gate fails closed when the engine and the UI disagree.** An `accept` verdict with
  any amber latex block opens as review (`src/vision-confirm.js:88`): Use waits for an edit,
  Use anyway is offered, and the review note shows. A node test reads `ACCEPT_MIN_CONFIDENCE`
  and `OPERATOR_MIN_PROB` out of `crates/kpack-engine/src/vision_parse.rs` and asserts they
  equal `CONFIDENCE_FLAG` and the now-exported `OPERATOR_FLAG` ("the gate thresholds equal the
  engine constants in vision_parse.rs").
- **L2: download trust is unchanged.** The vision files arrive only through
  `download_artifact`, which refuses shas the bundled catalog does not pin (Stage 1).

## What Stage 3 inherits

- **The 20-image probe set, the scorer and `vision-regression.yml` (CI).** Only five
  rendered items have been measured (Stage 0), and no photographed item.
- **Verdict calibration.** `CONFIDENCE_FLAG` 0.9 and `OPERATOR_FLAG` 0.97 in
  `src/vision-confirm.js`, and the engine's own constants, are plan values, uncalibrated. A
  node test keeps the two sides equal, so a recalibration changes both. The stored
  `override` flag says how often amber was overridden rather than corrected.
  V6 may need several tries to produce a review result for the same reason.
- **The page regime needs `single_formula: false`.** Task 1 measured that the default stop
  ends a page after its first formula: Stage 0's `p05-system` page lost its second equation.
  The crop flow is the only caller today, and it wants the default. The stop also drops a
  second formula in one crop without a warning (final review M6, deferred here). Item 7 of
  the final review's device-run watchlist, which is not a checklist step, asks for a crop of a
  two-equation system.
- **WebView memory beside the engine is unmeasured.** V2's decode-memory probe is the first
  reading.
- **The device numbers are Stage 0's plus four Stage 2 readings.** The founder's run measured four
  totals of 8,991–9,403 ms (see Device check); no larger sample exists yet.

## What did not run

- **The device steps ran after this record was first written** (see Device check): V1–V7 all
  passed on the A22 on 2026-11-13. Still unprobed: HEIC input, decode memory at the largest camera
  mode, one-thumb handles, Android back in each pane, a two-line problem, and the backup exclusion
  on a real backup.
- **A release build.** So the keep rules, including `CaptureHygiene`'s, are unexercised by R8,
  and `proguard-tauri.pro` is missing from this worktree.
- **`check-mobile-build.sh` and `vision_bench`.** No engine code changed in Task 6.
- **`cargo test -p cleophis` on WSL.** It was replaced by the Windows run.
- **In the fix wave:** `cargo test -p kpack-engine` and the headless page smoke. No engine file
  changed.

### Traps

- **`npm ci` in a fresh worktree.** `build-android-apk.sh:195` runs
  `$ROOT/node_modules/.bin/tauri`, and a new worktree has no `node_modules`. The build then
  stops within a second with `tauri exit status: 127`. Its log,
  `~/cleophis-mobile-logs/apk-debug-20261113-025026.log`, is 202 bytes:
  `node_modules/.bin/tauri: No such file or directory`. Run `npm ci` in the worktree first.
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
  runs, so no new capture overlaps the teardown sweep. The fix wave's two new sweep sites
  cannot overlap a capture either. Both run in the page, which is behind the camera app while a
  capture is open, and `resetVision`'s call as the modal opens comes before any capture. The
  one path left is the menu re-rendering itself to "unavailable" and closing while the camera
  app is in front, which needs the transcriber's files to vanish mid-capture.
- **Task 6:** `verify-apk.py` has no `CaptureHygiene` entry (checked by hand above); the
  foreground notification says "Writing a reply" while the device reads an image (for the
  copy pass); the build script still has no self-heal for the `TauriActivity.kt` trap
  (deferred since Phase 2).

- **Final review (S2-P16):** M6 (the single-formula stop drops a second formula without a
  warning) goes to Stage 3 and the device watchlist. M8 (`'Download'`, `'Downloading…'` and the
  no-size message are literals the checklist does not cite) goes to the copy pass. M9 (the store
  accepts any JSON as `attachments`) goes to a follow-up.

## Follow-ups outside this plan

- **`.lookupbar[hidden]{display:none}`.** `.lookupbar{display:flex}` defeats the `hidden`
  attribute, so "Look up a drug or condition" shows in the tutor chat on every platform.
  Pre-existing.
- **The M7 copy pass.** The Task 4 copy items above, the notification title, and the final
  review's M8 strings.
- **`attachments` validated in the store (final review M9).** `append_message` could accept
  only an object whose `kind` is `vision` and whose serialised size is small.
- **The metered-question order.** A tier switch during a vision download asks the metered
  question before it is refused.
- **The sweep's missing age guard.** Since the fix wave, an age guard on every sweep would
  defeat the decode-time sweep: the photo the crop pane has just decoded is seconds old. If
  one is ever added, it belongs on the start-up sweep only, where a capture under the camera is
  the case it protects.
- **`export_chat` carries `attachments` on photo turns.** The export contract widened in
  Task 2: counts, model, verdict, `edited` and, since the fix wave, `override`, never text. A
  reader of exports should expect the key.
- **From the plan:** the share-sheet target (ACTION_SEND) and a Tauri mobile plugin for
  capture (v0.2); a LaTeX renderer in the confirm modal (v0.1 shows monospace source);
  desktop drag-and-drop and paste, after the phone.

## Copy pass (2026-11-13, branch vision/copy-pass)

Three tasks plus the wrap-up closed the copy items deferred from Stage 2 (Task 4 copy items, the notification
title, and the final review's M8 strings). Reviewed on the branch at 484e58c and 813beba; the final review covered 58ff0c6..2b5c31e.

### What changed

**Task 1 (484e58c), the strings the checklist did not cite.**

| String | Text | Lives in |
|---|---|---|
| `OPENING_TEXT` | `Opening the photo…` (one ellipsis character) | `src/vision-confirm.js`, shown in `#cropOpening` by `app.js` |
| `DOWNLOAD_LABEL` | `Download` | `src/vision-download.js` |
| `DOWNLOADING_LABEL` | `Downloading…` | `src/vision-download.js` |
| `NO_SIZE_TEXT` | the former no-size literal, byte-identical | `src/vision-download.js` (thrown by `visionDownloadPlan`) |
| photo-reader download label | `Downloading the photo reader — N%` | `src/engine-state.js`, keyed on a progress event whose `modelId` starts with `vision/`; a `models/` path still says `Downloading your model — N%` |

Also in Task 1: `#cropOpening` in `index.html` (hidden, no text in markup, shown while the crop
pane decodes the photo and hidden when the frame appears); `visionTypeIt` no longer acts on a
disabled input (the Type-it guard); `phone-journey.test.mjs` resolves `visionDownload`; the V1
step 5 and V2 pins in `phone-journey.md` cite the constants.

**Task 2 (813beba), the foreground notification.** `TITLE_VISION = "Reading an image"` in
`InferenceService.kt`, chosen through `start(context, title)` and an intent extra that defaults
to `TITLE_REPLY`. `mobile_native.rs` gains `ServiceKind { Reply, Vision }` with `title()`;
`inference_service(running, kind)` passes it over JNI (`(Context;String)V`). `chat_cmds.rs` calls
with Reply, `vision_cmds.rs` with Vision. `phone-journey.md` V5 keeps the `Writing a reply` pin and
adds a `Reading an image` pin. Both pins cite `mobile_native.rs`: Rust owns the title strings in `ServiceKind::title()` and passes them over JNI, so Kotlin's `TITLE_REPLY` and `TITLE_VISION` are only a mirror (M1, deferred).

### Gates (verbatim from the task reports)

- `npm test` at 2b5c31e: 1286 tests, 0 failures.
- `npm test` (Task 1): `# pass 1286`, `# fail 0`. Task 2 re-run: tests 1286, pass 1286, fail 0.
- Android (Task 2): `RUSTFLAGS='-D warnings' cargo ndk -t arm64-v8a -P 24 check -p cleophis --all-targets --release` finished the `release` profile in 11m 04s, exit 0, zero warnings.
- Windows desktop on 813beba (Task 2): `test result: ok. 460 passed; 0 failed; 10 ignored`, exit 0.
- Fix wave (09a96e3, the title follows the latest holder): Android `-D warnings` check `Finished` with zero warnings on 09a96e3. Windows desktop 460 passed, 0 failed on 09a96e3 after one rerun. The first run's single failure was `cloud::session::tests::restore_refresh_400_purges_everything`, a real-credential-store test that collided with another branch's concurrent run.
- `npm test` on 8402ff7: `# tests 1286`, `# pass 1286`, `# fail 0`.

### The debug APK of record (tutor variant, built from 09a96e3)

Install `~/cleophis-artifacts/cleophis-vision-copy-09a96e3.apk`. The two commits after 09a96e3 (8402ff7, 10ef9fa) are docs-only; the code in the APK is HEAD's.

| Fact | Value |
|---|---|
| Commit | `09a96e37b5f82a7dbf6f700d59f632e409b6304f`, tree clean at PRE and POST |
| Build log | `~/cleophis-mobile-logs/apk-debug-20261113-232020.log` |
| Provenance | `~/cleophis-mobile-logs/apk-debug-20261113-232020.provenance`, `tauri_exit 0` |
| Built path | `src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk` |
| Archived | `~/cleophis-artifacts/cleophis-vision-copy-09a96e3.apk` |
| sha256 | `227e3eb35b801fc7a0f4a8c4c0d265c805f5c8fd51dbe4b0c4ba1472b7df5fa4` (rechecked on both copies) |
| Size | 380,267,262 bytes (362.65 MiB) |
| `verify-apk.py` | `VERDICT: PASS`; entries 970; unaccounted 178,268 (0.05%); largest `lib/arm64-v8a/libcleophis_lib.so` (370,638,712), `classes.dex` (10,260,852), `classes9.dex` (4,164,264) |
| `libcleophis_lib.so` sha256 | `ea45baf4426514dca82bf5953bf2477434a9b6f2edc37421af15d347c6289adb` |
| Arch audit | `audit-arch-floor.sh` PASS: nothing above the floor outside 8 outline-atomics helper(s) |
| Dex check | `Reading an image` occurs once in `classes8.dex` (the dex that holds `InferenceService`) and in no other dex. |
| Frontend strings | Not greppable from the APK: the frontend is embedded in the `.so`. The JS strings are covered by the 1286 tests, not by the APK. |

**Superseded: the 813beba build.** `~/cleophis-artifacts/cleophis-vision-copy-813beba.apk`
(sha256 `538291a40eea262d0306115fa9522927c6f0631a8cb313b48df7c51d4692b421`, 380,322,318 bytes,
log `apk-debug-20261113-214724.log`) lacks the title-refresh fix. Do not install it. Its checks
at the time were `verify-apk.py` PASS, the arch audit PASS with 8 helpers, and `Reading an image`
once in `classes8.dex`.

### What the founder re-checks on this APK

- **V1, the Download button and the photo-reader row.** The menu button reads `Download` (pin `visionDownload.DOWNLOAD_LABEL`), then `Downloading…` (pin `visionDownload.DOWNLOADING_LABEL`). With the modal closed, the bar above the composer reads `Downloading the photo reader — N%`; the row shows only with the modal closed. Preparation: the A22 still holds both vision files from the Stage 2 run, and `adb install -r` keeps app data, so the menu would go straight to Take a photo. First force-stop the app and remove the two files (a 269 MB re-download):

  ```text
  adb shell am force-stop ${PKG:-com.cleophis.app}
  adb shell run-as ${PKG:-com.cleophis.app} rm models/granite-docling-258M-Q8_0.gguf models/mmproj-granite-docling-258M-Q8_0.preproc512.gguf
  ```

  Relaunch: `vision_state` should report `installed: false` and the menu shows the 269 MB line and `Download`.
- **V2, the opening text.** After the camera returns, the crop pane says `Opening the photo…` until the frame appears. Pin: `visionConfirm.OPENING_TEXT`.
- **V5, the notification title.** During a reading, the shade shows `Reading an image`; a chat reply still shows `Writing a reply`. Pin: `expect-source` on `mobile_native.rs`. After a cancelled reading the title follows the latest holder (see Deferred and open).

### Deferred and open

- **Explicit hide on the error path: DONE in 2b5c31e.** `startCrop`'s error path now hides `#cropOpening` explicitly. The commit is JS-only and postdates the 813beba APK, it is inside the 09a96e3 APK of record (the refuse pane replaces the crop pane either way, so it is not visible on the device). It cannot be seen on the device, because the refuse pane replaces the crop pane either way.
- **The title follows the latest holder: fixed after the final review (I3).** Before the fix the counted service called Kotlin only on 0 to 1, so a reply sent while a cancelled reading was still finishing was titled "Reading an image"; `service_transition` now returns `start` on every begin and the shade names the latest holder. The 813beba APK predates this fix; the APK of record (09a96e3) carries it.
- **Minor items deferred (final review).** M1: the `ServiceKind` test cannot catch Rust/Kotlin drift and `TITLE_VISION` is unread; delete it with the next Kotlin change or add a comparing probe. M2: the notification channel name "Writing replies" and its description also cover readings. M3: during the photo-reader download the `requesting` phase still says "Asking for your copy of the model." and the percentage runs twice from 0%. M4: the picture moves up one line when `#cropOpening` hides. M5: the Kotlin comment about a null intent names a case `START_NOT_STICKY` all but rules out. `NO_SIZE_TEXT` is an error the pinned catalog cannot produce, so only its unit test pins it.
- **Kotlin first compiled in this build (Task 2 note).** The Android `cargo check` covers Rust only. The `InferenceService.kt` change compiled for the first time in the 813beba build (and again in 09a96e3), which passed with `tauri_exit 0`, and the dex check confirms the title string is present.
- **The concurrent Windows run (Task 2 concern).** Task 2's red run checked out a branch in the shared Windows worktree while another branch's run may have been mid-compile; that run finished ok, so treat its result as probably unaffected.
