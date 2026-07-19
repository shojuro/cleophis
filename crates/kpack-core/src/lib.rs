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
//! landing K4b). K5 adds `tree` (the parsed-document intermediate form K6's
//! parsers will produce) and `chunk` (the structure-aware sliding-window
//! chunker, spec §1.3). K6 adds `parse` (Markdown + plain-text parsers
//! producing `tree::Document`, spec §3.2). K7 adds `contract` (the versioned
//! prompt contract shared with the adapter-training track, and the
//! single-pass citation renderer, spec §4.2, plan D5). K8 adds `build` — the
//! capstone that wires every prior module into one real `.kpack` end to end
//! (`parse` → `chunk` → `embed` → `format` → `manifest`), proven against
//! `embed::MockEmbedder` (see that module's own determinism caveat: K8's
//! mock-based tests prove the build CODE is deterministic, not spec §5's
//! real cross-build claim, which needs K4b's real tokenizer). R1 (spec §4.1)
//! adds `retrieve` — per-pack dense+lexical retrieval fused with reciprocal
//! rank fusion (RRF), each fused candidate annotated with a dense cosine
//! similarity for R2's per-pack gate; `format::Pack::get_embedding` is the
//! small accessor R1 adds to `format` so that gate has a chunk's stored
//! vector to score against.

pub mod build;
pub mod chunk;
pub mod contract;
pub mod embed;
pub mod format;
pub mod manifest;
pub mod parse;
pub mod retrieve;
pub mod sign;
pub mod tree;

pub use build::{build_pack, passage_input, sha256_hex, BuildMeta, Error as BuildError, SourceInput};
pub use chunk::{chunk_document, ChunkConfig, ChunkDraft};
pub use contract::{
    contract_version, no_evidence_marker, refusal_with_offer, render_sources, system_contract,
    PromptContract, RenderChunk,
};
pub use embed::{dot_int8, l2_normalize, query_input, quantize_int8, EmbedError, Embedder, BGE_QUERY_INSTRUCTION};
#[cfg(any(test, feature = "test-util"))]
pub use embed::MockEmbedder;
pub use format::{Chunk, Doc, Error, Pack, SCHEMA_VERSION};
pub use manifest::{check_load, LoadContext, Manifest, PackTier, VEC_FORMAT_VERSION};
pub use parse::{extraction_quality, parse, parse_markdown, parse_txt};
pub use retrieve::{
    retrieve_pack, rrf, safe_fts5_query, Candidate, Error as RetrieveError, DEFAULT_K_RRF,
};
pub use sign::{curator_verifying_key, verify_detached, CURATOR_PUBLIC_KEY};
pub use tree::{Block, Document, Section};
