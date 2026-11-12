//! Chat-template resolution and the Qwen-only think-strip — the second thing
//! the sidecar gave the engine for free (via `--jinja`, which read the
//! template embedded in the GGUF) that the in-process engine must reproduce
//! itself (spec §8).
//!
//! Two rules the spec is emphatic about:
//!   - **The template is per-model, never hardcoded.** The floor hero is
//!     Llama-3.2-1B — a *non-ChatML* family. ChatML applies to the Qwen tiers.
//!     [`ChatTemplate::Auto`] is the shipping default: the native backend reads
//!     the GGUF's own template via `LlamaModel::chat_template` + `apply_chat_template`.
//!     [`ChatTemplate::Llama3`] / [`ChatTemplate::ChatMl`] exist for tests and
//!     for the case where a GGUF ships without an embedded template.
//!   - **Think-strip is Qwen-only and start-of-turn-only.** Strip a leading
//!     `<think>…</think>` block at the very start of the assistant turn, NEVER
//!     globally, and NEVER on a model that does not emit it (Llama). Once any
//!     visible text has streamed, a later literal `<think>` is content and is
//!     forwarded verbatim.
//!
//! ## The think policy (Phase 1c, task A2) — why stripping was never enough
//! Every gate number this programme has taken was served with Qwen3's thinking
//! **disabled**: `gate_on_pod.py` runs `llama-server` with
//! `--chat-template-kwargs '{"enable_thinking":false}'`, and the DPO trainer
//! renders through the same kwarg. Qwen3's Jinja template reads it and places a
//! PRE-CLOSED block — [`QWEN3_THINK_BLOCK`] — in the assistant prefix, so
//! decoding starts after `</think>` and the model physically cannot open one.
//!
//! The device cannot pass that kwarg. `llama_chat_apply_template` is the C
//! template API and takes no Jinja kwargs at all, so before this task the phone
//! served the *thinking* prompt: out of distribution relative to both the
//! training data and the gate, and sharing the catalog's 320-token budget
//! between the reasoning and the answer. Stripping the block afterwards fixed
//! the display and none of that.
//!
//! So the engine appends the block itself, in exactly ONE place
//! ([`ChatTemplate::finish_generation_prompt`]), and
//! [`ChatTemplate::rendered_prompt_sha256`] makes the result checkable against
//! the pod's own rendering rather than merely asserted. The stripper stays on
//! as a no-op safety net: a model that emits a block anyway is still stripped.

use sha2::{Digest, Sha256};

/// Qwen3's pre-closed thinking block: what `enable_thinking=False` moves into
/// the prompt. Byte-identical to `THINK_BLOCK` in the pod's
/// `pipeline/pod/train_dpo_generic.py`, which copied it from
/// `train_adapter_gkd.py`'s note — the one whose closing line is "Training and
/// inference must agree."
pub const QWEN3_THINK_BLOCK: &str = "<think>\n\n</think>\n\n";

/// What a family requires in the assistant prefix beyond what its chat template
/// emits. This is the whole of the device/pod rendering difference, named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkPolicy {
    /// Nothing is appended. The template's generation prompt is the prompt.
    Off,
    /// Qwen3 under `enable_thinking=false`: [`QWEN3_THINK_BLOCK`] follows the
    /// `<|im_start|>assistant\n` header, so the block is already closed before
    /// the model's first token.
    PreClosed,
}

/// Which chat-template family a model speaks, and (derived) whether its stream
/// carries a start-of-turn `<think>` block to strip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatTemplate {
    /// Read the template embedded in the GGUF (the sidecar's `--jinja`
    /// behavior). The shipping default — the model declares its own family.
    Auto,
    /// Llama-3.2 family (floor hero). Non-ChatML; emits no `<think>` block.
    Llama3,
    /// ChatML family (Qwen tiers). Emits a start-of-turn `<think>` block that
    /// the runtime strips.
    ChatMl,
}

impl ChatTemplate {
    /// Whether this family emits a start-of-turn `<think>` block the runtime
    /// strips. ChatML(Qwen)-only. `Auto` conservatively returns `false`: when
    /// the template comes from the GGUF, the caller sets the concrete family
    /// (or a per-model catalog flag) if think-strip is wanted — the engine
    /// never strips speculatively on a model that may not emit `<think>`.
    pub fn strips_think(self) -> bool {
        matches!(self, ChatTemplate::ChatMl)
    }

    /// What this family needs appended to its generation prompt. `Auto` is
    /// `Off` for the same reason [`strips_think`](Self::strips_think) is false:
    /// when the template comes from the GGUF the engine does not know the
    /// family, and appending a Qwen block to a model that does not speak Qwen
    /// would corrupt every prompt it ever renders. A model that wants the
    /// pre-closed block declares `chatTemplate: "qwen"` in the catalog.
    pub fn think_policy(self) -> ThinkPolicy {
        match self {
            ChatTemplate::ChatMl => ThinkPolicy::PreClosed,
            ChatTemplate::Auto | ChatTemplate::Llama3 => ThinkPolicy::Off,
        }
    }

    /// **The one place the decision lives.** Given what the model's own chat
    /// template produced for a generation prompt, return the exact bytes to
    /// tokenise.
    ///
    /// Idempotent by construction: a template that already ended in the block
    /// does not get a second one. Two blocks is a prompt shape neither the gate
    /// nor the training corpus has ever contained, and llama.cpp's template
    /// detection is a string match on someone else's Jinja — not something to
    /// bet the prompt on being stable across a llama.cpp bump.
    pub fn finish_generation_prompt(self, applied: &str) -> String {
        match self.think_policy() {
            ThinkPolicy::Off => applied.to_string(),
            ThinkPolicy::PreClosed if applied.ends_with(QWEN3_THINK_BLOCK) => applied.to_string(),
            ThinkPolicy::PreClosed => {
                let mut out = String::with_capacity(applied.len() + QWEN3_THINK_BLOCK.len());
                out.push_str(applied);
                out.push_str(QWEN3_THINK_BLOCK);
                out
            }
        }
    }

    /// Render `messages` into a generation prompt **without the model** —
    /// `Some` for [`ChatTemplate::ChatMl`], `None` for the families whose bytes
    /// only their own GGUF can produce.
    ///
    /// The ChatML arm reproduces llama.cpp's built-in `chatml` format, which is
    /// what `llama_chat_apply_template` actually applies for a Qwen3 GGUF: the
    /// C API detects the family from the embedded template string and renders
    /// its own, it does not run the Jinja. That equivalence is not asserted here
    /// in a comment — `tests/qwen3_render_parity.rs` checks these bytes against
    /// a fixture produced by the real Qwen3 tokenizer, and the `real`-gated test
    /// beside it checks them against llama.cpp itself.
    pub fn render_prompt(self, messages: &[ChatMessage]) -> Option<String> {
        if self != ChatTemplate::ChatMl {
            return None;
        }
        let mut out = String::new();
        for m in messages {
            out.push_str("<|im_start|>");
            out.push_str(role_name(m.role));
            out.push('\n');
            out.push_str(&m.content);
            out.push_str("<|im_end|>\n");
        }
        out.push_str("<|im_start|>assistant\n");
        Some(self.finish_generation_prompt(&out))
    }

    /// sha256 (hex) of the exact prompt bytes this family renders for
    /// `messages` — the parity check the plan review asked for, because a
    /// `promptFingerprint` match certifies the system prompt TEXT and nothing
    /// about the template wrapped around it. `None` where
    /// [`render_prompt`](Self::render_prompt) is `None`.
    pub fn rendered_prompt_sha256(self, messages: &[ChatMessage]) -> Option<String> {
        self.render_prompt(messages).map(|p| prompt_sha256(&p))
    }
}

/// The role name both shipping template families use in their embedded chat
/// templates. One definition, used by the pure ChatML renderer here and by the
/// native backend's `LlamaChatMessage` construction — a second copy is how the
/// two renderings start to disagree about a `tool` turn.
pub fn role_name(role: Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        // A GGUF whose template lacks a `tool` branch still renders the turn
        // (llama.cpp falls back rather than erroring), so a tool result is
        // always visible to the model even on a model we have not profiled.
        Role::Tool => "tool",
    }
}

/// sha256 (hex) of a rendered prompt's bytes. Same digest and same hex spelling
/// as [`crate::adapter::file_sha256`], so a prompt sha and an artifact sha can
/// be read side by side in one transcript.
pub fn prompt_sha256(prompt: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(prompt.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A chat role — the three the prompt contract and conversation store use,
/// plus `Tool` for tool-call results fed back into the conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
    /// The result of a tool call, appended to the conversation so the model can
    /// use it (or, when the tool errored, correct itself) on the next round.
    ///
    /// Both shipping template families render this as a `tool` turn: Llama-3.2
    /// via `<|start_header_id|>ipython<|end_header_id|>`-equivalent handling in
    /// its embedded template, and ChatML via `<|im_start|>tool`. Models whose
    /// GGUF template does not know the role degrade to a plainly-labelled turn
    /// rather than failing, which is why the loop never depends on the tool
    /// result being machine-parsed by the model.
    Tool,
}

/// One chat turn. The native backend maps these to `llama_cpp_2`'s
/// `LlamaChatMessage` and renders them through the model's own template; the
/// mock backend reads them directly.
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        ChatMessage { role: Role::System, content: content.into() }
    }
    pub fn user(content: impl Into<String>) -> Self {
        ChatMessage { role: Role::User, content: content.into() }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        ChatMessage { role: Role::Assistant, content: content.into() }
    }
    pub fn tool(content: impl Into<String>) -> Self {
        ChatMessage { role: Role::Tool, content: content.into() }
    }
}

const THINK_OPEN: &str = "<think>";
const THINK_CLOSE: &str = "</think>";

#[derive(Debug, PartialEq)]
enum StripState {
    /// Start of turn: buffering, deciding whether a `<think>` block opens.
    Deciding,
    /// Inside a `<think>` block: suppressing until `</think>`.
    Inside,
    /// A think block has been consumed, or the turn opened with visible text —
    /// everything from here streams verbatim.
    PassThrough,
}

/// Streaming, start-of-turn `<think>…</think>` stripper (spec §8).
///
/// Fed the model's streamed token text piece by piece; returns only the
/// visible portion to forward to the UI. The block boundary can fall anywhere,
/// including mid-token across two pushes, so the decision is buffered until
/// there is enough text to be sure. Leading ASCII whitespace before `<think>`
/// is tolerated (some templates emit a newline first) and is dropped together
/// with the block; if the turn's first non-whitespace text is anything other
/// than `<think>`, the block never opens and all buffered text flushes.
///
/// When `enabled` is false (Llama / any non-think model) it is a pure
/// pass-through — the "never on a model that does not emit it" half of the rule.
///
/// **Every stream must end in [`finish`](Self::finish).** `push` alone can hold
/// bytes back indefinitely — that is what makes the boundary decision correct —
/// so a caller that stops pushing and never flushes silently loses whatever was
/// buffered. Before Phase 1c that is precisely what happened to a reply cut
/// short inside `<think>`: the buffer was dropped and the turn returned an
/// empty string, indistinguishable from a model with nothing to say.
pub struct ThinkStripper {
    state: StripState,
    /// Buffered bytes while `Deciding` (a partial `<think>` prefix) or while
    /// `Inside` (a partial `</think>` suffix could straddle two pushes).
    buffer: String,
    /// Bytes of think-content already discarded in `Inside`. Reported by
    /// [`finish`](Self::finish) so a truncated turn can say how much reasoning
    /// it spent the token budget on — which is the number that says whether
    /// 320 tokens is the problem.
    suppressed: usize,
}

/// How a stream ended, from the stripper's point of view.
///
/// Returned by [`ThinkStripper::finish`]. The variants are not a formality: the
/// difference between them is the difference between "the model said nothing"
/// and "the model was cut off mid-thought", and the product must not render
/// those the same way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StripFinish {
    /// The turn ended outside any think block. The payload is the residual
    /// buffered text to forward — usually empty, non-empty when the stream
    /// stopped mid-decision (`"<thi"`) or on whitespace alone.
    Visible(String),
    /// The turn ended INSIDE an unclosed `<think>` block. Every byte the model
    /// produced was suppressed, so there is no visible text and there never
    /// will be: this emptiness is a truncation, not a silence. `suppressed` is
    /// how many bytes of hidden reasoning were consumed getting there.
    TruncatedInThink { suppressed: usize },
}

impl StripFinish {
    /// Whether the stream died inside a think block.
    pub fn truncated_in_think(&self) -> bool {
        matches!(self, StripFinish::TruncatedInThink { .. })
    }

    /// The residual visible text, empty for a truncation.
    pub fn text(&self) -> &str {
        match self {
            StripFinish::Visible(t) => t,
            StripFinish::TruncatedInThink { .. } => "",
        }
    }
}

impl ThinkStripper {
    /// `enabled` false (Llama / any non-think model) starts in pass-through —
    /// the "never on a model that does not emit it" half of the rule.
    pub fn new(enabled: bool) -> Self {
        ThinkStripper {
            state: if enabled { StripState::Deciding } else { StripState::PassThrough },
            buffer: String::new(),
            suppressed: 0,
        }
    }

    /// End of stream: flush whatever `push` is still holding, and say whether
    /// the turn died inside a think block.
    ///
    /// Idempotent — a second call reports `Visible("")`, so a caller that
    /// finishes defensively cannot emit the truncation notice twice.
    pub fn finish(&mut self) -> StripFinish {
        match self.state {
            // Nothing is ever held in pass-through; the take is for the
            // idempotence, not for the bytes.
            StripState::PassThrough => StripFinish::Visible(std::mem::take(&mut self.buffer)),
            // The block never opened, so those bytes were content. Whitespace
            // held in case `<think>` followed it, a partial `"<thi"` that never
            // resolved — both are what the model actually said, and the
            // stripper does not claim a truncation it cannot prove.
            StripState::Deciding => {
                self.state = StripState::PassThrough;
                StripFinish::Visible(std::mem::take(&mut self.buffer))
            }
            StripState::Inside => {
                let suppressed = self.suppressed + self.buffer.len();
                self.buffer.clear();
                self.suppressed = 0;
                self.state = StripState::PassThrough;
                StripFinish::TruncatedInThink { suppressed }
            }
        }
    }

    /// Push a streamed text chunk; returns the visible text to forward.
    pub fn push(&mut self, chunk: &str) -> String {
        if self.state == StripState::PassThrough {
            return chunk.to_string();
        }
        self.buffer.push_str(chunk);
        match self.state {
            StripState::Deciding => self.decide(),
            StripState::Inside => self.consume_inside(),
            StripState::PassThrough => unreachable!(),
        }
    }

    /// In `Deciding`: is the buffer (past any leading whitespace) opening a
    /// `<think>` block, definitely not, or still ambiguous (a partial prefix)?
    fn decide(&mut self) -> String {
        let trimmed = self.buffer.trim_start();
        let lead_ws = self.buffer.len() - trimmed.len();

        if trimmed.is_empty() {
            // Only whitespace so far — keep waiting (hold the whitespace; it is
            // dropped if a block opens, flushed if not).
            return String::new();
        }
        if trimmed.starts_with(THINK_OPEN) {
            // Block opens. Drop the leading whitespace + the tag, keep the rest
            // for the Inside scan.
            let rest = trimmed[THINK_OPEN.len()..].to_string();
            self.buffer = rest;
            self.state = StripState::Inside;
            self.suppressed = 0;
            return self.consume_inside();
        }
        if THINK_OPEN.starts_with(trimmed) {
            // Still a viable partial prefix of "<think>" (e.g. "<thi") — wait
            // for more before deciding.
            let _ = lead_ws;
            return String::new();
        }
        // Definitely not a think block — this turn opened with visible text.
        // Flush everything buffered (including any leading whitespace) verbatim
        // and pass through from here on.
        self.state = StripState::PassThrough;
        std::mem::take(&mut self.buffer)
    }

    /// In `Inside`: suppress until `</think>` closes, then emit whatever
    /// follows and switch to pass-through. A single leading newline right after
    /// `</think>` (common in ChatML think templates) is swallowed too.
    fn consume_inside(&mut self) -> String {
        if let Some(idx) = self.buffer.find(THINK_CLOSE) {
            let after = self.buffer[idx + THINK_CLOSE.len()..].to_string();
            let after = after.strip_prefix('\n').unwrap_or(&after).to_string();
            self.buffer.clear();
            self.state = StripState::PassThrough;
            after
        } else {
            // Close tag not seen yet. Retain only enough of a possible partial
            // "</think>" suffix to catch a boundary split across pushes; the
            // rest is think-content and stays suppressed.
            let keep = partial_suffix_len(&self.buffer, THINK_CLOSE);
            let drop_to = self.buffer.len() - keep;
            self.buffer.drain(..drop_to);
            self.suppressed += drop_to;
            String::new()
        }
    }
}

/// Length of the longest suffix of `s` that is a proper prefix of `needle` —
/// how many trailing bytes to retain so a `needle` split across two pushes is
/// still detected on the next push.
fn partial_suffix_len(s: &str, needle: &str) -> usize {
    let max = needle.len().min(s.len());
    for len in (1..=max).rev() {
        let suffix = &s[s.len() - len..];
        if needle.starts_with(suffix) {
            return len;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stream `pieces` through a stripper and concatenate the visible output.
    fn run(enabled: bool, pieces: &[&str]) -> String {
        let mut s = ThinkStripper::new(enabled);
        let mut out = String::new();
        for p in pieces {
            out.push_str(&s.push(p));
        }
        out
    }

    #[test]
    fn chatml_family_strips_think_llama_does_not() {
        assert!(ChatTemplate::ChatMl.strips_think());
        assert!(!ChatTemplate::Llama3.strips_think());
        assert!(!ChatTemplate::Auto.strips_think());
    }

    #[test]
    fn strips_leading_think_block_single_chunk() {
        assert_eq!(run(true, &["<think>reasoning here</think>hello"]), "hello");
    }

    #[test]
    fn strips_think_block_streamed_char_by_char() {
        let pieces: Vec<String> = "<think>abc</think>visible"
            .chars()
            .map(|c| c.to_string())
            .collect();
        let refs: Vec<&str> = pieces.iter().map(|s| s.as_str()).collect();
        assert_eq!(run(true, &refs), "visible");
    }

    #[test]
    fn strips_think_block_split_across_pushes() {
        // Boundary of both tags falls between pushes.
        assert_eq!(run(true, &["<thi", "nk>hidden</thi", "nk>shown"]), "shown");
    }

    #[test]
    fn swallows_single_newline_after_close_tag() {
        assert_eq!(run(true, &["<think>x</think>\nafter"]), "after");
    }

    #[test]
    fn tolerates_leading_whitespace_before_think() {
        assert_eq!(run(true, &["\n<think>x</think>y"]), "y");
    }

    #[test]
    fn no_think_block_passes_through_verbatim() {
        assert_eq!(run(true, &["hello ", "world"]), "hello world");
    }

    #[test]
    fn think_after_visible_text_is_literal_not_stripped() {
        // Start-of-turn only: once "hi " is visible we are pass-through, so a
        // later <think> is content and streams verbatim.
        assert_eq!(
            run(true, &["hi ", "<think>not stripped</think> end"]),
            "hi <think>not stripped</think> end"
        );
    }

    #[test]
    fn disabled_stripper_never_strips() {
        // Llama / non-think model: the block is content and streams verbatim.
        assert_eq!(
            run(false, &["<think>keep me</think>rest"]),
            "<think>keep me</think>rest"
        );
    }

    // ---- finish(): the flush the stripper never had (Phase 1c A2) --------
    //
    // Until this existed, a run that ended inside `<think>` discarded its
    // buffer and returned an EMPTY STRING — a device-only catastrophic failure
    // mode that no test covered and that no caller could tell apart from a
    // model with nothing to say.

    #[test]
    fn a_stream_that_ends_inside_a_think_block_reports_truncation() {
        let mut s = ThinkStripper::new(true);
        assert_eq!(s.push("<think>I am still reasoning about"), "");
        assert_eq!(
            s.finish(),
            StripFinish::TruncatedInThink {
                suppressed: "I am still reasoning about".len()
            }
        );
    }

    #[test]
    fn truncation_is_reported_even_when_the_block_only_just_opened() {
        // The budget ran out on the token that opened the block. Nothing was
        // suppressed, and the answer is still empty for a reason the caller
        // must be able to state.
        let mut s = ThinkStripper::new(true);
        assert_eq!(s.push("<think>"), "");
        assert_eq!(s.finish(), StripFinish::TruncatedInThink { suppressed: 0 });
    }

    #[test]
    fn a_closed_block_followed_by_nothing_is_not_truncation() {
        let mut s = ThinkStripper::new(true);
        assert_eq!(s.push("<think>done</think>"), "");
        assert_eq!(s.finish(), StripFinish::Visible(String::new()));
    }

    #[test]
    fn finish_flushes_a_turn_that_ended_mid_decision() {
        // "<thi" is a viable prefix of "<think>", so `push` held it. The turn
        // ended without resolving the ambiguity: the block never opened, so
        // those bytes were content and the caller gets them rather than "".
        let mut s = ThinkStripper::new(true);
        assert_eq!(s.push("<thi"), "");
        assert_eq!(s.finish(), StripFinish::Visible("<thi".to_string()));
    }

    #[test]
    fn finish_flushes_a_whitespace_only_turn() {
        // Leading whitespace is held in case `<think>` follows it. It did not,
        // so the whitespace was the whole reply.
        let mut s = ThinkStripper::new(true);
        assert_eq!(s.push("  \n"), "");
        assert_eq!(s.finish(), StripFinish::Visible("  \n".to_string()));
    }

    #[test]
    fn finish_after_visible_text_has_nothing_left_to_flush() {
        let mut s = ThinkStripper::new(true);
        assert_eq!(s.push("hello"), "hello");
        assert_eq!(s.finish(), StripFinish::Visible(String::new()));
    }

    #[test]
    fn finish_is_idempotent() {
        let mut s = ThinkStripper::new(true);
        s.push("<think>x");
        assert!(s.finish().truncated_in_think());
        // A second call must not re-report the truncation: a caller that
        // finishes defensively would otherwise print the notice twice.
        assert_eq!(s.finish(), StripFinish::Visible(String::new()));
    }

    #[test]
    fn a_disabled_stripper_never_reports_truncation() {
        // Llama / any non-think model: `<think>` is content, so a stream that
        // ends inside one ended inside ordinary text.
        let mut s = ThinkStripper::new(false);
        assert_eq!(
            s.push("<think>this is just prose"),
            "<think>this is just prose"
        );
        assert_eq!(s.finish(), StripFinish::Visible(String::new()));
    }

    // ---- the think policy: ONE place decides, and the two agree -----------

    #[test]
    fn the_chatml_family_renders_the_pre_closed_block_and_the_others_do_not() {
        assert_eq!(ChatTemplate::ChatMl.think_policy(), ThinkPolicy::PreClosed);
        assert_eq!(ChatTemplate::Llama3.think_policy(), ThinkPolicy::Off);
        assert_eq!(ChatTemplate::Auto.think_policy(), ThinkPolicy::Off);
    }

    #[test]
    fn the_policy_and_the_stripper_agree_on_every_family() {
        // The stripper is a no-op safety net for exactly the family whose
        // prompt already closes the block. If these two ever disagreed, one of
        // them would be acting on a model the other says does not think.
        for t in [
            ChatTemplate::Auto,
            ChatTemplate::Llama3,
            ChatTemplate::ChatMl,
        ] {
            assert_eq!(
                t.strips_think(),
                t.think_policy() == ThinkPolicy::PreClosed,
                "{t:?}: strips_think and think_policy disagree"
            );
        }
    }

    #[test]
    fn finish_generation_prompt_appends_the_block_for_chatml_only() {
        let applied = "<|im_start|>assistant\n";
        assert_eq!(
            ChatTemplate::ChatMl.finish_generation_prompt(applied),
            "<|im_start|>assistant\n<think>\n\n</think>\n\n"
        );
        assert_eq!(
            ChatTemplate::Llama3.finish_generation_prompt(applied),
            applied
        );
        assert_eq!(
            ChatTemplate::Auto.finish_generation_prompt(applied),
            applied
        );
    }

    #[test]
    fn finish_generation_prompt_does_not_double_a_block_the_template_already_rendered() {
        // A GGUF whose embedded template llama.cpp resolves to something that
        // already closes the block must not get a second one — two blocks is a
        // prompt shape neither the gate nor the training data has ever seen.
        let already = format!("<|im_start|>assistant\n{QWEN3_THINK_BLOCK}");
        assert_eq!(
            ChatTemplate::ChatMl.finish_generation_prompt(&already),
            already
        );
    }

    #[test]
    fn only_chatml_can_be_rendered_without_the_model() {
        let msgs = [ChatMessage::user("hi")];
        assert!(ChatTemplate::ChatMl.render_prompt(&msgs).is_some());
        // Llama3 and Auto need the model's own template; the engine will not
        // guess at bytes it cannot prove.
        assert!(ChatTemplate::Llama3.render_prompt(&msgs).is_none());
        assert!(ChatTemplate::Auto.render_prompt(&msgs).is_none());
    }

    #[test]
    fn the_rendered_sha_is_the_sha_of_the_rendered_bytes() {
        let msgs = [ChatMessage::system("S"), ChatMessage::user("U")];
        let rendered = ChatTemplate::ChatMl.render_prompt(&msgs).unwrap();
        assert_eq!(
            ChatTemplate::ChatMl.rendered_prompt_sha256(&msgs).unwrap(),
            prompt_sha256(&rendered)
        );
        // Known-answer, so a refactor of the hex formatting cannot pass by
        // agreeing with itself.
        assert_eq!(
            prompt_sha256(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn every_role_renders_with_its_chatml_name() {
        let msgs = [
            ChatMessage::system("s"),
            ChatMessage::user("u"),
            ChatMessage::assistant("a"),
            ChatMessage::tool("t"),
        ];
        assert_eq!(
            ChatTemplate::ChatMl.render_prompt(&msgs).unwrap(),
            "<|im_start|>system\ns<|im_end|>\n\
             <|im_start|>user\nu<|im_end|>\n\
             <|im_start|>assistant\na<|im_end|>\n\
             <|im_start|>tool\nt<|im_end|>\n\
             <|im_start|>assistant\n<think>\n\n</think>\n\n"
        );
    }

    #[test]
    fn partial_suffix_len_finds_split_boundary() {
        // Longest suffix of "foo</th" that is a prefix of "</think>" is "</th".
        assert_eq!(partial_suffix_len("foo</th", "</think>"), 4);
        // No overlap -> retain nothing.
        assert_eq!(partial_suffix_len("plain text", "</think>"), 0);
    }
}
