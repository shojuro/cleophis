# Low tier → Qwen3-1.7B — design

**Status:** approved, pending implementation. **Date:** 2026-07-21.
**Branch:** `feat/low-tier-qwen3-1.7b`.

## Problem

The wrapper tier-selection milestone (PR #24) shipped low→`Llama-3.2-1B`,
mid→`Qwen3-4B`, high→`Qwen3-8B`. User E2E confirmed the 1B's behavior (Socratic +
honest) held, but its **math is weak — a base-level limitation of Llama-3.2-1B**, not
the behavioral adapter. Llama-3.2-1B is also the lineup's only licensing oddball
(Meta Community License, gated repo, served via the unsloth mirror), while the 4B/8B
are Qwen3/Apache-2.0.

## Decision

Replace the low tier's base with **`Qwen3-1.7B` (non-thinking)**:

- **Better math** — Qwen3's small models are markedly stronger on grade-school /
  algebra math than Llama-3.2-1B, and Qwen3-1.7B (newer gen) edges Qwen2.5-1.5B.
- **One family** — same generation as the 4B/8B: identical ChatML template, the same
  empty-`<think>` quirk the behavioral adapters already emit and the app already
  strips, the same training recipe. The low tier stops being a special case.
- **Clean license** — Apache-2.0; retires the Llama gating/mirror workaround. Whole
  lineup becomes Qwen3/Apache-2.0.
- **Still low-tier-sized** — ~1.7B params, ~1.1 GB at Q4_K_M, runs on the weak-device
  tier (<16 GB RAM, no dGPU).

Rejected alternatives: keep Llama-3.2-1B + stack a math LoRA / add a calculator tool
(more complexity at 1B capacity, or tool-use plumbing, for less gain); a
math-specialized base like Qwen2.5-Math-1.5B (fights the Socratic/behavioral role —
the low tier is still the tutor, not an equation cruncher). Thinking-mode (Qwen3's
CoT) would boost math further but conflicts with the Socratic "ask, don't solve"
ethos and the leading-`<think>` strip — deferred as a future lever, not this
milestone.

## Scope

Because the tier-selection code already generalizes over `base_model`
(`catalog::hero_variant`), this is a **data + artifacts** milestone — the low tier's
base+adapter identity changes; **no app logic changes**. Three tracks, supplier →
producer → consumer.

### S1 — Re-train `behavioral-v1` on Qwen3-1.7B (supplier, GPU — critical path)
- Reuse the existing `datasets/behavioral/v1/` (in the private `cleophis-models`
  bucket) and the documented behavioral-v1 recipe: QLoRA on the unsloth Qwen3-1.7B
  4-bit variant, ChatML, the empty-`<think>` convention — identical to how the 4B/8B
  behavioral adapters were produced (a re-run on a new base, not a new recipe).
- **Gate in the training env** with the four Stage-5 probes (fake-entity→refusal,
  "5+5=9"→pushback, correction→concession, medical→decline) **plus a math
  spot-check** vs. the old Llama-1B — confirming the math actually improved is the
  point of the milestone.
- Output: PEFT adapter dir + manifest (`license: Apache-2.0`) uploaded to
  `cleophis-models` at `adapters/behavioral/v1/Qwen3-1.7B-unsloth-bnb-4bit/`.
- Note: the original training Colab is gone, but the dataset + recipe survive — S1
  reconstructs the training env (Colab/RunPod) for the new base.

### S2 — Pipeline → signed catalog v4 (producer, local WSL)
- Add `"Qwen3-1.7B"` to `KNOWN_BASE_MODELS` in `tools/pipeline/build_catalog.py` and
  `verify_published.py` (keep `Llama-3.2-1B` — its v3 artifacts remain immutable and
  in-catalog).
- `build_base.py Qwen/Qwen3-1.7B` → `convert_hf_to_gguf` → `llama-quantize Q4_K_M` →
  `Qwen3-1.7B-Instruct-Q4_K_M.gguf`.
- `build_adapter.py --license Apache-2.0` (pull PEFT from `cleophis-models`) →
  `convert_lora_to_gguf` → `behavioral-v1-Qwen3-1.7B.gguf`.
- `build_catalog.py` → **catalog v4** = v3's 6 artifacts **+** the 2 new Qwen3-1.7B
  artifacts (8 total; existing entries never mutated). `sign_catalog.py` (detached
  ed25519 over the bytes, same curator key). `publish.py` uploads the 2 new immutable
  artifacts, archives v3→`cleophis-models/archive/catalogs/v3/`, publishes v4.
- `verify_published.py` consumer self-check: fetch catalog+sig from the public path →
  verify signature + downgrade guard → GET + sha256 every artifact (8) → green.

### S3 — App: swap the low-tier entry (consumer)
- `src-tauri/resources/catalog.json` `socratic-tutor.tiers.low` → Qwen3-1.7B:
  `baseModel: "Qwen3-1.7B"`, `sizeParams: "1.7B"`, `modelFile:
  "models/Qwen3-1.7B-Instruct-Q4_K_M.gguf"` + its sha256, `adapterFile:
  "models/behavioral-v1-Qwen3-1.7B.gguf"` + its sha256, `adapterId:
  "behavioral-v1-qwen3-1.7b"`, `fileBytes` — all from the signed catalog v4.
- Update the `catalog.rs` low-tier test assertion (`Llama-3.2-1B` → `Qwen3-1.7B`) and
  the FE `TIER_OPTS` low label (`Small · 1B ≈0.8 GB` → `Small · 1.7B ≈1.1 GB`).
- Rebuild the MSI.

### S4 — Acceptance + PR
- In-app on the mid box via the tier override → the Small tier now runs Qwen3-1.7B:
  the **4 Stage-5 probes pass**, and a **math spot-check** is visibly better than the
  old Llama-1B. `verification-milestone-low-tier-qwen3.md`; PR.

**Ordering:** S1 → S2 → S3 → S4 (each consumes the prior's output). S1 is the
critical path (GPU training).

## Files

- Producer: `tools/pipeline/build_catalog.py`, `tools/pipeline/verify_published.py`
  (`KNOWN_BASE_MODELS` += Qwen3-1.7B); build/publish are re-runs of the existing
  scripts (`build_base.py`, `build_adapter.py`, `sign_catalog.py`, `publish.py`).
- Consumer: `src-tauri/resources/catalog.json` (tiers.low), `src-tauri/src/catalog.rs`
  (low-tier test), `src/app.js` (`TIER_OPTS` low label).
- No changes to the tier-selection logic (`tier_select.rs`, `inference.rs`) — it is
  already base-model-agnostic.

## Verification

- Training env (S1): 4 Stage-5 probes + math spot-check pass on Qwen3-1.7B before the
  adapter is published.
- Pipeline (S2): `verify_published.py` green — signed catalog v4, 8 artifacts, every
  sha256 verified from the public path.
- App (S3): `cargo test -p cleophis` green (low-tier assertion updated); MSI builds.
- E2E (S4, user, MSI): Small tier downloads + launches Qwen3-1.7B, deletes the prior
  pair, 4 probes pass, math visibly improved.

## Risks / notes

- **S1 is the real effort** — reconstructing the training env (old Colab gone; dataset
  + recipe survive in `cleophis-models`).
- **Immutability** — catalog v4 *adds* the Qwen3-1.7B artifacts; the Llama-3.2-1B
  ones stay published-but-unreferenced. Never overwrite an existing artifact path.
- **Non-thinking is deliberate** — if Qwen3-1.7B math still underwhelms, thinking-mode
  is a separate future lever (conflicts with the Socratic strip today).
- Scope guard: low tier only; do not touch the mid/high tiers, the tier-selection
  logic, or auth/payment/RAG.
