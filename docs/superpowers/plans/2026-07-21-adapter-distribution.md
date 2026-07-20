# Adapter + Distribution-Origin Implementation Plan (Qwen3-4B hero slice)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax. NOTE: this session's subagent limit is reached — inline execution (or a fresh session) may be required.

**Goal:** Walk a byte through the whole company end-to-end — HF → local pipeline → signed catalog in `cleophis-dist` → public GET → hash check → engine — so the app runs a chat on the Qwen3-4B behavioral adapter pulled through the real path and passes the four Stage-5 probes.

**Architecture:** Two tracks. **Pipeline** = env-dumb Python/shell scripts in `tools/pipeline/` that build the base + adapter GGUFs, hash + manifest them, assemble + sign `catalog.json`, upload to `cleophis-dist`, and self-check from the public path. **Wrapper** = Rust/JS app changes: a signed-catalog client (ed25519 verify + monotonic downgrade guard), artifact download reusing `download.rs` but from public URLs, and engine launch with `--lora` (ChatML, leading-only `<think>`-strip). A **handoff gate** (user provides bucket/keys/token/curator-key) sits between building the code and running it live.

**Tech Stack:** Rust (Tauri 2 backend, `ed25519-dalek` already via kpack-core, `sha2`, existing `download.rs`), vanilla JS FE, Python 3 pipeline (`huggingface_hub`, an S3 client for B2, llama.cpp `convert_hf_to_gguf.py` / `convert_lora_to_gguf.py` / `llama-quantize`).

## Global Constraints

- Repo path has a space: `/mnt/c/Users/JM505 Computers/dev/cleophis` — always quote it. Branch `feat/adapter-distribution` (spec already committed there).
- Reuse `kpack_core::sign::{curator_verifying_key, verify_detached}` for the catalog signature — no new crypto path. Detached ed25519 over the exact catalog bytes.
- **Downgrade guard:** `catalog.json` carries a monotonic `catalog_version`; the app persists the highest ever verified and refuses anything lower.
- **Compile-time test-key guard:** release builds must be compile-time incapable of trusting a test keypair — any test key is `#[cfg(test)]`/debug-only; the shipped app pins the production curator key only.
- Retire `mint_download_url`/`DownloadAuth` for catalog artifacts ONLY (base, adapter). Leave it for any user-specific content. Public GET, integrity by sha256 + catalog signature.
- Base + adapter loaded **separately** via llama.cpp `--lora` — never merged.
- Qwen3 = **ChatML** template; strip a **leading-only** empty `<think></think>` at turn start, never globally.
- Artifact base URL is a single config value (`cleophis-dist` native B2 URL now → Cloudflare later); catalog `path`s relative.
- **No B2/S3 credentials ship in the app.** Pipeline creds live in a gitignored `.env`; the curator private key is read by the `sign_catalog` step from a file OUTSIDE the repo, tight perms — never in `.env`, never committed.
- Files in `cleophis-dist` are immutable — new build = new versioned path; superseded catalogs archived to `cleophis-models`.
- Acceptance = the FOUR Stage-5 probes in-app (fake-entity refusal, "5+5=9" pushback, correction concession, medical-boundary), on artifacts pulled through the real path. v1 is `prompt-contract-v0` — it does NOT promise the `[1](source,locator)` citation format.
- Rust build/test (from bash): `PWSH='/mnt/c/Program Files/PowerShell/7-preview/pwsh.exe'; "$PWSH" -Command "cd 'C:\Users\JM505 Computers\dev\cleophis'; $env:LIBCLANG_PATH='C:\Program Files\LLVM\bin'; $env:PATH='C:\Program Files\CMake\bin;'+$env:PATH; & 'C:\Users\JM505 Computers\.cargo\bin\cargo.exe' <cmd> -p cleophis"`. FE: `node -c src/app.js`. Commit trailer `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`; each task `git add`s only its own files, never `-A`.

---

## Track B — Wrapper (build first; unit-testable without the handoff)

### Task B1: Signed-catalog module — types + ed25519 verify + downgrade guard

**Files:**
- Create: `src-tauri/src/catalog_dist.rs` (the signed distribution catalog — distinct from the bundled product `catalog.rs`)
- Modify: `src-tauri/src/main.rs` (`mod catalog_dist;`)
- Test: inline `#[cfg(test)]`

**Interfaces:**
- Produces: `pub struct DistCatalog { pub catalog_version: u64, pub generated_at: String, pub artifacts: Vec<Artifact> }`, `pub struct Artifact { pub path: String, pub sha256: String, pub size: u64, pub kind: String, pub base_model: String, pub version: String, pub license: String }` (serde).
- `pub fn parse_and_verify(catalog_bytes: &[u8], sig: &[u8], key: &VerifyingKey) -> Result<DistCatalog, String>` — `verify_detached` first (reuse kpack sign), then `serde_json::from_slice`.
- `pub fn check_not_downgrade(new_version: u64, highest_seen: u64) -> Result<(), String>` — `Err` if `new_version < highest_seen`.

**Implementation notes:** production callers pass `kpack_core::sign::curator_verifying_key()`. Tests build a throwaway `SigningKey` (dev-dependency `ed25519-dalek` rand), sign test bytes, and pass its `VerifyingKey` — this is `#[cfg(test)]` only, so a release binary can never reach it (satisfies the compile-time guard). The persisted `highest_seen` lives in app-data (a tiny JSON) — Task B2 wires the persistence; B1 is the pure logic.

- [ ] Step 1: Write failing tests — `verify_rejects_bad_signature` (flip a byte → Err); `verify_accepts_good_signature_and_parses` (sign a v0 catalog with a test key → Ok, fields parse); `downgrade_refused` (`check_not_downgrade(3, 5)` → Err); `same_or_newer_ok` (`(5,5)` and `(6,5)` → Ok).
- [ ] Step 2: Run → fail (module not defined). `... cargo test -p cleophis catalog_dist`.
- [ ] Step 3: Implement `catalog_dist.rs` + `mod` in main.rs.
- [ ] Step 4: Run → pass; `... cargo build -p cleophis` exit 0.
- [ ] Step 5: Commit (`src-tauri/src/catalog_dist.rs`, `src-tauri/src/main.rs`): `feat(catalog): signed distribution-catalog verify + downgrade guard`.

### Task B2: Catalog fetch + persisted highest-version + artifact resolution

**Files:** Modify `src-tauri/src/catalog_dist.rs` (fetch + app-data persistence + a `#[tauri::command] fetch_dist_catalog`), `src-tauri/src/main.rs` (register). Config: add the artifact base URL as a compiled-in constant (or `tauri.conf.json`/env) — a single value.

**Interfaces:** Consumes B1. Produces `#[tauri::command] async fn fetch_dist_catalog(app) -> Result<DistCatalog, String>`: GET `<base>/catalog.json` + `<base>/catalog.json.sig` → `parse_and_verify` with `curator_verifying_key()` → `check_not_downgrade` against the persisted highest → persist the new highest → return. Refuse (Err, no state change) on any failure.

- [ ] Step 1: Test the persistence helper (read/write highest-seen JSON round-trips; missing file → 0) + a fetch test against a local mock HTTP server (mirror `download.rs`'s `start_mock_server` test pattern) serving a signed catalog → returns parsed; serving an older version after a newer → Err.
- [ ] Step 2–4: RED → implement (reuse the HTTP client `download.rs` uses; base URL constant) → GREEN + build.
- [ ] Step 5: Commit.

### Task B3: Artifact download via public URL (adapt `download.rs`)

**Files:** Modify `src-tauri/src/cloud/download.rs` (+ `main.rs` if a new command). Read `download_model` (the `auth_provider`/`DownloadAuth` mint path, the sha256 verify + atomic rename) first.

**Interfaces:** Produces a download path that takes a public URL + expected sha256 + dest filename (no token). Reuse the existing resumable-GET + streaming-sha256 + atomic-rename core; drop the `auth_provider` for this path. A `#[tauri::command] download_artifact(app, artifact: Artifact)` (or extend the hero-download flow) → downloads `<base>/<artifact.path>` → verifies `artifact.sha256` → atomic rename into the models dir. `mint_download_url` stays for anything else.

- [ ] Step 1: Test — against the mock server (existing pattern), download a blob whose sha256 matches the record → Ok, file present, bytes verified; a mismatched sha256 → Err, no finalize. Resumability test if the existing suite has one to mirror.
- [ ] Step 2–4: RED → implement (factor the shared core so `download_model` and `download_artifact` don't duplicate the GET/verify/rename) → GREEN + build.
- [ ] Step 5: Commit.

### Task B4: Engine `--lora` + ChatML + leading-only `<think>`-strip + hero repoint

**Files:** Modify `src-tauri/src/inference.rs` (launch args), the hero/catalog resolution (point the hero at Qwen3-4B base + adapter), `src/app.js` (leading-only `<think>`-strip on the streamed turn; ChatML if the FE builds the prompt — confirm where the template lives), and provenance (`adapter_ids`).

**Interfaces:** `inference.rs` launch gains `--lora <adapter_path>` when an adapter is present for the model. The `<think>`-strip: at turn start, buffer the opening of the assistant stream; if it begins with an optionally-whitespace-wrapped `<think></think>`, drop that prefix; then stream the remainder untouched (never strip mid-turn).

- [ ] Step 1: (Rust) Test that the launch-arg builder includes `--lora <path>` iff an adapter path is provided, and omits it otherwise (a pure arg-vec builder is the testable unit — factor it out if needed).
- [ ] Step 2: (FE) Implement the leading-only strip in the stream handler (`sendCompletion`/`finishStream`): a small turn-scoped buffer that removes a leading `<think></think>`; `node -c` + reason about the mid-turn-safe behavior. Confirm ChatML is emitted by whatever builds the model prompt (llama-server applies the model's chat template from the GGUF metadata by default — verify the Qwen3 GGUF carries ChatML; if the app injects a template, set ChatML).
- [ ] Step 3: Hero repoint — the hero catalog entry resolves to the Qwen3-4B base + its adapter; the previous hero base is no longer launched. Stamp `adapter_ids` on new chats with the behavioral adapter id/version.
- [ ] Step 4: Build + `node -c`; commit.

---

## Track A — Pipeline (`tools/pipeline/`, env-dumb; write now, run after handoff)

### Task A1: Pipeline scaffold + tooling setup + `.env` contract

**Files:** Create `tools/pipeline/` — `requirements.txt` (`huggingface_hub`, `boto3` or `b2sdk`), `setup_tools.sh` (clone llama.cpp at a pinned tag, build `llama-quantize`, note the convert scripts' paths), `.env.example` (`B2_KEY_ID`, `B2_APP_KEY`, `B2_ENDPOINT`, `HF_TOKEN`, `CURATOR_KEY_FILE` pointing OUTSIDE the repo), and a `README.md` documenting the env-dumb contract (inputs/outputs/hashes). Add `tools/pipeline/.env` + any large scratch dirs to `.gitignore`.

- [ ] Step 1: Write the scaffold + `setup_tools.sh` (idempotent; pins the llama.cpp revision — record the commit).
- [ ] Step 2: Run `setup_tools.sh` locally; confirm `convert_hf_to_gguf.py`, `convert_lora_to_gguf.py`, and a `llama-quantize` binary are available (this needs no creds). Fix until green.
- [ ] Step 3: Commit `tools/pipeline/**` (NOT `.env`).

### Task A2: Base → Q4_K_M GGUF (`build_base.py`)

**Files:** `tools/pipeline/build_base.py`. Inputs: HF repo id + revision (pin the commit), quant (`Q4_K_M`). Outputs: `<out>/<model>/<quant>/model.gguf` + `manifest.json` (name, source repo+revision commit, quant, sha256, license+attribution, date). Env-dumb: paths + hashes only.

- [ ] Step 1: Implement: `huggingface_hub` download (Qwen3-4B anon) → `convert_hf_to_gguf.py` → f16 GGUF → `llama-quantize ... Q4_K_M` → sha256 → write manifest.
- [ ] Step 2: Run on Qwen3-4B (no creds needed for Qwen). Verify the GGUF loads in a quick `llama-cli`/`llama-server` smoke run and the manifest sha256 matches. Fix until green.
- [ ] Step 3: Commit `build_base.py`.

### Task A3: Adapter PEFT → `adapter.gguf` (`build_adapter.py`)

**Files:** `tools/pipeline/build_adapter.py`. Inputs: the PEFT dir (pulled from `cleophis-models` — needs the read key) for `Qwen3-4B`. Outputs: `adapter.gguf` + updated manifest (sha256, base_model, contract_version from the PEFT manifest). **Read the actual PEFT manifest format in the bucket first** to carry its fields forward.

- [ ] Step 1: Implement the B2 read (S3 client) of the PEFT dir → `convert_lora_to_gguf.py` → `adapter.gguf` → sha256 → manifest. (Needs the handoff read-key to run; the code is writable now.)
- [ ] Step 2 (after handoff): Run; verify `adapter.gguf` + hash; keep the PEFT files as lineage.
- [ ] Step 3: Commit `build_adapter.py`.

### Task A4: Catalog assemble + `sign_catalog`

**Files:** `tools/pipeline/build_catalog.py` (assemble `catalog.json` v0 from the artifact manifests: `catalog_version` monotonic, `generated_at`, `artifacts[]`), `tools/pipeline/sign_catalog.py` (its OWN step — read the curator private key from `$CURATOR_KEY_FILE` outside the repo, detached ed25519 over the exact `catalog.json` bytes → `catalog.json.sig`; must match `verify_detached`'s expectations byte-for-byte).

- [ ] Step 1: Implement both. Unit-test the sign/verify round-trip locally with a THROWAWAY key (never the production key in a test): sign catalog bytes → verify with the matching pubkey → Ok; a flipped byte → fail. (Mirror the Rust B1 test at the Python layer.)
- [ ] Step 2: Confirm `catalog_version` increments monotonically vs any existing catalog in `cleophis-dist` (read the current one; +1).
- [ ] Step 3: Commit `build_catalog.py`, `sign_catalog.py`.

### Task A5: Upload + archive (`publish.py`)

**Files:** `tools/pipeline/publish.py`. Upload base GGUF, `adapter.gguf`, `catalog.json`, `.sig` to `cleophis-dist` at immutable versioned paths (write key). Before overwriting the live `catalog.json`, copy the current `catalog.json`+`.sig` to a `cleophis-models` archive path. Refuse to overwrite an existing immutable artifact path.

- [ ] Step 1: Implement (S3 client, write to `cleophis-dist`, archive-copy to `cleophis-models`). Guard: `head` each artifact path; if it exists, error (immutability), except the mutable `catalog.json`/`.sig` (which archive-then-replace).
- [ ] Step 2 (after handoff): Run; confirm objects present + public-readable.
- [ ] Step 3: Commit `publish.py`.

### Task A6: Consumer self-check (`verify_published.py`) — the packet-capture test

**Files:** `tools/pipeline/verify_published.py`. Impersonate the app from the PUBLIC path: GET `<public base>/catalog.json` + `.sig` → verify ed25519 (pinned pubkey) → for each artifact GET the public URL → verify sha256. Exit non-zero on any failure. This is the same shape as Track B B1–B3 and is the publish's definition of done.

- [ ] Step 1: Implement (reuse the Python verify from A4). No creds — public path only.
- [ ] Step 2 (after handoff): Run against `cleophis-dist`; must be green before the wrapper is pointed at it.
- [ ] Step 3: Commit `verify_published.py`.

---

## Handoff gate + end-to-end (after the code above is built)

- [ ] **HANDOFF (user, one sitting):** create `cleophis-dist` (public-read); scoped B2 keypair (READ `cleophis-models`, WRITE `cleophis-dist`); HF token (Meta-accepted); place `$CURATOR_KEY_FILE` outside the repo with tight perms. (User parallel homework: rclone-mirror `cleophis-models`.) Place creds in `tools/pipeline/.env` (gitignored).
- [ ] **Run the pipeline E2E:** `build_base.py` → `build_adapter.py` → `build_catalog.py` → `sign_catalog.py` → `publish.py` → `verify_published.py` green.
- [ ] **Point the wrapper** at the `cleophis-dist` public base URL; build the app MSI (reuse `msi-build`); launch.
- [ ] **Acceptance — the four Stage-5 probes in-app** on artifacts pulled through the real path: fake-entity → refusal; "5+5=9" → pushback; correction → concession; medical-boundary → educate/decline/redirect. If any fail, walk the §6 debug ladder (template → `<think>`-strip → LoRA-on-quant, isolate on f16). Slice done when all four pass.
- [ ] **Then:** 1B + 8B tiers are replication (re-run A2/A3 per base; add catalog entries) — a follow-up, not this plan.

---

## Self-Review

**Spec coverage:** bucket split (Track A A5 / constraints), signed catalog + ed25519 (B1/A4), downgrade guard (B1/B2, catalog_version), retire mint_download_url (B3), base+adapter separate `--lora` (B4), ChatML + leading-only `<think>`-strip (B4), pipeline stages incl. consumer self-check (A2–A6), curator-key handling + compile-time test-key guard (constraints, B1, A4), four Stage-5 probes + debug ladder (acceptance), handoff dependency (gate). All spec sections map to a task.

**Placeholder scan:** the environment-dependent steps (read the actual PEFT manifest format; confirm ChatML in the GGUF; pin the llama.cpp revision) are real first actions, not TBDs — each names exactly what to inspect. No "add error handling"-style hand-waves.

**Type consistency:** `DistCatalog`/`Artifact` field names are shared across B1/B2/B3 and the Python `build_catalog.py`; `catalog_version`/`catalog.json.sig` naming is consistent between Track A (produce) and Track B (consume); `verify_detached`'s byte-exact contract is referenced identically in B1 and A4.
