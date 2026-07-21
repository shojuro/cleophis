# Wrapper tier-selection — design

**Status:** implemented (branch `feat/tier-selection`), pending user E2E on the MSI.
**Date:** 2026-07-21

## Problem

All three behavioral tiers are published and live in the signed dist catalog v3
(`cleophis-dist`): **low → Llama-3.2-1B, mid → Qwen3-4B, high → Qwen3-8B**, each a
base GGUF + its behavioral-v1 LoRA adapter. But the shipped app launches the one
bundled hero — `inference::resolve_launch → catalog::hero` picks the first `real`
entry (`socratic-tutor`), whose flat fields hardcode the 4B pair. The published
1B/8B are never consumed. The app must pick the base+adapter by **device hardware
tier**, with a **manual override** — which also makes all three tiers testable on
one machine.

The download spine already exists (PR #21, "B4"): `src/app.js heroDownload()` does
`fetch_dist_catalog()` → picks the base+adapter for a hardcoded `HERO_BASE_MODEL` →
`download_artifact` ×2 → launch via `--lora`. `hardware::detect().tier` already
returns `low`/`mid`/`high`. So this is wiring the two hardcoded-to-4B spots to an
effective tier, plus a selection/switch-limit state machine.

## Decisions

- **Approach A — per-tier variants on the bundled hero entry.** `socratic-tutor`
  gains a `tiers` block (low/mid/high), each pinning `baseModel · sizeParams ·
  modelFile · sha256 · adapterFile · adapterSha256 · adapterId · fileBytes` from the
  signed dist catalog v3. Fully offline; the signed dist catalog stays the download +
  hash authority and cross-checks bytes at download. The flat fields stay mirrored to
  `mid` for back-compat.
- **Effective tier** = override `mode` if set, else `hardware::detect().tier`.
  Persisted in `app_data/tier_selection.json`.
- **One active model on disk** — after a successful switch, delete the previous
  tier's pair and sweep orphaned `*.gguf` (also clears the pre-tiers `Llama-3.2-3B`).
  Local cleanup by the running app; no remote wipe.
- **Switch limit tied to the entitlement** — the tier is freely changeable **until
  the first chat** (a one-way `committed` latch), then **≤1 change per billing
  period**, keyed off the active hero entitlement's `expires_at` (the period key).
  No active entitlement → refused (same as the lapse/paused-downloads UX). Perpetual
  grant (`expires_at: null`) → rolling 30-day cooldown. Enforced in Rust against the
  locally-cached entitlements, so it works offline; the FE only reflects it. The
  limit is not DRM — bytes already on disk aren't clawed back.
- **FE** — a demo-friendly "pick your engine size" selector in the hero drawer.

## Architecture

### Data (`catalog.rs`, `resources/catalog.json`)
- `TierVariant` + `Tiers { low, mid, high }`; `CatalogEntry.tiers: Option<Tiers>`.
- `hero_variant(entry, tier) -> ResolvedHero` selects a variant (mid-default, flat-
  field fallback for pre-tiers entries). Every field is `Option` to unify both
  sources.

### Selection + limit (`tier_select.rs`)
- `TierSelection { mode, active_tier, committed, period_key, switched_at }` with
  atomic `read_selection`/`write_selection`.
- `effective_tier(app)` = override else `hardware::detect().tier` — the single tier
  the launch, verify, and download paths key off, so they agree.
- `resolve_hero_entitlement(entitlements, now) -> HeroEntitlement` (live Period /
  Perpetual / None) from the cached entitlements.
- `switch_allowed(committed, ent, recorded_period_key, switched_at, now)` — the pure
  state machine: uncommitted → free; committed + Period → allowed iff the period key
  advanced; committed + Perpetual → 30-day cooldown; None → refused.
- `sweep_models(models_dir, keep)` — delete every `*.gguf` not in the active pair.

### Launch / status (`inference.rs`, `cloud/download.rs`)
- `resolve_launch` + `hero_hash` resolve base/adapter + hashes through
  `hero_variant(hero, effective_tier(app))`.
- `download_status` reports "installed" for the effective tier's pair.
- `Engine.thread_alive` + `restart(app, engine)` — stop the running engine and
  relaunch on the new pair, waiting for the old watchdog thread to fully exit first
  (so the two never contend over the child process / VRAM).

### Commands (`tier_select.rs`, registered in `main.rs`)
- `get_tier_selection` → `{ mode, activeTier, effectiveTier, committed,
  switchAvailable, nextChangeAt }`.
- `begin_tier_switch(mode)` → validate + enforce the limit → `{ targetTier,
  baseModel, needsDownload, noOp }` or `Err(user message)`.
- `complete_tier_switch(mode)` → persist, relaunch on the new pair, sweep the old
  pair + orphans (re-checks the limit — authoritative, not FE-trusted).
- `mark_tier_committed()` → latch `committed` on the first chat.

### Frontend (`src/app.js`, `src/styles.css`)
- `heroBaseModel`/`heroAdapterId`/`tierVariant` read the catalog `tiers` block + the
  cached `get_tier_selection`. `heroDownload` refactored onto a shared
  `downloadHeroPair(baseModel)`.
- "Pick your engine size" selector (Auto / Small·1B / Balanced·4B / Large·8B) →
  `selectTier` → `begin_tier_switch` → download-if-needed → `complete_tier_switch`;
  the per-period limit renders as disabled options + a hint. First chat fires
  `mark_tier_committed`. Chat provenance stamps the active tier's adapter id.

## Acceptance

The Stage-5 probes on all three tiers, exercised on the one mid machine via the
override: fake-entity→refusal, "5+5=9"→pushback, correction→concession,
medical-boundary→decline. See `verification-milestone-tier-selection.md`.

## Non-goals / risks

- Hash duplication (bundled `tiers` vs. signed catalog) — acceptable; a future
  pipeline step can emit the block. The dist catalog re-hashes at download.
- 8B on the demo box (16 GB RAM, 4 GB VRAM) runs on CPU — fine for a probe, slow for
  sustained use; that is why auto maps it to `high` only.
- The limit is not DRM. Scope guard: no changes to payment/auth/CSP/RAG/pipeline.
