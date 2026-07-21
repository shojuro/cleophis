# Session Handoff — Execute the Adapter / Distribution-Origin Milestone (subagent-driven)

Paste-in briefing for a **fresh Claude Code session** that will execute the adapter/distribution
milestone with `superpowers:subagent-driven-development` (the prior session hit its 200-subagent
limit; a fresh session resets it and restores the independent per-task review + `/code-review ultra`).

## Launch context
- **Launch from the repo dir** so git + `/code-review ultra` work: working dir must be
  `/mnt/c/Users/JM505 Computers/dev/cleophis` (note the space in `JM505 Computers`).
- You should already be on branch **`feat/adapter-distribution`**.

## Read these first (in order)
1. **Plan** — `docs/superpowers/plans/2026-07-21-adapter-distribution.md` (the two-track task list you execute).
2. **Spec** — `docs/superpowers/specs/2026-07-21-adapter-distribution-design.md` (design + the 5 review refinements).
3. **Memory** — recall `cleophis-adapter-distribution.md` (every architecture decision, condensed) via MEMORY.md.
4. **Ledger** — `.superpowers/sdd/progress.md` (full milestone history; gitignored, on disk).

## Where we are
- **offline-auth shipped** (PR #20 merged to `main`) — do NOT touch it.
- The adapter milestone's **spec + plan are done and approved**; you are executing the plan.
- Milestone = one thin slice: **Qwen3-4B hero end-to-end** — base + behavioral adapter pulled through
  a **signed catalog** from our B2 **`cleophis-dist`** (public) bucket, loaded via llama.cpp `--lora`.
- **Acceptance = the four Stage-5 probes in-app**: fake-entity → refusal, "5+5=9" → pushback,
  correction → concession, medical-boundary → educate/decline/redirect. (v1 is `prompt-contract-v0` —
  it does NOT promise the `[1](source,locator)` citation format; that's adapter v2.)

## How to execute
- Use **`superpowers:subagent-driven-development`** on the plan. Two tracks:
  - **Wrapper** (Rust/JS, tasks B1–B4): signed-catalog client (ed25519 verify + monotonic downgrade
    guard), public-URL artifact download (reuse `download.rs`, retire `mint_download_url` for catalog
    artifacts only), engine `--lora` + ChatML + **leading-only** `<think>`-strip, hero repoint + `adapter_ids`.
  - **Pipeline** (Python in `tools/pipeline/`, tasks A1–A6): scaffold/tooling → base→Q4_K_M GGUF →
    PEFT→adapter.gguf → assemble+sign catalog → publish → **consumer self-check from the public path**.
- **Build all the code first** — it's unit-testable and needs no credentials.
- Then the **handoff gate**: the user creates the `cleophis-dist` bucket, a scoped B2 keypair
  (READ `cleophis-models`, WRITE `cleophis-dist`), an HF token (Meta-license-accepted), and places a
  curator-key file OUTSIDE the repo. **Only then** can the pipeline run live + the probes validate.
  Ping the user for that handoff when the code is ready to pull-convert-upload-fetch.

## Environment / build
- Windows toolchain via WSL. Rust build/test from bash:
  `PWSH='/mnt/c/Program Files/PowerShell/7-preview/pwsh.exe'; "$PWSH" -Command "cd 'C:\Users\JM505 Computers\dev\cleophis'; $env:LIBCLANG_PATH='C:\Program Files\LLVM\bin'; $env:PATH='C:\Program Files\CMake\bin;'+$env:PATH; & 'C:\Users\JM505 Computers\.cargo\bin\cargo.exe' test -p cleophis <filter>"`
- FE: `node -c src/app.js`. Commit trailer: `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`;
  each task `git add`s only its own files, never `-A`.
- For the MSI, spawn a **fresh** build agent (the prior session's `msi-build` won't exist).

## Key guardrails (in the spec — do not miss)
- Reuse `kpack_core::sign::{curator_verifying_key, verify_detached}` for the catalog signature — no new crypto path.
- Monotonic `catalog_version` **downgrade guard**: app persists the highest ever verified, refuses older.
- Retire `mint_download_url` for **public catalog artifacts only** (leave user-specific content alone).
- **Release builds must be compile-time incapable of trusting a test keypair** — ship pins the production curator key only.
- Base + adapter loaded **separately** via `--lora` — never merged.
- `<think>`-strip is **leading-only at turn start**, never global.
- Curator PRIVATE key read from a file OUTSIDE the repo by the `sign_catalog` step only — never in `.env`, never committed.
- B2 bucket split: `cleophis-models` PRIVATE (PEFT/datasets/lineage), `cleophis-dist` PUBLIC (GGUFs, catalog+sig).

When the slice ships (all four probes pass in the app on artifacts pulled through the real path),
the next roadmap steps are the 1B + 8B tier replication, then §2 curated + calibration.
