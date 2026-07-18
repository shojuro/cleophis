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
//! K2 adds `manifest` (self-description + load-time integrity gate). The
//! rest of the module list above lands K3 onward.

pub mod format;
pub mod manifest;

pub use format::{Chunk, Doc, Error, Pack, SCHEMA_VERSION};
pub use manifest::{check_load, LoadContext, Manifest, PackTier, VEC_FORMAT_VERSION};
