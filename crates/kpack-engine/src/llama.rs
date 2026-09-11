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
use crate::template::{role_name, ChatMessage, ChatTemplate, ThinkStripper};

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

/// The **compile-time** CPU kernel macros llama.cpp reports
/// (`llama_print_system_info`): NEON, ARM_FMA, DOTPROD, MATMUL_INT8 (i8mm),
/// LLAMAFILE, AVX2, etc.
///
/// 🔴 **This is a build constant, not a device fact.** It reports
/// `__ARM_FEATURE_DOTPROD` and friends — the macros the C compiler had — so on
/// this project's aarch64-android build (vendored `armv8.2-a+dotprod` patch) it
/// prints `DOTPROD = 1` on **every** device, including one that then takes
/// SIGILL on a dotprod instruction. It did exactly that on a Galaxy A51.
/// Callers must pair it with [`crate::cpu::runtime_cpu`]; [`print_kernel_report`]
/// is the wiring that makes doing so the default.
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

/// Print the CPU capability block: what the silicon implements, what the binary
/// was compiled to require, and the verdict between them.
///
/// **Two `eprintln!`s, not one, and that is the property.** The runtime line is
/// emitted before `backend_system_info()` is called, so it reaches the log
/// before anything of llama.cpp's runs. On a device whose CPU the build does
/// not match, backend initialisation is itself a candidate for SIGILL — and a
/// diagnostic that only prints after it would be silent on precisely the
/// devices it exists for.
///
/// Use this instead of `eprintln!("[kernels] {}", backend_system_info())`. That
/// line was the project's most load-bearing diagnostic and it could not print a
/// failing value; the composition here is what makes the honest form the easy
/// one, per the standing lesson that a rule which must be remembered at the
/// moment of use will not be.
pub fn print_kernel_report() {
    let runtime = crate::cpu::runtime_cpu();
    eprintln!("{}", crate::cpu::runtime_line(runtime));
    let info = backend_system_info();
    eprintln!("{}", crate::cpu::verdict_block(&info, runtime));
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

        // Resolve the per-model chat template STRING. `chat_template(None)`
        // reads the one embedded in the GGUF — the in-process equivalent of the
        // sidecar's `--jinja` — and that is true whatever family the caller
        // declared.
        //
        // 🔴 THE DECLARED FAMILY IS NO LONGER COSMETIC, and the comment that
        // used to sit here said the opposite. It claimed an explicit family was
        // "only a fallback / for a model whose GGUF ships no template", which
        // was true while the family decided nothing but whether to strip.
        //
        // Since A2 it decides the THINK POLICY, which shapes the prompt:
        // `ChatMl` appends the 19-byte pre-closed `<think>\n\n</think>\n\n`
        // block so the model cannot open one, and `Auto` and `Llama3` append
        // nothing. So `Auto` and `ChatMl` no longer render the same bytes for a
        // Qwen3 GGUF — they differ by that block and by whether the model
        // reasons inside the catalog's 320-token budget. `Auto` on a Qwen3
        // model serves the THINKING prompt, which is the prompt no gate this
        // programme has ever run under.
        //
        // Nothing here detects the family from the GGUF; it comes from the
        // `LoadRequest`. The app is safe because the catalog says `"qwen"` and
        // `template_for` maps that to `ChatMl` (pinned by A2's parity test).
        // The device-probe harness refuses `--template auto` in its gate mode
        // for the same reason, rather than trusting the flag to be remembered.
        let chat_template = self
            .model
            .chat_template(None)
            .map_err(|e| EngineError::Backend(format!("chat template: {e}")))?;

        // The FAMILY is handed down, not a derived bool. The session needs it
        // twice — for the think policy that shapes the prompt and for the
        // stripper that guards the stream — and a session holding only the
        // second is how those two came to disagree in the first place.
        Ok(Box::new(LlamaSession {
            model: &self.model,
            ctx,
            chat_template,
            template: self.template,
            cfg,
            cached: Vec::new(),
            prompt_sha_logged: false,
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
    /// The declared family. Decides the think policy applied to every rendered
    /// prompt AND whether the stripper runs.
    template: ChatTemplate,
    cfg: SessionConfig,
    /// Mirror of the tokens currently resident in sequence 0's KV cache, in
    /// position order — prompt tokens plus every generated token that was fed
    /// back in. This is what makes prefix reuse possible AND safe: the cache is
    /// invisible from Rust, so without a mirror there is no way to know which
    /// prefix is actually valid, and reusing a cache you cannot describe is how
    /// you get silent context corruption rather than a fast turn.
    cached: Vec<LlamaToken>,
    /// Whether this session has already printed its rendered-prompt sha. Once
    /// per session, not once per turn: the value is a property of the template
    /// and the system prompt, and a per-turn line would bury it.
    prompt_sha_logged: bool,
}

impl EngineSession for LlamaSession<'_> {
    fn rendered_prompt_sha256(&self, messages: &[ChatMessage]) -> Result<String, EngineError> {
        Ok(crate::template::prompt_sha256(
            &self.render_prompt(messages)?,
        ))
    }

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
        let prompt = self.render_prompt(messages)?;

        // Once per session, at the front of the transcript: the sha of the
        // exact bytes about to be tokenised. This is the device half of the
        // rendering-parity check — the pod prints the same digest for the same
        // message — and it is printed rather than merely returned because a
        // device transcript has to self-evidence what it served. A run whose
        // sha differs from the pod's is not a model difference, and without
        // this line that would be argued about afterwards instead of read off.
        let tokens = self
            .model
            .str_to_token(&prompt, AddBos::Always)
            .map_err(|e| EngineError::Backend(format!("tokenize: {e}")))?;
        let prompt_tokens = tokens.len();

        // Printed AFTER tokenising, so the line can carry the token count.
        //
        // The sha pins the prompt STRING and stops there. It says nothing about
        // whether this tokenizer prepended a BOS token on the way to the model
        // — `AddBos::Always` adds one only if the vocab declares one, and Qwen3
        // does not, so it *should* be a no-op. "Should" is not a measurement:
        // the pod and the phone could differ by one leading token with
        // identical digests, and a one-token shift is exactly the kind of
        // difference that produces a handful of legitimate-looking
        // disagreements and no explanation. `tokens=` is what would show it.
        if !self.prompt_sha_logged {
            self.prompt_sha_logged = true;
            eprintln!(
                "[prompt] template={:?} think={:?} bytes={} tokens={} sha256={}",
                self.template,
                self.template.think_policy(),
                prompt.len(),
                prompt_tokens,
                crate::template::prompt_sha256(&prompt)
            );
        }

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
        // Kept ON as a no-op safety net: the prompt now closes the block
        // before the first token, so a well-behaved Qwen3 emits none — but a
        // model that emits one anyway is still stripped rather than shown.
        // The family decides, unless a caller overrides it. The ONE caller that
        // does is the device-probe harness, which must record the engine's
        // output on BOTH sides of the strip: `raw` is the model contract's
        // evidence and `text` is the product contract's, and a harness that can
        // only see one of them cannot tell a reply the model never gave from
        // one the stripper ate. It then applies THIS stripper, `finish()` and
        // all, so nothing about the rule is re-implemented — only observed
        // twice. See `SessionConfig::strip_think`.
        let strip = self
            .cfg
            .strip_think
            .unwrap_or_else(|| self.template.strips_think());
        let mut stripper = ThinkStripper::new(strip);
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

        // End of stream. `push` holds bytes back by design, so a turn that
        // never flushes loses whatever the stripper was still deciding about —
        // and a turn that ended inside `<think>` loses ALL of it and returns an
        // empty string. That was a device-only catastrophic failure mode with
        // no test and no symptom other than a blank reply.
        let end = stripper.finish();
        if !end.text().is_empty() {
            if let ControlFlow::Break(()) = sink.on_token(end.text()) {
                stop = StopReason::Cancelled;
            }
        }
        let stop = stop.with_think_truncation(end.truncated_in_think());

        Ok(GenStats {
            prompt_tokens,
            generated_tokens: generated,
            stop,
        })
    }

    /// Render `messages` into the exact bytes this session tokenises.
    ///
    /// Two steps, and the second is the whole of task A2. `apply_chat_template`
    /// goes through `llama_chat_apply_template`, the C template API, which
    /// takes **no Jinja kwargs** — so the phone cannot ask Qwen3 for
    /// `enable_thinking=false`, which is what every gate this programme has run
    /// was served under. `finish_generation_prompt` appends the pre-closed
    /// block the kwarg would have produced, in the one place that decision
    /// lives, so the device serves the prompt the gate serves and the model
    /// physically cannot spend the 320-token budget thinking.
    ///
    /// The prompt contract is still honored byte-for-byte: the caller passes the
    /// contract system prompt and rendered sources in as `ChatMessage`s and the
    /// engine never re-types them (see the crate docs).
    fn render_prompt(&self, messages: &[ChatMessage]) -> Result<String, EngineError> {
        let chat: Vec<LlamaChatMessage> = messages
            .iter()
            .map(|m| {
                LlamaChatMessage::new(role_name(m.role).to_string(), m.content.clone())
                    .map_err(|e| EngineError::Backend(format!("chat message: {e}")))
            })
            .collect::<Result<_, _>>()?;
        let applied = self
            .model
            .apply_chat_template(&self.chat_template, &chat, true)
            .map_err(|e| EngineError::Backend(format!("apply chat template: {e}")))?;
        Ok(self.template.finish_generation_prompt(&applied))
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
