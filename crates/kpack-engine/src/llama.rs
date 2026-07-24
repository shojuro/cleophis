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

use crate::adapter::{prepare_stack, AdapterRole, VerifyCache};
use crate::backend::{
    EngineBackend, EngineHandle, EngineSession, GenStats, LoadRequest, SessionConfig, StopReason,
    TokenSink,
};
use crate::error::EngineError;
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
}

impl EngineSession for LlamaSession<'_> {
    fn stream(
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

        // Decode the prompt. One sequence (id 0); request logits only on the
        // final prompt token so the first sample reads from it.
        let n_ctx = self.cfg.n_ctx as usize;
        let mut batch = LlamaBatch::new(n_ctx.max(prompt_tokens.max(1)), 1);
        let last = prompt_tokens.saturating_sub(1);
        for (i, tok) in tokens.iter().enumerate() {
            batch
                .add(*tok, i as i32, &[0], i == last)
                .map_err(|e| EngineError::Backend(format!("batch add: {e}")))?;
        }
        self.ctx
            .decode(&mut batch)
            .map_err(|e| EngineError::Backend(format!("decode prompt: {e}")))?;

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
            pos += 1;
        }

        Ok(GenStats {
            prompt_tokens,
            generated_tokens: generated,
            stop,
        })
    }
}

impl LlamaSession<'_> {
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
    }
}
