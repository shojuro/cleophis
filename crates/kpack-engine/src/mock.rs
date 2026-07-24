//! `MockBackend` — a deterministic [`EngineBackend`] with no native
//! dependency, the engine's analogue of `kpack-core`'s `MockEmbedder`. It runs
//! the *real* [`prepare_stack`] (so composition order, fail-closed resolution,
//! and the sha256 gate are exercised against real temp files) and a *fake*
//! generator (so streaming, the think-strip pipeline, cancellation, and the
//! token limit are exercised) — all under `cargo test` with no cmake/libclang.
//!
//! The fake generator emits a fixed token sequence. When the session's template
//! strips think ([`ChatTemplate::strips_think`]), it prepends a `<think>…</think>`
//! block, so a test can prove the stripper is wired into the stream path — not
//! merely unit-tested in isolation.

use std::ops::ControlFlow;

use crate::adapter::{prepare_stack, AdapterRole, VerifyCache};
use crate::backend::{
    EngineBackend, EngineHandle, EngineSession, GenStats, LoadRequest, SessionConfig, StopReason,
    TokenSink,
};
use crate::error::EngineError;
use crate::template::{ChatMessage, ChatTemplate, ThinkStripper};

/// A deterministic, dependency-free backend for tests and for bring-up before
/// a native build exists on a target.
#[derive(Default)]
pub struct MockBackend {
    verify: VerifyCache,
}

impl MockBackend {
    pub fn new() -> Self {
        MockBackend::default()
    }
}

impl EngineBackend for MockBackend {
    fn load(&self, req: LoadRequest) -> Result<Box<dyn EngineHandle>, EngineError> {
        // Real policy: order + fail-closed + sha256 gate. A missing or
        // mismatched artifact fails here exactly as the native backend would.
        let stack = prepare_stack(&req.base, &req.adapters, &self.verify)?;
        Ok(Box::new(MockHandle {
            roles: stack.roles(),
            template: req.template,
        }))
    }
}

struct MockHandle {
    roles: Vec<AdapterRole>,
    template: ChatTemplate,
}

impl EngineHandle for MockHandle {
    fn session(
        &mut self,
        cfg: SessionConfig,
    ) -> Result<Box<dyn EngineSession + '_>, EngineError> {
        Ok(Box::new(MockSession {
            template: self.template,
            cfg,
        }))
    }

    fn mounted_adapters(&self) -> &[AdapterRole] {
        &self.roles
    }

    fn unload(self: Box<Self>) {}
}

struct MockSession {
    template: ChatTemplate,
    cfg: SessionConfig,
}

impl EngineSession for MockSession {
    fn stream(
        &mut self,
        messages: &[ChatMessage],
        sink: &mut dyn TokenSink,
    ) -> Result<GenStats, EngineError> {
        let prompt_tokens = messages.iter().map(|m| m.content.split_whitespace().count()).sum();

        // A fake but structured token stream. When the template strips think,
        // lead with a think block so the stripper (wired below, same as the
        // native path) has something to remove.
        let mut tokens: Vec<&str> = Vec::new();
        if self.template.strips_think() {
            tokens.extend(["<think>", "planning ", "the reply", "</think>"]);
        }
        tokens.extend(["tok0", "tok1", "tok2", "tok3", "tok4"]);

        let mut stripper = ThinkStripper::new(self.template.strips_think());
        let mut generated = 0usize;
        let mut stop = StopReason::Eos;

        for raw in tokens {
            // The think block tokens count as "generated" (the model produced
            // them) but never reach the sink — same accounting the native path
            // uses. The token limit counts generated tokens, think included.
            generated += 1;
            let visible = stripper.push(raw);
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
        }

        Ok(GenStats {
            prompt_tokens,
            generated_tokens: generated,
            stop,
        })
    }
}

/// A tiny sink that collects streamed text — handy for tests and bring-up.
pub struct CollectSink {
    pub text: String,
    limit: Option<usize>,
    count: usize,
}

impl CollectSink {
    pub fn new() -> Self {
        CollectSink { text: String::new(), limit: None, count: 0 }
    }
    /// Cancel after `n` sink calls — exercises the cooperative-cancel path.
    pub fn cancel_after(n: usize) -> Self {
        CollectSink { text: String::new(), limit: Some(n), count: 0 }
    }
}

impl Default for CollectSink {
    fn default() -> Self {
        CollectSink::new()
    }
}

impl TokenSink for CollectSink {
    fn on_token(&mut self, text: &str) -> ControlFlow<()> {
        self.text.push_str(text);
        self.count += 1;
        match self.limit {
            Some(l) if self.count >= l => ControlFlow::Break(()),
            _ => ControlFlow::Continue(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::{AdapterSpec, ModelSpec};
    use std::path::PathBuf;

    fn unique_dir(name: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir()
            .join(format!("kpack-engine-mock-{}-{}-{}", std::process::id(), unique, name));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn full_stack(dir: &PathBuf) -> (ModelSpec, Vec<AdapterSpec>) {
        std::fs::write(dir.join("base.gguf"), b"base").unwrap();
        std::fs::write(dir.join("beh.gguf"), b"b").unwrap();
        std::fs::write(dir.join("con.gguf"), b"c").unwrap();
        std::fs::write(dir.join("voice.gguf"), b"v").unwrap();
        (
            ModelSpec::new(dir.join("base.gguf")),
            vec![
                AdapterSpec::new(AdapterRole::Behavioral, dir.join("beh.gguf")),
                AdapterSpec::new(AdapterRole::Contract, dir.join("con.gguf")),
                AdapterSpec::new(AdapterRole::Voice, dir.join("voice.gguf")),
            ],
        )
    }

    #[test]
    fn load_reports_full_stack_in_composition_order() {
        let dir = unique_dir("full");
        let (base, adapters) = full_stack(&dir);
        let backend = MockBackend::new();
        let handle = backend.load(LoadRequest::new(base, adapters)).unwrap();
        assert_eq!(
            handle.mounted_adapters(),
            &[AdapterRole::Behavioral, AdapterRole::Contract, AdapterRole::Voice]
        );
    }

    #[test]
    fn load_fails_closed_when_contract_adapter_missing() {
        let dir = unique_dir("missing-contract");
        std::fs::write(dir.join("base.gguf"), b"base").unwrap();
        std::fs::write(dir.join("beh.gguf"), b"b").unwrap();
        // contract adapter declared but not written.
        let req = LoadRequest::new(
            ModelSpec::new(dir.join("base.gguf")),
            vec![
                AdapterSpec::new(AdapterRole::Behavioral, dir.join("beh.gguf")),
                AdapterSpec::new(AdapterRole::Contract, dir.join("con.gguf")),
            ],
        );
        // Box<dyn EngineHandle> is not Debug, so match on the Result directly
        // rather than unwrap_err (which needs the Ok type to be Debug).
        let res = MockBackend::new().load(req);
        assert!(matches!(res, Err(EngineError::Missing { .. })), "expected Missing");
    }

    #[test]
    fn llama_template_streams_visible_tokens_no_think() {
        let dir = unique_dir("stream-llama");
        let (base, adapters) = full_stack(&dir);
        let backend = MockBackend::new();
        let mut handle = backend
            .load(LoadRequest::new(base, adapters).with_template(ChatTemplate::Llama3))
            .unwrap();
        let mut session = handle.session(SessionConfig::default()).unwrap();
        let mut sink = CollectSink::new();
        let stats = session
            .stream(&[ChatMessage::user("hi there")], &mut sink)
            .unwrap();
        assert_eq!(sink.text, "tok0tok1tok2tok3tok4");
        assert_eq!(stats.stop, StopReason::Eos);
        assert_eq!(stats.prompt_tokens, 2);
    }

    #[test]
    fn chatml_template_strips_think_from_the_stream() {
        let dir = unique_dir("stream-chatml");
        let (base, adapters) = full_stack(&dir);
        let backend = MockBackend::new();
        let mut handle = backend
            .load(LoadRequest::new(base, adapters).with_template(ChatTemplate::ChatMl))
            .unwrap();
        let mut session = handle.session(SessionConfig::default()).unwrap();
        let mut sink = CollectSink::new();
        session.stream(&[ChatMessage::user("hi")], &mut sink).unwrap();
        // The <think> block the mock emits is stripped in the stream path; the
        // sink only ever saw the visible tokens.
        assert_eq!(sink.text, "tok0tok1tok2tok3tok4");
    }

    #[test]
    fn max_tokens_limit_stops_generation() {
        let dir = unique_dir("maxtok");
        let (base, adapters) = full_stack(&dir);
        let backend = MockBackend::new();
        let mut handle = backend
            .load(LoadRequest::new(base, adapters).with_template(ChatTemplate::Llama3))
            .unwrap();
        let mut cfg = SessionConfig::default();
        cfg.sampling.max_tokens = 2;
        let mut session = handle.session(cfg).unwrap();
        let mut sink = CollectSink::new();
        let stats = session.stream(&[ChatMessage::user("hi")], &mut sink).unwrap();
        assert_eq!(stats.stop, StopReason::MaxTokens);
        assert_eq!(sink.text, "tok0tok1");
    }

    #[test]
    fn sink_break_cancels_generation() {
        let dir = unique_dir("cancel");
        let (base, adapters) = full_stack(&dir);
        let backend = MockBackend::new();
        let mut handle = backend
            .load(LoadRequest::new(base, adapters).with_template(ChatTemplate::Llama3))
            .unwrap();
        let mut session = handle.session(SessionConfig::default()).unwrap();
        let mut sink = CollectSink::cancel_after(1);
        let stats = session.stream(&[ChatMessage::user("hi")], &mut sink).unwrap();
        assert_eq!(stats.stop, StopReason::Cancelled);
        assert_eq!(sink.text, "tok0");
    }
}
