//! `LlamaEngine` — the real, linked [`EngineBackend`], backed by llama.cpp via
//! `llama-cpp-2` (the SAME `=0.1.151` pin `kpack-embed` uses for the BGE
//! embedder). Everything here is gated behind the `real` Cargo feature, so the
//! default build stays toolchain-free; only a build that opts into `real`
//! (ultimately `src-tauri`, and the mobile NDK build) pulls in llama.cpp.
//!
//! ## Relationship to `inference.rs`
//! `load` routes through [`crate::adapter::prepare_stack`], so composition
//! order, fail-closed resolution, and the per-artifact sha256 gate are the
//! exact same policy the desktop sidecar enforces — this file only does the
//! FFI. It composes the adapter stack with per-adapter `lora_adapter_set`
//! calls in order, the in-process equivalent of the sidecar's `--lora a,b,c`
//! (`build_server_args`).
//!
//! ## Native-build / on-device verification points (P0 gate, spec §0, H14)
//! This module is written against the confirmed 0.1.151 API surface but is not
//! host-compilable in the spike environment (no libclang for `bindgen`); it is
//! typechecked by the `--features real` NDK build and validated on device.
//! Two specifics to confirm there, flagged inline:
//!   1. **Multi-LoRA apply** — that N ordered `lora_adapter_set` calls *stack*
//!      (compose) rather than replace, and are stable/memory-sane on 1B and 4B.
//!      This is the single highest-risk item (H14); the fallback ladder
//!      (behavioral+contract, contract never dropped) is [`PreparedStack::without_voice`].
//!   2. **`LlamaLoraAdapter` ownership** — whether it carries a borrow of the
//!      model. This code assumes it is owned (stored beside the model in the
//!      handle). If it borrows the model, the handle needs a self-referential
//!      cell (`self_cell`) or a load-adapters-per-session structure.

use std::num::NonZeroU32;
use std::ops::ControlFlow;
use std::sync::{Mutex, OnceLock};

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::context::LlamaContext;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaChatMessage, LlamaChatTemplate, LlamaLoraAdapter, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;
use llama_cpp_2::token::LlamaToken;

use crate::adapter::{prepare_stack, AdapterRole, VerifyCache};
use crate::backend::{
    EngineBackend, EngineHandle, EngineSession, GenStats, LoadRequest, SessionConfig, StopReason,
    TokenSink,
};
use crate::error::EngineError;
use crate::prefix::reusable_prefix;
use crate::template::{ChatMessage, ChatTemplate, Role, ThinkStripper};

/// Process-wide llama.cpp backend, initialized once. Same rationale as
/// `kpack-embed::bge::shared_backend`: `LlamaBackend::init()` is a global
/// singleton (a second call errors), and multiple `LlamaEngine`s (or an engine
/// plus the embedder) can coexist in one process — so the init is behind a
/// `OnceLock` with a double-checked `Mutex`. One native backend for the whole
/// process, embedder and engine both (spec §1: "one native library in the
/// process, not two").
static BACKEND: OnceLock<LlamaBackend> = OnceLock::new();
static BACKEND_INIT_LOCK: Mutex<()> = Mutex::new(());

fn shared_backend() -> Result<&'static LlamaBackend, EngineError> {
    if let Some(b) = BACKEND.get() {
        return Ok(b);
    }
    let _guard = BACKEND_INIT_LOCK
        .lock()
        .map_err(|_| EngineError::Backend("backend init lock poisoned".into()))?;
    if let Some(b) = BACKEND.get() {
        return Ok(b);
    }
    let backend =
        LlamaBackend::init().map_err(|e| EngineError::Backend(format!("llama backend init: {e}")))?;
    Ok(BACKEND.get_or_init(|| backend))
}

/// The compiled + runtime-detected CPU kernel features llama.cpp reports
/// (`llama_print_system_info`): NEON, ARM_FMA, DOTPROD, MATMUL_INT8 (i8mm),
/// LLAMAFILE, AVX2, etc. Printed at harness startup so a device transcript
/// **self-evidences** whether ARM SIMD kernels are active — a build compiled
/// baseline-only (no `+dotprod`/`+i8mm`) shows those features missing and,
/// together with the tok/s, makes a scalar build unmistakable (spec H3).
pub fn backend_system_info() -> String {
    // Initialize the backend before querying (no-op if already done).
    let _ = shared_backend();
    // SAFETY: `llama_print_system_info` returns a pointer to a static C string
    // owned by llama.cpp; we only borrow it to copy into an owned String.
    unsafe {
        let ptr = llama_cpp_sys_2::llama_print_system_info();
        if ptr.is_null() {
            return String::new();
        }
        std::ffi::CStr::from_ptr(ptr).to_string_lossy().into_owned()
    }
}

/// The real `llama-cpp-2`-backed engine. Stateless factory over the shared
/// backend; `load` produces a handle owning the model + adapter stack.
#[derive(Default)]
pub struct LlamaEngine {
    verify: VerifyCache,
}

impl LlamaEngine {
    pub fn new() -> Self {
        LlamaEngine::default()
    }
}

impl EngineBackend for LlamaEngine {
    fn load(&self, req: LoadRequest) -> Result<Box<dyn EngineHandle>, EngineError> {
        // Same policy the desktop sidecar runs: order + fail-closed + sha256
        // gate. Nothing native happens until every artifact has passed.
        let stack = prepare_stack(&req.base, &req.adapters, &self.verify)?;
        let backend = shared_backend()?;

        // CPU-first: n_gpu_layers defaults to 0 on Android (spec §1/H3).
        let model_params = LlamaModelParams::default().with_n_gpu_layers(req.params.n_gpu_layers);
        let model = LlamaModel::load_from_file(backend, &stack.base.path, &model_params)
            .map_err(|e| EngineError::Backend(format!("model load: {e}")))?;

        // Initialize each LoRA adapter in composition order (behavioral→
        // contract→voice). Applying them onto a context happens in `session`.
        let mut adapters = Vec::with_capacity(stack.adapters.len());
        for a in &stack.adapters {
            let lora = model
                .lora_adapter_init(&a.path)
                .map_err(|e| EngineError::Backend(format!("lora init {}: {e}", a.role.as_str())))?;
            adapters.push(MountedAdapter { role: a.role, scale: a.scale, lora });
        }

        let roles = stack.roles();
        Ok(Box::new(LlamaHandle {
            model,
            adapters,
            roles,
            template: req.template,
        }))
    }
}

struct MountedAdapter {
    role: AdapterRole,
    scale: f32,
    lora: LlamaLoraAdapter,
}

struct LlamaHandle {
    model: LlamaModel,
    adapters: Vec<MountedAdapter>,
    roles: Vec<AdapterRole>,
    template: ChatTemplate,
}

impl EngineHandle for LlamaHandle {
    fn session(
        &mut self,
        cfg: SessionConfig,
    ) -> Result<Box<dyn EngineSession + '_>, EngineError> {
        let backend = shared_backend()?;
        let ctx_params = LlamaContextParams::default().with_n_ctx(NonZeroU32::new(cfg.n_ctx));
        // `ctx` need not be `mut` here: `lora_adapter_set` takes `&self`, and the
        // context is then moved into the session (where `decode` uses it via the
        // session's own `&mut self`).
        let ctx = self
            .model
            .new_context(backend, ctx_params)
            .map_err(|e| EngineError::Backend(format!("context create: {e}")))?;

        // H14 verification point: compose the LoRA stack onto the context, in
        // order. Each `lora_adapter_set` is additive (llama_set_adapter_lora),
        // reproducing the sidecar's comma-joined `--lora`. Confirm on device
        // that N calls stack rather than replace, and are stable on 1B/4B.
        for a in &mut self.adapters {
            ctx.lora_adapter_set(&mut a.lora, a.scale).map_err(|e| {
                EngineError::Backend(format!("lora apply {}: {e}", a.role.as_str()))
            })?;
        }

        // Resolve the per-model chat template. `Auto` reads the template
        // embedded in the GGUF — the in-process equivalent of the sidecar's
        // `--jinja`. An explicit family is used only as a fallback / for a
        // model whose GGUF ships no template.
        let chat_template = self
            .model
            .chat_template(None)
            .map_err(|e| EngineError::Backend(format!("chat template: {e}")))?;

        // Whether to strip a start-of-turn <think> block. `Auto` defers to the
        // declared family; the engine never strips speculatively.
        let strip_think = self.template.strips_think();

        Ok(Box::new(LlamaSession {
            model: &self.model,
            ctx,
            chat_template,
            strip_think,
            cfg,
            cached: Vec::new(),
        }))
    }

    fn mounted_adapters(&self) -> &[AdapterRole] {
        &self.roles
    }

    fn unload(self: Box<Self>) {
        // Drop releases the context (already dropped with any session), the
        // adapters, and the model.
    }
}

struct LlamaSession<'a> {
    model: &'a LlamaModel,
    ctx: LlamaContext<'a>,
    chat_template: LlamaChatTemplate,
    strip_think: bool,
    cfg: SessionConfig,
    /// Mirror of the tokens currently resident in sequence 0's KV cache, in
    /// position order — prompt tokens plus every generated token that was fed
    /// back in. This is what makes prefix reuse possible AND safe: the cache is
    /// invisible from Rust, so without a mirror there is no way to know which
    /// prefix is actually valid, and reusing a cache you cannot describe is how
    /// you get silent context corruption rather than a fast turn.
    cached: Vec<LlamaToken>,
}

impl EngineSession for LlamaSession<'_> {
    fn stream(
        &mut self,
        messages: &[ChatMessage],
        sink: &mut dyn TokenSink,
    ) -> Result<GenStats, EngineError> {
        let result = self.stream_turn(messages, sink);
        if result.is_err() {
            // A `decode` that failed part-way may have left the native cache
            // holding tokens the mirror does not list, and a mirror that
            // overstates or understates the cache is exactly the condition the
            // mirror exists to prevent. Throw the prefix away rather than
            // reason about which half of a failed batch landed.
            //
            // This costs nothing before task 1.5's wiring, because a session
            // was discarded after every turn. It costs a full re-decode now
            // that a session serves many — which is the correct direction to
            // fail in: slow, never wrong.
            self.invalidate_prefix();
        }
        result
    }
}

impl LlamaSession<'_> {
    /// One turn, from rendered prompt to stop reason. Wrapped by
    /// `EngineSession::stream` so that *every* early return goes through the
    /// prefix invalidation above. There are nine `?`s in this function;
    /// remembering to invalidate at each one is not a plan, and the one that
    /// gets forgotten is silent.
    fn stream_turn(
        &mut self,
        messages: &[ChatMessage],
        sink: &mut dyn TokenSink,
    ) -> Result<GenStats, EngineError> {
        // Render the chat through the model's own template (honors the prompt
        // contract byte-for-byte: the caller passes the contract system prompt
        // and rendered sources in as ChatMessages; see the crate docs).
        let chat: Vec<LlamaChatMessage> = messages
            .iter()
            .map(|m| {
                LlamaChatMessage::new(role_str(m.role).to_string(), m.content.clone())
                    .map_err(|e| EngineError::Backend(format!("chat message: {e}")))
            })
            .collect::<Result<_, _>>()?;
        let prompt = self
            .model
            .apply_chat_template(&self.chat_template, &chat, true)
            .map_err(|e| EngineError::Backend(format!("apply chat template: {e}")))?;

        let tokens = self
            .model
            .str_to_token(&prompt, AddBos::Always)
            .map_err(|e| EngineError::Backend(format!("tokenize: {e}")))?;
        let prompt_tokens = tokens.len();

        // ---- Prefix-KV reuse (spec task 1.5) ----------------------------
        //
        // Re-decoding the whole conversation every turn is what makes
        // first-token latency grow with history — 25-40 s mid-chat on the
        // floor device. The KV cache already holds the previous turn, and a
        // chat prompt is almost entirely a prefix of the next one, so only the
        // suffix genuinely needs decoding.
        //
        // Two invariants make this safe rather than merely fast:
        //   1. Trim before extending. Whatever the cache holds beyond the
        //      shared prefix is stale and MUST be removed — leaving it means
        //      the model attends to tokens from an older turn that are no
        //      longer in the prompt. That is silent context corruption, and it
        //      is exactly what "just keep the session alive" would have caused.
        //   2. Always decode at least one token, so the sampler has fresh
        //      logits to read. A fully-reused prefix would leave the final
        //      logits belonging to the previous turn.
        let mut reuse = reusable_prefix(&self.cached, &tokens);

        // Trimmed unconditionally, not only when the mirror says there is
        // something past the prefix. The mirror exists precisely because the
        // cache is invisible from Rust, so a guard that trusts it to be right
        // about emptiness is a guard that fails in exactly the case that
        // matters — and the removal is a no-op when there is nothing there.
        let trimmed = self
            .ctx
            .clear_kv_cache_seq(Some(0), Some(reuse as u32), None)
            .map_err(|e| EngineError::Backend(format!("kv trim: {e}")))?;
        if !trimmed {
            // llama.cpp reports a partial removal it cannot perform by
            // returning false (recurrent/state-space memory says so); removing
            // a whole sequence never fails. Reuse is an optimisation and
            // correctness is not, so drop everything and re-decode.
            self.ctx
                .clear_kv_cache_seq(Some(0), None, None)
                .map_err(|e| EngineError::Backend(format!("kv clear: {e}")))?;
            self.cached.clear();
            reuse = 0;
        }
        self.cached.truncate(reuse);
        let reuse = reuse;

        // Decode the un-cached suffix. One sequence (id 0); request logits only
        // on the final prompt token so the first sample reads from it.
        let n_ctx = self.cfg.n_ctx as usize;
        let mut batch = LlamaBatch::new(n_ctx.max(prompt_tokens.max(1)), 1);
        let last = prompt_tokens.saturating_sub(1);
        for (i, tok) in tokens.iter().enumerate().skip(reuse) {
            batch
                .add(*tok, i as i32, &[0], i == last)
                .map_err(|e| EngineError::Backend(format!("batch add: {e}")))?;
        }
        self.ctx
            .decode(&mut batch)
            .map_err(|e| EngineError::Backend(format!("decode prompt: {e}")))?;
        self.cached.extend_from_slice(&tokens[reuse..]);

        let mut sampler = self.build_sampler();
        let mut stripper = ThinkStripper::new(self.strip_think);
        // One decoder for the whole turn: a multi-byte UTF-8 char can straddle
        // two tokens, and the Decoder buffers the partial sequence across calls.
        let mut decoder = encoding_rs::UTF_8.new_decoder();
        let mut generated = 0usize;
        let mut pos = prompt_tokens as i32;
        let mut stop = StopReason::Eos;

        loop {
            // Sample from the last decoded position's logits.
            let token = sampler.sample(&self.ctx, batch.n_tokens() - 1);
            sampler.accept(token);

            // End-of-generation: leave `stop` at its Eos default.
            if self.model.is_eog_token(token) {
                break;
            }

            generated += 1;
            let piece = self
                .model
                .token_to_piece(token, &mut decoder, false, None)
                .unwrap_or_default();
            let visible = stripper.push(&piece);
            if !visible.is_empty() {
                if let ControlFlow::Break(()) = sink.on_token(&visible) {
                    stop = StopReason::Cancelled;
                    break;
                }
            }

            if generated >= self.cfg.sampling.max_tokens {
                stop = StopReason::MaxTokens;
                break;
            }

            // Feed the sampled token back in for the next step.
            batch.clear();
            batch
                .add(token, pos, &[0], true)
                .map_err(|e| EngineError::Backend(format!("batch add gen: {e}")))?;
            self.ctx
                .decode(&mut batch)
                .map_err(|e| EngineError::Backend(format!("decode gen: {e}")))?;
            // Only tokens that were actually decoded are in the cache. The
            // token that ends the turn (EOG, cancel, or the max-tokens cap) is
            // sampled but never fed back, so it must not be mirrored here.
            self.cached.push(token);
            pos += 1;
        }

        Ok(GenStats {
            prompt_tokens,
            generated_tokens: generated,
            stop,
        })
    }

    /// Drop the reusable prefix entirely — the native cache and the mirror
    /// together, so the two cannot disagree. The next turn re-decodes its whole
    /// prompt, which is the pre-1.5 behaviour and is always correct.
    fn invalidate_prefix(&mut self) {
        // Best-effort is sufficient *because* the next turn trims
        // unconditionally: even if this clear fails, that turn removes
        // everything past its own shared prefix — which, against an empty
        // mirror, is everything. Clearing the mirror is the part that must not
        // fail, and it cannot.
        let _ = self.ctx.clear_kv_cache_seq(Some(0), None, None);
        self.cached.clear();
    }

    /// Build the sampler chain from the session's sampling config. Greedy when
    /// temperature is 0 (deterministic — used by the probe runs); otherwise
    /// top_k → top_p → temp → dist(seed).
    fn build_sampler(&self) -> LlamaSampler {
        let s = &self.cfg.sampling;
        if s.temperature <= 0.0 {
            LlamaSampler::greedy()
        } else {
            LlamaSampler::chain_simple([
                LlamaSampler::top_k(s.top_k),
                LlamaSampler::top_p(s.top_p, 1),
                LlamaSampler::temp(s.temperature),
                LlamaSampler::dist(s.seed),
            ])
        }
    }
}

fn role_str(role: Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        // The role name both shipping families use in their embedded chat
        // templates. A GGUF whose template lacks a `tool` branch still renders
        // the turn (llama.cpp falls back rather than erroring), so a tool result
        // is always visible to the model even on a model we have not profiled.
        Role::Tool => "tool",
    }
}
