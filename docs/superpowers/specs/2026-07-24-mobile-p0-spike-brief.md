# Mobile P0 Spike — Kickoff Brief (for the mobile worktree instance)

**You are the mobile instance.** This brief is your standing orders for the P0 spike. The authoritative spec is `docs/superpowers/specs/2026-07-24-mobile-app-spec-v1.1.md` — read it first; where this brief and the spec disagree, the spec wins. The desktop/demo instance is steering trunk; you never entangle with it (ownership rules below).

## Mission and gate

Retire the one risk that decides whether Cleophis mobile is real: **llama.cpp in-process over FFI, on a phone, with the full product configuration.**

**The gate (spec §0, verbatim):** 1B streams tokens at usable speed on the reference low-end device with the **full hero adapter stack (behavioral + contract + voice) mounted via in-process FFI**, and the **four Stage-5 behavioral probes pass on-device**: fake-entity refusal, 5+5=9 pushback, concession, medical boundary. No unresolved build blockers.

A token-streaming demo without the adapter stack does not pass the gate. The probes were gated against the composed stack on desktop; mobile proves the same product, not a simpler one. If multi-LoRA over FFI proves flaky, the pre-agreed fallback is behavioral + contract with a re-gated probe run (spec §0 fallback ladder) — a measured retreat you escalate to the founder, never a silent simplification. The contract adapter is never dropped.

## Setup

```bash
git worktree add ../cleophis-mobile mobile/p0-spike
```
- Open your instance in `../cleophis-mobile`. Own `target/` (do not share or symlink it — expect 10–20 GB once NDK builds start; that's normal).
- **Rebase from main every day or two; merge to main only when the gate passes**, and additive-only (new crate, new dirs, CI job that runs on mobile branches only).

## Architecture: the trait is the demilitarized zone

First deliverable: a new crate **`crates/kpack-engine`** defining an `EngineBackend` trait —

```
load(base, adapters[]) → session(ctx) → stream(tokens) → unload()
```

— and the in-process `llama-cpp-2` implementation behind it. `adapters[]` is plural and ordered (behavioral→contract→voice); preserve the semantics of `src-tauri/src/inference.rs` (`build_server_args()` composition order, `verify_lora_once()` per-adapter sha256 gate, fail-closed on missing artifacts) without touching that file. No mobile-only assumptions in the trait — desktop inherits it after the demo ships.

Chat templating is your code now: the sidecar's `--jinja` read the template from the GGUF; your engine must resolve the template **per-model** (floor hero is Llama-3.2-1B — not ChatML; Qwen tiers are ChatML with the start-of-turn-only think-strip). Prompt rendering honors `contracts/prompt-contract.v1.toml` byte-for-byte via `kpack_core::contract`.

## Ownership rules (hard boundaries)

- **You own:** `crates/kpack-engine`, the `android/`/`ios/` scaffolding Tauri generates, mobile build config, a mobile-only CI job.
- **You may NOT modify:** `src-tauri/src/inference.rs`, `src-tauri/src/cloud/download.rs`, `src-tauri/src/catalog.rs`, `src-tauri/src/catalog_dist.rs`. These belong to the desktop instance through the demo.
- **Genuinely shared changes** (workspace `Cargo.toml` membership, root CI config, the `llama-cpp-2` workspace pin): flag to the founder, merge through main first, rebase after. Never carry them as local divergence.

## Hour-one checks (fail fast, in order)

1. **`llama-cpp-2` API coverage:** generation + multi-LoRA + chat-template APIs, at a pin `crates/kpack-embed` (currently `= 0.1.151`, feature `real`, embeddings-only) can also live with. One workspace, one version, one set of backend flags. If the pin must move, that's a flagged shared change (see above), and the embedder revalidates via golden-pack CI.
2. **NDK cross-compile of `llama-cpp-sys-2`** with **16 KB page alignment** (spec H2) — misaligned native libs crash on exactly the newest Android devices. Add the CI check the moment the build works.
3. **ARM dotprod/i8mm runtime detection** on the CPU path. CPU-first is policy (spec §1/H3); GPU stays behind a flag, never load-bearing.

If any of these three is a wall, report it the same day — that's the spike doing its job, not the spike failing.

## Measurements P0 must produce (hardware only — emulators lie, H1)

1. **Tokens/sec** on the reference floor device (4–6 GB Xiaomi/A-series class), full stack mounted.
2. **Total app RSS** with the WebView UI up and model loaded, per tier attempted, and observed LMK behavior. Model-file size is not the number; app-total is (spec §2).
3. **Three-LoRA composition**: load time and stability on 1B and 4B — plus probe results.

## What already ports (do not rebuild)

- sqlite-vec **static** registration + FTS5: solved — `crates/kpack-core/src/format.rs::register_vec()`; K1 result in `docs/superpowers/verification-milestone-kpack.md` ("the only path that reaches iOS"). Your job is to confirm golden-pack tests green on-device, not to re-derive the approach.
- Resumable download (`run_download`/`run_public_download` → `run_download_core` in `src-tauri/src/cloud/download.rs`), catalog verify (`parse_and_verify` in `catalog_dist.rs`), conversation store (`convstore.rs`) — all port as-is in P1; P0 doesn't touch them.

## Founder-serialized moments (ration these)

Your only hard dependency on human hands in P0: **on-device runs** — USB debugging enabled, a build side-loaded, results reported back. Surface at most one checkpoint per day ("APK builds, needs a device run"), fifteen minutes of founder time each. Signing-key ceremony and Google developer-verification registration are **P1-start** items — listed in spec §12, not yours to trigger in P0.

## Non-goals (P0)

Everything in spec §0 non-goals plus: no sync, no speech, no pack building, no store listings, no self-update implementation (that's P1 against `apps.json` — spec §4.1), no English-tutor surface (post-P1), no desktop migration onto the trait, no UI beyond the minimum needed to type a prompt and see tokens stream.

## Definition of done

Gate evidence posted: device model, tokens/sec, RSS numbers, probe transcript (4/4), build-blocker list (empty or with owners). Then stop — P1 is a separate decision, not a rolling continuation.
