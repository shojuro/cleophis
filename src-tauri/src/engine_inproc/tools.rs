//! Tool-call plumbing for the in-process engine.
//!
//! Desktop gets tool calling from `llama-server`: it passes an OpenAI-style
//! `tools` array, the server renders it through `--jinja`, and streamed deltas
//! arrive with structured `tool_calls`. In-process there is no server and
//! `llama_chat_apply_template` takes no tools argument, so both halves are ours:
//! the tool contract is injected into the prompt as a preamble, and calls are
//! recovered by parsing the model's generated *text* in whatever shape its
//! family emits.
//!
//! # The registry is closed (security review, M2)
//!
//! [`Tool`] is an enum, not a name→handler map, and [`dispatch`] matches on it.
//! A name the model invents cannot reach any handler: it fails
//! [`Tool::from_name`] and comes back as a tool *error result*, which is fed to
//! the model exactly like a calc error so it can correct itself. Adding a tool
//! requires adding a variant and a dispatch arm — there is no registration path
//! a prompt can reach, and no string that becomes a callable by accident.
//!
//! This matters more here than on desktop. A tool name arriving from a model is
//! attacker-influenceable input whenever the conversation contains text the user
//! did not write (a pack, a pasted document), and in a Tauri WebView a
//! dispatch-by-name table is one step from the whole `invoke()` surface.

// Phase 1.3 lands this module complete and tested; 1.4's generation loop is its
// first production caller, so in a non-test build every item here is currently
// dead. Remove this attribute when `chat_stream` lands — anything still dead
// then is genuinely unused rather than merely early. Scoped to this one pure
// module on purpose: a crate-wide allow would have hidden the real findings the
// aarch64 build surfaced in 1.1 and 1.2.
#![allow(dead_code)]

use serde_json::Value;

/// The prompt shape a model family uses for tool calls.
///
/// Deliberately independent of `kpack_engine::ChatTemplate`: kpack-engine is an
/// Android-only dependency, and tying this module to it would mean the closed-
/// registry tests below could never run, since the desktop suite is the only
/// place tests actually execute. `engine_inproc` maps one onto the other at the
/// seam, which is also the honest place for that mapping to live — the tool
/// wire format is a property of the family, not of the template engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolFamily {
    /// Qwen3 / ChatML: a fenced `<tool_call>{…}</tool_call>` block.
    ChatMl,
    /// Llama-3.2: a bare JSON object using a `parameters` key.
    JsonFunction,
}

/// Every tool the model may call. Closed by construction — see the module doc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tool {
    /// The arithmetic evaluator (`kpack-calc`), same one the desktop tool loop
    /// calls through the `calc` command.
    Calc,
}

impl Tool {
    /// The only place a name becomes a callable tool. Unknown names return
    /// `None` and are never dispatched.
    fn from_name(name: &str) -> Option<Tool> {
        match name {
            "calc" => Some(Tool::Calc),
            _ => None,
        }
    }
}

/// One tool call recovered from generated text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolCall {
    /// Correlates the call with its result turn. The families we support do not
    /// emit ids, so these are assigned positionally.
    pub id: String,
    pub name: String,
    /// The raw JSON arguments object, as the model wrote it.
    pub arguments: String,
}

/// What running a tool produced. An error is NOT a failure of the turn: it
/// becomes the tool result so the model can self-correct, exactly as the
/// desktop loop does (`calc-loop.js`: "error becomes the tool result").
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ToolOutcome {
    Ok(String),
    Err(String),
}

impl ToolOutcome {
    /// The text handed back to the model as the tool turn's content. Both
    /// variants render the same way — the model is told what happened in
    /// prose, not given a status code to branch on.
    pub fn as_content(&self) -> &str {
        match self {
            ToolOutcome::Ok(s) | ToolOutcome::Err(s) => s,
        }
    }

    pub fn is_err(&self) -> bool {
        matches!(self, ToolOutcome::Err(_))
    }
}

/// Run a recovered call. The single dispatch point, and the only caller of
/// [`Tool::from_name`].
pub(crate) fn dispatch(call: &ToolCall) -> ToolOutcome {
    let Some(tool) = Tool::from_name(&call.name) else {
        // Never dispatched, and deliberately phrased so the model's next round
        // has enough to correct itself rather than repeating the invention.
        return ToolOutcome::Err(format!(
            "There is no tool named {:?}. The only available tool is \"calc\".",
            call.name
        ));
    };

    match tool {
        Tool::Calc => {
            let expression = match argument_str(&call.arguments, "expression") {
                Some(e) => e,
                None => {
                    return ToolOutcome::Err(
                        "calc requires an \"expression\" argument, for example \
                         {\"expression\": \"(3/4)*88\"}."
                            .to_string(),
                    )
                }
            };
            match kpack_calc::evaluate_display(&expression) {
                Ok(display) => ToolOutcome::Ok(display),
                Err(e) => ToolOutcome::Err(e.to_string()),
            }
        }
    }
}

/// Pull a string argument out of the model's JSON arguments object, tolerating
/// the two shapes models actually emit: a JSON object, or a JSON *string*
/// containing an object (which OpenAI-style `arguments` is, and which some
/// fine-tunes imitate).
fn argument_str(arguments: &str, key: &str) -> Option<String> {
    let parsed: Value = serde_json::from_str(arguments).ok()?;
    let obj = match &parsed {
        Value::String(inner) => serde_json::from_str::<Value>(inner).ok()?,
        other => other.clone(),
    };
    match obj.get(key)? {
        // A number is accepted so `{"expression": 42}` is not a hard failure.
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// The tool contract, injected into the system prompt.
///
/// `llama_chat_apply_template` has no tools parameter, so unlike desktop — where
/// `llama-server` renders the `tools` array itself — the calling convention has
/// to be stated in the prompt, in the shape the family actually emits.
pub(crate) fn tools_preamble(family: ToolFamily) -> &'static str {
    match family {
        // Qwen3 / ChatML emit a fenced block containing a JSON object.
        ToolFamily::ChatMl => {
            "\n\nYou can call one tool:\n\
             calc(expression) — evaluates an arithmetic expression and returns \
             the result. Use it for any arithmetic rather than computing in your \
             head.\n\
             To call it, emit exactly:\n\
             <tool_call>\n\
             {\"name\": \"calc\", \"arguments\": {\"expression\": \"(3/4)*88\"}}\n\
             </tool_call>\n\
             Emit nothing else on those lines. The result is returned to you as a \
             tool message; then answer normally."
        }
        // Llama-3.2's function-calling form is a bare JSON object with
        // "parameters" rather than "arguments".
        ToolFamily::JsonFunction => {
            "\n\nYou can call one tool:\n\
             calc(expression) — evaluates an arithmetic expression and returns \
             the result. Use it for any arithmetic rather than computing in your \
             head.\n\
             To call it, reply with ONLY this JSON object and nothing else:\n\
             {\"name\": \"calc\", \"parameters\": {\"expression\": \"(3/4)*88\"}}\n\
             The result is returned to you as a tool message; then answer normally."
        }
    }
}

/// Recover tool calls from a completed turn's visible text.
///
/// Both shapes are always accepted regardless of the declared family: the
/// preamble asks for one, but a fine-tune may answer in the other, and a missed
/// call is a wrong answer to the user. Being permissive here is safe precisely
/// because [`dispatch`] is strict — a parsed call still has to name a tool in
/// the closed registry to run.
pub(crate) fn parse_tool_calls(text: &str) -> Vec<ToolCall> {
    let mut calls = parse_fenced(text);
    if calls.is_empty() {
        calls = parse_bare_json(text);
    }
    for (i, call) in calls.iter_mut().enumerate() {
        call.id = format!("call_{i}");
    }
    calls
}

const FENCE_OPEN: &str = "<tool_call>";
const FENCE_CLOSE: &str = "</tool_call>";

/// ChatML/Qwen3: one or more `<tool_call>{…}</tool_call>` blocks.
fn parse_fenced(text: &str) -> Vec<ToolCall> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(FENCE_OPEN) {
        let after = &rest[start + FENCE_OPEN.len()..];
        let Some(end) = after.find(FENCE_CLOSE) else {
            break;
        };
        if let Some(call) = call_from_json(after[..end].trim()) {
            out.push(call);
        }
        rest = &after[end + FENCE_CLOSE.len()..];
    }
    out
}

/// Llama-3.2: the turn is a bare JSON object. Scanning for the first balanced
/// `{…}` rather than trusting the whole turn to be JSON, since models routinely
/// wrap it in a markdown fence or a sentence.
fn parse_bare_json(text: &str) -> Vec<ToolCall> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'{' {
            i += 1;
            continue;
        }
        if let Some(end) = balanced_end(text, i) {
            if let Some(call) = call_from_json(&text[i..=end]) {
                return vec![call];
            }
            i = end + 1;
        } else {
            break;
        }
    }
    Vec::new()
}

/// Index of the `}` closing the `{` at `start`, honouring nesting and skipping
/// braces inside JSON strings (and their escapes).
fn balanced_end(text: &str, start: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (i, c) in text.char_indices().skip_while(|(i, _)| *i < start) {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// Build a call from one JSON object, accepting `arguments` (ChatML) or
/// `parameters` (Llama-3.2) for the argument payload.
fn call_from_json(raw: &str) -> Option<ToolCall> {
    let v: Value = serde_json::from_str(raw).ok()?;
    let name = v.get("name")?.as_str()?.to_string();
    let args = v
        .get("arguments")
        .or_else(|| v.get("parameters"))
        .cloned()
        .unwrap_or(Value::Object(Default::default()));
    // Re-serialize so downstream sees one shape whichever key the model used.
    let arguments = match args {
        Value::String(s) => s,
        other => other.to_string(),
    };
    Some(ToolCall {
        id: String::new(),
        name,
        arguments,
    })
}

// ── streaming suppression ────────────────────────────────────────────────────
//
// Desktop never needed this: `llama-server` returned `tool_calls` structurally,
// separate from `content`, so the UI only ever saw prose. Recovering calls from
// generated text puts the syntax in the same stream the user is watching, and a
// naive forward renders `<tool_call>{…}</tool_call>` into the chat.
//
// The design's spine, in strict priority order — when these conflict, the higher
// one wins and the lower one is sacrificed deliberately:
//   1. Never block streaming unboundedly.
//   2. Never leak a full call body.
//   3. Minimize holdback latency.
//
// Which makes the failure modes acceptable by construction: a flushed
// false-positive buffer is slightly-late prose; a missed call is a
// wrong-but-safe answer (already accepted as the cost of permissive parsing);
// leaked syntax is ugly and harmless, and is only reachable through a bounded
// flush.
//
// Applies to tool-enabled turns ONLY. `chat_complete` paths (auto-title,
// analysis) never construct a suppressor and stream verbatim.

/// Cap on how much a suppressor will hold before giving up and flushing. Generous
/// for a calc call (whose whole body is a short JSON object) and small enough
/// that a model which opens a fence and never closes it cannot stall the stream —
/// priority 1 over priority 2.
const HOLDBACK_CAP: usize = 2048;

#[derive(Debug, PartialEq)]
enum SuppressState {
    /// Streaming prose, watching for an opening sentinel.
    Scan,
    /// Inside a tool call; discarding until it closes.
    Suppress,
    /// A decision was made that nothing further in this round is a call —
    /// everything from here streams verbatim with zero holdback.
    Live,
}

/// Streams a turn's text to the UI with tool-call syntax removed.
///
/// Feed every chunk through [`push`](ToolStream::push) and emit what it returns;
/// call [`finish`](ToolStream::finish) at end of turn. The complete raw text is
/// retained separately for [`parse_tool_calls`] — suppression is a *display*
/// concern and never changes what the loop dispatches.
pub(crate) struct ToolStream {
    family: ToolFamily,
    state: SuppressState,
    /// Text withheld pending a decision.
    held: String,
    /// The full raw turn, for the parser.
    full: String,
    /// JSON path: whether the first-non-whitespace decision has been made.
    decided: bool,
    /// JSON path: byte offset of the candidate object's opening brace.
    brace_start: usize,
}

impl ToolStream {
    pub fn new(family: ToolFamily) -> Self {
        ToolStream {
            family,
            state: SuppressState::Scan,
            held: String::new(),
            full: String::new(),
            decided: false,
            brace_start: 0,
        }
    }

    /// The complete turn text as the model produced it, suppression included.
    pub fn full_text(&self) -> &str {
        &self.full
    }

    /// Feed one streamed chunk; returns the text to show the user.
    pub fn push(&mut self, chunk: &str) -> String {
        self.full.push_str(chunk);
        self.held.push_str(chunk);
        if self.state == SuppressState::Live {
            return std::mem::take(&mut self.held);
        }
        match self.family {
            ToolFamily::ChatMl => self.push_chatml(),
            ToolFamily::JsonFunction => self.push_json(),
        }
    }

    /// End of turn: release anything still withheld that is genuinely prose.
    ///
    /// An unterminated tool call is discarded rather than flushed. At end of
    /// turn there is no stream left to block, so priority 1 no longer applies
    /// and priority 2 governs — and the discarded text is a half-written call,
    /// never something the user was waiting to read.
    pub fn finish(&mut self) -> String {
        let held = std::mem::take(&mut self.held);
        let out = match self.state {
            SuppressState::Suppress => String::new(),
            _ => held,
        };
        self.state = SuppressState::Live;
        out
    }

    fn push_chatml(&mut self) -> String {
        let mut out = String::new();
        loop {
            match self.state {
                SuppressState::Scan => {
                    if let Some(pos) = self.held.find(FENCE_OPEN) {
                        out.push_str(&self.held[..pos]);
                        let rest = self.held[pos + FENCE_OPEN.len()..].to_string();
                        self.held = rest;
                        self.state = SuppressState::Suppress;
                        continue;
                    }
                    // Hold back only the longest suffix that could still become
                    // the sentinel — a few characters, not a buffered turn.
                    let keep = partial_sentinel_len(&self.held, FENCE_OPEN);
                    let split = self.held.len() - keep;
                    out.push_str(&self.held[..split]);
                    self.held = self.held[split..].to_string();
                    return out;
                }
                SuppressState::Suppress => {
                    if let Some(pos) = self.held.find(FENCE_CLOSE) {
                        let rest = self.held[pos + FENCE_CLOSE.len()..].to_string();
                        self.held = rest;
                        self.state = SuppressState::Scan;
                        continue;
                    }
                    if self.held.len() > HOLDBACK_CAP {
                        // Priority 1: an unclosed fence must not stall the
                        // stream, even at the cost of showing its body.
                        self.state = SuppressState::Live;
                        out.push_str(&std::mem::take(&mut self.held));
                    }
                    return out;
                }
                SuppressState::Live => {
                    out.push_str(&std::mem::take(&mut self.held));
                    return out;
                }
            }
        }
    }

    fn push_json(&mut self) -> String {
        // The whole decision rests on the FIRST non-whitespace character of the
        // round. A `{` appearing later in prose never triggers buffering, which
        // is what keeps ordinary answers at zero holdback.
        if !self.decided {
            let Some(i) = self.held.find(|c: char| !c.is_whitespace()) else {
                return String::new(); // still only whitespace; nothing to show
            };
            self.decided = true;
            if self.held.as_bytes()[i] != b'{' {
                self.state = SuppressState::Live;
                return std::mem::take(&mut self.held);
            }
            self.brace_start = i;
        }

        if let Some(end) = balanced_end(&self.held, self.brace_start) {
            let object = self.held[self.brace_start..=end].to_string();
            let is_real_call = call_from_json(&object)
                .map(|c| Tool::from_name(&c.name).is_some())
                .unwrap_or(false);
            self.state = SuppressState::Live;
            if is_real_call {
                // Suppress the object; anything after it is prose.
                let after = self.held[end + 1..].to_string();
                self.held.clear();
                return after;
            }
            // Parsed to something that is not a call we would dispatch — it was
            // never tool syntax, so it is the user's text. Flush it.
            return std::mem::take(&mut self.held);
        }

        if self.held.len() > HOLDBACK_CAP {
            self.state = SuppressState::Live;
            return std::mem::take(&mut self.held);
        }
        String::new()
    }
}

/// Length of the longest suffix of `s` that is a proper prefix of `sentinel`.
///
/// This is what lets a sentinel split across token boundaries still be caught
/// while holding back only a few characters. `sentinel` is ASCII, so a matched
/// suffix is ASCII too and `s.len() - k` is always a char boundary.
fn partial_sentinel_len(s: &str, sentinel: &str) -> usize {
    let sb = s.as_bytes();
    let nb = sentinel.as_bytes();
    let max = nb.len().saturating_sub(1).min(sb.len());
    (1..=max).rev().find(|&k| sb.ends_with(&nb[..k])).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: &str, args: &str) -> ToolCall {
        ToolCall {
            id: "call_0".to_string(),
            name: name.to_string(),
            arguments: args.to_string(),
        }
    }

    #[test]
    fn parses_a_chatml_fenced_call() {
        let text = "Let me compute that.\n<tool_call>\n{\"name\": \"calc\", \
                    \"arguments\": {\"expression\": \"(3/4)*88\"}}\n</tool_call>";
        let calls = parse_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "calc");
        assert_eq!(calls[0].id, "call_0");
        assert_eq!(dispatch(&calls[0]), ToolOutcome::Ok("66".to_string()));
    }

    #[test]
    fn parses_multiple_fenced_calls_in_order() {
        let text = "<tool_call>{\"name\":\"calc\",\"arguments\":{\"expression\":\"1+1\"}}</tool_call>\
                    <tool_call>{\"name\":\"calc\",\"arguments\":{\"expression\":\"2+2\"}}</tool_call>";
        let calls = parse_tool_calls(text);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].id, "call_0");
        assert_eq!(calls[1].id, "call_1");
        assert_eq!(dispatch(&calls[0]), ToolOutcome::Ok("2".to_string()));
        assert_eq!(dispatch(&calls[1]), ToolOutcome::Ok("4".to_string()));
    }

    #[test]
    fn parses_llama_bare_json_with_parameters_key() {
        let text = "{\"name\": \"calc\", \"parameters\": {\"expression\": \"10*10\"}}";
        let calls = parse_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(dispatch(&calls[0]), ToolOutcome::Ok("100".to_string()));
    }

    /// Models wrap the object in prose or a markdown fence constantly; a missed
    /// call is a wrong answer shown to the user.
    #[test]
    fn finds_bare_json_embedded_in_prose_or_a_fence() {
        for text in [
            "Sure! ```json\n{\"name\":\"calc\",\"parameters\":{\"expression\":\"6*7\"}}\n```",
            "I'll use the tool: {\"name\":\"calc\",\"parameters\":{\"expression\":\"6*7\"}} okay?",
        ] {
            let calls = parse_tool_calls(text);
            assert_eq!(calls.len(), 1, "failed on: {text}");
            assert_eq!(dispatch(&calls[0]), ToolOutcome::Ok("42".to_string()));
        }
    }

    /// A brace inside a string argument must not end the object early.
    #[test]
    fn balanced_scan_ignores_braces_inside_strings() {
        let text = "{\"name\":\"calc\",\"parameters\":{\"expression\":\"1+1\",\"note\":\"a } brace\"}}";
        let calls = parse_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(dispatch(&calls[0]), ToolOutcome::Ok("2".to_string()));
    }

    #[test]
    fn plain_prose_yields_no_calls() {
        assert!(parse_tool_calls("The answer is 66. No tools needed.").is_empty());
        assert!(parse_tool_calls("").is_empty());
    }

    // ── the closed registry ──────────────────────────────────────────────────

    /// The security-review property (M2): an invented name is never dispatched,
    /// and comes back as a correctable tool error.
    #[test]
    fn unknown_tool_names_are_never_dispatched() {
        for name in [
            "exec",
            "read_file",
            "invoke",
            "calc ",
            "Calc",
            "../calc",
            "calc\u{0}",
        ] {
            let outcome = dispatch(&call(name, "{\"expression\":\"1+1\"}"));
            assert!(outcome.is_err(), "{name:?} was dispatched");
            assert!(
                outcome.as_content().contains("no tool named"),
                "{name:?} gave: {}",
                outcome.as_content()
            );
        }
    }

    #[test]
    fn a_calc_error_becomes_the_tool_result_rather_than_failing_the_turn() {
        let outcome = dispatch(&call("calc", "{\"expression\":\"sqrt(-1)\"}"));
        assert!(outcome.is_err());
        // The model gets the evaluator's own message, which is what lets it
        // correct the expression on the next round.
        assert!(
            outcome.as_content().contains("negative"),
            "got: {}",
            outcome.as_content()
        );
    }

    #[test]
    fn missing_or_malformed_arguments_are_reported_not_panicked() {
        for args in ["{}", "not json", "{\"expr\":\"1+1\"}", "null"] {
            let outcome = dispatch(&call("calc", args));
            assert!(outcome.is_err(), "{args:?} unexpectedly succeeded");
            assert!(
                outcome.as_content().contains("expression"),
                "{args:?} gave: {}",
                outcome.as_content()
            );
        }
    }

    /// OpenAI-style `arguments` is a JSON *string* containing an object, and
    /// fine-tunes imitate it.
    #[test]
    fn accepts_arguments_double_encoded_as_a_json_string() {
        let outcome = dispatch(&call("calc", "\"{\\\"expression\\\":\\\"7*6\\\"}\""));
        assert_eq!(outcome, ToolOutcome::Ok("42".to_string()));
    }

    // ── streaming suppression ────────────────────────────────────────────────

    /// Drive a suppressor with a chunking, returning what the user would see.
    fn stream(family: ToolFamily, chunks: &[&str]) -> String {
        let mut s = ToolStream::new(family);
        let mut out = String::new();
        for c in chunks {
            out.push_str(&s.push(c));
        }
        out.push_str(&s.finish());
        out
    }

    /// Every one-character chunking is the worst case for a sentinel scanner.
    fn stream_by_char(family: ToolFamily, text: &str) -> String {
        let chunks: Vec<String> = text.chars().map(|c| c.to_string()).collect();
        let refs: Vec<&str> = chunks.iter().map(|s| s.as_str()).collect();
        stream(family, &refs)
    }

    #[test]
    fn chatml_prose_before_a_fence_stays_visible() {
        let text = "Let me calculate that.\n<tool_call>\n{\"name\":\"calc\",\
                    \"arguments\":{\"expression\":\"1+1\"}}\n</tool_call>";
        assert_eq!(stream(ToolFamily::ChatMl, &[text]), "Let me calculate that.\n");
    }

    /// The sentinel arriving a character at a time must still be caught — this
    /// is the case a naive `contains` check on each chunk fails.
    #[test]
    fn chatml_sentinel_split_across_token_boundaries_is_still_caught() {
        let text = "ok <tool_call>{\"name\":\"calc\",\"arguments\":{\"expression\":\"2+2\"}}</tool_call> done";
        assert_eq!(stream_by_char(ToolFamily::ChatMl, text), "ok  done");
        // And an awkward split straddling the sentinel itself.
        let split = ["ok <tool", "_ca", "ll>{\"name\":\"calc\",\"arguments\":{}}</tool_", "call> done"];
        assert_eq!(stream(ToolFamily::ChatMl, &split), "ok  done");
    }

    /// Text that merely *looks* like the start of a sentinel must not be eaten.
    #[test]
    fn chatml_partial_sentinel_that_turns_out_to_be_prose_is_released() {
        assert_eq!(stream(ToolFamily::ChatMl, &["1 <tool", "box> 2"]), "1 <toolbox> 2");
        assert_eq!(stream_by_char(ToolFamily::ChatMl, "a < b and c <tool"), "a < b and c <tool");
    }

    #[test]
    fn json_round_opening_with_a_call_suppresses_it_entirely() {
        let text = "{\"name\":\"calc\",\"parameters\":{\"expression\":\"6*7\"}}";
        assert_eq!(stream(ToolFamily::JsonFunction, &[text]), "");
        assert_eq!(stream_by_char(ToolFamily::JsonFunction, text), "");
    }

    /// The false-positive flush: a round that legitimately opens with `{` must
    /// end up fully visible, just slightly late.
    #[test]
    fn json_false_positive_is_flushed_not_swallowed() {
        for text in [
            "{ this is not JSON at all",
            "{\"answer\": 42} is the set notation you wanted",
            "{\"name\":\"exec\",\"parameters\":{}}",
        ] {
            let seen = stream(ToolFamily::JsonFunction, &[text]);
            assert_eq!(seen, text, "flush lost text for: {text}");
        }
    }

    /// A `{` in the middle of prose must never trigger buffering — the decision
    /// is made once, on the first non-whitespace character of the round.
    #[test]
    fn json_brace_later_in_prose_never_triggers_holdback() {
        let text = "The set is {1, 2, 3} exactly.";
        assert_eq!(stream(ToolFamily::JsonFunction, &[text]), text);
        assert_eq!(stream_by_char(ToolFamily::JsonFunction, text), text);
    }

    /// Priority 1 over priority 2: an unterminated call must not stall the
    /// stream forever, even though flushing shows its body.
    #[test]
    fn an_unclosed_call_flushes_at_the_cap_rather_than_stalling() {
        let runaway = format!("{{\"name\":\"calc\",\"arguments\":{}", "x".repeat(HOLDBACK_CAP + 64));
        let seen = stream(ToolFamily::JsonFunction, &[&runaway]);
        assert!(!seen.is_empty(), "cap did not release the stream");

        let fence = format!("<tool_call>{}", "y".repeat(HOLDBACK_CAP + 64));
        let seen = stream(ToolFamily::ChatMl, &[&fence]);
        assert!(!seen.is_empty(), "unclosed fence stalled the stream");
    }

    /// Priority 2 at end of turn: once there is no stream left to block, a
    /// half-written call is discarded rather than leaked.
    #[test]
    fn a_truncated_call_at_end_of_turn_is_discarded_not_leaked() {
        let seen = stream(ToolFamily::ChatMl, &["answer soon <tool_call>{\"name\":\"ca"]);
        assert_eq!(seen, "answer soon ");
    }

    /// The property that keeps display and dispatch honest: whenever the
    /// suppressor hides a call, the parser must find one in the same text, and
    /// vice versa. If these drift, the user sees syntax the loop ignored — or
    /// loses prose to a call that never ran.
    #[test]
    fn suppressor_and_parser_agree_on_what_a_call_is() {
        let cases = [
            (ToolFamily::ChatMl, "hi <tool_call>{\"name\":\"calc\",\"arguments\":{\"expression\":\"1+1\"}}</tool_call>"),
            (ToolFamily::JsonFunction, "{\"name\":\"calc\",\"parameters\":{\"expression\":\"1+1\"}}"),
        ];
        for (family, text) in cases {
            let mut s = ToolStream::new(family);
            let mut shown = s.push(text);
            shown.push_str(&s.finish());

            let calls = parse_tool_calls(s.full_text());
            assert_eq!(calls.len(), 1, "parser missed the call in: {text}");
            assert!(
                !shown.contains("\"name\""),
                "suppressor leaked a call body the parser dispatched: {shown:?}"
            );
        }

        // The negative direction: no call parsed => nothing suppressed.
        for (family, text) in [
            (ToolFamily::ChatMl, "just prose, no tools"),
            (ToolFamily::JsonFunction, "{\"answer\": 42} plain text"),
        ] {
            let mut s = ToolStream::new(family);
            let mut shown = s.push(text);
            shown.push_str(&s.finish());
            assert!(parse_tool_calls(s.full_text()).is_empty());
            assert_eq!(shown, text, "suppressed text the parser saw no call in");
        }
    }

    #[test]
    fn preamble_matches_the_shape_each_family_is_parsed_for() {
        assert!(tools_preamble(ToolFamily::ChatMl).contains(FENCE_OPEN));
        assert!(!tools_preamble(ToolFamily::JsonFunction).contains(FENCE_OPEN));
        // Whatever the preamble asks for must round-trip through the parser.
        for family in [ToolFamily::ChatMl, ToolFamily::JsonFunction] {
            let example = tools_preamble(family);
            let calls = parse_tool_calls(example);
            assert_eq!(calls.len(), 1, "preamble example unparseable for {family:?}");
            assert_eq!(calls[0].name, "calc");
        }
    }
}
