# Vision Input — Stage 2 (capture, confirm, handoff on Android) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A student on the Android app taps a camera button, photographs or picks an image of a maths problem, frames the formula, waits while the device reads it, confirms or edits the LaTeX the transcriber produced, and the tutor starts from that text — entirely on-device, with the image never persisted and never reaching the model that teaches.

**Architecture:** Stage 1 built the engine (`transcribe_image` / `prewarm_vision` / `vision_state`, the parser, the verdict). Stage 2 is the WebView side plus two small native pieces. Pure modules first, tested under `node --test`: crop geometry (`vision-crop.js`), the confirmation gate and handoff (`vision-confirm.js`), download availability (`vision-download.js`). `app.js` and `index.html` get the DOM glue: a composer camera button that is visible only when the tier declares a transcriber, a capture flow through the WebView's own `<input type="file">` (camera via `capture="environment"`, gallery/screenshot via a second input), a crop frame, a "Reading the image…" state, the hard confirmation modal, and the handoff into the existing `sendCompletion` as a user-role message prefixed "Problem from photo:". Provenance (model, verdict, edited flag, block count — never bytes) goes into a new nullable `messages.attachments` column. Android gets a manifest `<queries>` entry (without it the WebView falls back to the file picker silently) and a capture-file sweep. The engine gets the single-formula stop the Stage 1 review recommended.

**Tech Stack:** vanilla ES modules (`src/*.js`, `node --test`), Tauri 2 IPC (`invoke` + `Channel`), Rust (`src-tauri`, rusqlite), Kotlin (one small object + JNI call), the Stage 1 engine.

**Spec:** `docs/superpowers/specs/2026-10-31-vision-input-spec-v0.1.md` (the founder's spec: §1 goal and the confirmation gate, §5 UX, §7 task list), argued through `docs/superpowers/specs/2026-11-01-vision-input-review.md` (Stage 2 section). Stage 1's contract for this stage: `docs/superpowers/verification-vision-stage1.md` ("What Stage 2 inherits": command surface, wire shape, `EngineInfo.vision`, security inheritances).

## Global Constraints

- Worktree `~/cleophis-wt/vision-p2`, branch `mobile/vision-p2` (from `mobile/vision-p1`; the catalog block from `mobile/vision-catalog` is merged in before Task 4). Never `cd` into the original repository roots. Never bare `git stash`. No secrets in the repo.
- Verification ruling V1 stands: the desktop suite does not build on the WSL box; Rust changes are gated by `RUSTFLAGS='-D warnings' cargo ndk -t arm64-v8a -P 24 check -p cleophis --all-targets --release` (from `src-tauri/`, after `source .superpowers/ndk-env.sh`) and by the Windows run `cargo test --locked -p cleophis -- --test-threads=1` in the Windows worktree `C:\Users\JM505 Computers\dev\cleophis-wt-win\vision-p1` (checked out detached at the commit under test) via `/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe`. Front-end changes are gated by `npm test`. Engine changes by `cargo test -p kpack-engine` and the host `vision_bench --via-engine` run.
- Security inheritances from Stage 1 (binding): only `latex` blocks reach the tutor; the handoff is a USER-role message beginning `Problem from photo:`; the confirm UI renders every block with `textContent` (never `innerHTML` with model text); the image is downscaled/cropped in the WebView before `transcribe_image` so the engine's 20 MiB / 50 MP caps are a backstop; image bytes never touch the conversation store or disk on the Rust side; the thumbnail is a `data:` URL (the CSP allows `img-src data:`); `prewarm_vision` is invoked only when `vision_state` reports `coresident`; one transcription in flight (the engine refuses a second; the UI disables the button while one runs).
- Text the phone shows is pinned: every user-visible string this stage adds is an exported constant in a pure module (so `probes/phone-journey.test.mjs` can pin the checklist to it) or a literal the checklist cites by file.
- The crop sent to the engine has its longest edge at most 512 px (the measured regime), JPEG quality 0.92, from a canvas re-encode (which also drops EXIF, including any GPS tag).
- D-3: no bare `#![allow(dead_code)]`; cfg'd allows carry a rationale. Zero warnings on the Android check.
- Commit trailers (last lines, this order): `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>` then `Claude-Session: https://claude.ai/code/session_01W9ZnszzonHJc3yCaT3d2qM`. Commit after every task.
- Tests first (RED → GREEN) for every pure module and every Rust change.

## File structure

| File | Responsibility |
|---|---|
| `crates/kpack-engine/src/backend.rs`, `src/vision.rs`, `src/vision_prep.rs` | single-formula stop (`VisionConfig.single_formula`), alpha composited over white |
| `src-tauri/src/convstore.rs` | `messages.attachments` (nullable JSON), `MessageInfo.attachments`, `append_message(.., attachments)` |
| `src-tauri/src/chat_cmds.rs`, `src-tauri/src/lib.rs` | the `append_message` command signature and any other caller of `append_message` |
| `src/vision-crop.js` (+ `.test.mjs`) | crop-frame geometry, display↔image mapping, target size |
| `src/vision-confirm.js` (+ `.test.mjs`) | verdict → UI mode, block flags, `canUse`, `composeHandoff`, `attachmentFor`, pinned copy |
| `src/vision-download.js` (+ `.test.mjs`) | availability from `vision_state` + catalog; the two-artefact download plan |
| `src/index.html`, `src/styles.css`, `src/app.js` | composer camera button, hidden file inputs, the vision modal (crop / reading / confirm / refuse), the flow, `state.vision`, download, handoff, attachment on the user turn |
| `src-tauri/gen/android/app/src/main/AndroidManifest.xml` | `<queries>` for `android.media.action.IMAGE_CAPTURE` |
| `src-tauri/gen/android/app/src/main/java/com/cleophis/app/CaptureHygiene.kt`, `MainActivity.kt`, `src-tauri/src/mobile_native.rs`, `src-tauri/src/vision_cmds.rs`, `proguard-rules.pro` | sweep of the WebView's captured JPEGs (on create, after each transcription) |
| `docs/superpowers/mobile-tools/phone-journey.md`, `probes/phone-journey.test.mjs` | checklist steps V1–V7, pinned to the constants |
| `docs/superpowers/verification-vision-stage2.md` | the record |

---

### Task 1: Engine hardening — single-formula stop and alpha over white

**Files:**
- Modify: `crates/kpack-engine/src/backend.rs` (`VisionConfig`), `crates/kpack-engine/src/vision.rs`, `crates/kpack-engine/src/vision_prep.rs`
- Modify: `crates/kpack-engine/examples/vision_bench.rs` (a `--multi-formula` flag that sets `single_formula: false` in the `--via-engine` path, so the page regime stays measurable)

**Interfaces:**
- Produces: `VisionConfig.single_formula: bool` (default `true`): when true, generation stops after the first `</formula>` token is sampled AND rendered (the parser still sees the closer). Doc: the crop regime expects one formula; the page regime sets it false.
- Produces: `vision_prep::prepare_image` composites images that carry alpha over opaque white before downscaling.

- [ ] **Step 1: RED** — `backend.rs` tests: `fn vision_config_defaults_to_the_single_formula_crop_regime() { assert!(VisionConfig::default().single_formula); }`. `vision_prep.rs` tests:

```rust
    #[test]
    fn alpha_is_composited_over_white_not_dropped() {
        // A 2x1 RGBA PNG: left pixel fully transparent black, right pixel opaque black.
        let mut img = image::RgbaImage::new(2, 1);
        img.put_pixel(0, 0, image::Rgba([0, 0, 0, 0]));
        img.put_pixel(1, 0, image::Rgba([0, 0, 0, 255]));
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        let p = prepare_image(&bytes, 512).unwrap();
        assert_eq!(&p.rgb[0..3], &[255, 255, 255], "transparent becomes white, not black");
        assert_eq!(&p.rgb[3..6], &[0, 0, 0]);
    }
```
Run `cargo test -p kpack-engine vision_prep alpha` → FAIL (black). Run the backend test → FAIL (no field).

- [ ] **Step 2: Implement.** `backend.rs`: add `pub single_formula: bool` to `VisionConfig` with the doc above; `Default` sets `true`. `vision_prep.rs`: after orientation and downscale, if `img.color().has_alpha()`, convert with `img.to_rgba8()` and blend each pixel `c' = (c*a + 255*(255-a)) / 255` into an `RgbImage` (integer arithmetic, rounding to nearest); otherwise `to_rgb8()` as now. `vision.rs`: at load, look up the `</formula>` token the same way the `</doctag>` id is found (exactly one token or `None`), keep it as `formula_end: Option<LlamaToken>`; in the loop, after pushing the piece for a sampled token, `if self.cfg.single_formula && Some(token) == self.formula_end { stop = StopReason::Eos; break; }` (document in a comment that the closer is rendered first so `vision_parse` ends the element on it). `vision_bench.rs`: `--multi-formula` → `single_formula: false` in the via-engine config.

- [ ] **Step 3: GREEN + native check.** `cargo test -p kpack-engine` green, zero warnings. Host: build with the vision feature (env from Stage 1's plan: `CARGO_TARGET_DIR=~/cleophis-host-target BINDGEN_EXTRA_CLANG_ARGS="-I$HOME/android-ndk-r27c/toolchains/llvm/prebuilt/linux-x86_64/lib/clang/18/include" LIBCLANG_PATH=$HOME/.local/lib/python3.10/site-packages/clang/native`), run `vision_bench --via-engine --one-pass` on `~/cleophis-artifacts/vision/spike-images-formula/items.json` with `--max-tokens 96 --prompt "Convert formula to latex."`: five Accept, raws identical to the Stage 1 fix-wave table (they already end at `</formula>`). Then a probe that proves the stop: the same run with `--max-tokens 200` must still stop at `</formula>` (token count unchanged, `stop Eos`).

- [ ] **Step 4: Commit** — `vision(s2-1): single-formula stop for the crop regime; alpha composited over white`.

---

### Task 2: Provenance column — `messages.attachments`

**Files:**
- Modify: `src-tauri/src/convstore.rs` (schema SQL, `migrate_messages_columns`, `MESSAGE_COLUMNS`, `message_from_row`, `MessageInfo`, `append_message`), `src-tauri/src/chat_cmds.rs` and any other caller of `ConvStore::append_message` (find with `grep -rn 'append_message(' src-tauri/src`)

**Interfaces:**
- Produces: `messages.attachments TEXT NULL` (JSON); `MessageInfo.attachments: Option<serde_json::Value>` with `#[serde(default, skip_serializing_if = "Option::is_none")]`; `ConvStore::append_message(user_id, chat_id, role, content, citations, tool_calls, guard, attachments: Option<Value>)`; the `append_message` Tauri command gains an optional `attachments: Option<Value>` argument (camelCase `attachments` on the wire; omitted = `None`).
- The value the front end writes for a photo turn (Task 3 defines it): `{"kind":"vision","model":{"id":"granite-docling-258M","family":"docling"},"verdict":"accept"|"review","edited":false,"blocks":2}`. Never image bytes, never block text.

- [ ] **Step 1: RED** — convstore tests (follow the file's existing test helpers for an in-memory or temp store):

```rust
    #[test]
    fn attachments_round_trip_and_are_omitted_when_absent() {
        let (store, user) = test_store();
        let chat = store.create_chat(&user, "t", None, &[], "m", &[]).unwrap();
        let att = serde_json::json!({"kind":"vision","model":{"id":"granite-docling-258M","family":"docling"},"verdict":"accept","edited":false,"blocks":1});
        let with = store.append_message(&user, chat.id, "user", "Problem from photo: x=2", None, None, None, Some(att.clone())).unwrap();
        let without = store.append_message(&user, chat.id, "assistant", "ok", None, None, None, None).unwrap();
        let rows = store.get_chat(&user, chat.id).unwrap().messages;
        assert_eq!(rows[0].attachments, Some(att));
        assert_eq!(rows[1].attachments, None);
        let json = serde_json::to_value(&rows[1]).unwrap();
        assert!(json.get("attachments").is_none(), "absent attachments are not on the wire");
        let _ = (with, without);
    }

    #[test]
    fn an_old_database_gains_the_attachments_column() {
        // Open a store whose messages table predates the column (create it with the
        // pre-Stage-2 DDL in a temp file), then open through ConvStore and assert
        // PRAGMA table_info(messages) lists `attachments`.
        …
    }
```
(Adapt helper names to the file's existing test module; the second test uses the same technique the existing `partial`/`guard` migration tests use — read them.) Run on Windows later; for now `cargo ndk … check --all-targets` compiles them.

- [ ] **Step 2: Implement** — schema SQL gains `attachments TEXT`; `migrate_messages_columns` gains `("attachments", "ALTER TABLE messages ADD COLUMN attachments TEXT")`; `MESSAGE_COLUMNS` appends `, attachments`; `message_from_row` reads it as `Option<String>` → `serde_json::from_str` → `Option<Value>`; `append_message` writes it (both the INSERT and the partial-row UPDATE paths); the Tauri command `append_message` in `convstore.rs` (or wherever `#[tauri::command] append_message` lives) gains `attachments: Option<Value>` and passes it through; every other caller passes `None`.

- [ ] **Step 3: GREEN** — Android `-D warnings` check Finished; then the Windows desktop suite on the commit (both tests must appear in the log).

- [ ] **Step 4: Commit** — `vision(s2-2): messages.attachments — provenance for a photo turn, never bytes`.

---

### Task 3: Pure modules — crop geometry, the confirmation gate, download availability

**Files:**
- Create: `src/vision-crop.js`, `src/vision-crop.test.mjs`, `src/vision-confirm.js`, `src/vision-confirm.test.mjs`, `src/vision-download.js`, `src/vision-download.test.mjs`

**Interfaces (all pure, no DOM, no `invoke`):**

`vision-crop.js`
```js
export const MAX_SIDE = 512;           // the measured regime's longest edge
export const MIN_CROP_PX = 48;         // smallest crop edge in DISPLAY pixels
export function fitContain(imgW, imgH, boxW, boxH)  // → {scale, x, y, w, h}: the image drawn to fit the box, centred
export function defaultCrop(fit)                   // → {x, y, w, h} in display px: centred, 88% of the drawn width, 34% of the drawn height, clamped
export function clampCrop(rect, fit)               // keeps the rect inside the drawn image, ≥ MIN_CROP_PX each side
export function moveCrop(rect, dx, dy, fit)        // translate then clamp
export function resizeCrop(rect, handle, dx, dy, fit) // handle ∈ 'nw'|'ne'|'sw'|'se'|'n'|'s'|'e'|'w'; opposite edge fixed; then clamp
export function cropToImagePixels(rect, fit)       // → {sx, sy, sw, sh} integers in SOURCE image pixels
export function targetSize(sw, sh, maxSide = MAX_SIDE) // → {w, h}: downscale so the longest edge ≤ maxSide, never upscale, ≥ 1
```

`vision-confirm.js`
```js
export const READING_TEXT = 'Reading the image…';
export const TAKE_PHOTO_LABEL = 'Take a photo';
export const CHOOSE_IMAGE_LABEL = 'Choose an image';
export const USE_LABEL = 'Use this';
export const USE_ANYWAY_LABEL = 'Use anyway';
export const RETAKE_LABEL = 'Retake';
export const TYPE_IT_LABEL = 'Type it instead';
export const HANDOFF_PREFIX = 'Problem from photo:';
export const REFUSE_NO_MATHS = 'I could not find any maths in this image. Try a closer photo of the problem, or type it.';
export const REFUSE_LOW_CONFIDENCE = 'I could not read this clearly enough to be sure. Try a closer, straighter photo, or type it.';
export const REVIEW_NOTE = 'Check the parts marked in amber before you continue.';
export const WARNING_COPY = { lowConfidence: 'Some symbols were hard to read.', operatorUncertain: 'A sign or operator may be wrong.', noMathFound: 'No maths was found.', invalidLatex: 'A bracket is unbalanced.', truncated: 'The reading was cut short.' };
export const CONFIDENCE_FLAG = 0.9;    // below this a block is shown in amber (plan threshold; Stage 3 calibrates)
export function confirmState(result)   // result = TranscribeResult (wire). → { mode: 'accept'|'review'|'refuse', blocks: [{id, kind, content, confidence, flagged}], notes: string[], refuseText: string|null, canUse: bool, canUseAnyway: bool }
export function applyEdit(state, id, text) // → new state with blocks[id].content = text, edited: true, flagged: false (a human looked at it)
export function composeHandoff(state)  // → `${HANDOFF_PREFIX}\n${latex blocks joined by '\n'}` using ONLY kind === 'latex' blocks; throws if none
export function attachmentFor(result, state) // → { kind:'vision', model:{id,family}, verdict, edited, blocks } (counts only; never text)
```
Rules: `refuse` when `result.verdict === 'refuse'` (refuseText from `noMathFound` → `REFUSE_NO_MATHS`, else `REFUSE_LOW_CONFIDENCE`), `canUse=false`, `canUseAnyway=false`. `review` when verdict is `review`: `canUse=false`, `canUseAnyway=true`, notes = the warnings' copy + `REVIEW_NOTE`. `accept`: `canUse=true`. A block is `flagged` when `confidence < CONFIDENCE_FLAG` or (`minOperatorProb` present and `< 0.97`). After any edit, `canUse` becomes true in review mode (a human confirmed). Text blocks are shown (grey) but never handed off.

`vision-download.js`
```js
export const VISION_DOWNLOAD_PROMPT = (bytes) => `Photo input needs a one-time ${formatBytes(bytes)} download.`;  // formatBytes from download-policy.js
export function visionAvailability({ variant, visionState })
  // variant = visionVariantFor(hero, tier) | null; visionState = vision_state() | null
  // → 'unavailable' (no variant or desktop null), 'needs-download' (declared && !installed), 'ready' (installed)
export function visionDownloadPlan(artifacts, variant) // → { base, mmproj, totalBytes } via pickVisionArtifacts (throws its readable errors)
```

- [ ] **Step 1: RED tests** (write all three test files first; representative cases — the implementer writes them in full):
  - crop: `fitContain(4000,3000, 360,640)` scales by 0.09 and centres vertically; `defaultCrop` lies inside the fit and is ≥ MIN; `clampCrop` pushes an out-of-bounds rect back and enforces the minimum; `resizeCrop(rect,'se',…)` keeps `x,y` fixed; `cropToImagePixels` inverts `fitContain` (round-trip within 1 px); `targetSize(2000,700)` → `{512,179}`; `targetSize(300,100)` → `{300,100}` (no upscale).
  - confirm: an accept result with one latex block → `mode 'accept'`, `canUse`, handoff `Problem from photo:\n3x-7=2x+5`; a review result with `operatorUncertain` → `canUse false`, `canUseAnyway true`, the block flagged, notes contain `WARNING_COPY.operatorUncertain` and `REVIEW_NOTE`; after `applyEdit` the block is unflagged and `canUse` true; a refuse with `noMathFound` → `refuseText === REFUSE_NO_MATHS`; `composeHandoff` ignores `text` blocks and throws when no latex block exists; `attachmentFor` carries counts only (assert no block content string appears in `JSON.stringify(attachment)`).
  - download: `visionAvailability({variant:null})` → `'unavailable'`; `{declared:true,installed:false}` → `'needs-download'`; installed → `'ready'`; `visionDownloadPlan` totals the two sizes and throws `pickVisionArtifacts`'s message when the projector is missing.
  `npm test` → FAIL (modules missing).

- [ ] **Step 2: Implement** the three modules exactly to the interfaces (import `pickVisionArtifacts` from `./dist-pick.js`, `formatBytes` from `./download-policy.js`).

- [ ] **Step 3: GREEN** — `npm test` all pass. **Step 4: Commit** — `vision(s2-3): pure modules — crop geometry, the confirmation gate, download availability`.

---

### Task 4: The WebView flow — button, capture, crop, reading, confirm, handoff, download

Pre-step for the controller: merge `mobile/vision-catalog` into `mobile/vision-p2` (the bundled catalog's `tiers.*.vision` block) before dispatching.

**Files:**
- Modify: `src/index.html`, `src/styles.css`, `src/app.js`

**Interfaces consumed:** Task 3's modules; `invoke('vision_state')`, `invoke('prewarm_vision')`, `invoke('transcribe_image', {requestId, imageBase64, onEvent})` with `VisionEvent` `{event:'done', data: TranscribeResult}` | `{event:'error', data:{message}}`; `invoke('chat_cancel', {requestId})`; `invoke('fetch_dist_catalog')`, `downloadArtifact(artifact)`, `meteredGate(bytes)`, `visionVariantFor(heroEntry(), effectiveTier())`; `sendCompletion(text)`; `append_message` with `attachments` (Task 2).

- [ ] **Step 1: Markup** (`index.html`): in `.composer`, before `#chatInput`: `<button class="btn iconbtn" id="photoBtn" type="button" hidden aria-label="Add a photo of a problem">📷</button>`; after the composer, two hidden inputs: `<input type="file" id="photoCapture" accept="image/*" capture="environment" hidden/>` and `<input type="file" id="photoPick" accept="image/*" hidden/>`; a modal `#visionModal` (same `.modal > .box` pattern as `#loginModal`) containing four stage panes: `#visionMenu` (two buttons with `TAKE_PHOTO_LABEL` / `CHOOSE_IMAGE_LABEL` text set from the constants at runtime, plus a download line `#visionDl`), `#visionCrop` (a `<canvas id="cropCanvas">` with an absolutely positioned frame `<div id="cropFrame">` carrying 8 handle `<i data-h="nw">` …, and buttons Use selection / Retake), `#visionReading` (`<p id="visionReadingText"></p>`, a progress bar, a Cancel button), `#visionConfirm` (`<img id="visionThumb">`, `<div id="visionBlocks">` filled at runtime, `<p id="visionNotes">`, buttons `#visionUse`, `#visionUseAnyway`, `#visionRetake`, `#visionTypeIt`), `#visionRefuse` (`<p id="visionRefuseText">`, Retake / Type it). All text set via `textContent` from the constants.

- [ ] **Step 2: Styles** (`styles.css`): `.iconbtn` (square, same height as `#sendBtn`), `#cropCanvas{max-width:100%;display:block}`, `#cropFrame{position:absolute;border:2px solid var(--teal);box-shadow:0 0 0 9999px rgba(0,0,0,.45);touch-action:none}` with handles `#cropFrame i{position:absolute;width:22px;height:22px;…}`, `.vblock{font-family:ui-monospace,monospace;white-space:pre-wrap}`, `.vblock.flag{border-color:#d99a2b;background:rgba(217,154,43,.12)}`, `.vblock.text{color:var(--muted)}`, `.is-mobile #cropFrame i{width:28px;height:28px}`.

- [ ] **Step 3: State and availability** (`app.js`): `state.vision = { info: null, requestId: null, file: null, fit: null, crop: null, result: null, confirm: null }`. `async function refreshVisionState()` → `state.vision.info = await invoke('vision_state').catch(() => null)`; call it on `engine-ready` and in `openChat`/`enterModel`; `function renderPhotoButton()` shows `#photoBtn` iff `visionAvailability({variant: visionVariantFor(heroEntry(), effectiveTier()), visionState: state.vision.info}) !== 'unavailable'` AND the entered model is the hero (not the supervised triage entry). Button disabled while `state.chat.streaming` or a transcription is in flight.

- [ ] **Step 4: Menu and download** — `#photoBtn` click → if availability is `'needs-download'`: show `#visionMenu` with `#visionDl` text `VISION_DOWNLOAD_PROMPT(totalBytes)` and a Download button → `downloadVision()`: `fetch_dist_catalog` → `visionDownloadPlan` → `meteredGate(totalBytes)` → `downloadArtifact(base)` → `downloadArtifact(mmproj)` → `refreshVisionState()` (the `download-progress` listener already drives the bar; reuse `state.artDl`). If `'ready'`: show the two buttons; if `state.vision.info.coresident` call `invoke('prewarm_vision').catch(()=>{})`.

- [ ] **Step 5: Capture → crop** — the two buttons `.click()` the matching hidden input; on `change`: `const file = input.files[0]; input.value = '';` → `startCrop(file)`: `const bmp = await createImageBitmap(file)`; size the canvas to the modal box (`fit = fitContain(bmp.width, bmp.height, boxW, boxH)`), `drawImage(bmp, fit.x, fit.y, fit.w, fit.h)`, `state.vision.crop = defaultCrop(fit)`, position `#cropFrame`; pointer events (`pointerdown/move/up` with `setPointerCapture`) on the frame body → `moveCrop`, on a handle → `resizeCrop`; re-position after each. "Use selection" → `const {sx,sy,sw,sh} = cropToImagePixels(crop, fit); const {w,h} = targetSize(sw, sh);` draw to an offscreen canvas `w×h` via `drawImage(bmp, sx, sy, sw, sh, 0, 0, w, h)`, `toDataURL('image/jpeg', 0.92)`; keep that data URL as the thumbnail; `imageBase64 = dataUrl.slice(dataUrl.indexOf(',') + 1)`; `bmp.close()`; → `transcribe(imageBase64, dataUrl)`.

- [ ] **Step 6: Reading** — `transcribe(b64, thumb)`: `requestId = newRequestId()` (the same helper `transport.js` uses, or `crypto.randomUUID()`); show `#visionReading` with `READING_TEXT`; `const events = new Channel(); events.onmessage = (e) => { if (e.event === 'done') showConfirm(e.data, thumb); else showError(e.data.message); }`; `await invoke('transcribe_image', { requestId, imageBase64: b64, onEvent: events })` wrapped so a rejection shows the error pane (the refusals "A transcription is already in progress." and the engine-not-running text are shown verbatim). Cancel → `invoke('chat_cancel', { requestId })` and close. The existing `engine-restarting` banner covers the sequential path (the composer is disabled by it).

- [ ] **Step 7: Confirm** — `showConfirm(result, thumb)`: `state.vision.confirm = confirmState(result)`; if `mode === 'refuse'` show `#visionRefuse` with `refuseText`; else render each block as a `<textarea class="vblock [flag] [text]" data-id>` whose `.value` is the content (`textarea.value` is text, never HTML; `text` blocks `readOnly`), `#visionNotes.textContent = notes.join(' ')`, buttons per `canUse` / `canUseAnyway`; on `input` in a textarea → `state.vision.confirm = applyEdit(state, id, value)` and re-render buttons. Use / Use anyway → `handoff()`.

- [ ] **Step 8: Handoff** — `handoff()`: `const text = composeHandoff(state.vision.confirm)`; `pendingAttachment = attachmentFor(state.vision.result, state.vision.confirm)`; close the modal; then exactly what `sendMessage` does for typed text: `state.chat.messages.push({ role: 'user', content: text }); appendBubble('user', text); sendCompletion(text);`. In `sendCompletion`, the existing user-turn persist (`invoke('append_message', { chatId: turnChatId, role: 'user', content: userText })`) passes `attachments: pendingAttachment` and then clears `pendingAttachment` (so a typed turn never inherits it). `ensureTurnChat` titles the chat from the first line — `Problem from photo:` — acceptable for v0.1 and noted.

- [ ] **Step 9: Tests and checks** — `npm test` (the pure modules); a manual desktop smoke in the browser is not possible (the command refuses on desktop) — the device checklist (Task 6) is the test of this task. `grep -n innerHTML src/app.js` must show no new use with model text. **Step 10: Commit** — `vision(s2-4): the camera button, capture, crop, reading, the confirmation gate, and the handoff`.

---

### Task 5: Android — the `<queries>` entry and the capture-file sweep

**Files:**
- Modify: `src-tauri/gen/android/app/src/main/AndroidManifest.xml`
- Create: `src-tauri/gen/android/app/src/main/java/com/cleophis/app/CaptureHygiene.kt`
- Modify: `src-tauri/gen/android/app/src/main/java/com/cleophis/app/MainActivity.kt`, `src-tauri/src/mobile_native.rs`, `src-tauri/src/vision_cmds.rs`, `src-tauri/gen/android/app/proguard-rules.pro` (only if the other JNI-called classes are kept that way — check how `ShareSheet`/`NetworkPolicy` survive the minified release build and do the same)

- [ ] **Step 1: Manifest** — inside `<manifest>`, before `<application>`:
```xml
    <!-- Vision input (Stage 2): the WebView's own <input type="file" capture>
         launches ACTION_IMAGE_CAPTURE and checks resolveActivity() first; on
         targetSdk 30+ that returns null unless the intent is declared here, and
         the WebView then falls back to the file picker without a word. No CAMERA
         permission is needed for intent capture. -->
    <queries>
        <intent>
            <action android:name="android.media.action.IMAGE_CAPTURE" />
        </intent>
    </queries>
```

- [ ] **Step 2: Kotlin** — `CaptureHygiene.kt`:
```kotlin
package com.cleophis.app

import android.content.Context
import android.os.Environment
import androidx.annotation.Keep

/** The WebView's camera capture writes `JPEG_<stamp>_*.jpg` into the app's
 *  external pictures dir (wry's RustWebChromeClient.createImageFile). Nothing
 *  else reads them after the page has read the file, so they are swept on
 *  activity start and after every transcription. App-private storage: no
 *  other app sees them, but a stale photo of a worksheet is still a leak into
 *  the next backup. */
@Keep
object CaptureHygiene {
    @JvmStatic
    fun sweep(context: Context): Int {
        val dir = context.getExternalFilesDir(Environment.DIRECTORY_PICTURES) ?: return 0
        var n = 0
        dir.listFiles()?.forEach { f ->
            if (f.isFile && f.name.startsWith("JPEG_") && f.name.endsWith(".jpg") && f.delete()) n++
        }
        return n
    }
}
```
`MainActivity.onCreate`: `CaptureHygiene.sweep(this)` after `super.onCreate`. `mobile_native.rs`: `pub(crate) fn sweep_capture_files()` calling the static method through `with_app_class("com.cleophis.app.CaptureHygiene", …)` with signature `(Landroid/content/Context;)I`, logging the count under `[vision] swept N capture file(s)`; desktop stub is a no-op. `vision_cmds.rs`: call it in `transcribe_image`'s teardown (after `inference_service(false)`), on every exit path.

- [ ] **Step 3: Verify** — Android `-D warnings` check Finished; a Gradle compile of the Kotlin is part of the APK build (Task 6); `grep -n 'IMAGE_CAPTURE' AndroidManifest.xml` shows the entry once. **Step 4: Commit** — `vision(s2-5): camera intent declared for the WebView chooser; captured JPEGs swept`.

---

### Task 6: Device checklist, debug APK, and the record

**Files:**
- Modify: `docs/superpowers/mobile-tools/phone-journey.md` (new section "## Photo input (Stage 2)" with steps V1–V7), `probes/phone-journey.test.mjs` (the step list gains V1–V7 and the new constants' module is importable), create `docs/superpowers/verification-vision-stage2.md`

- [ ] **Step 1: Steps** — V1 the camera button appears only on the tutor (not the triage entry) once `vision_state` says declared (`expect-source: src/app.js` for the aria-label); V2 "Take a photo" opens the system camera (not the file picker) — pins the manifest `<queries>` (`expect-source: AndroidManifest.xml` → `android.media.action.IMAGE_CAPTURE`); V3 "Choose an image" picks a screenshot; V4 an EXIF-rotated photo (orientation 6) transcribes upright (`expect: vision-confirm.READING_TEXT` shows during reading); V5 Cancel mid-reading returns to the menu and the foreground notification disappears; V6 the confirm gate: an amber block when the verdict is `review`, `USE_ANYWAY_LABEL` present, `USE_LABEL` disabled until an edit; the handoff bubble begins `HANDOFF_PREFIX`; V7 `adb shell run-as com.cleophis.app ls files/../files/Pictures` (or the external-files path) shows no `JPEG_*.jpg` after the flow. Each step: PASS/FAIL boxes and evidence (screenshot name). Update the test's step list and run `npm test`.

- [ ] **Step 2: Debug APK** — `docs/superpowers/mobile-tools/build-android-apk.sh` as the Phase 1i build did (read its header for the variant flag; the tutor variant is the default). Record the APK sha256 and size in the Stage 2 record's build table; `verify-apk.py` on it. If signing material is absent, record exactly what is missing.

- [ ] **Step 3: Record** — `verification-vision-stage2.md`: what each task built, the gates (npm, kpack-engine, Android check, Windows suite on the final head), the APK facts, the checklist handed to the founder (steps V1–V7 unrun), the security inheritances honoured (where in the code each lives), and what Stage 3 inherits (the 20-image probe set, verdict calibration, CI).

- [ ] **Step 4: Commit** — `docs(vision): Stage 2 record, the photo-input checklist, the debug APK facts`.

---

## Follow-ups outside this plan

- Stage 3: the 20-image probe set, the scorer, `vision-regression.yml`; verdict threshold calibration (the `CONFIDENCE_FLAG` and the engine's constants).
- The share-sheet target (ACTION_SEND) and a Tauri mobile plugin for capture: v0.2.
- A LaTeX renderer in the confirm modal (v0.1 shows monospace source).
- Desktop drag-and-drop/paste: inherits the pipeline unchanged; after the phone.

## Security Assessment

<security_assessment>

### Critical — None identified

### High
**H1. Model text reaching the tutor or the DOM.** Mitigations: `composeHandoff` uses only `latex` blocks and the user role with the `Problem from photo:` prefix (Task 3, tested); block content is rendered through `textarea.value`/`textContent` only (Task 4; `grep innerHTML` check); the tutor's system prompt is untouched; `attachmentFor` stores counts, never text (Task 3, tested).

### Medium
**M1. Memory and DoS on the floor phone from large captures.** A 12 MP camera JPEG decoded in the WebView is ~48 MB of bitmap; `createImageBitmap` + the crop canvas keep one copy; the crop sent is ≤ 512 px (Task 3 `targetSize`, Task 4). The engine's caps remain the backstop. The button is disabled while a transcription runs and the engine refuses a second (Stage 1 M1).
**M2. Stale captured photos.** The WebView writes the camera JPEG to app-private external storage; Task 5 sweeps it on start and after each transcription; backups: `backup_rules.xml` / `data_extraction_rules.xml` should already exclude external files — Task 5 checks and reports.

### Low
**L1. EXIF/GPS.** The canvas re-encode drops EXIF before the bytes reach Rust (Task 4); the original file never leaves the WebView.
**L2. Download trust.** Vision artefacts arrive only through `download_artifact`, which refuses shas the bundled catalog does not pin (Stage 1) and verifies the signed dist catalog.
**L3. Prewarm as a resource lever.** `prewarm_vision` is advisory and gated on `coresident`; it only loads a 0.3 GB model the device can hold.

### Edge cases
- The user picks a non-image or a corrupt file: `createImageBitmap` rejects → the menu shows the error text and offers Retake.
- The engine is `Restarting` (sequential path) when the user types: the existing banner disables the composer; the transcription result still arrives.
- `vision_state` is `null` on desktop: the button never appears (availability `'unavailable'`).

</security_assessment>
