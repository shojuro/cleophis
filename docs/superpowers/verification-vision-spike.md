# Vision Input — Stage 0 spike: go/no-go on the floor phone (2026-11-01)

The approved review (`~/.claude/plans/keen-baking-karp.md`, 2026-11-01) asked for one
thing before any product code: prove that llama.cpp's multimodal library links into the
Android build, and put measured Galaxy A22 numbers beside the gates for the small
document-OCR candidates. This is that record. Every number is from a JSONL record or a
device log under `~/cleophis-artifacts/vision/runs/` (the decisive records are copied
into `docs/superpowers/vision-spike/`); nothing is estimated unless marked.

## The call

**GO on the A22, on the formula-crop regime, under the plan's lever 6 (honest floor
gate).** With the shippable `armv8-a` build, granite-docling-258M reads a printed
formula crop exactly in one encoder pass at a median of 8.9 s and a 95th percentile
of 9.4 s, inside 389 MB, deterministically. That misses the strict 8 s median by
0.9 s and sits inside the 8–12 s band the plan reserved for shipping with a staged
progress state. The strict gate is reachable: the same binary built with the
dot-product kernels this phone has (never shippable as a static build, because of the
Galaxy A51 crash) does the same work at 6.4 s. Runtime kernel dispatch is therefore the
one remaining lever for the ≤ 8 s number, and it is a steering decision about the
static link, not a vision-feature task.

| Gate (floor phone, 512 px, 4 threads, greedy) | Threshold | Shippable floor build, one pass | Dotprod experiment build |
|---|---|---|---|
| exact after normalisation | ≥ 4/5 | **15/15** over 3 repeats | 5/5 |
| latency p50 / p95 | ≤ 8 s / ≤ 12 s | **8.9 s / 9.4 s** (p50 miss by 0.9 s; p95 pass) | **6.4 s / 6.7 s** |
| peak memory (VmHWM, model + projector + context) | ≤ 1.2 GB | **389 MB** | 559 MB |
| lever 6 band for shipping with honest UI | 8–12 s | **inside** | not needed |

Not a product yet: the bench is a measuring instrument. The confirmation gate, the
capture path and the catalog plumbing are Stages 1–2 of the plan.

## The build (the day-one question)

- `kpack-engine` gained a `vision` feature (llama.cpp `mtmd` through `llama-cpp-2`'s
  `mtmd` feature, plus `image` and `kamadak-exif`), separate from `real`, so the shipping
  APK is byte-identical until a product stage flips it. `examples/vision_bench.rs` is
  the instrument (`--features vision`).
- `cargo ndk -t arm64-v8a -P 24 build --release -p kpack-engine --features vision --example vision_bench`
  **links.** The native library compiled and linked on the first try; the first failure
  was one Rust argument in the bench. Example binary 6.9 MB.
- The multimodal tool is compiled by a separate `cc` build in the vendored `build.rs`
  that never received the cmake arch floor. It now carries `-march=armv8-a` (the fourth
  floor home; `check-cpu-floor.py` reads it: `PASS: aarch64-android CPU floor is armv8-a in all 4 homes`),
  and `audit-arch-floor.sh` disassembles the artifact:
  `PASS: …/vision_bench — nothing above the floor outside 4 outline-atomics helper(s)`,
  the same four helpers the shipped probe binary carries.
- `build.rs` did not track the vendored mtmd sources, so a source patch was once silently
  not compiled (cargo relinked the old objects). It now emits `rerun-if-changed` for each.
- Two bench defects met on the phone and fixed: a second `llama_backend_init`
  (`BackendAlreadyInitialized`; the engine's `shared_backend()` is now public and the
  bench reuses it, as the product's vision path will), and the binding's logit
  bookkeeping not seeing mtmd's prefill (`get_logits_ith` panics; log-probabilities are
  read only where the binding tracks them).
- All vendored edits are re-applied by `vendor-llama-sys-dotprod.sh`; the script was
  run against the committed pre-patch files and reproduced both patched files byte for
  byte (security item P08).

## The phone and the settings

Galaxy A22 5G (SM-A226B, Dimensity 700: 2×Cortex-A76 + 6×Cortex-A55, 8 GB RAM),
Android 13, USB. Kernel report from the device (both builds): `asimddp=1 i8mm=0 sve=0
asimdhp=1 lse-atomics=1`. Shipped build: `[kernels] CPU : NEON = 1 | ARM_FMA = 1 | REPACK = 1`,
verdict `OK (UNDERBUILT) — this CPU implements [asimddp asimdhp] which the build does not use`.
Dotprod experiment build: `… | DOTPROD = 1 | …`, verdict `OK (UNDERBUILT) — [asimdhp]`;
894 `sdot`/`udot` instructions in the binary. Greedy decoding, `n_ctx` 2048, explicit
`n_batch`, four threads unless stated.

Inputs rendered from our own LaTeX (licence-clean): five printed problems — linear,
fraction, quadratic, exponent/subscript with `\times`, a two-equation system — in three
shapes: a 900×330 strip (downscaled to 512×188), a 512×512 white page with a label
("Solve for x:"), and a 512×160 formula crop; plus an EXIF-orientation-8 JPEG copy.
Ground truth beside each image in `items.json`.

## Models

| File | sha256 (first 16) | bytes | `clip.vision.preproc_image_size` |
|---|---|---|---|
| granite-docling-258M-Q8_0.gguf | b687245c212b59da | 177,918,144 | — |
| mmproj-granite-docling-258M-Q8_0.gguf (as published) | 3d440e65a91e640d | 103,769,792 | **2048** (`image_size` 512) |
| mmproj-granite-docling-258M-Q8_0.preproc512.gguf (one u32 patched) | bb75a65aa3ec14ac | 103,769,792 | **512** |
| mmproj-SmolVLM-256M-Instruct-Q8_0.gguf | (control) | | absent → 512 |

The published projector's 2048 makes mtmd upscale every input to 2048 px and tile it
(7+ encoder passes, ~50 s/image on the A22, the first run crashed before output). The
patched header is a 4-byte change; the product should set the value at load time
rather than ship a patched file.

## Loading and memory (every run)

| | granite-docling-258M Q8, floor build | dotprod build | SmolVLM-256M Q8 |
|---|---|---|---|
| model load | 0.36–0.38 s | 0.74–0.77 s | 0.19 s |
| projector load (incl. warm-up encode) | 0.11 s | 0.11 s | 0.10 s |
| resident after both (VmRSS) | 388 MB | 558 MB | 295 MB |
| peak during an image (VmHWM) | 389–394 MB | 559–564 MB | 377 MB |

Memory gate (≤ 1.2 GB, for co-residency with the Qwen3-1.7B tutor under 3.0 GB on 8 GB
phones): **PASS** with wide margin. The WebView is not in these numbers (Stage 3's
`dumpsys meminfo` reading is).

## What the model is fed decides everything

| Input shape, prompt | Passes / image tokens | Exact on the A22 | Note |
|---|---|---|---|
| strip 512×188, page prompt, projector as published (2048) | 7+ | — (crashed) | the tiling trap above |
| strip 512×188, page prompt, projector 512 | 2 / 128 | 0/5 | padded to a black square, read as an empty page: "Powered by TCPDF" to the token cap |
| **page 512×512, page prompt, projector 512** | 2 / 128 | **4/5 × 3 repeats**, deterministic | the miss is one dropped coefficient ("x + y = 7" for "2x + y = 7"), identical every time |
| **formula crop 512×160, "Convert formula to latex.", projector 512** | 2 / 128 | **5/5** | exact LaTeX including the system |
| page, input downscaled to 448 / 384 (projector 512) | 2 / 128 | 1/5, 0/5 | the preprocessor upscales back to 512; detail lost |
| page, page prompt, **one pass** (opt-in) | 1 / 64 | 1/5 (host) | fractions and exponents flattened to plain text |
| **formula crop, formula prompt, one pass** (opt-in) | 1 / 64 | **15/15** (3 repeats) | the regime: same accuracy, half the encoder cost |
| SmolVLM-256M, strip, LaTeX prompt | 1 / 64 | 0/5 | describes shapes correctly; cannot read maths (control) |

The image path was proven with a sanity image (large text, a red circle, a blue
square): SmolVLM describes it identically through the Rust decode/EXIF/downscale path
and through mtmd's own file loader. The EXIF-8 copy is uprighted and reads like the
original.

"Exact" is after the normalisation pinned in `vision-spike/score-spike.py`: DocTags
`<loc_N>` tokens dropped; `\begin{array}{..}`/`\end{array}` wrappers, `\left`,
`\right`, `\,`, `\;` dropped; `\text{…}` labels, `\colon` and a leading
"Solve for x :" label dropped; grouping braces that are not an argument of `^`, `_`,
`\frac`, `\sqrt` unwrapped; single-character `^`/`_` arguments unbraced; whitespace
removed. One two-pass fraction output was cut at the 60-token cap after its closing
`\end{` — the maths before it is complete and the scorer tolerates the cut; Stage 3's
runs use a higher cap.

## Time on the A22 (4 threads unless stated)

| Run | Build | Passes | Prefill (≈ encode) | Decode | Total p50 / p95 | Exact |
|---|---|---|---|---|---|---|
| **formula crop, one pass** (15 records) | floor | 1 | 7.61–7.65 s | 32–33 tok/s, 31–57 tokens, 0.94–1.76 s | **8.91 s / 9.37 s** (min 8.57, max 9.37) | 15/15 |
| formula crop, two passes (run 4; re-run as the run-6 control: 16.41 s / 17.02 s, 5/5) | floor | 2 | 15.1 s | 31 tok/s | 16.4 s / 17.0 s | 5/5 |
| page, page prompt (15 records) | floor | 2 | 15.1 s | 31 tok/s, 40–61 tokens | 16.8 s / 18.2 s | 12/15 |
| formula crop, one pass | dotprod | 1 | 5.37–5.41 s | 42–46 tok/s | **6.39 s / 6.66 s** | 5/5 |
| formula crop, two passes | dotprod | 2 | 10.8 s | 39–44 tok/s | 11.96 s / 12.25 s | 5/5 |
| page, page prompt | dotprod | 2 | 10.8 s | 43–45 tok/s | 11.93 s / 12.27 s | 4/5 |
| strip, threads 2 | floor | 2 | 17.5 s | 27 tok/s | (256-token cap; encode column only) | — |
| strip, threads 8 | floor | 2 | 13.6 s | 15–22 tok/s, thermal drift | (256-token cap; encode column only) | — |

One 512-px encoder pass costs **7.6 s** on this CPU without dot-product kernels and
**5.4 s** with them; the pass count and the kernels are the whole story of the latency.
Decode is 1–2 s either way. The 15 one-pass records spread 8.57–9.37 s with prefill
within 44 ms of each other across three repeats: no thermal drift at four threads over
a 2.5-minute run.

## The six A22-first levers, as measured

1. **Thread placement** — 2, 4, 8 threads: encode 17.5 / 15.1 / 13.6 s for two passes,
   but 8 threads halves decode speed and drifts thermally. Four is the setting. Small gain.
2. **Smaller input** — 448 and 384 px inputs, and a 448 projector: accuracy collapses
   (1/5, 0/5) and the preprocessor still emits two passes. Dead.
3. **Hide the load** — model + projector load is 0.5 s. Nothing to hide for this model
   (it matters for the tutor reload on the sequential path, not here).
4. **Dot-product kernels** — measured with an experiment build (`armv8.2-a+dotprod` in
   all three `build.rs` homes, separate target directory, source restored and verified,
   binary never shipped): encode 7.6 → 5.4 s per pass, decode 32 → 45 tok/s, one-pass
   total **8.9 → 6.4 s**. The biggest lever, and the one that needs runtime dispatch
   (`GGML_CPU_ALL_VARIANTS`) to ship safely beside ARMv8.0 phones.
5. **Q8 projector** — in use throughout.
6. **Honest floor gate** — the one-pass formula regime lands at 8.9 s median / 9.4 s
   p95, inside the 8–12 s band. **This is the shipping configuration for the A22**
   until lever 4 lands: a staged progress state ("Reading the image…") over ~9 s.

Plus one the plan did not list, and the one that mattered most: **the preprocessor's
tiling rule.** Upstream mtmd always emits the global image and a tile grid for idefics3
models; Hugging Face's processor with `do_image_splitting=false` emits only the global
image. A six-line opt-in patch (`MTMD_IDEFICS3_NO_TILE=1`, 1×1 grids only) reproduces
HF's mode and halves the encoder cost. It costs page-prompt accuracy but nothing on
formula crops, which is why the product regime is the crop.

## Determinism

- **Across repeats on the device:** one distinct raw output per item over 3 repeats
  (one-pass formula, 15 records) and over 3 repeats (two-pass page, 15 records).
- **Host (x86-64, dev box) vs A22 floor build:** the five one-pass formula outputs are
  byte-identical.
- **Floor build vs dotprod build on the A22:** identical on all formula crops; on the
  page prompt the label text differs (`E \, \text {evaluate} \colon` vs
  `E a v i l u a t e \colon`) while the maths is identical. CI expectations for the
  page prompt would therefore be per-kernel-set; for the formula regime one expectation
  suffices.

## What this licenses for Stage 1

- **Transcriber:** granite-docling-258M Q8 + Q8 projector, `preproc_image_size` forced
  to 512 at load time (not a patched file), 4 threads, `n_ctx` 2048, greedy.
- **Regime:** the confirmation UI frames a **formula crop** (the student taps the
  problem; the app crops to it, longest edge 512); prompt "Convert formula to latex.";
  overview-only (one pass) on the floor tier. Two passes with the page prompt remain
  available for mid-tier phones if a whole-page mode is wanted later.
- **Stage 1 change list from the spike:** make the overview-only switch a context
  parameter in the vendored mtmd instead of an environment variable; raise the
  generation cap above 60 tokens (the array-wrapped fraction needs ~62); parse DocTags
  (`<loc_*>`, `\begin{array}`, `\text{…}\colon` labels) in `vision_parse`; the
  confidence signal is available (per-token log-probabilities after the first token);
  the progress state shows "Reading the image…" for the 8–10 s the floor tier takes.
- **Owed before the product gate (Stage 3):** the spec's own 20-image probe set with
  photographed pages, a diagram-only and a non-maths image (this spike used five rendered
  items and one EXIF copy on one phone); the WebView-inclusive memory reading; a
  mid-tier phone if one is available (none was).

## Caveats, stated plainly

- Five items, rendered not photographed, one phone. The 4/5 and 5/5 and 15/15 counts are
  small samples; the deterministic repeats say the model is stable, not that it is
  accurate on photographs.
- The strict ≤ 8 s median is missed on the shippable build (8.9 s). The call rests on
  the plan's lever 6 clause, taken as written.
- The dotprod numbers are a measurement of what runtime dispatch would give; that build
  must not ship (the A51 SIGILL of 2026-07-31).
- VmHWM is the bench process only; the app adds the WebView and, when co-resident, the
  tutor.

## Files and records

- Branch `mobile/vision-spike` (from 93769b9): `crates/kpack-engine/{Cargo.toml,src/llama.rs,src/lib.rs,examples/vision_bench.rs}`;
  `third_party/llama-cpp-sys-2/{build.rs,llama.cpp/tools/mtmd/mtmd-image.cpp}` (vendored
  edits, listed in `vendor-llama-sys-dotprod.sh`); `docs/superpowers/mobile-tools/{run-vision-bench.sh,audit-arch-floor.sh,check-cpu-floor.py,vendor-llama-sys-dotprod.sh}`.
- `docs/superpowers/vision-spike/`: the decisive JSONL records (floor one-pass ×3,
  floor two-pass, page ×3, dotprod ×3, SmolVLM control), both `items.json`, and
  `score-spike.py`. Device logs, models and images stay in `~/cleophis-artifacts/vision/`
  (shas in `MANIFEST.txt`).
