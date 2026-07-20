# LoRA Adapter + Distribution-Origin — Design Spec

**Date:** 2026-07-21
**Status:** Approved (design), pending implementation plan
**Ref:** on-device-rag-spec v1.3 §2.6 (ed25519 detached signature, compiled-in pinned pubkey)

## 1. Goal & the rule this implements

Every byte a user device loads — base models, LoRA adapters, knowledge packs, embedders — is fetched from **our** bucket via Cloudflare, hash-pinned against a **signed catalog**, and refused on mismatch. HuggingFace is **supplier-side only** (training pulls from it). One origin = one supply chain to secure, sign, and test; we are the redistributor, so licenses are complied with deliberately (Qwen3 = Apache-2.0, anonymous pull; Llama-3.2 = Meta Community License, token from a Meta-accepted account + attribution).

The load-bearing product payoff: the **behavioral adapter** is the real fix for the RAG over-refusal problem.

**Scope caveat (important — do not overclaim):** the v1 adapters shipping in this slice are stamped `contract_version: prompt-contract-v0` — the *pre-contract* era. They were trained on generic grounded refusal (SQuAD2-passage style), healthy pushback, and concession — **NOT** on the runtime's `[1] (source, locator)` citation rendering or the `NO_EVIDENCE` marker. So v1 meaningfully improves refusal *honesty*, pushback, and concession *behavior*; it does **not** natively speak the citation format. Closing the citation discipline is exactly what adapter **v2** (trained on the shared prompt-contract artifact) exists for — a later milestone. §8's probes are chosen for what v1 can deliver; **this slice does not promise citation discipline**, and the framing must not imply it.

## 2. Scope — the thin slice, and non-goals

**This milestone = the Qwen3-4B hero path end-to-end, both tracks.** When a chat in the app runs on the behavioral adapter pulled through the real signed-catalog path *and passes the Stage 5 probes* (§8), the slice is done. The 1B and 8B tiers are then **replication, not design**.

**Non-goals (explicitly deferred):** the runtime per-chat adapter hot-swap toggle (llama.cpp `/lora-adapters`); paid-pack entitlement gating (later = a Cloudflare Worker edge token check, *not* B2 presigned URLs); the Cloudflare front wiring itself (ops; the app uses a swappable base URL meanwhile); the offline signing ceremony; merging adapters into bases (never — merging turns 40 MB specialists into multi-GB downloads).

## 3. Architecture

### 3.1 Bucket split (B2)
- **`cleophis-models`** — PRIVATE lineage/pipeline warehouse: PEFT adapter dirs (`adapters/behavioral/v1/<base>/`), `datasets/behavioral/v1/`, manifests. The pipeline reads PEFT from here.
- **`cleophis-dist`** — PUBLIC-READ distribution: base GGUFs, adapter GGUFs, `catalog.json` + `catalog.json.sig`, packs later.
- Rationale: B2 public/private is **bucket-wide**, not per-prefix; publishing `cleophis-models` would publish the training internals. The split physicalizes the archival-vs-distribution distinction.
- Files are **immutable once uploaded** — a new build is a new versioned path, never an overwrite.

### 3.2 Signed catalog (reuse the pack trust root)
- `catalog.json` v0 (minimal; grow later, **never mutate existing entries**):
  ```json
  { "catalog_version": 1, "generated_at": "2026-07-21T...Z",
    "artifacts": [ { "path": "...", "sha256": "...", "size": 0, "kind": "base|adapter", "base_model": "Qwen3-4B", "version": "...", "license": "..." } ] }
  ```
- Signature: a **detached ed25519** signature over the exact catalog bytes → `catalog.json.sig`, using the **same curator key as knowledge packs**. The app verifies with the compiled-in `CURATOR_PUBLIC_KEY` via the existing `kpack_core::sign::{curator_verifying_key, verify_detached}` — no new crypto path.
- **Downgrade guard (the one thing the signature alone doesn't stop):** the catalog is the single object that gets *replaced*, so a network attacker could replay an OLDER, still-validly-signed catalog to roll a user back to a known-bad artifact. `catalog_version` is a **monotonic** integer; the app persists the highest version it has ever verified and **refuses any catalog older than that** (below §5). Superseded catalogs + their `.sig` are archived to `cleophis-models` for history (never deleted).
- `path`s are **relative** to a single configured artifact base URL (`cleophis-dist` native B2 URL now → Cloudflare hostname later).

### 3.3 Retire `mint_download_url` for catalog artifacts
Bases, behavioral adapters, and free packs are **public** artifacts whose **integrity** matters, not secrecy — and the signed catalog + sha256 gives us integrity. Signed URLs fight Cloudflare caching (unique query strings defeat cache keys) and put a server mint step in the download path (against the availability story). So the app's "download" for these becomes: **catalog-fetch → verify signature → per-artifact GET (public URL) → verify sha256 → atomic rename.** `mint_download_url` is left intact for any user-specific content it still serves; this change is scoped to catalog artifacts.

## 4. Track A — Pipeline (repo scripts, run locally, env-dumb)

CPU+RAM only (no GPU); fine in WSL2 for 4B/1B (8B slower). Keep each script environment-dumb — inputs, outputs, hashes — so they migrate to the RunPod successor unchanged.

For the 4B hero slice:
1. **Base:** pull Qwen3-4B from HF → `convert_hf_to_gguf.py` → f16 GGUF → `llama-quantize` → **Q4_K_M** GGUF.
2. **Adapter:** pull the Qwen3-4B PEFT dir from `cleophis-models` (B2) → `convert_lora_to_gguf.py` → `adapter.gguf` (keep the PEFT files as lineage).
3. **Hash + manifest:** sha256 each artifact; per-artifact `manifest.json` (name, source repo + revision commit, quant, sha256, license + required attribution, date).
4. **Catalog:** assemble `catalog.json` v0 from the artifact manifests.
5. **Sign (own final step, `sign_catalog`):** detached ed25519 over `catalog.json` with the curator private key (read from a file **outside the repo**, tight perms) → `catalog.json.sig`.
6. **Upload:** put base GGUF, adapter GGUF, `catalog.json`, `.sig` into `cleophis-dist` at immutable versioned paths (S3-compatible API with the write-scoped key). Archive any superseded `catalog.json`+`.sig` to `cleophis-models` history.
7. **Consumer self-check (final stage, mandatory):** impersonate the app from the **public** path — fetch `catalog.json` + `.sig` from the `cleophis-dist` public URL, verify the ed25519 signature, GET each artifact, verify its sha256. This catches the entire class of publishing mistakes (wrong path, wrong permissions, truncated upload, stale catalog) *before* the wrapper ever fetches. It is the same code shape as Track B steps 1–2, so it also serves as the reference implementation. The publish is not "done" until this asserts green from the outside.

One-time pipeline setup: llama.cpp checkout/build for the convert scripts + `llama-quantize`; Python deps (`huggingface_hub`, an S3 client for B2). The tooling setup is itself a scripted step.

## 5. Track B — Wrapper (Cleophis app)

1. **Catalog client:** fetch `catalog.json` + `.sig` from the configured artifact base URL; verify the detached ed25519 signature against the compiled-in curator pubkey; parse into typed artifact records. Refuse (no downloads) on signature failure. **Downgrade guard:** persist the highest `catalog_version` ever verified (an app-data record) and **refuse a catalog whose version is lower** — keep using what's already on disk rather than accept a rollback. (This defends against a network attacker replaying an old signed catalog; a local-file attacker who could reset the stored value already has device access, out of this threat model.)
2. **Artifact download:** reuse `download.rs`'s resumable ranged GET + streaming sha256 + atomic rename + progress events, but source the URL from the catalog record (public, no token) instead of `mint_download_url`/`DownloadAuth`. Verify each artifact's sha256 against its catalog entry; refuse on mismatch.
3. **Hero resolution:** the "hero" catalog entry repoints to the Qwen3-4B base + its behavioral adapter (the previous hero base retires). The app downloads both.
4. **Engine launch (§6):** start the sidecar with the base + `--lora adapter.gguf`.
5. **Provenance:** stamp the chat's `adapter_ids` (schema already present) with the behavioral adapter's id/version.

## 6. Engine integration (`inference.rs`)

- Launch llama-server with `-m <base Q4_K_M gguf> --lora <adapter.gguf>` (loaded separately; adapter always-on for the hero this slice). Hot-swap-capable shape; the runtime toggle is deferred.
- **Chat template:** Qwen3 uses **ChatML** — update whatever template-sensitive engine/prompt config assumed the old base.
- **`<think>` strip:** the Qwen adapters emit an empty `<think></think>` prefix (known artifact). v1 fix = runtime-strip it — but **leading-only, at the start of a turn's stream, not globally**: buffer the turn's opening, strip a leading (optionally whitespace-wrapped) `<think></think>` if present, then pass the remainder through untouched. A tutor legitimately discussing think-tags mid-answer must keep that content.

**Debug ladder (pre-decided — if the §8 probes FAIL in-app after passing in Colab, investigate in THIS order, do not retrain first):**
1. **Chat-template mismatch** (most likely) — confirm the engine/prompt path emits ChatML exactly.
2. **`<think>`-strip interference** — confirm the strip isn't eating real content or mis-framing the turn.
3. **LoRA-on-quantized-base interaction** (least likely; llama.cpp applies adapter GGUFs onto quantized bases fine, but quantization can shift behavior at the margins) — isolate with **one run of the adapter against the f16 base before quantization**; if it passes on f16 and fails on Q4_K_M, it's a quant-margin effect, not the pipeline.

## 7. Security

- **Curator private key:** never in `.env`. The `sign_catalog` step reads it from a file **outside the repo** with tight permissions (placed by the user for this slice). Longer-term = an offline/isolated signing ceremony — not built now.
- **Compile-time test-key guard (carry-forward from offline-auth):** release builds must be **compile-time incapable** of trusting any test keypair — the shipped app pins the **production** curator key only. Any test-key path is `#[cfg(test)]` / debug-only, never reachable in a release binary.
- **Routine creds:** the write-scoped B2 key (read `cleophis-models`, write `cleophis-dist`) and the HF token live in env / a gitignored `.env`, never committed.
- **No B2 credentials ship in the app** — the app only ever does public catalog-fetch → verify → GET. B2/S3 keys are pipeline-only.

## 8. Acceptance — the Stage 5 probes

The slice is accepted only when, in the running app on the adapter pulled through the real path, the behavioral probes hold — proving the adapter **survived the GGUF conversion**, not merely that "a chat runs":
- **Fake entity → refusal** (asked about a non-existent thing, it declines rather than confabulates).
- **"5 + 5 = 9" → pushback** (it disputes the wrong claim).
- **correction → concession** (given a valid correction, it concedes).
- **Medical-boundary** (asked for a diagnosis, it educates, declines to diagnose, and redirects to a clinician).

This is the **full four-probe set the Colab gate passed** — test all four, because a GGUF-conversion regression doesn't announce which behavior it ate. If any fail, walk the §6 debug ladder (do not retrain).

## 9. Dependencies / handoff (blocks running the pipeline E2E)

The wrapper code + pipeline scripts can be built without these; **running the pipeline and validating the slice** needs the user's one-sitting handoff:
- Create `cleophis-dist` (public-read).
- Scoped B2 keypair: READ `cleophis-models` (pull PEFT), WRITE `cleophis-dist` (publish).
- HF token (Meta-license-accepted).
- Place the curator private-key file (outside the repo, tight perms).
- (User homework, parallel: rclone-mirror `cleophis-models` → local + R2 — the PEFT dirs are single-copy.)

## 10. Deferred / future

Runtime per-chat adapter hot-swap toggle; 1B + 8B tier replication; paid-pack entitlement via a Cloudflare Worker edge token check; Cloudflare front wiring (swap the base URL); offline signing ceremony; embedder + pack migration onto the same signed-catalog origin.
