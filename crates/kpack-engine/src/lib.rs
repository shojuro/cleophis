//! `kpack-engine` — the in-process inference engine for Cleophis mobile, and
//! the `EngineBackend` trait that is the demilitarized zone between the product
//! and the runtime (P0 spike brief §"the trait is the demilitarized zone").
//!
//! Desktop runs llama.cpp as a **sidecar process** (`src-tauri/src/inference.rs`).
//! Mobile cannot (iOS forbids spawning subprocesses; Android makes them
//! kill-prone), so it links llama.cpp **in-process** via `llama-cpp-2` over FFI.
//! Both live behind [`EngineBackend`]; desktop inherits the trait after the
//! mobile demo ships, so nothing here assumes mobile.
//!
//! ## What this crate reproduces from `inference.rs` (without touching it)
//! The pure-Rust policy — composition order behavioral→contract→voice,
//! fail-closed artifact resolution, and the per-artifact verify-once sha256
//! gate — lives in [`adapter`] and is proven against the [`mock`] backend with
//! no native toolchain. The native backend ([`llama`], behind the `real`
//! feature) routes `load` through the same [`adapter::prepare_stack`], so it
//! never re-implements policy — it loads exactly what it is handed.
//!
//! ## Layers
//! - [`backend`] — the `EngineBackend` / `EngineHandle` / `EngineSession`
//!   traits and their request/config/stats types.
//! - [`adapter`] — roles, specs, `prepare_stack`, the `VerifyCache` gate.
//! - [`template`] — per-model chat template selection and the Qwen-only,
//!   start-of-turn `<think>` stripper.
//! - [`mock`] — a deterministic backend for tests and pre-native bring-up.
//! - `prefix` — how much of a prompt the KV cache already holds (task 1.5),
//!   kept out of the feature gate so it is tested rather than only checked.
//! - [`llama`] — the real `llama-cpp-2`-backed backend (`--features real`).
//!
//! ## Prompt contract
//! Prompt rendering honors `contracts/prompt-contract.v1.toml`. This crate
//! stays lean (no `kpack-core` dependency, so the native/NDK build is about
//! llama.cpp, not SQLite): the **caller** supplies the contract system prompt
//! and rendered sources — produced by `kpack_core::contract::system_contract()`
//! / `render_sources()` — as [`template::ChatMessage`]s. The contract is thus
//! honored byte-for-byte (the engine never re-types it), and the DMZ trait
//! avoids depending on the whole core. See the crate README for the wiring.

pub mod adapter;
pub mod backend;
/// Runtime CPU capability (`AT_HWCAP`) and its differential against the
/// compile-time macros llama.cpp reports. Deliberately **outside** the `real`
/// gate: the adjudication is pure and must be tested by the desktop suite, and
/// the capability read is what a caller needs *before* deciding to load a
/// native backend at all.
pub mod cpu;
pub mod error;
pub mod mock;
pub mod template;

/// The prefix-reuse arithmetic (task 1.5). Deliberately *outside* the `real`
/// gate so its tests run in a build with no llama.cpp — the module docs explain
/// why that one function is worth the split. Its only production caller is the
/// `real` backend, which is what the attribute states.
#[cfg_attr(not(feature = "real"), allow(dead_code))]
mod prefix;

#[cfg(feature = "real")]
pub mod llama;

pub use adapter::{
    file_sha256, prepare_stack, AdapterRole, AdapterSpec, ModelSpec, PreparedStack, VerifyCache,
};
pub use backend::{
    EngineBackend, EngineHandle, EngineSession, GenStats, LoadParams, LoadRequest, Sampling,
    SessionConfig, StopReason, TokenSink,
};
pub use error::EngineError;
pub use mock::{CollectSink, MockBackend};
pub use template::{ChatMessage, ChatTemplate, Role, ThinkStripper};

#[cfg(feature = "real")]
pub use llama::{backend_system_info, print_kernel_report, LlamaEngine};
