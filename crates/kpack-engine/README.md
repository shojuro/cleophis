# kpack-engine

The in-process inference engine for Cleophis mobile, and the `EngineBackend`
trait that is the **demilitarized zone** between the product and the inference
runtime (P0 spike brief).

Desktop runs llama.cpp as a **sidecar process** (`src-tauri/src/inference.rs`).
Mobile cannot (iOS forbids spawning subprocesses; Android makes them
kill-prone), so it links llama.cpp **in-process** via `llama-cpp-2` over FFI.
Both sit behind `EngineBackend`. Desktop inherits the trait after the mobile
demo ships, so nothing in the trait assumes mobile.

## Lifecycle (spec §1, verbatim)

```
load(base, adapters[]) → session(ctx) → stream(tokens) → unload()
```

- `EngineBackend::load` — validates + integrity-gates the base and the
  composition-ordered adapter stack (`adapter::prepare_stack`), returns a live
  `EngineHandle` owning model + adapters.
- `EngineHandle::session` — opens an `EngineSession` bound to the handle with a
  context config (n_ctx, sampling). The session borrows the handle, modelling
  "the native context borrows the model" without a self-referential struct.
- `EngineSession::stream` — renders a chat through the per-model template and
  streams visible token text into a `TokenSink` until EOS / token-limit / cancel.
- `EngineHandle::unload` — releases everything (also happens on drop).

## What is reproduced from `inference.rs` (without touching it)

The pure-Rust policy lives in `adapter.rs` and is proven against the `mock`
backend with **no native toolchain**:

- **Composition order** behavioral→contract→voice. The `AdapterRole`
  discriminant *is* the order; `prepare_stack` sorts by it, reproducing
  `LaunchPaths::loras()` (with voice appended, the layer desktop doesn't carry).
- **Fail-closed resolution**. A declared-but-absent artifact collapses the whole
  stack (`EngineError::Missing`), mirroring `resolve_launch_paths`. Never load
  base-only, never silently reduce the stack — the §0 fallback ladder is an
  explicit, escalated call (`PreparedStack::without_voice`, which can only drop
  voice, never contract).
- **Per-artifact verify-once sha256 gate** (`VerifyCache`). Hash each artifact
  against its catalog-pinned hash before a byte is parsed; fail closed on
  mismatch; skip when the catalog pins none; cache success for the process
  (mirrors `verify_model_once`/`verify_adapter_once`/`verify_contract_adapter_once`).

The native backend (`llama.rs`) routes `load` through the same `prepare_stack`,
so it never re-implements policy — it loads exactly what it is handed.

## Chat templates (spec §8)

`template.rs` resolves the chat template **per-model** — the floor hero
(Llama-3.2-1B) is *not* ChatML; Qwen tiers are. `ChatTemplate::Auto` reads the
template embedded in the GGUF (the in-process equivalent of the sidecar's
`--jinja`). The Qwen-only `<think>` strip is **start-of-turn only, never
global**, implemented as a streaming state machine (`ThinkStripper`).

## Prompt contract

Prompt rendering honors `contracts/prompt-contract.v1.toml`. This crate stays
lean (no `kpack-core` dependency, so the native/NDK build is about llama.cpp,
not SQLite): the **caller** supplies the contract system prompt and rendered
sources — from `kpack_core::contract::system_contract()` / `render_sources()` —
as `ChatMessage`s. The contract is honored byte-for-byte (the engine never
re-types it); the DMZ trait avoids depending on the whole core.

## Building

**Standalone crate during the P0 spike.** `Cargo.toml` carries an empty
`[workspace]` table so the crate is detached from the root Cleophis workspace —
the spike never edits root `Cargo.toml` `members` (a founder-gated shared change
per the brief). Fold into the workspace at gate time by deleting that table and
adding the crate to root `members`.

### Use a WSL-native target dir (drvfs is slow)

The repo lives on a Windows-mounted drive (`/mnt/c`, drvfs), which is 5–10× slower
for Rust builds and where the 10–20 GB of NDK build artifacts should NOT land.
Point `CARGO_TARGET_DIR` at a WSL-native path for all cargo work:

```bash
export CARGO_TARGET_DIR=/home/$USER/cleophis-mobile-target
cargo test                       # default = mock only, toolchain-free
```

(Not committed as `.cargo/config.toml` because the path is user-specific and a
root `.cargo/config.toml` would be a shared change affecting desktop.)

### Features

| Feature | What it pulls in | Toolchain needed |
|---|---|---|
| *(default)* | mock backend + all policy | none — `cargo test` stays green |
| `real` | `llama-cpp-2 =0.1.151` (llama.cpp) | **libclang** (bindgen) + **cmake** |

```bash
cargo build --features real      # links llama.cpp; needs libclang + cmake
```

`=0.1.151` is the SAME pin `kpack-embed` uses for the BGE embedder. Hour-one
check confirmed it exposes generation, multi-LoRA, and chat-template APIs — no
pin move, so one workspace / one version / one native backend.

### Android cross-compile (P0 gate)

The gate is 16 KB-page-aligned aarch64 native libs (spec H2). With a Linux NDK
installed and `cargo-ndk`:

```bash
export ANDROID_NDK_HOME=/path/to/android-ndk-r27c   # r27+ = 16 KB align default
cargo ndk -t arm64-v8a build --features real
```

See the spike report for the current state of the NDK toolchain in this
environment (libclang / JDK / NDK install status).
