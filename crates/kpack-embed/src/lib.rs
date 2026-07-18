//! `kpack-embed` — the concrete, linked `Embedder` backend for the
//! knowledge-pack core.
//!
//! Isolated from `kpack-core` on purpose: this crate carries the heavy native
//! dependency (llama.cpp via `llama-cpp-2`, cmake/cc) and loads the bundled
//! `bge-base-en-v1.5` Q8_0 GGUF in embedding mode. Embedding runs in-process
//! with no socket — the only integration that works on both desktop and the
//! spec's mobile targets (iOS forbids the subprocess sidecar model).
//!
//! Scaffold only at K0 — the real `llama-cpp-2`-backed `Embedder` lands in K4.
