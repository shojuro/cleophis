//! The `EngineBackend` trait — the demilitarized zone between the product and
//! the inference runtime (P0 brief). Desktop runs llama.cpp as a sidecar
//! process; mobile links it in-process over FFI. Both sit behind this one
//! trait, so the majority of the app never learns which it is talking to, and
//! desktop inherits the trait after the mobile demo ships (hence: no
//! mobile-only assumptions in these signatures).
//!
//! The lifecycle is the spec's, verbatim:
//! `load(base, adapters[]) → session(ctx) → stream(tokens) → unload()`.
//!   - [`EngineBackend::load`] validates + integrity-gates the base and the
//!     composition-ordered adapter stack (via `prepare_stack`) and returns a
//!     live [`EngineHandle`] owning model + adapters.
//!   - [`EngineHandle::session`] opens an [`EngineSession`] bound to the
//!     handle's model+adapters with a context config (n_ctx, sampling). The
//!     session borrows the handle, modelling "the native context borrows the
//!     model" without a self-referential struct.
//!   - [`EngineSession::stream`] renders a chat through the per-model template
//!     and streams visible token text into a [`TokenSink`] until stop/limit.
//!   - [`EngineHandle::unload`] releases everything (also happens on drop).

use std::ops::ControlFlow;

use crate::adapter::{AdapterRole, AdapterSpec, ModelSpec};
use crate::error::EngineError;
use crate::template::{ChatMessage, ChatTemplate};

/// A backend that can load a base + adapter stack and generate from it. `load`
/// is the only entry point; everything else hangs off the returned handle.
///
/// `Send + Sync`: a backend is shareable (it is a stateless factory over a
/// process-global native backend, like `kpack-embed`'s shared `LlamaBackend`).
pub trait EngineBackend: Send + Sync {
    /// `load(base, adapters[])` — validate + integrity-gate the artifacts (see
    /// [`crate::adapter::prepare_stack`]) and load them. Fail-closed: a missing
    /// or hash-mismatched artifact errors here rather than loading a reduced
    /// stack. Returns a handle owning the model and its composed adapters.
    fn load(&self, req: LoadRequest) -> Result<Box<dyn EngineHandle>, EngineError>;
}

/// A loaded base + adapter stack. Owns the native model and the mounted LoRA
/// adapters; hands out sessions.
///
/// **Thread-affine, deliberately not `Send`.** The native aarch64 build
/// surfaced that `llama-cpp-2`'s `LlamaLoraAdapter` holds a raw `NonNull` and
/// is `!Send` (the model itself is `Send + Sync`, but a mounted LoRA stack is
/// not), and a `LlamaContext` is likewise thread-bound. So a handle lives
/// entirely on the thread that created it — on mobile, a dedicated inference
/// thread (Android foreground service, spec §8); prompts and streamed tokens
/// cross threads via channels, never the handle. This matches desktop, where
/// the sidecar runs in its own process. Only [`EngineBackend`] (the stateless
/// factory) is `Send + Sync`.
pub trait EngineHandle {
    /// `session(ctx)` — open a generation session with the given context
    /// config. Borrows the handle for the session's lifetime (`+ '_`), so a
    /// handle yields one session at a time — the borrow models the native
    /// context's borrow of the model.
    fn session(&mut self, cfg: SessionConfig)
        -> Result<Box<dyn EngineSession + '_>, EngineError>;

    /// The adapter roles actually mounted, in composition order — what a probe
    /// run records as the tested stack (spec §11: a probe run against a reduced
    /// stack is a different, explicitly labeled gate).
    fn mounted_adapters(&self) -> &[AdapterRole];

    /// `unload()` — release the model, adapters, and native context. Dropping
    /// the handle does the same; this is the explicit lifecycle spelling.
    fn unload(self: Box<Self>);
}

/// A live generation session over a loaded handle.
pub trait EngineSession {
    /// `stream(tokens)` — render `messages` through the model's chat template
    /// (the in-process equivalent of the sidecar's `--jinja`) and stream the
    /// visible generated text into `sink`, one piece at a time, until an
    /// end-of-generation token, the token limit, or a caller cancel. The
    /// Qwen-only start-of-turn think-strip is applied inside this method, so
    /// the sink only ever sees display text.
    fn stream(
        &mut self,
        messages: &[ChatMessage],
        sink: &mut dyn TokenSink,
    ) -> Result<GenStats, EngineError>;

    /// sha256 (hex) of the **exact prompt bytes** this session would tokenise
    /// for `messages` — without generating anything.
    ///
    /// The device/pod rendering-parity handle (Phase 1c, A2). The pod computes
    /// the same digest over its own rendered prompt, and equal digests are the
    /// only evidence that the phone and the gate served the same string. The
    /// catalog's `promptFingerprint` cannot stand in for it: that pins the
    /// system prompt's TEXT and says nothing about the template around it —
    /// including whether the assistant prefix carries the pre-closed think
    /// block the gate has always served under.
    fn rendered_prompt_sha256(&self, messages: &[ChatMessage]) -> Result<String, EngineError>;
}

/// Where streamed token text goes. Returning [`ControlFlow::Break`] cancels
/// generation cooperatively (the §8 backgrounding/cancel contract lands on
/// top of this). A blanket impl makes any `FnMut(&str) -> ControlFlow<()>` a
/// sink, so callers can pass a closure.
pub trait TokenSink {
    fn on_token(&mut self, text: &str) -> ControlFlow<()>;
}

impl<F> TokenSink for F
where
    F: FnMut(&str) -> ControlFlow<()>,
{
    fn on_token(&mut self, text: &str) -> ControlFlow<()> {
        self(text)
    }
}

/// The `load` request: the base, the (unordered) adapter set, the chat-template
/// family, and native load params. `prepare_stack` inside the backend orders
/// and gates the adapters, so the caller may pass them in any order.
pub struct LoadRequest {
    pub base: ModelSpec,
    pub adapters: Vec<AdapterSpec>,
    pub template: ChatTemplate,
    pub params: LoadParams,
}

impl LoadRequest {
    /// The hero configuration: base + full behavioral→contract→voice stack,
    /// template resolved from the GGUF, CPU-first. Adapters may be passed in
    /// any order; the backend sorts them.
    pub fn new(base: ModelSpec, adapters: Vec<AdapterSpec>) -> Self {
        LoadRequest {
            base,
            adapters,
            template: ChatTemplate::Auto,
            params: LoadParams::default(),
        }
    }

    pub fn with_template(mut self, template: ChatTemplate) -> Self {
        self.template = template;
        self
    }

    pub fn with_params(mut self, params: LoadParams) -> Self {
        self.params = params;
        self
    }
}

/// Native load-time params. CPU-first is policy on Android (spec §1/H3):
/// `n_gpu_layers` defaults to 0, and a GPU path is opportunistic, never
/// load-bearing.
#[derive(Debug, Clone)]
pub struct LoadParams {
    /// Layers to offload to GPU. 0 = CPU-only (the Android default).
    pub n_gpu_layers: u32,
}

impl Default for LoadParams {
    fn default() -> Self {
        LoadParams { n_gpu_layers: 0 }
    }
}

/// Per-session context + sampling config.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// Context window. Per-tier cap: 2048 on the floor (1B), 4096 above (§2).
    pub n_ctx: u32,
    pub sampling: Sampling,
    /// Whether the backend applies the start-of-turn `<think>` strip before
    /// the sink sees a token. `None` — the shipping default — derives it from
    /// the template family, which is what every product caller wants.
    ///
    /// `Some(false)` exists for ONE caller: the device-probe harness, which
    /// has to record the engine's output *before* any stripping as well as
    /// after it. The release bar describes what the patient reads and the
    /// model bar describes what the model said; the think block sits between
    /// them, and a harness that can only see one side cannot tell a reply that
    /// was never generated from one the stripper ate (`ThinkStripper` returns
    /// the empty string when a run dies inside a block). The harness then
    /// applies the SAME `ThinkStripper` itself, so nothing is re-implemented —
    /// only observed at both points.
    pub strip_think: Option<bool>,
}

impl Default for SessionConfig {
    fn default() -> Self {
        SessionConfig {
            n_ctx: 2048,
            sampling: Sampling::default(),
            strip_think: None,
        }
    }
}

/// Sampling parameters for a session. Maps onto a `LlamaSampler` chain in the
/// native backend (top_k → top_p → temp → dist), or greedy when `temperature`
/// is 0.
#[derive(Debug, Clone)]
pub struct Sampling {
    pub temperature: f32,
    pub top_k: i32,
    pub top_p: f32,
    pub seed: u32,
    /// Hard cap on generated tokens (the token limit stop reason).
    pub max_tokens: usize,
}

impl Default for Sampling {
    fn default() -> Self {
        Sampling {
            temperature: 0.7,
            top_k: 40,
            top_p: 0.95,
            seed: 0,
            max_tokens: 512,
        }
    }
}

/// Why a `stream` ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// The model emitted an end-of-generation token.
    Eos,
    /// The `max_tokens` limit was reached.
    MaxTokens,
    /// The sink returned `ControlFlow::Break` — cooperative cancel.
    Cancelled,
    /// The stream ended INSIDE an unclosed `<think>` block, so every byte the
    /// model produced was suppressed and the sink saw nothing (Phase 1c, A2).
    ///
    /// Its own variant rather than a flavour of `MaxTokens` because the product
    /// has to say something different: an empty turn that stopped mid-thought
    /// is "the reply was cut short", and rendering it as a blank bubble is the
    /// device-only failure this variant exists to make impossible to miss.
    ///
    /// With the pre-closed think block now in the rendered prompt
    /// ([`crate::template::ThinkPolicy::PreClosed`]) the model cannot open a
    /// block at all, so this should never fire on the Qwen3 triage hero. That
    /// is the point: if it does, the prompt is not the one the gate served.
    TruncatedInThink,
}

impl StopReason {
    /// Fold the stripper's verdict into the generation loop's own stop reason.
    ///
    /// The loop and the stripper each know half of why a turn ended, and the
    /// precedence between them is a judgement rather than an obvious fact — so
    /// it is written down once, here, instead of twice in two backends. See the
    /// test below for the reasoning on each arm.
    pub fn with_think_truncation(self, truncated: bool) -> StopReason {
        match (truncated, self) {
            (false, _) => self,
            // The user stopped it; that is their fact, not the budget's.
            (true, StopReason::Cancelled) => StopReason::Cancelled,
            (true, _) => StopReason::TruncatedInThink,
        }
    }
}

/// What a completed (or cancelled) `stream` produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenStats {
    pub prompt_tokens: usize,
    pub generated_tokens: usize,
    pub stop: StopReason,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The precedence rule, stated once and tested here because its two
    /// production callers (`llama.rs` and `mock.rs`) are a native backend and a
    /// fake, and neither is the right place to decide it.
    #[test]
    fn truncation_outranks_the_budget_that_caused_it_but_never_a_cancel() {
        // A turn that died inside `<think>` has nothing to show, and saying
        // "max tokens" about it invites the product to render an empty bubble.
        assert_eq!(
            StopReason::MaxTokens.with_think_truncation(true),
            StopReason::TruncatedInThink
        );
        assert_eq!(
            StopReason::Eos.with_think_truncation(true),
            StopReason::TruncatedInThink
        );
        // A cancel is the USER's action. Reporting it as a truncation would
        // tell them the model was cut short by the budget when in fact they
        // stopped it, and the UI already has a story for a cancelled turn.
        assert_eq!(
            StopReason::Cancelled.with_think_truncation(true),
            StopReason::Cancelled
        );
        for s in [
            StopReason::Eos,
            StopReason::MaxTokens,
            StopReason::Cancelled,
        ] {
            assert_eq!(s.with_think_truncation(false), s, "{s:?} must be untouched");
        }
    }
}
