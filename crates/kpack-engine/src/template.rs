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
}

/// A chat role — the three the prompt contract and conversation store use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
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
pub struct ThinkStripper {
    state: StripState,
    /// Buffered bytes while `Deciding` (a partial `<think>` prefix) or while
    /// `Inside` (a partial `</think>` suffix could straddle two pushes).
    buffer: String,
}

impl ThinkStripper {
    /// `enabled` false (Llama / any non-think model) starts in pass-through —
    /// the "never on a model that does not emit it" half of the rule.
    pub fn new(enabled: bool) -> Self {
        ThinkStripper {
            state: if enabled { StripState::Deciding } else { StripState::PassThrough },
            buffer: String::new(),
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

    #[test]
    fn partial_suffix_len_finds_split_boundary() {
        // Longest suffix of "foo</th" that is a prefix of "</think>" is "</th".
        assert_eq!(partial_suffix_len("foo</th", "</think>"), 4);
        // No overlap -> retain nothing.
        assert_eq!(partial_suffix_len("plain text", "</think>"), 0);
    }
}
