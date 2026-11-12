# Vision Input Spec v0.1 — viability review for the Cleophis mobile app

Review date 2026-11-01. Read-only review of the spec (Oct 31, 2026) against the app as
it stands on `mobile/triage-p6` (93769b9; PR #34 open, `main` still at 0073d5c). Codebase
facts come from three read-only explorations and a design pass; Hugging Face and
llama.cpp facts are marked (web). No code was changed.

## Verdict

**Viable, with conditions. Not viable as written for the $200 phone.**

- The spec's shape fits the codebase unusually well: a transcriber that hands text to
  the existing tutor, one model resident at a time, a confirmation gate. The engine
  already frees the old model before loading a new one, and the vision library the
  spec calls "the one genuinely new runtime piece" is **already vendored** in the app's
  llama.cpp and exposed by its Rust binding behind an unused feature flag. No new
  runtime is needed.
- What fails is the spec's numbers on the floor phone. The 1–2 s swap is really 3–23 s
  on the test phone; the 2 B-class vision models the spec names would take tens of
  seconds to encode one image on that CPU, and their projector files (1.3 GB for
  Qwen2-VL-2B) sit in memory unmapped; the dot-product kernels that would help are
  switched off after a crash on another phone. On the Galaxy A22 class only a
  ~250 M-parameter document model (granite-docling-258M, SmolVLM) can come near the
  2.0 s gate, and even that is likelier to land at 4–12 s. **It must be measured.**
- Recommendation: proceed phone-first as the spec decides, with three changes: aim
  the floor tier at the small document-OCR models and drop Pix2Text; make the latency
  gate tier-specific; run a 3–4 day go/no-go spike on the real phone before any UI
  work. **The A22 is the primary target** (founder, 2026-11-01): the spike measures it
  first, and the tuning levers below are exhausted before any fallback. Only if the
  A22 cannot be reached does vision ship on mid-tier phones first, with the floor tier
  keeping "type it" and nothing else regressing.

## Corrections from the founder (2026-11-01)

- **The tutor's base model is the Qwen3 family, not Llama.** The bundled catalog at
  93769b9 still lists Llama-3.2-1B for the `low` tier (`src-tauri/resources/catalog.json`),
  but that is stale: the product decision is Qwen3 across the tiers, possibly moving to
  Qwen3.5 once compared. The published dist catalog v11 already carries the floor-tier
  artefacts: `Qwen3-1.7B-Instruct-Q4_K_M.gguf` (1,282 MB) with `behavioral-v1-Qwen3-1.7B`
  (35 MB), beside the Q6_K the triage build uses. Switching the catalog's `low` tier to
  them is a separate catalog change, not part of vision. Every memory and swap figure in
  this review therefore uses Qwen3-1.7B as the floor tutor: Q4_K_M file 1.28 GB (RSS
  about 1.5–1.7 GB), Q6_K 1.67 GB (1.70 GB RSS measured), load 23 s on the shipped build.
  Co-residency of a 0.3 GB transcriber beside it is about 2.0–2.3 GB, still inside the
  3.0 GB gate on 8 GB phones. The transcriber choice is independent of the tutor family;
  the one family-aligned option is Qwen3-VL-2B (vendored projector support exists) as the
  mid-tier candidate in place of Qwen2-VL-2B. A later move to Qwen3.5 re-measures the
  tutor numbers only.
- **Primary objective: reach the A22.** The decision rule in Stage 0 and the "A22 first"
  levers below are ordered accordingly; mid-tier-only is the accepted fallback, not the plan.

## What the codebase already has (reuse)

| Need in the spec | Exists | Where |
|---|---|---|
| Vision runtime | llama.cpp `mtmd` vendored and buildable; projector types for Qwen2-VL / 2.5-VL / 3-VL, Gemma 3, idefics3 (SmolVLM, granite-docling), PaddleOCR-VL, DeepSeek-OCR, LightOnOCR | `third_party/llama-cpp-sys-2/llama.cpp/tools/mtmd/`, `build.rs:359-364, 1028-1071`; `llama-cpp-2 0.1.151 src/mtmd.rs` (`MtmdContext::init_from_file`, `MtmdBitmap::from_image_data`, `tokenize`, `eval_chunks`, `decode_use_mrope`) |
| Sequential load/unload | one inference thread owns one model; `Load` frees the old model first | `src-tauri/src/engine_inproc.rs:208-334` |
| Integrity of model files | sha256 of every file before load; signed dist catalog, per-artifact pins picked by sha | `crates/kpack-engine/src/llama.rs:137-165`, `src/dist-pick.js`, `cloud/download.rs` |
| Camera / picker on Android | the WebView's generated file chooser already implements image capture (ACTION_IMAGE_CAPTURE via FileProvider) and a picker; no CAMERA permission needed for intent capture | generated `RustWebChromeClient.kt:274-415`; `res/xml/file_paths.xml` |
| Image decode | `image 0.25` (png) + JPEG via `zune-jpeg` already in the tree | `Cargo.lock` |
| Tutor entry with a tool loop | text first message → `sendCompletion` → `chat_stream`; calc tool in the loop | `src/app.js:2023-2395`, `src-tauri/src/chat_cmds.rs` |
| Bench harness with RSS and timing | `VmRSS`/`VmHWM` and per-phase ms, JSONL per item | `crates/kpack-engine/examples/{stream,probe}.rs` |
| Fixture pinning + pure scorer patterns | manifest of sha256+count enforced by a test; adjudicator judged on desktop, probes run on device | `src/triage/fixtures/lookup-guard/manifest.json`; `src-tauri/src/engine_inproc/probes.rs` |
| Model-in-the-loop CI template | cached sha-verified public GGUF, `--ignored` test, skip guard, exact pass count | `.github/workflows/determinism.yml` |

## Where the spec and the codebase disagree (and what to change in the spec)

| Spec says | The app actually | Change the spec to |
|---|---|---|
| "Cleophis uses its own inference engine, not llama.cpp" (§3) | Inference is llama.cpp via `llama-cpp-2`; desktop runs `llama-server`, Android runs it in-process; the multimodal library is vendored | "llama.cpp in-process via kpack-engine; vision via llama.cpp's mtmd; our code owns policy, orchestration and the confirm gate." The "2–4 days, the one unknown" row becomes "enable the feature and prove the Android link". |
| Swap "from flash ~1–2 s" (§2) | A22: a 1 B model loaded in 3.5 s (dotprod build), 4 B in 18 s, the Qwen3-1.7B Q6_K in 23 s on the shipped build; the projector is read into a buffer, not mapped | "Co-resident when RAM ≥ 6 GB (a 0.3–0.5 GB transcriber beside the Qwen3-1.7B tutor's ~1.7 GB). Below that, sequential: the swap is a tutor reload of 5–25 s, shown in the UI." |
| Tutor = Qwen3-4B Q4 on a 6–8 GB phone (§2 table) | Tier rules make the A22 `low` (`hardware.rs:104-119`); the floor tutor is the small Qwen3 (1.7B Q4_K_M 1.28 GB, RSS ~1.7 GB; the catalog's Llama-3.2-1B entry is stale, see Corrections); 4 B ran at 1.0–1.8 tok/s on the A22 and was ruled out | "Qwen3-1.7B (or the Qwen3.5 equivalent) on the floor tier; 4 B only on mid (≥ 8 GB and i8mm or a ≥ 2.75 GHz core)." Memory is easier than feared; speed is harder. |
| Pix2Text as the lightest candidate (§3) | PyTorch/ONNX; the app has no ONNX or TFLite runtime, and adding one is a second native stack | Drop it. Candidates must be GGUF + mmproj with a projector type the vendored code knows. The light candidates are granite-docling-258M (IBM, Apache-2.0, trained on SynthFormulaNet LaTeX, DocTags output; 178 MB + 104 MB projector at Q8) and SmolVLM-256M/500M (web). |
| Qwen2-VL-2B as the phone default, Gemma 3 4B for mid (§3) | Qwen2-VL-2B = 986 MB + **1.33 GB f16 projector, no quantised projector published** (web); its 675 M-param encoder ≈ 1–1.5 TFLOP per image on cores without dotprod | Both are laptop/desktop-tier here. The mid-phone bake-off entry is Qwen3-VL-2B (in-family with the tutor; vendored projector support) or Qwen2-VL-2B; Qwen2.5-VL-3B only after a licence check. |
| Transcribe ≤ 2.0 s on the slowest target phone (§6) | Under mtmd's idefics3 preprocessing a ≤ 512 px image still costs 2 encoder passes (tile + overview, 128 image tokens); ~0.2 TFLOP each on 2×A76+6×A55 without dotprod → 4–12 s total is the honest prior; no in-app timing exists today | Two gates: "≤ 2.0 s on mid tier" and "≤ 8 s p50 / 12 s p95 on the floor device", both from the device harness; revisit when runtime CPU kernel dispatch restores dotprod. |
| Peak ≤ 3.0 GB during the vision stage (§6) | A22 has 8 GB (device docs; a code comment saying 4 GB is stale); total-app memory with the WebView has never been measured | Keep 3.0 GB; define "vision stage" as everything resident, tutor included when co-resident; measure with the WebView running. |
| ≥ 18/20 exact-match LaTeX (§6) | `\frac12` vs `\frac{1}{2}`, `\cdot` vs `\times`, `\left(` break raw equality | "Exact match after the pinned normalisation rules in `probes/vision/normalise.mjs`." |
| "Transcriber, never a teacher" (§1) | granite-docling emits DocTags and cannot teach; a Qwen VLM will solve the problem unless told not to | Keep; add: the parser discards everything outside transcript tags, and one probe item checks nothing beyond the transcription reaches the tutor. |
| Preprocess with "OpenCV/ndk or pure Rust" (§7) | No OpenCV; `image` present; no EXIF reader | Pure Rust: `image` + `kamadak-exif`; the crucial step is the downscale to the encoder's native 512 px, because the document models' preprocessor otherwise tiles a 2048 px photo into 17 encoder passes. |
| Camera needs the CAMERA permission (§7) | Intent capture needs none; it needs a `<queries>` entry for IMAGE_CAPTURE on targetSdk 36 or it silently falls back to the file picker | "No new runtime permissions in v0.1; one manifest `<queries>` entry." |
| Two-photo handwriting v0.2 | Not contradicted; the 250 M models' handwriting is weak | Keep deferred; likely mid-phone-only. |

## Gaps the implementation must close (none is a blocker)

1. **Build.** `mtmd` is not enabled anywhere and has never linked under `cargo ndk`. Its sources are compiled by a separate `cc` build that does not receive the ARM arch-floor define (`build.rs:1031-1071` vs `:664`); it inherits the NDK default, which is safe but unaudited. `mobile-check.yml` only runs `cargo check`, so a link failure would not show until a real build.
2. **Engine API.** Messages are `{role, content: String}` at every layer; no mmproj field in `ModelSpec`/`LoadRequest`, `TierVariant`, `ResolvedHero` or the dist catalog; the tier switch's `sweep_models` would delete an mmproj `.gguf`; the generation loop assumes one token per position (KV-prefix mirror) and sends the whole prompt in one decode, which hard-aborts above `n_batch` (`llama-context.cpp:1712`).
3. **Shared backend.** `kpack-engine` and `kpack-embed` each call `LlamaBackend::init()` (spec item A34(g), unfixed). Vision reuses the engine's backend and does not depend on this fix, but it should land before three users share the process.
4. **Image input on Android.** No `<queries>` entry, no ACTION_SEND target, no way to read a `content://` URI from Rust, no Kotlin→Rust callback. The WebView chooser is the cheapest route; the share-sheet target needs a Tauri mobile plugin.
5. **Preprocessing.** No EXIF orientation, no resize helper; JPEG decode exists only through a transitive dependency.
6. **Measurement.** No load/encode/prefill/RSS telemetry in the app; the bench examples have it but no VLM path.
7. **Probe set and CI.** No image fixtures, no LaTeX normaliser/scorer, no model-in-the-loop job for a VLM. The CI template exists but Linux runners give transcription regression, not device latency; CI has no secrets, so the model must be public; the repo is public, so fixtures must be licence-clean (security items P07/D5).

## Risks and mitigations

| Risk | Likelihood | Mitigation |
|---|---|---|
| The ~250 M document model misreads printed algebra (signs, exponents, fractions) | Medium | The spec's gates catch it: sign/operator errors weighted ×5, zero may survive to the tutor, confirmation mandatory below 0.9. If it fails the probe, the floor phone ships **without** vision and mid phones get a 2–3 B VLM; the feature is additive. |
| Encode latency on the A22 exceeds even the relaxed gate | Medium | Downscale to 512 px before encoding; refuse images that tile to more than 320 image tokens; measure before UI work; if it lands at 8–12 s, show a staged progress state rather than drop the feature. Longer term: runtime kernel selection to restore dotprod where the CPU has it. |
| Projector memory plus tutor reload thrash on 4–6 GB phones | Low on 8 GB, Medium below | Co-residency only at ≥ 6 GB; otherwise the sequential path, exercised with a forced flag in the device checklist; mmproj kept in the model folder's keep-list. |
| Wrong problem taught with confidence (a misread minus sign) | Medium | The hard gate as a modal the tutor cannot bypass; confidence from token probabilities plus a LaTeX validity check; sign tokens below 0.97 force review; calibrated on the probe set. |
| Android capture plumbing eats the schedule | Medium | Start with the WebView `<input type=file accept="image/*" capture>` path plus the `<queries>` entry; defer the share-sheet target to v0.2. |
| `mtmd` fails to link under `cargo ndk` or bloats the `.so` | Low–Medium | Day-1 spike item; no product code before it links; measure the size delta on a release build (none has ever been built: do it). |
| Vendored `build.rs` edits are hidden by `third_party -diff` (P08) | Certain | Change the vendor script in the same PR; add an archive audit for dotprod instructions to `check-mobile-build.sh`. |
| Privacy | Low | Image bytes stay in app-private storage for the session; the confirmed text, not the image, is what persists; the capture temp file is swept; no network path exists. |

## Staged plan (v0.1, printed only, phone first)

### Stage 0 — go/no-go spike on the real phone (3–4 days)

Goal: prove `mtmd` links under `cargo ndk` and put measured A22 numbers beside the gates for three candidates before any product code.

- **Build.** `crates/kpack-engine/Cargo.toml`: a `vision = ["real", "llama-cpp-2/mtmd"]` feature separate from `real`, so the shipping APK stays byte-identical until Stage 1; an example `vision_bench` requiring it. Build with the `cargo ndk` invocation `mobile-check.yml` uses. Give the mtmd `cc` build the same `-march=armv8-a` floor (`third_party/llama-cpp-sys-2/build.rs:1031-1047`, keyed on the existing floor marker; `check-cpu-floor.py` extended to the second home; `vendor-llama-sys-dotprod.sh` updated) and add an `llvm-objdump` audit for `sdot/udot/casal/ldadd` to `check-mobile-build.sh`.
- **Bench** `crates/kpack-engine/examples/vision_bench.rs`, modelled on `examples/stream.rs` and `probe.rs`: per image, load ms for model and projector (the projector init includes a warm-up encode), preprocess ms, image tokens, encode ms, prefill ms, time to first token, decode ms, tokens/s, raw output, per-token log-probabilities (the future confidence signal), `VmRSS`/`VmHWM`. Image path in Rust: decode, EXIF orientation, downscale to `--max-side`, RGB8 → `MtmdBitmap::from_image_data`; a fresh context per image with `n_batch`/`n_ubatch` set explicitly; `LlamaBatch::get_one` for the M-RoPE family (Qwen2-VL). Device script `run-vision-bench.sh` cloned from `run-device-probes.sh` (sha-verified pushes, pulls JSONL, refuses on a CPU verdict of INCOMPATIBLE).
- **Matrix.** Devices: the A22 first (the primary target) and, if available, one mid-tier phone. Candidates (all public, Apache-2.0 unless noted): granite-docling-258M Q8 + projector Q8; SmolVLM-256M Q8 + projector Q8 (same-size generalist control); Qwen3-VL-2B (in-family, if a GGUF + projector is published) else Qwen2-VL-2B Q4_K_M + f16 projector (upper bound). Five printed problems photographed by the founder (linear, fraction, quadratic with a minus sign, exponent/subscript, a two-equation system), each at `--max-side` 512 and 1024 plus one EXIF-rotated copy; threads 2/4/8; three repeats. Also run on the dev box and, if possible, an arm64 Linux runner to see whether greedy outputs differ by architecture (decides whether CI expectations are per-arch).
- **Decision rule** (floor device, 512 px, 4 threads, greedy, `VmHWM` with model + projector + context):
  - PASS-A: some candidate reaches ≥ 4/5 exact after normalisation, p50 ≤ 8 s, p95 ≤ 12 s, `VmHWM` ≤ 1.2 GB → ship that one model on all tiers; Stage 1 proceeds.
  - PASS-B: the rule holds only on the mid-tier phone → vision on mid/high; the floor tier hides the camera; the spec's latency gate becomes tier-split.
  - NO-GO: no candidate ≥ 3/5 exact at any latency, or none fits 3.0 GB with the tutor unloaded, or `mtmd` cannot be linked → stop; the options are desktop-first vision or another model family.
  - Honest prior: granite-docling PASS-A or PASS-B on accuracy at 4–10 s; SmolVLM-256M fails accuracy; the 2 B VLMs fail latency and memory on the A22.
- **A22 first: the levers to pull before accepting PASS-B.** If the first A22 measurement misses the floor gate, the spike tries these in order, each a day or less, and records every result:
  1. **Thread placement.** Try 2, 6 and 8 threads: the Dimensity 700 has two A76 and six A55 cores, and llama.cpp's default of 4 may land on the slow cluster.
  2. **Smaller input.** `max_side` 384 if accuracy holds on the printed items (fewer encoder tokens); cap `image_max_tokens` so the document models never tile beyond 2 passes.
  3. **Hide the load.** Pre-load the transcriber when the camera opens (the user is framing the photo for several seconds anyway) and keep it resident on 8 GB phones; the user then pays encode + decode only.
  4. **Restore dotprod on phones that have it.** The A22 reports `asimddp` but the shipped build cannot use it since the arch floor fell to `armv8-a` after the A51 crash. Runtime kernel selection (`GGML_CPU_ALL_VARIANTS`, already noted as scheduled in the repo) would give this phone its int8 dot-product kernels without breaking ARMv8.0 phones; expect a large speedup on the encoder and the decode. This is the single biggest lever and benefits the tutor too.
  5. **Projector in Q8.** Both small candidates ship a Q8 projector (~100 MB); use it rather than f16.
  6. **Accept a slower floor gate with honest UI.** If the result lands at 8–12 s after the above, ship on the A22 with a staged progress state ("Reading the image…") rather than withhold the feature; the spec's 2.0 s stays the mid-tier gate.
  Only after these six are measured does PASS-B (mid-tier first) apply.
- **Deliverable.** `docs/superpowers/verification-vision-spike.md` with the table, kernel report lines, the call, the chosen `max_side` and thread count, the projector's `preproc_image_size` read from the GGUF header, and the cross-architecture determinism result. Bench JSONL committed; spike photos not committed (Stage 3 makes a licence-clean set).

### Stage 1 — engine integration (5–7 days, after a PASS)

Implemented: see verification-vision-stage1.md

- **Decision: a separate `transcribe` command, not image parts in the chat loop.** The tutor never sees pixels, only confirmed text; threading images through `ChatMessage`/`WireMessage` would break the KV-prefix mirror (`llama.rs:268-274`), M-RoPE positions and prefix reuse, and touch `wire()`, convstore and the context-window estimate for no benefit. `ChatMessage`/`WireMessage` stay `{role, content: String}`.
- **kpack-engine.** New feature-free types in `src/backend.rs` (`VisionLoadRequest`, `PreparedImage`, `TranscribeOptions`, `Transcription` with token log-probabilities and timings, `VisionBackend`/`VisionHandle` traits); `prepare_vision_stack` in `src/adapter.rs` with the same fail-closed sha256 gate; `src/vision_prep.rs` (decode, EXIF 1–8, downscale, RGB8; hard caps of 20 MB / 50 MP so a hostile file from the webview cannot exhaust memory; a fixture's output pinned by sha so phone and CI preprocess identically); `src/vision_parse.rs` (DocTags and plain-LaTeX parsing; everything outside transcript tags discarded, which is where "never a teacher" is enforced; per-block confidence = exp of the mean token log-probability); `src/vision.rs` behind `vision` (`LlamaVisionHandle`: fresh context per image with explicit `n_ctx`/`n_batch`/`n_ubatch`/threads; refuse if image tokens + max tokens exceed the context or image tokens > 320; `eval_chunks`; greedy loop capturing log-probabilities; no KV reuse; projector resident, base mmapped). Factor the sampler/detokenise loop out of `llama.rs:455-508` only if the diff stays mechanical; otherwise duplicate ~40 lines and say so. Vision-only `n_threads` setting. One-line tutor hardening in a separate PR: set `n_batch` = `n_ctx` at `llama.rs:187` so the latent assert cannot fire.
- **Catalog, dist, sweep, launch.** `TierVariant.vision: Option<VisionVariant>` (`#[serde(default)]`; base and projector files with sha and bytes, family, `max_side`, threads) in `src-tauri/src/catalog.rs:145-213`; `LaunchPaths.vision` and `EngineInfo.vision` in `inference.rs:328-350`; both basenames added to `tier_select.rs keep_basenames` (`:283-296`) so a tier switch does not delete the transcriber; `variant_on_disk` must not require vision. Dist catalog gains `kind: "vision_base"` and `"mmproj"` (the kind is a free string, `catalog_dist.rs:48-52`); `src/dist-pick.js` picks them by sha; the download allowlist includes the two shas; download is opt-in from the camera button through the existing metered-data policy. Fix the stale 4 GB comments in `hardware.rs`.
- **Orchestration.** `Command::Transcribe` in `engine_inproc/serve.rs` (a transcribe during an open chat session yields the session; one re-prefill next turn); in `engine_inproc.rs run()` a `vision` handle beside the tutor handle; co-residency policy `ram_gb >= 6` as a pure, tested function (sized for the Qwen3-1.7B tutor, ~1.7 GB, plus a ≤ 0.5 GB transcriber); below it, unload the tutor, transcribe, then reload with the existing `engine-restarting`/`engine-ready` events the composer already honours; a `Command::PrewarmVision` issued when the camera opens so the load overlaps the user's framing; vision idle timeout 120 s; `[vision]` timing lines in the log. New `src-tauri/src/vision_cmds.rs` with `transcribe_image(request_id, bytes, on_event)` wrapped in the foreground service exactly as `chat_stream` is (`chat_cmds.rs:303-399`), cancel via `ChatCancels`; desktop body refuses like the other mobile commands. Broaden the foreground-service justification string in the manifest to include image transcription.
- **Tests (RED first).** Engine: fail-closed on a missing or mismatched projector; the EXIF orientation table; size caps; pinned preprocess sha; DocTags fixtures including "solution text outside tags is discarded"; confidence arithmetic; the co-residency table. Desktop suite: catalog round-trip with and without `vision`; keep-list keeps vision files; launch resolves the tutor when vision is absent; `serve_loop` yields on `Transcribe`; the desktop refusal; `EngineInfo` JSON. Node: `dist-pick` vision picks. Device: `[vision]` timing lines for one image; the forced sequential path reloads the tutor and the composer re-enables.

### Stage 2 — capture and confirm on Android (5–6 days)

- **Input path, ranked.** (1) WebView `<input type="file" accept="image/*" capture="environment">` for the camera and `accept="image/*"` for photos/screenshots: no Kotlin, no plugin, no new permission; one manifest `<queries>` entry for `android.media.action.IMAGE_CAPTURE` (without it the generated chooser silently falls back to Files, so the device checklist gets "the camera button opens the camera"); JS reads the `File` to bytes and invokes `transcribe_image` (a 1–3 MB IPC hop is fine); a small `CaptureHygiene.kt` static sweeps the temp JPEG afterwards, with its proguard keep. (2) A Tauri mobile plugin with `startActivityForResult`: the proper callback path, 2–3 days, v0.2. (3) The share-sheet target: needs an exported activity that forwards extras (today `LaunchGateActivity.kt:44` drops them) plus (2); v0.2.
- **Preprocessing** is Stage 1's `vision_prep` (EXIF, downscale to the catalog's `max_side`); grayscale/contrast only if Stage 3 shows it helps on the photographed items.
- **Output contract** (serde camelCase): `blocks[{kind: latex|text|description, latex, plain, confidence}]`, `figures[{kind, description, confidence}]`, `warnings[]` (`low_confidence`, `no_math_found`, `non_maths_image`, `diagram_only`, `rotated`, `too_many_tiles`), `verdict: accept|review|refuse`, `model{id, sha256Short, family}`, `timings`, `peakRssKb`. Verdict from per-block confidence at the spec's provisional 0.9 / 0.7 thresholds plus a LaTeX validity check; sign/operator tokens below 0.97 force review; thresholds calibrated in Stage 3 against the zero-surviving-sign-errors gate. `diagram_only` and `non_maths_image` refuse.
- **Confirm UI (the hard gate).** `src/vision-confirm.js` (pure, node-tested): modes, highlighted blocks, `canUse`, `composeHandoff`; editing a block marks it user-confirmed. In `app.js`/`index.html`: a camera button in the composer shown only when `state.engine.vision.available`; a modal with the thumbnail, editable LaTeX source plus plain reading (no LaTeX renderer exists in the repo; v0.1 shows monospace source and flags a renderer for v0.2), amber for < 0.9, actions Use / Use anyway / Retake / Type it. `transcribe_image` never writes to the chat; only the confirm action does, with the edited text. Handoff text begins "Problem from photo: …" so `ensureTurnChat` (`app.js:2052`) gets a sensible title; `wire()` and the catalog `systemPrompt` untouched (its fingerprint is pinned). Provenance in a new nullable `messages.attachments` JSON column (`convstore.rs:217-227`): model, verdict, edited flag, never image bytes.
- **Tests.** Node: threshold routing, refuse yields no message, edited block, handoff and title. Rust: preprocess cases, refusal, the column migration. Device checklist appended to `phone-journey.md`: camera opens the camera, screenshot pick, an EXIF-rotated photo transcribes upright, cancel mid-transcribe, the foreground notification appears and disappears, the forced low-RAM path, the temp JPEG is gone.

### Stage 3 — the 20-image probe set and CI (4–5 days; fixtures and scorer in parallel with Stage 1)

- **Fixtures** `probes/vision/fixtures/v1/`: 16 printed items typeset from our own LaTeX (licence-clean by construction; fractions, signs, exponents/subscripts, `\times` vs `x`, `\le`, roots, a two-equation system, mixed text and formula), 2 founder photos of the printed sheet (one EXIF-rotated; self-licensed CC0), 1 diagram-only drawing, 1 non-maths photo; `items.json` with expected verdict, LaTeX and plain text; `manifest.json` of sha256 + count enforced by `probes/vision/manifest.test.mjs`; `*.jpg` added to `.gitattributes`. Because the repo is public this is a regression set; the release call also uses a founder-only 10-image set kept out of the repo.
- **Scorer** `probes/vision/normalise.mjs` and `score.mjs` (pure, node): pinned normalisation (delimiters, `\dfrac→\frac`, single-token braces, whitespace, a small declared equivalence list); per item exact match, character error rate, sign/operator substitutions ×5, refusal correctness, "survives to the tutor" = a sign error with verdict accept; gates as exported constants; a Markdown table and a JSON verdict; non-zero exit on failure; its own tests (a sign flip accepted fails, in review passes, a missed refusal fails).
- **Device run** `run-vision-probes.sh` from `run-device-probes.sh`: sha-verified pushes, the bench over `items.json`, pull, score with `--device` adding p50/p95 latency and max `VmHWM` against the tier-split gates; one `dumpsys meminfo` reading with the app running, recorded for WebView-inclusive memory.
- **CI** `.github/workflows/vision-regression.yml` from `determinism.yml`: path-filtered PR/push plus weekly; `ubuntu-latest` and `ubuntu-24.04-arm`; the granite-docling GGUF and projector cached with the sha verified on hit and miss; build the bench with `--features vision`; run the 20 fixtures; score; a guard that 20 records were produced. Linux is transcription regression only, never latency or memory, stated in the workflow header. Per-arch expectations if greedy outputs differ. `mobile-check.yml` covers the feature once Stage 1 flips it on; the objdump audit and the second `-march` home join the existing audits.

### Sequencing and estimate

| Stage | Days | Depends on |
|---|---|---|
| 0 spike | 3–4 | — |
| 3a fixtures + scorer | 2 | — (parallel with 1) |
| 1 engine + catalog + orchestration | 5–7 | 0 = PASS |
| 2 capture + confirm | 5–6 | 1 |
| 3b device run + CI | 2–3 | 1, 3a, 2 |
| Spec v0.2 edits | 0.5 | 0 |
| **Total** | **~18–22 working days** | the spec's "~2 weeks" assumed the runtime was the unknown; it is not, the phone's speed is |

### The spec's open questions, answered

- *Does the engine handle non-text input graphs?* Yes, in principle: llama.cpp's multimodal library is vendored and the binding exposes it; it has never been built for Android or run here. Stage 0 settles it.
- *Handwriting samples from design-partner families:* still the founder's call under the parent agreement; v0.1 needs none (printed only).
- *Phone first:* aligned with the codebase; the floor-tier phone is also the hardest case, which is why the spike precedes everything.

## Verification (how the review's claims get tested)

1. Stage 0's bench on the A22 is the test of this review: if a 250 M-class model transcribes printed algebra ≥ 4/5 at ≤ 8 s p50 within 1.2 GB, the verdict stands; if not, the six A22-first levers are measured in order; only if those fail does the "mid-tier first" fallback apply and the floor tier keep typing.
2. The three cross-checks that cost nothing: `cargo ndk` build of `vision_bench` (links or not); the projector header's `preproc_image_size`; the objdump audit of the mtmd archive for dotprod instructions.
3. Every later stage has its RED-then-GREEN tests listed above; the device checklist additions are run on the A22 by the founder as in Phase 1i.
