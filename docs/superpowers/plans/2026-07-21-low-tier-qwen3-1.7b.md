# Low tier → Qwen3-1.7B Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the low tier's base+adapter (`Llama-3.2-1B` → `Qwen3-1.7B`, non-thinking) so the weak-device tier is meaningfully better at math and the lineup unifies on Qwen3/Apache-2.0.

**Architecture:** A data + artifacts change, not a code change — the app's tier-selection already generalizes over `base_model` (`catalog::hero_variant`). Supplier→producer→consumer: re-train `behavioral-v1` on Qwen3-1.7B (GPU) → run the existing pipeline to publish signed catalog **v4** → swap the low-tier catalog entry in the app.

**Tech Stack:** Unsloth QLoRA (Colab/RunPod GPU) for training; the WSL pipeline (`tools/pipeline/`, pinned llama.cpp `b10042`, boto3→B2) for GGUF build/sign/publish; Tauri 2 + Rust app consumer.

## Global Constraints

Exact values — every task inherits these:

- **Base HF repo:** `Qwen/Qwen3-1.7B` (Apache-2.0, ungated — anon pull OK). Adapter trained on the unsloth 4-bit variant (`unsloth/Qwen3-1.7B` bnb-4bit), same as the 4B/8B lineage.
- **Artifact basenames (case-sensitive):** base `Qwen3-1.7B-Instruct-Q4_K_M.gguf`; adapter `behavioral-v1-Qwen3-1.7B.gguf`. Quant **Q4_K_M**.
- **`base_model` catalog field:** exactly `"Qwen3-1.7B"`. **`adapterId`:** `behavioral-v1-qwen3-1.7b`. **`sizeParams`:** `"1.7B"`. **License:** `Apache-2.0`.
- **llama.cpp pin:** `b10042` (already set up by `setup_tools.sh` — do not bump).
- **Catalog immutability:** v4 ADDS the 2 Qwen3-1.7B artifacts; NEVER mutate/overwrite an existing artifact path. `Llama-3.2-1B` stays in `KNOWN_BASE_MODELS` and in the catalog (its v3 artifacts remain published, just unreferenced by the app). `catalog_version` monotonic 3→4.
- **Curator private key:** read only by `sign_catalog.py` from `CURATOR_KEY_FILE` (`~/.cleophis/curator_ed25519.key`, ext4 0600) — never in `.env`, never printed.
- **B2 keys:** `tools/pipeline/.env` (gitignored). Primary pair = `cleophis-dist`; `B2_MODELS_*` = `cleophis-models`.
- **Runtime style:** ChatML template; behavioral adapter emits the leading empty `<think></think>` the app strips; **non-thinking** (no CoT) — do not enable Qwen3 thinking mode.
- **Bucket paths:** PEFT+dataset+manifests in private `cleophis-models`; GGUFs+catalog in public `cleophis-dist`. New adapter PEFT → `cleophis-models/adapters/behavioral/v1/Qwen3-1.7B-unsloth-bnb-4bit/`.

---

## Task 1: Recover the exact behavioral-v1 training recipe

The original training Colab is gone; the recipe survives in the published adapters' manifests in `cleophis-models`. Pull one to mirror its hyperparameters exactly (so the Qwen3-1.7B adapter is the same recipe, just a new base).

**Files:**
- Read-only: `cleophis-models` object(s) under `adapters/behavioral/v1/Qwen3-4B-unsloth-bnb-4bit/` (manifest) and `datasets/behavioral/v1/`.

**Interfaces:**
- Produces: `RECIPE` — the exact training config (base 4-bit repo, LoRA rank/alpha/dropout, target modules, lr, epochs/steps, batch/grad-accum, max_seq_len, seed, dataset path + format) recorded for Task 2.

- [ ] **Step 1: List the surviving behavioral-v1 artifacts + manifests in cleophis-models**

Run (WSL, uses the pipeline `.env` models key):
```bash
cd "tools/pipeline"
.venv/bin/python3 - <<'PY'
import os, boto3
from botocore.config import Config
def env(p):
    for l in open('.env'):
        l=l.strip()
        if l.startswith(p+'='): return l.split('=',1)[1].strip().strip('"').strip("'")
s3=boto3.client('s3',endpoint_url=env('B2_ENDPOINT'),
  aws_access_key_id=env('B2_MODELS_KEY_ID'),aws_secret_access_key=env('B2_MODELS_APP_KEY'),
  config=Config(signature_version='s3v4'))
for k in s3.list_objects_v2(Bucket='cleophis-models',Prefix='adapters/behavioral/v1/').get('Contents',[]):
    print(k['Size'], k['Key'])
print('--- datasets ---')
for k in s3.list_objects_v2(Bucket='cleophis-models',Prefix='datasets/behavioral/v1/').get('Contents',[]):
    print(k['Size'], k['Key'])
PY
```
Expected: lists the Qwen3-4B / Llama-3.2-1B / Qwen3-8B PEFT dirs (adapter_config.json, adapter_model.safetensors, a manifest json) and the `datasets/behavioral/v1/` files.

- [ ] **Step 2: Download the Qwen3-4B adapter manifest + `adapter_config.json` and record the recipe**

Run (fill the exact keys from Step 1's output):
```bash
cd "tools/pipeline"
mkdir -p work/recipe
.venv/bin/python3 - <<'PY'
import boto3
from botocore.config import Config
def env(p):
    for l in open('.env'):
        l=l.strip()
        if l.startswith(p+'='): return l.split('=',1)[1].strip().strip('"').strip("'")
s3=boto3.client('s3',endpoint_url=env('B2_ENDPOINT'),
  aws_access_key_id=env('B2_MODELS_KEY_ID'),aws_secret_access_key=env('B2_MODELS_APP_KEY'),
  config=Config(signature_version='s3v4'))
for key in [
  'adapters/behavioral/v1/Qwen3-4B-unsloth-bnb-4bit/adapter_config.json',
  # add the manifest json key from Step 1 if named differently:
]:
    dst='work/recipe/'+key.split('/')[-1]
    s3.download_file('cleophis-models',key,dst); print('got',dst)
PY
cat work/recipe/adapter_config.json
```
Expected: `adapter_config.json` shows `r`, `lora_alpha`, `lora_dropout`, `target_modules`, `base_model_name_or_path`. Record these + any manifest hyperparameters (lr, steps, batch, seq len, seed) as `RECIPE`. **These exact values are Task 2's inputs.**

- [ ] **Step 3: Confirm unsloth has the Qwen3-1.7B 4-bit variant**

Use the Hugging Face MCP (`hub_repo_details` / `hf_hub_query`) or the browser to confirm an ungated `unsloth/Qwen3-1.7B` (or `unsloth/Qwen3-1.7B-unsloth-bnb-4bit`) exists. Record the exact repo id for Task 2. Expected: an Apache-2.0, non-gated repo resolves.

- [ ] **Step 4: Commit the recipe note**

```bash
cd "/mnt/c/Users/JM505 Computers/dev/cleophis"
mkdir -p docs/superpowers/recipes
# Write docs/superpowers/recipes/behavioral-v1-recipe.md with the recorded RECIPE values.
git add docs/superpowers/recipes/behavioral-v1-recipe.md
git commit -m "docs(low-tier): record behavioral-v1 training recipe from Qwen3-4B manifest"
```

---

## Task 2: Train `behavioral-v1` on Qwen3-1.7B (GPU) + gate + upload

Re-run the recovered recipe on the new base. Not TDD — an ML job — but every step has a concrete check. Runs on a GPU env (RunPod via the runpod MCP, or Colab via colab-mcp — a ~1.7B QLoRA fits a single mid GPU; expect well under the prior $0.72×-ish cost).

**Files:**
- Create (in the GPU env): a training notebook/script mirroring `RECIPE`.
- Write to: `cleophis-models/adapters/behavioral/v1/Qwen3-1.7B-unsloth-bnb-4bit/` (PEFT dir + manifest).

**Interfaces:**
- Consumes: `RECIPE` (Task 1), `datasets/behavioral/v1/` (cleophis-models).
- Produces: a PEFT adapter (`adapter_config.json` + `adapter_model.safetensors`) + a manifest (`base_model`, `contract_version: prompt-contract-v0`, `license: Apache-2.0`, recipe echo) in cleophis-models — Task 3's `build_adapter.py` input.

- [ ] **Step 1: Provision a GPU env + install unsloth**

RunPod (preferred, non-interactive): via the runpod MCP create a pod with a CUDA PyTorch template (e.g. 1×A40/4090), then in it `pip install unsloth`. Colab alt: follow `memory/colab-mcp-operations` (first-call rule: the first tool call's timeout poisons the instance — keep it short). Expected: `import unsloth` succeeds; `nvidia-smi` shows the GPU.

- [ ] **Step 2: Load base + dataset**

In the env:
```python
from unsloth import FastLanguageModel
model, tok = FastLanguageModel.from_pretrained("unsloth/Qwen3-1.7B", load_in_4bit=True, max_seq_length=RECIPE.max_seq_len)
model = FastLanguageModel.get_peft_model(model, r=RECIPE.r, lora_alpha=RECIPE.lora_alpha,
    lora_dropout=RECIPE.lora_dropout, target_modules=RECIPE.target_modules, random_state=RECIPE.seed)
# download datasets/behavioral/v1/ from cleophis-models (boto3, same as Task 1) and load it
```
Expected: model loads in 4-bit; the dataset rows load with the same ChatML format the 4B/8B used.

- [ ] **Step 3: Train with the recovered hyperparameters**

Use the exact `RECIPE` lr / steps (or epochs) / batch / grad-accum / seed. Expected: loss decreases and the run completes without OOM (1.7B 4-bit QLoRA is light).

- [ ] **Step 4: Gate — 4 Stage-5 probes + a math spot-check (IN the training env)**

Run the merged/adapter-applied model against:
1. fake-entity → refusal, 2. "5+5=9" → pushback, 3. correction → concession, 4. medical diagnosis request → educate/decline/redirect.
Plus a **math spot-check**: a handful of grade-school/algebra problems, comparing to Llama-3.2-1B's answers on the same. Expected: all four probes pass AND math is visibly better than Llama-1B. If a probe fails, apply the debug ladder from `memory/cleophis-adapter-distribution` (ChatML mismatch → `<think>`-strip → LoRA-on-quantized-base) — do NOT retrain blindly.

- [ ] **Step 5: Save PEFT + manifest and upload to cleophis-models**

```python
model.save_pretrained("behavioral-v1-Qwen3-1.7B")  # adapter_config.json + adapter_model.safetensors
# write manifest.json: {"base_model":"Qwen3-1.7B","kind":"adapter","contract_version":"prompt-contract-v0","license":"Apache-2.0", ...recipe echo...}
# boto3 upload the dir to cleophis-models/adapters/behavioral/v1/Qwen3-1.7B-unsloth-bnb-4bit/
```
Expected: `list_objects_v2` shows the new PEFT dir + manifest in cleophis-models. **Tear down the GPU pod** (runpod MCP `stop-pod`/`delete-pod`) to stop billing.

---

## Task 3: Pipeline — KNOWN_BASE_MODELS, build, catalog v4, publish, verify

**Files:**
- Modify: `tools/pipeline/build_catalog.py` (`KNOWN_BASE_MODELS`), `tools/pipeline/verify_published.py` (`KNOWN_BASE_MODELS`)
- Run (no code change): `build_base.py`, `build_adapter.py`, `sign_catalog.py`, `publish.py`, `verify_published.py`

**Interfaces:**
- Consumes: the PEFT adapter in cleophis-models (Task 2); the pinned llama.cpp toolchain.
- Produces: signed catalog **v4** in cleophis-dist (8 artifacts) + the 2 Qwen3-1.7B GGUFs; their sha256/size — Task 4's catalog inputs.

- [ ] **Step 1: Write the failing check — Qwen3-1.7B not yet known**

Run:
```bash
cd "/mnt/c/Users/JM505 Computers/dev/cleophis"
grep -n "KNOWN_BASE_MODELS" tools/pipeline/build_catalog.py tools/pipeline/verify_published.py
```
Expected: the frozenset currently = `{"Qwen3-4B","Llama-3.2-1B","Qwen3-8B"}` — no `Qwen3-1.7B`.

- [ ] **Step 2: Add `"Qwen3-1.7B"` to `KNOWN_BASE_MODELS` in both files**

Edit the frozenset in `build_catalog.py` and `verify_published.py` to:
```python
KNOWN_BASE_MODELS = frozenset({"Qwen3-4B", "Llama-3.2-1B", "Qwen3-8B", "Qwen3-1.7B"})
```

- [ ] **Step 3: Verify the self-tests still pass**

Run:
```bash
cd "tools/pipeline"
.venv/bin/python3 build_catalog.py --self-test && .venv/bin/python3 verify_published.py --self-test
```
Expected: both self-tests PASS (they exercise `expected_basename` + the KNOWN check; the invalid-base negative case still uses a not-a-tier string).

- [ ] **Step 4: Commit the pipeline generalization**

```bash
git add tools/pipeline/build_catalog.py tools/pipeline/verify_published.py
git commit -m "feat(pipeline): add Qwen3-1.7B to KNOWN_BASE_MODELS"
```

- [ ] **Step 5: Build the base GGUF**

```bash
cd "tools/pipeline"
.venv/bin/python3 build_base.py --base-model Qwen3-1.7B --hf-repo Qwen/Qwen3-1.7B
```
Expected: `work/.../Qwen3-1.7B-Instruct-Q4_K_M.gguf` (~1.1 GB) + its sha256 logged. (Match the flag names build_base.py actually exposes — see its `--help`.)

- [ ] **Step 6: Build the adapter GGUF**

```bash
.venv/bin/python3 build_adapter.py --base-model Qwen3-1.7B --license Apache-2.0
```
Expected: pulls the PEFT from cleophis-models, `convert_lora_to_gguf` → `behavioral-v1-Qwen3-1.7B.gguf` + sha256; the manifest carries `license: Apache-2.0` (the A2 fix — no manual edit).

- [ ] **Step 7: Build catalog v4 (adds the 2 new artifacts to the live v3 set)**

```bash
.venv/bin/python3 build_catalog.py --base-url https://cleophis-dist.s3.us-east-005.backblazeb2.com
```
Expected: reads live v3, increments to `catalog_version: 4`, emits 8 artifacts (the 6 existing UNCHANGED + Qwen3-1.7B base + adapter). Confirm the 6 existing entries are byte-identical to v3.

- [ ] **Step 8: Sign + publish + verify**

```bash
.venv/bin/python3 sign_catalog.py   # reads CURATOR_KEY_FILE, writes catalog.json.sig (64 raw bytes)
.venv/bin/python3 publish.py        # uploads the 2 new artifacts, archives v3, publishes v4
.venv/bin/python3 verify_published.py
```
Expected: `verify_published` → "OK: catalog v4, 8 artifacts verified" (sig vs pinned key + every sha256 from the PUBLIC path). Record the Qwen3-1.7B base+adapter **sha256 + size** for Task 4.

---

## Task 4: App — swap the low-tier catalog entry

**Files:**
- Modify: `src-tauri/resources/catalog.json` (`socratic-tutor.tiers.low`)
- Modify: `src-tauri/src/catalog.rs` (low-tier test assertion, ~line 130)
- Modify: `src/app.js` (`TIER_OPTS` low label)

**Interfaces:**
- Consumes: the Qwen3-1.7B base+adapter sha256/size from Task 3 Step 8.
- Produces: an app whose low tier resolves+downloads+launches Qwen3-1.7B.

- [ ] **Step 1: Update the failing test first (catalog.rs low assertion)**

In `src-tauri/src/catalog.rs`, the test `hero_carries_all_three_tier_variants_with_pinned_hashes` asserts `(&tiers.low, "Llama-3.2-1B")`. Change it to `(&tiers.low, "Qwen3-1.7B")`.

- [ ] **Step 2: Run it to verify it fails (catalog.json still Llama)**

```bash
"/mnt/c/Program Files/PowerShell/7-preview/pwsh.exe" -NoProfile -Command "Set-Location 'C:\Users\JM505 Computers\dev\cleophis'; & 'C:\Users\JM505 Computers\.cargo\bin\cargo.exe' test -p cleophis hero_carries"
```
Expected: FAIL — `tiers.low.base_model` is still `"Llama-3.2-1B"`.

- [ ] **Step 3: Swap the `tiers.low` block in catalog.json**

In `src-tauri/resources/catalog.json`, replace the `socratic-tutor.tiers.low` object with Qwen3-1.7B (hashes/size from Task 3 Step 8):
```json
"low": {
  "baseModel": "Qwen3-1.7B",
  "sizeParams": "1.7B",
  "modelFile": "models/Qwen3-1.7B-Instruct-Q4_K_M.gguf",
  "sha256": "<qwen3-1.7b base sha256 from v4>",
  "adapterFile": "models/behavioral-v1-Qwen3-1.7B.gguf",
  "adapterSha256": "<adapter sha256 from v4>",
  "adapterId": "behavioral-v1-qwen3-1.7b",
  "fileBytes": <base size bytes from v4>
}
```

- [ ] **Step 4: Run the test to verify it passes**

```bash
"/mnt/c/Program Files/PowerShell/7-preview/pwsh.exe" -NoProfile -Command "Set-Location 'C:\Users\JM505 Computers\dev\cleophis'; & 'C:\Users\JM505 Computers\.cargo\bin\cargo.exe' test -p cleophis catalog"
```
Expected: PASS — all catalog tests green (low variant now Qwen3-1.7B @ 64-hex).

- [ ] **Step 5: Update the FE low label**

In `src/app.js`, `TIER_OPTS`, change the `low` entry:
```js
{ mode: 'low', label: 'Small · 1.7B', sub: '≈1.1 GB · fastest' },
```
Then `node --check src/app.js` → syntax OK.

- [ ] **Step 6: Full suite + commit**

```bash
"/mnt/c/Program Files/PowerShell/7-preview/pwsh.exe" -NoProfile -Command "Set-Location 'C:\Users\JM505 Computers\dev\cleophis'; & 'C:\Users\JM505 Computers\.cargo\bin\cargo.exe' test -p cleophis"
git add src-tauri/resources/catalog.json src-tauri/src/catalog.rs src/app.js
git commit -m "feat(low-tier): point low tier at Qwen3-1.7B (catalog v4)"
```
Expected: 245 tests green.

---

## Task 5: Acceptance (MSI E2E) + docs + PR

**Files:**
- Create: `docs/superpowers/verification-milestone-low-tier-qwen3.md`

- [ ] **Step 1: Rebuild the MSI**

```bash
"/mnt/c/Program Files/PowerShell/7-preview/pwsh.exe" -NoProfile -Command "$env:Path += ';C:\Users\JM505 Computers\.cargo\bin'; Set-Location 'C:\Users\JM505 Computers\dev\cleophis'; & 'C:\Program Files\nodejs\npm.cmd' run tauri build"
```
Expected: `target\release\bundle\msi\Cleophis_0.1.0_x64_en-US.msi` rebuilt.

- [ ] **Step 2: User E2E on the mid box (override → Small tier)**

Install; open the Socratic Tutor; override to **Small · 1.7B** → downloads the Qwen3-1.7B pair, deletes the prior low pair, relaunches. Confirm: the **4 Stage-5 probes pass** and a **math spot-check is visibly better** than the old Llama-1B.

- [ ] **Step 3: Write the verification doc + open the PR**

Record automated results (self-tests, `verify_published` v4, `cargo test` 245) + the user E2E. Then:
```bash
git add docs/superpowers/verification-milestone-low-tier-qwen3.md
git commit -m "docs(low-tier): verification milestone"
git push -u origin feat/low-tier-qwen3-1.7b
gh pr create --base main --title "feat: low tier -> Qwen3-1.7B (better math, one Qwen family)" --body "..."
```

---

## Self-review

- **Spec coverage:** S1→Task 1-2, S2→Task 3, S3→Task 4, S4→Task 5. All four spec tracks covered.
- **Immutability:** Task 3 Step 7 explicitly keeps the 6 existing entries byte-identical and only adds 2 (v4). `Llama-3.2-1B` stays in `KNOWN_BASE_MODELS` (Task 3 Step 2).
- **Types/names consistent:** `Qwen3-1.7B` / `behavioral-v1-Qwen3-1.7B.gguf` / `behavioral-v1-qwen3-1.7b` / `Qwen3-1.7B-Instruct-Q4_K_M.gguf` used verbatim across Tasks 2-4.
- **Open dependency (not a placeholder):** the sha256/size in Task 4 Step 3 are genuine outputs of Task 3 Step 8 — filled from the verified catalog v4, by design.
- **Recipe dependency:** Task 1 recovers the exact hyperparameters from the surviving manifest rather than guessing — the one real unknown, handled first.
