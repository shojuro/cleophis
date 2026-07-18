//! `kpack-core` — the shared knowledge-pack format core (§1 of the on-device
//! RAG spec).
//!
//! This crate is deliberately **network-free**: it links no HTTP client, no
//! `tokio`, nothing that opens a socket. That is the compiler-enforced form of
//! the spec's hard rule — no network I/O occurs in the pack build or retrieval
//! path — upgrading the app's "cloud is the only network module" convention
//! into a dependency-graph guarantee. If a future change reaches for the
//! network here, it will not compile.
//!
//! It is also **Tauri-free** and reusable by both the on-device app and the
//! future company-server pipeline (the ~80% shared core, spec Scope). Modules
//! to come: `format` (the `.kpack` SQLite schema), `manifest` (self-description
//! + load-time integrity gate), `sign` (ed25519 curated-pack verification),
//! `tree`/`parse`/`chunk` (structure-aware chunking), `embed` (the `Embedder`
//! trait + int8 quantization math), `contract` (the versioned prompt contract
//! + citation renderer), and `build` (the end-to-end pack builder).
//!
//! K1 landed the first real module: `format` (the `.kpack` SQLite schema,
//! open/create, typed doc/chunk I/O, and the vec0 + fts5 dual-lane proof).
//! K2 added `manifest` (self-description + load-time integrity gate). K3
//! added `sign` (ed25519 curated-pack signature verification, wired into
//! `manifest::Pack::mount`). K4a adds `embed` (the `Embedder` trait, the
//! int8 quantization math, and a deterministic mock — pure Rust; the real
//! `llama.cpp`-backed implementation is a separate crate, `kpack-embed`,
//! landing K4b). The rest of the module list above lands K5 onward.

pub mod embed;
pub mod format;
pub mod manifest;
pub mod sign;

pub use embed::{dot_int8, l2_normalize, query_input, quantize_int8, EmbedError, Embedder, BGE_QUERY_INSTRUCTION};
#[cfg(any(test, feature = "test-util"))]
pub use embed::MockEmbedder;
pub use format::{Chunk, Doc, Error, Pack, SCHEMA_VERSION};
pub use manifest::{check_load, LoadContext, Manifest, PackTier, VEC_FORMAT_VERSION};
pub use sign::{curator_verifying_key, verify_detached, CURATOR_PUBLIC_KEY};
