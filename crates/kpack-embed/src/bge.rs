//! `BgeEmbedder` — the real, linked `Embedder` implementation, backed by
//! `llama.cpp` (via `llama-cpp-2`) running the bundled `bge-base-en-v1.5`
//! Q8_0 GGUF in embedding mode. Everything in this file is gated behind
//! the `real` Cargo feature (see `Cargo.toml`) so the default workspace
//! build needs no native toolchain (cmake/libclang) — only crates that
//! explicitly opt into `real` (ultimately `src-tauri`, at K9) pull in
//! llama.cpp.
//!
//! ## Pooling (the part that's easy to get silently wrong)
//! BGE v1.5 (`bge-base-en-v1.5`, `bge-small-en-v1.5`, …) is trained and
//! evaluated with **CLS pooling** — the embedding is the final hidden
//! state of the `[CLS]` token, NOT a mean over all tokens. `llama.cpp`'s
//! GGUF conversion for BERT-family models supports both `LLAMA_POOLING_TYPE_MEAN`
//! and `LLAMA_POOLING_TYPE_CLS`; getting this wrong doesn't error, it just
//! silently produces a different-but-plausible-looking vector — the exact
//! "bad-but-plausible" failure mode the K4b task brief warns would fail
//! K8's cross-build determinism test. [`LlamaPoolingType::Cls`] is set
//! explicitly in [`ctx_params`] rather than relying on any default.
//!
//! ## Why a fresh `LlamaContext` per call, not one reused context
//! `LlamaContext<'a>` borrows `&'a LlamaModel`. Storing both the model and
//! a context derived from it in the same struct would be self-referential
//! (not expressible in safe Rust without an unsafe pinning trick). Instead
//! each `embed_raw` call creates its own short-lived context sized to
//! exactly the input's token count (capped at [`BGE_N_CTX`]). This is
//! simpler and trivially thread-safe (`LlamaModel` is `Send + Sync`; each
//! call gets an independent context, no shared mutable state) at the cost
//! of re-paying context-alloc overhead per call — acceptable for a
//! CPU-only, small (109M-param) model. If K8's bulk pack-build profiling
//! ever shows this is a bottleneck, the fix is a persistent context guarded
//! by a `Mutex`, batching multiple chunks per `decode`.
//!
//! ## The shared `LlamaBackend`
//! `LlamaBackend::init()` is a process-global singleton (guarded by an
//! `AtomicBool` inside `llama-cpp-2` itself) — a second call errors
//! `BackendAlreadyInitialized` rather than returning a second handle. Since
//! nothing stops a caller from constructing more than one `BgeEmbedder` in
//! the same process (e.g. multiple tests in one `cargo test` binary),
//! [`shared_backend`] lazily inits it once behind a `OnceLock`, with a
//! double-checked-locking `Mutex` guarding the init race.

use std::num::NonZeroU32;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use kpack_core::embed::{query_input, EmbedError, Embedder};
use llama_cpp_2::context::params::{LlamaContextParams, LlamaPoolingType};
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaModel};

/// `bge-base-en-v1.5`'s embedding dimensionality (BAAI's published model
/// card; also what K1's `.kpack` schema and K2's manifest gate assume).
/// [`BgeEmbedder::new`] asserts the loaded GGUF actually reports this via
/// `n_embd()` rather than trusting the constant blindly — a wrong/corrupt
/// GGUF fails loudly at load time, not silently at query time.
const BGE_DIMS: usize = 768;

/// `bge-base-en-v1.5`'s trained max sequence length (BERT-style
/// `max_position_embeddings`; also llama.cpp's default `n_ctx` of 512,
/// unrelated coincidence but convenient). Inputs tokenizing longer than
/// this are rejected rather than silently truncated or run past the
/// model's trained positions (either of which would produce a
/// bad-but-plausible vector, not an error).
const BGE_N_CTX: u32 = 512;

/// Lazily-initialized, process-wide `llama.cpp` backend handle. See the
/// module doc's "The shared `LlamaBackend`" section for why this can't
/// just be `LlamaBackend::init()` inside `BgeEmbedder::new`.
static BACKEND: OnceLock<LlamaBackend> = OnceLock::new();
static BACKEND_INIT_LOCK: Mutex<()> = Mutex::new(());

fn shared_backend() -> Result<&'static LlamaBackend, EmbedError> {
    if let Some(backend) = BACKEND.get() {
        return Ok(backend);
    }
    // Double-checked locking: only one thread actually calls
    // `LlamaBackend::init()` (which itself would error on a concurrent
    // second call); everyone else waits for the lock, then finds `BACKEND`
    // already populated and returns that instead of racing `init()` too.
    let _guard = BACKEND_INIT_LOCK
        .lock()
        .map_err(|_| EmbedError::Backend("backend init lock poisoned".to_string()))?;
    if let Some(backend) = BACKEND.get() {
        return Ok(backend);
    }
    let backend = LlamaBackend::init()
        .map_err(|e| EmbedError::Backend(format!("llama backend init: {e}")))?;
    Ok(BACKEND.get_or_init(|| backend))
}

/// Context params shared by every `embed_raw` call: embeddings on, CLS
/// pooling (see module doc), context sized to exactly [`BGE_N_CTX`].
fn ctx_params() -> LlamaContextParams {
    LlamaContextParams::default()
        .with_n_ctx(NonZeroU32::new(BGE_N_CTX))
        .with_embeddings(true)
        .with_pooling_type(LlamaPoolingType::Cls)
}

/// The real, linked `bge-base-en-v1.5` `Embedder`, backed by `llama.cpp`
/// via `llama-cpp-2`. Construct with [`BgeEmbedder::new`] pointed at the
/// bundled Q8_0 GGUF (`src-tauri/resources/embedders/bge-base-en-v1.5-q8_0.gguf`,
/// fetched by `tools/fetch-embedder.mjs`).
pub struct BgeEmbedder {
    model: LlamaModel,
}

impl BgeEmbedder {
    /// Load `model_path` in embedding mode and verify it actually reports
    /// [`BGE_DIMS`] embedding dimensions (a wrong/mismatched GGUF errors
    /// here, not on the first `embed_passage`/`embed_query` call). CPU
    /// only — `bge-base-en-v1.5` is small enough that GPU offload isn't
    /// needed for this to be fast; `LlamaModelParams::default()` leaves
    /// `n_gpu_layers` at 0.
    pub fn new(model_path: &Path) -> Result<Self, EmbedError> {
        let backend = shared_backend()?;
        let model = LlamaModel::load_from_file(backend, model_path, &LlamaModelParams::default())
            .map_err(|e| EmbedError::Backend(format!("model load: {e}")))?;

        let got = usize::try_from(model.n_embd()).unwrap_or(0);
        if got != BGE_DIMS {
            return Err(EmbedError::Dims {
                expected: BGE_DIMS,
                got,
            });
        }

        Ok(Self { model })
    }

    /// Tokenize, decode, and read back the pooled (CLS) embedding for
    /// `text` — the shared path behind both `embed_passage` (raw text) and
    /// `embed_query` (prefixed via [`query_input`] by the caller). Returns
    /// raw fp32, matching `Embedder`'s documented contract — no
    /// normalization here; that's the `kpack-core` quantization path's job
    /// (spec §1.4).
    fn embed_raw(&self, text: &str) -> Result<Vec<f32>, EmbedError> {
        let backend = shared_backend()?;

        // `AddBos::Always` maps to llama.cpp's `add_special`, which — per
        // this GGUF's own tokenizer metadata for a BERT/WordPiece vocab —
        // adds both the leading `[CLS]` and trailing `[SEP]`, i.e. real
        // BGE-style tokenization, not just a bare BOS.
        let tokens = self
            .model
            .str_to_token(text, AddBos::Always)
            .map_err(|e| EmbedError::Backend(format!("tokenize: {e}")))?;

        if tokens.is_empty() {
            return Err(EmbedError::Backend(
                "tokenize produced zero tokens for non-trivial input".to_string(),
            ));
        }
        if tokens.len() as u32 > BGE_N_CTX {
            return Err(EmbedError::Backend(format!(
                "input tokenizes to {} tokens, exceeds bge-base-en-v1.5's {BGE_N_CTX}-token context window",
                tokens.len()
            )));
        }

        let mut ctx = self
            .model
            .new_context(backend, ctx_params())
            .map_err(|e| EmbedError::Backend(format!("context create: {e}")))?;

        let mut batch = LlamaBatch::new(tokens.len(), 1);
        batch
            .add_sequence(&tokens, 0, false)
            .map_err(|e| EmbedError::Backend(format!("batch add: {e}")))?;

        ctx.decode(&mut batch)
            .map_err(|e| EmbedError::Backend(format!("decode: {e}")))?;

        let embedding = ctx
            .embeddings_seq_ith(0)
            .map_err(|e| EmbedError::Backend(format!("embeddings read: {e}")))?;

        Ok(embedding.to_vec())
    }
}

impl Embedder for BgeEmbedder {
    fn dims(&self) -> usize {
        BGE_DIMS
    }

    fn embed_passage(&self, text: &str) -> Result<Vec<f32>, EmbedError> {
        self.embed_raw(text)
    }

    fn embed_query(&self, text: &str) -> Result<Vec<f32>, EmbedError> {
        // MUST route through `query_input` (kpack-core's single drift-guarded
        // place the BGE query prefix lives) rather than a hand-rolled
        // prefix — see kpack-core's embed.rs module doc and the K4a review.
        self.embed_raw(&query_input(text))
    }

    /// The real tokenizer's count, including the `[CLS]`/`[SEP]` special
    /// tokens `embed_raw` will actually feed the model — the unit K5's
    /// chunker targets. `Embedder::token_count` returns a bare `usize` (no
    /// `Result`), so the one failure mode `str_to_token` has — an interior
    /// NUL byte in `text`, which real document text extracted upstream
    /// should never contain — degrades to `0` rather than panicking. That
    /// degradation is a known, documented limitation of the trait's
    /// infallible signature, not a silent correctness bug for realistic
    /// input.
    fn token_count(&self, text: &str) -> usize {
        self.model
            .str_to_token(text, AddBos::Always)
            .map(|tokens| tokens.len())
            .unwrap_or(0)
    }
}
