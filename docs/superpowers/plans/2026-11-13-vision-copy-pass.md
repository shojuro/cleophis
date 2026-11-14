# Vision Input — Copy Pass Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every string the photo-input feature shows the user says the true thing and is a pinned constant: the foreground notification names the reading, the engine row names the photo-reader download, the busy crop pane has words, and the three remaining literals become constants the checklist pins.

**Architecture:** Three small, disjoint changes. (1) JS: new exported constants in `src/vision-confirm.js` / `src/vision-download.js`, a vision-aware download label in `src/engine-state.js` keyed on the progress event's `modelId` path (no new state), and `src/app.js` wired to them. (2) Android: `InferenceService.start(context, title)` takes the title through an Intent extra; `mobile_native::inference_service(running, kind)` passes `"reply"` or `"vision"`; the two callers name their kind. (3) A debug APK and a dated addendum to the Stage 2 record so the founder re-checks the three screens.

**Tech Stack:** Tauri 2.11.5 (Rust + Kotlin), vanilla JS modules tested with `node --test`, the existing checklist pin test.

**Spec:** the Stage 2 plan's Global Constraints and its deferred M7/M8 items — `docs/superpowers/plans/2026-11-13-vision-stage2-capture-confirm.md`; the final review's M8 and the fix wave's concerns in `~/cleophis-artifacts/sdd-archive/2026-11-13-vision-stage2-capture-confirm/final-review-report.md`.

## Global Constraints

- Worktree `~/cleophis-wt/copy-pass`, branch `vision/copy-pass` (from `origin/main` 4aadcc9). Never `cd` into the original repository roots. Never bare `git stash`. No secrets in the repo.
- Verification ruling V1 stands: JS by `npm test`; Rust by the Android check `RUSTFLAGS='-D warnings' cargo ndk -t arm64-v8a -P 24 check -p cleophis --all-targets --release` (from `src-tauri/`, after `source ../.superpowers/ndk-env.sh`) and the Windows run `cargo test --locked -p cleophis -- --test-threads=1` in `C:\Users\JM505 Computers\dev\cleophis-wt-win\vision-p1` detached at the commit under test, via `/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe`. Kotlin compiles only in the APK build (Task 3).
- Text the phone shows is pinned: every user-visible string this plan adds or moves is an exported constant in a pure module, cited by the checklist with `<!-- expect: module.CONST -->`, or a literal the checklist cites by file with `<!-- expect-source: path -->`. `probes/phone-journey.test.mjs` must pass.
- No `innerHTML` with model text; copy is set with `textContent`.
- D-3: no bare `#![allow(dead_code)]`; cfg'd allows carry a rationale. Zero warnings on the Android check.
- Commit trailers (last lines, this order): `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>` then `Claude-Session: https://claude.ai/code/session_01W9ZnszzonHJc3yCaT3d2qM`. Commit after every task.
- Tests first (RED → GREEN) for every pure-module change.

## File structure

| File | Responsibility |
|---|---|
| `src/vision-confirm.js` (+ test) | `OPENING_TEXT` — the busy crop pane's words |
| `src/vision-download.js` (+ test) | `DOWNLOAD_LABEL`, `DOWNLOADING_LABEL`, `NO_SIZE_TEXT` |
| `src/engine-state.js` (+ test) | the photo-reader download label and detail |
| `src/app.js`, `src/index.html` | wire the constants; the busy text element; Type-it does not focus a disabled input |
| `probes/phone-journey.test.mjs`, `docs/superpowers/mobile-tools/phone-journey.md` | register `visionDownload`; pins for V1 (Download), V2 (opening text), V5 (the notification title) |
| `src-tauri/gen/android/app/src/main/java/com/cleophis/app/InferenceService.kt` | `start(context, title)`; `onStartCommand` reads the extra; default title on a null intent |
| `src-tauri/src/mobile_native.rs`, `chat_cmds.rs`, `vision_cmds.rs` | `inference_service(running, kind)`; desktop and iOS stubs |
| `docs/superpowers/verification-vision-stage2.md` | a dated "Copy pass" addendum with the APK facts |

---

### Task 1: The JS copy — constants, the photo-reader download label, the busy crop pane, the pins

**Files:**
- Modify: `src/vision-confirm.js`, `src/vision-confirm.test.mjs`, `src/vision-download.js`, `src/vision-download.test.mjs`, `src/engine-state.js`, `src/engine-state.test.mjs`, `src/app.js`, `src/index.html`, `probes/phone-journey.test.mjs`, `docs/superpowers/mobile-tools/phone-journey.md`

**Interfaces:**
- Produces: `vision-confirm.js` → `export const OPENING_TEXT = 'Opening the photo…';` `vision-download.js` → `export const DOWNLOAD_LABEL = 'Download'; export const DOWNLOADING_LABEL = 'Downloading…'; export const NO_SIZE_TEXT = 'The signed catalog lists no size for the vision files — please update the app.';` (the throw in `visionDownloadPlan` uses `NO_SIZE_TEXT`). `engine-state.js`: `describeEngineState` — when `download.phase === 'downloading'` and `typeof download.modelId === 'string' && download.modelId.startsWith('vision/')`, `label` is `` `Downloading the photo reader — ${pct}%` `` and the no-total `detail` is `'Fetching the photo reader files.'`; `kind`, `short`, `tone`, `prominent` unchanged. (The progress event's `modelId` is the artifact path; `app.js` already stores the whole event in `state.dlProgress`.)
- Consumes: nothing new.

- [ ] **Step 1: RED tests**
  - `vision-confirm.test.mjs`: `OPENING_TEXT === 'Opening the photo…'` (the ellipsis is the single character `…`).
  - `vision-download.test.mjs`: the three constants equal the strings above; `visionDownloadPlan` with a non-numeric size throws `NO_SIZE_TEXT` exactly (`assert.throws(fn, { message: NO_SIZE_TEXT })`).
  - `engine-state.test.mjs`: `describeEngineState({ download: { phase: 'downloading', modelId: 'vision/granite-docling-258M/v1/x.gguf', bytesDownloaded: 1, totalBytes: 4 } })` → `label === 'Downloading the photo reader — 25%'`, `kind === 'downloading'`; the same with `modelId: 'models/Qwen3-1.7B/v1/x.gguf'` → `label` starts with `'Downloading your model'`; with `modelId` absent → unchanged label; `totalBytes: 0` + vision → `detail === 'Fetching the photo reader files.'`.
  - `npm test` → the new assertions FAIL (constants undefined; old label).
- [ ] **Step 2: Implement** the constants and the label.
- [ ] **Step 3: app.js + index.html.** Replace the module-level `VISION_DOWNLOAD_LABEL` / `VISION_DOWNLOADING_LABEL` with the imported `DOWNLOAD_LABEL` / `DOWNLOADING_LABEL`. In the crop pane add `<p class="sub" id="cropOpening" hidden></p>` (above the canvas); `startCrop` sets its `textContent = OPENING_TEXT` once at startup with the other static labels and shows it (`hidden = false`) for the busy state, hiding it when the frame appears or on error. `visionTypeIt`: `const input = $('chatInput'); hide($('visionModal')); if (!input.disabled) input.focus();`. `grep -n innerHTML src/app.js` unchanged.
- [ ] **Step 4: Pins.** In `probes/phone-journey.test.mjs` register `import * as visionDownload from '../src/vision-download.js'` beside `visionConfirm` (same resolver). In the checklist: V1 step 4/5 cite `<!-- expect: visionDownload.DOWNLOAD_LABEL -->` with a ```text block `Download`; V2 adds one sentence that the crop pane says the opening text while the photo decodes, pinned `<!-- expect: visionConfirm.OPENING_TEXT -->`. Every ```text block must be claimed (the existing "every text block is claimed" test).
- [ ] **Step 5: GREEN** — `npm test` all pass (baseline 1283 on main). **Step 6: Commit** — `vision(copy): the photo reader's own words — opening, download, no-size; pinned`.

---

### Task 2: The notification names the reading

**Files:**
- Modify: `src-tauri/gen/android/app/src/main/java/com/cleophis/app/InferenceService.kt`, `src-tauri/src/mobile_native.rs`, `src-tauri/src/chat_cmds.rs`, `src-tauri/src/vision_cmds.rs`, `docs/superpowers/mobile-tools/phone-journey.md`

**Interfaces:**
- Produces: Kotlin `@JvmStatic fun start(context: Context, title: String)` (the old one-argument `start` is removed; `stop` unchanged); `companion object` holds `const val EXTRA_TITLE = "title"`, `const val TITLE_REPLY = "Writing a reply"`, `const val TITLE_VISION = "Reading an image"`; `onStartCommand` builds the notification with `intent?.getStringExtra(EXTRA_TITLE) ?: TITLE_REPLY` (a null intent on a system restart keeps the reply title). Rust: `pub(crate) enum ServiceKind { Reply, Vision }` with `fn title(self) -> &'static str` returning exactly those two strings; `pub(crate) fn inference_service(running: bool, kind: ServiceKind)`; the start call's JNI signature becomes `(Landroid/content/Context;Ljava/lang/String;)V` with the title as a `JObject` string (`env.new_string`), the stop call unchanged `(Landroid/content/Context;)V`; `service_transition` unchanged. Desktop and iOS stubs take the same two parameters (`_kind`). Callers: `chat_cmds.rs` → `ServiceKind::Reply`; `vision_cmds.rs` → `ServiceKind::Vision` for both the start and the stop (the count, not the kind, decides the transition; a reading that starts while a reply is streaming cannot happen — the composer is disabled — so the title shown is the latest start's).
- Consumes: nothing from Task 1.

- [ ] **Step 1: RED** — a desktop-visible unit test in `mobile_native.rs` (or beside `service_transition`'s tests) asserting `ServiceKind::Reply.title() == "Writing a reply"` and `ServiceKind::Vision.title() == "Reading an image"`; it fails to compile until the enum exists. **Step 2: Implement** Kotlin + Rust + callers. **Step 3: Checklist** — V5 keeps the `expect-source: InferenceService.kt` pin `Writing a reply` for the chat case and adds a second pin for the reading: `<!-- expect-source: src-tauri/gen/android/app/src/main/java/com/cleophis/app/InferenceService.kt -->` ```text `Reading an image` ``` with one sentence: during a reading the notification's title is the second string. **Step 4: Gates** — `npm test` (pin test); Android `-D warnings` check `Finished`, zero warnings; Windows desktop suite on the commit (expect 459 + the new test). **Step 5: Commit** — `vision(copy): the foreground notification says "Reading an image" during a reading`.

---

### Task 3: Debug APK and the record addendum

**Files:**
- Modify: `docs/superpowers/verification-vision-stage2.md` (append `## Copy pass (2026-11-13)`)

- [ ] **Step 1:** build the debug APK from the Task 2 commit with `docs/superpowers/mobile-tools/build-android-apk.sh` (the worktree needs `npm ci` and `generated/TauriActivity.kt` first — copy that one file from `~/cleophis-wt/p2-final/src-tauri/gen/android/app/src/main/java/com/cleophis/app/generated/`; both traps are in the Stage 2 record). Record path, sha256, bytes/MiB, log; `verify-apk.py` PASS; the arch audit; archive to `~/cleophis-artifacts/cleophis-vision-copy-<sha>.apk`. The Kotlin change compiles for the first time here — a Gradle failure in `InferenceService.kt` is a BLOCKED report, not a patch.
- [ ] **Step 2:** the addendum: what each task changed (the five strings and where they live), the gates verbatim, the APK table, and the three screens the founder re-checks (V1 Download, V2 the opening text, V5 the notification title), with their pins.
- [ ] **Step 3: Commit** — `docs(vision): copy pass — record addendum and the debug APK facts`.
