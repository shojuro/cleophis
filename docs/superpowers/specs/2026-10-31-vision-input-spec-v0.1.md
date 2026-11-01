<!-- The founder's Vision Input Spec v0.1 as pasted on 2026-11-01, kept verbatim as the
     authority the vision plans argue from. Review + staged plan: 2026-11-01-vision-input-review.md.
     Stage 0 result: ../verification-vision-spike.md. -->

# Cleophis — Vision Input Spec v0.1

Oct 31, 2026 · the founder

Lets a student photograph, screenshot, or paste a maths problem — and optionally their own handwritten working — and have the tutor start from it, entirely on-device. Companion to the on-device RAG spec v1.3 and the mobile app spec. Written to be handed to an agent; sections 6 and 7 are the task list.

## 1. Goal, non-goals, design rule

**Goal.** A student points the phone at a problem in a textbook or worksheet, or screenshots one, or drags a PDF crop onto the desktop app, and within three seconds the tutor says: *"Here's what I read — is that right?"* and then *"Okay, what's this problem asking us to find?"* Optionally the student also photographs their own handwritten working, and the tutor can say *"You're fine through line three; look at the sign on line four."*

**Non-goals (v0.1).**

- Not a solver. The vision stage never produces an answer, only a transcription.
- Not diagrams. Geometry figures, graphs and tables are out of scope; the transcriber reports "figure present" and the tutor asks the student to describe it.
- Not live camera. Still images only; no video stream.
- Not cloud. Nothing leaves the device, same as everything else.

**The one design rule: the vision model is a transcriber, not a teacher.** It converts pixels to text (LaTeX for maths, plain text for prose) and hands that text to the existing tutor exactly as if the student had typed it. Consequences:

- The behaviour adapter, calc() tool, refusal logic and RAG are untouched. Zero retraining.
- The vision model can be any family — whichever reads handwriting best — because it never needs to know any pedagogy.
- It can be swapped the day a better one ships, by changing a loader and a prompt.
- Keeping it stupid is what keeps it small enough for a $200 phone.

**Forward note (Oct 31): the camera is the input layer for the whole library, not an algebra feature.** Every specialist Cleophis ships is about something in the world, and the natural way to bring that thing to the model is to point a phone at it. Two modes, same pipe:

- **Transcribe** — algebra, physics, chemistry, any problem on a page. The vision model reads symbols into LaTeX/text. This is v0.1 and the whole of this spec.
- **Describe** — a plant, a circuit, a cell slide, later a joist or a pipe fitting. The vision model does not reason about the thing; it writes a neutral, structured plain-language description ("compound leaf, serrated margin, alternate arrangement, \~5 cm") and the subject specialist reasons on that, exactly as the algebra tutor reasons on the LaTeX.

In both modes the rule holds: the vision model is a pipe, the specialist is the teacher. The output contract in §5 should therefore allow `kind: "description"` alongside `problem` and `working` from day one, even though nothing consumes it yet, so the agent does not build a LaTeX-only pipe we have to rip out when biology ships. Describe mode will want a larger VLM (Gemma 3 4B class or above) and is not gated by this spec.

## 2. Pipeline and memory

```
capture → preprocess → [load VLM] transcribe [unload VLM] → confirm → [load tutor] teach
```

1. **Capture.** Camera, screenshot, share-sheet, paste, or drag-drop. Produces one image (or two: problem + working).
2. **Preprocess** (CPU, <200 ms). Auto-rotate from EXIF, deskew, crop to the largest text block, downscale to the model's native resolution (typically 448–896 px on the long side), grayscale and contrast-stretch. Most OCR failures are lighting and skew, not model capacity — this step is cheap and does half the work.
3. **Transcribe.** Load the vision model, run one prompt, get structured text (§5), unload. Target: under 2 s on a mid-range Android, under 1 s on laptop.
4. **Confirm.** Show the transcription rendered (LaTeX → formatted maths). Student taps ✓ or edits. This is a hard gate, not a nicety — see §5.
5. **Teach.** Load the tutor (if not already resident) and pass the confirmed text as the student's first message. From here the session is identical to a typed one.

**Sequential loading is the whole trick.** The two models are never resident together on the phone. The vision model's job lasts two seconds; it loads, transcribes, and is evicted before the tutor loads. Peak memory is therefore max(VLM, tutor), not the sum.

| Device | RAM | Tutor (Qwen3-4B Q4) | VLM candidate | Strategy | Peak |
| --- | --- | --- | --- | --- | --- |
| $200 Android | 6–8 GB | \~2.5 GB | Pix2Text (\~0.4 GB) or Qwen2-VL-2B (\~1.5 GB) | Sequential; swap from flash \~1–2 s | \~2.5 GB + KV |
| Mid Android / iPhone | 8–12 GB | \~2.5 GB | Gemma 3 4B or Qwen2.5-VL-3B (\~2.5–3 GB) | Sequential | \~3 GB + KV |
| Laptop (16 GB) | 16 GB | \~2.5 GB (or 8B at \~5 GB) | Any of the above | Both resident, no swap | \~8 GB |
| Desktop (128 GB box) | 128 GB | 8B | Qwen2.5-VL-7B (\~5 GB) | Both resident | trivial |

The swap costs the student a "reading your photo…" moment of about a second on a phone. On laptop and desktop both models stay loaded and there is no pause at all.

## 3. Model candidates and bake-off

No training. Each candidate is a loader plus a prompt; the decision is made by the 20-photo probe in §6, not by benchmarks. Run the bake-off on the laptop first, then on the slowest target phone.

| Candidate | Params | Size (Q4) | Printed maths | Handwriting | Licence | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| **Pix2Text** (pix2tex + layout) | \~0.1–0.3B | \~0.4 GB | Excellent — purpose-built for LaTeX OCR | Weak | MIT | Lightest option; printed-only fallback for the smallest phones. Not a VLM; no prompting, just output |
| **Qwen2-VL-2B-Instruct** | 2B | \~1.5 GB | Good | Fair | Apache 2.0 | Stays in family with the tutor; same tokenizer habits. Likely phone default if handwriting is adequate |
| **Qwen2.5-VL-3B-Instruct** | 3B | \~2.5 GB | Very good | Good | Apache 2.0 | Better document understanding than 2B; fits mid phones |
| **Gemma 3 4B** (multimodal) | 4B | \~2.7 GB | Very good | Good–very good | Gemma licence (permissive, check terms) | Strongest handwriting of the small set in community reports; cross-family, which is fine for a transcriber |
| **Qwen2.5-VL-7B-Instruct** | 7B | \~5 GB | Excellent | Very good | Apache 2.0 | Laptop/desktop tier; too large for phones |
| **SmolVLM / Moondream2** | 0.25–2B | 0.3–1.5 GB | Fair | Weak–fair | Apache 2.0 | Hold as ultra-light alternates if Pix2Text layout handling disappoints |

**Recommended bake-off order:**

1. Pix2Text vs Qwen2-VL-2B on the printed half of the probe set — decides the $200-phone default.
2. Qwen2.5-VL-3B vs Gemma 3 4B on the handwritten half — decides the mid-phone default and whether handwriting support ships in v0.1 or is deferred.
3. Qwen2.5-VL-7B on everything — the laptop/desktop tier, where the choice matters less.

**Family question, answered.** The transcriber is a dumb pipe, so family alignment with the tutor buys nothing pedagogically. Lean Qwen where quality is equal (one tooling stack, one quantisation workflow); switch families without hesitation where the probe says so. This is the same rule as the staffing plan: makers can be any family, graders must differ — and a transcriber is neither.

**Runtime note.** Cleophis uses its own inference engine, not llama.cpp. Vision models need an image encoder (ViT) in front of the language model; confirm the engine can run the ViT projector or plan to run it as a separate small graph (ONNX/TFLite on mobile, \~50–200 ms). This is the one genuinely new runtime piece.

## 4. Input modes

| Mode | Source | Typical quality | Preprocess emphasis |
| --- | --- | --- | --- |
| Textbook photo | Phone camera, often angled, mixed lighting, page curl | Medium | Deskew, crop, contrast; detect and isolate the single problem the student tapped |
| Worksheet photo | Printed, flat, may include the student's pencil marks | Medium–good | Separate printed problem from handwritten additions (two-layer output, §5) |
| Handwritten working | Student's own notebook page | Low–medium | Line segmentation; preserve order; expect crossings-out |
| Screenshot | Khan Academy, PDF viewer, LMS | High | Minimal; may contain UI chrome to crop |
| PDF crop (desktop) | Drag-drop or paste from a textbook PDF | High | Text may already be extractable — try PDF text layer first, OCR only if absent |
| Clipboard image (desktop) | Snipping tool / cmd-shift-4 | High | Minimal |

**Multi-problem pages.** A textbook photo usually shows five to ten problems. v0.1 behaviour: run layout detection, draw boxes, ask the student to tap the one they mean. Do not guess. If layout detection fails, ask them to crop.

**The two-photo case — the real tutoring moment.** The student photographs the problem *and* their working. The transcriber returns both as separate blocks, tagged. The tutor then has what no typed interface gives it: the student's actual reasoning, line by line. Its first move is not "here's the answer" but *"Let's check your work together — read me line two."* It locates the first error, asks a question that points at it, and lets the student find it. This is the feature that turns a photo-solver into a tutor and the single strongest demo for a parent: *it caught the mistake and made her find it herself.*

v0.1 ships problem-only. The two-photo case is v0.2, gated on handwriting accuracy from the §6 probe — if the best small model can't read a thirteen-year-old's pencil reliably, shipping it would teach from misread lines, which is worse than not shipping.

## 5. Output contract, confirmation, refusal

**What the transcriber hands the tutor.** One JSON object, never free text:

```json
{
  "blocks": [
    {"kind": "problem", "latex": "\\text{Solve for } x: \\; 3x - 7 = 2x + 5", "plain": "Solve for x: 3x - 7 = 2x + 5", "confidence": 0.94},
    {"kind": "working", "lines": ["3x - 7 = 2x + 5", "3x - 2x = 5 - 7", "x = -2"], "confidence": 0.71}
  ],
  "figures": 0,
  "warnings": ["low_confidence_line", "possible_sign_ambiguity:line2"]
}
```

- `latex` is what the tutor reasons over; `plain` is the fallback if rendering fails.
- `confidence` is the model's self-report where available, else a heuristic from token logprobs; it drives the confirmation UX.
- `figures > 0` makes the tutor ask the student to describe the diagram; it never pretends to have seen it.
- The tutor receives this as a system-tagged first message, so the adapter can be prompted to treat it as *the student's problem* rather than a question to answer.

**Confirmation is a hard gate.** Before the tutor speaks, the rendered transcription appears with: *"Here's what I read — is that right?"* and ✓ / edit. Rules:

- Confidence ≥ 0.9: show rendered, one tap to confirm.
- 0.7–0.9: show rendered with the uncertain tokens highlighted; edit field pre-focused.
- < 0.7: *"I'm not sure I read that right — can you retake it closer, or type it?"* No tutoring from a low-confidence read, ever.

A misread minus sign is the single failure that destroys a parent's trust, because the tutor will then patiently teach the wrong problem with total conviction. The gate is the refuse-rather-than-hallucinate principle applied to pixels.

**Refusal and edge cases.**

- Not maths (a photo of the dog): *"That doesn't look like a problem — want to try another photo?"*
- Diagram-dependent: transcribe the text, flag the figure, tutor asks for a description.
- Multiple problems, no selection: ask which one; do not pick.
- Answer already written: the transcriber reports it as `working`; the tutor does not confirm or deny it — it asks the student to walk through how they got there, as it would for any working.
- The image is never stored beyond the session unless the student explicitly saves the problem. Same privacy posture as everything else: nothing leaves, nothing lingers.

## 6. Test set and acceptance gates

**The 20-photo probe.** Build it before touching a model. Same discipline as the Stage-5 gates: a fixed, versioned set, hand-labelled with ground-truth LaTeX, that every candidate and every future update must pass.

| # | Content | Capture condition |
| --- | --- | --- |
| 1–5 | Printed algebra problems (linear, quadratic, fractions, exponents, systems) | Flat, good light, phone camera |
| 6–8 | Printed problems | Angled 20–30°, page curl, dim light, shadow across the page |
| 9–10 | Printed problems, multi-problem page | Student must select one; tests layout detection |
| 11–15 | Handwritten working, 3–6 lines, real teen handwriting (ask design-partner families; parents' written OK) | Notebook paper, pencil, ruled lines |
| 16–17 | Handwritten with crossings-out and a corrected line | As above |
| 18 | Screenshot from Khan Academy or a PDF viewer | Clean |
| 19 | Problem with a diagram | Must return `figures: 1`, not hallucinate the figure |
| 20 | Not maths at all (a page of prose, or a photo of a desk) | Must refuse cleanly |

Score each candidate on: exact-match LaTeX after normalisation; character error rate; sign/operator errors specifically (weighted 5× — these are the dangerous ones); refusal correctness on #19–20; latency on target hardware.

**Acceptance gates for v0.1 (printed only):**

- ≥ 18/20 exact-match on items 1–10 and 18, after the confirmation step is allowed to fix ≤ 1 token.
- **Zero** uncorrected sign or operator errors that survive to the tutor across the set.
- Correct refusal on #19 and #20.
- Transcribe latency ≤ 2.0 s on the slowest target phone, ≤ 1.0 s on laptop.
- Peak memory ≤ 3.0 GB on the 6 GB phone during the vision stage.

**Acceptance gates for v0.2 (handwriting / two-photo):**

- ≥ 80% line-level exact match on items 11–17.
- Order of lines preserved 100%.
- Crossed-out lines excluded or flagged, never silently included.
- If the best small model misses these, defer v0.2 — do not ship a tutor that teaches from a misread line.

**Regression.** The probe set runs in CI on every model or quantisation change. Add every real-world misread that reaches a design-partner family to the set, with their permission. The set only grows.

## 7. Platform plumbing, effort, task list

The model is the easy part. The time goes into getting an image *into* the app from wherever the student already is.

| Platform | Entry points | Notes |
| --- | --- | --- |
| Android | In-app camera; **share sheet** target (from Camera, Gallery, Chrome, any PDF app); clipboard paste | Share-sheet registration is a manifest intent filter plus a receiver activity. Camera via CameraX; no new permissions beyond CAMERA |
| iOS | In-app camera; **Share Extension**; paste; Shortcuts action (later) | Share Extension runs in a separate process with a memory cap (\~120 MB) — it must *hand off* the image to the main app via App Group container, not run the model itself |
| Desktop (Tauri) | Drag-drop onto the window; paste (cmd/ctrl-V); file picker; PDF crop | Try the PDF text layer before OCR. Drag-drop and paste are a day's work in Tauri |

**Effort estimate** (agent-driven, with the existing runtime and one human reviewing):

| Task | Est. |
| --- | --- |
| Build and label the 20-photo probe set (needs design-partner handwriting) | 1–2 days, mostly waiting on families |
| Preprocess pipeline (rotate, deskew, crop, resize) — OpenCV/ndk or pure-Rust | 1 day |
| Bake-off harness: load each candidate, run probe, score | 1 day |
| ViT/projector support in the inference engine, or sidecar ONNX graph | 2–4 days — **the one unknown** |
| Output contract + confirmation UI | 1–2 days |
| Android share sheet + camera | 1 day |
| iOS share extension + app group hand-off | 2 days |
| Desktop drag-drop / paste / PDF layer | 1 day |
| Sequential load/unload orchestration + memory test on 6 GB phone | 1 day |
| **v0.1 total (printed only)** | **\~2 weeks**, dominated by the engine's ViT support |
| v0.2 (handwriting / two-photo) | +1 week if the probe passes; otherwise deferred |

**Task list for the agent, in order:**

1. Create `/vision/probe/` with the 20-item manifest schema (`id`, `image`, `ground_truth_latex`, `kind`, `expected_figures`, `expected_refusal`). Populate printed items 1–10, 18–20 immediately from textbook scans; leave handwriting slots for family photos.
2. Write the preprocess module with a CLI; verify on items 6–8 that deskew/contrast measurably improves a baseline OCR.
3. Confirm whether the Cleophis inference engine can execute a ViT encoder. If yes, load Qwen2-VL-2B end-to-end. If no, export the encoder to ONNX, run it as a sidecar, feed embeddings to the engine — and write up which path was taken.
4. Build the bake-off harness; run Pix2Text and Qwen2-VL-2B on printed items; report the §6 metrics.
5. Implement the JSON output contract and the confidence-tiered confirmation UI on desktop first (fastest iteration), then mobile.
6. Implement sequential load/unload; measure peak RSS on the 6 GB test phone; must be ≤ 3.0 GB.
7. Android share sheet, then iOS share extension, then desktop paste/drag.
8. Wire the probe set into CI.
9. Only then: collect handwriting items 11–17, run Qwen2.5-VL-3B vs Gemma 3 4B, and decide v0.2.

**Open questions for the founder:**

- Does the inference engine already handle non-text input graphs, or is step 3 a genuinely new capability?
- Is "ask the design-partner families for handwriting samples" acceptable under the current parent agreement, or does it need a one-line addendum?
- ~~Desktop or phone first for the v0.1 demo?~~ **Decided (Oct 31): phone first.** The desktop version proves it works; the phone version proves the thesis. A student photographing a textbook on a $200 Android in airplane mode and being tutored is the demo — it is exactly what the cloud-wrapper model cannot do. Consequences: the bake-off runs on the 6 GB test phone from day one, not the laptop; the §6 latency and memory gates are the primary acceptance criteria; desktop drag-drop/paste moves to the end of the task list and inherits the phone pipeline unchanged. Step 5 (confirmation UI) builds on Android first, not desktop.
