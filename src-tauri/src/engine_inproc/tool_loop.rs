//! The calc tool-loop, Rust-side.
//!
//! A transcription of `src/calc-loop.js`, whose header names this port as the
//! reason it must stay a reference implementation. The orchestration is
//! identical — bounded rounds, tool results appended as conversation turns,
//! errors handed back to the model rather than failing the turn — but the
//! transport is not: desktop streams SSE from `llama-server` and receives
//! `tool_calls` *structurally*, while here a turn is raw generated text that
//! [`super::tools`] has to parse and suppress.
//!
//! The loop is written against [`TurnSource`] rather than `EngineSession` so it
//! compiles and is tested on desktop, where the Windows suite is the only place
//! tests actually run. `engine_inproc` supplies the real implementation over the
//! inference thread's session; the tests supply a scripted one.

// (Dead-code allow on the `mod` declaration in lib.rs — canonical placement,
// and cfg'd to `desktop` so the check stays LIVE on the platform that ships.)

use crate::engine_tools::{
    dispatch, parse_tool_calls, ToolFamily, ToolOutcome, ToolStream,
};

/// The JS reference's `maxRounds`. Five is enough for a calculation that needs
/// correcting twice and still answering.
pub(crate) const MAX_ROUNDS: usize = 5;

/// Shown when the model spent the turn calculating and never wrote prose.
const SILENT_SUCCESS: &str = "Calculation complete.";
/// Shown when the round cap is hit with nothing else to say. Also what a
/// cap-round tool call degrades to — the raw syntax is suppressed and never
/// rendered as if it were the answer.
const ROUND_CAP: &str = "I reached the calculation limit for this turn.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoopRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone)]
pub(crate) struct LoopMessage {
    pub role: LoopRole,
    pub content: String,
}

impl LoopMessage {
    /// Test-only, and gated rather than suppressed so the dead-code check
    /// stays honest on both platforms.
    ///
    /// Production never calls it: `chat_cmds::to_loop_messages` converts each
    /// incoming `WireMessage` with a struct literal (`chat_cmds.rs:162,167`)
    /// because the role comes from the wire rather than from the call site.
    /// Its siblings `assistant` and `tool` DO have production callers
    /// (`tool_loop.rs:151,169`), which is why only this one was ever dead —
    /// invisible since 1.4 behind a bare `#![allow(dead_code)]` that switched
    /// the check off on Android as well as desktop.
    #[cfg(test)]
    pub fn user(content: impl Into<String>) -> Self {
        LoopMessage { role: LoopRole::User, content: content.into() }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        LoopMessage { role: LoopRole::Assistant, content: content.into() }
    }
    pub fn tool(content: impl Into<String>) -> Self {
        LoopMessage { role: LoopRole::Tool, content: content.into() }
    }
}

/// One calculation the turn performed, surfaced to the UI beside the answer so
/// the arithmetic is auditable rather than asserted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Calculation {
    pub expression: String,
    /// The evaluator's rendering, or `None` when it errored.
    pub display: Option<String>,
    /// The error text, which was also fed back to the model.
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoopOutcome {
    pub content: String,
    pub calculations: Vec<Calculation>,
}

/// Produces one assistant turn at a time.
///
/// Implementations feed **raw** generated text to `sink` — suppression is the
/// loop's job, so that display and dispatch are decided in one place from one
/// copy of the text.
pub(crate) trait TurnSource {
    fn turn(
        &mut self,
        messages: &[LoopMessage],
        sink: &mut dyn FnMut(&str),
    ) -> Result<(), String>;

    /// Cooperative cancel, checked between rounds. Mid-round cancel is the
    /// sink's job (`ControlFlow::Break`), since only it sees each token.
    fn cancelled(&self) -> bool {
        false
    }
}

/// Run the loop to an answer.
///
/// `on_delta` receives display text only: tool syntax is removed by
/// [`ToolStream`] before it is ever forwarded, so a caller can pipe it straight
/// to the UI.
pub(crate) fn run(
    src: &mut dyn TurnSource,
    family: ToolFamily,
    mut convo: Vec<LoopMessage>,
    on_delta: &mut dyn FnMut(&str),
) -> Result<LoopOutcome, String> {
    let mut content = String::new();
    let mut calculations: Vec<Calculation> = Vec::new();

    for round in 0..MAX_ROUNDS {
        if src.cancelled() {
            break;
        }

        // One suppressor per round: its state (sentinel scan, first-character
        // decision) is scoped to a single turn.
        let mut stream = ToolStream::new(family);
        let mut visible = String::new();
        {
            let mut sink = |chunk: &str| {
                let shown = stream.push(chunk);
                if !shown.is_empty() {
                    visible.push_str(&shown);
                    on_delta(&shown);
                }
            };
            src.turn(&convo, &mut sink)?;
        }
        let tail = stream.finish();
        if !tail.is_empty() {
            visible.push_str(&tail);
            on_delta(&tail);
        }
        content.push_str(&visible);

        let raw = stream.full_text().to_string();
        let calls = parse_tool_calls(&raw);
        if calls.is_empty() {
            return Ok(finish(content, calculations, false));
        }

        // The final round may not spend its calls — there is no round left to
        // return their results to. The syntax was suppressed above, so the user
        // sees whatever prose accompanied it, or the cap message.
        if round + 1 == MAX_ROUNDS {
            break;
        }

        // The model's own turn goes back verbatim, so the next round sees the
        // call it made rather than a reconstruction of it.
        convo.push(LoopMessage::assistant(raw));
        for call in &calls {
            let outcome = dispatch(call);
            let expression = call_expression(&call.arguments);
            calculations.push(match &outcome {
                ToolOutcome::Ok(display) => Calculation {
                    expression,
                    display: Some(display.clone()),
                    error: None,
                },
                ToolOutcome::Err(message) => Calculation {
                    expression,
                    display: None,
                    error: Some(message.clone()),
                },
            });
            // An error is the tool's result, not a failure of the turn — the
            // model reads it and corrects on the next round.
            convo.push(LoopMessage::tool(outcome.as_content().to_string()));
        }
    }

    Ok(finish(content, calculations, true))
}

/// Apply the reference implementation's fallback text: a turn that calculated
/// but never spoke still needs something to show.
fn finish(content: String, calculations: Vec<Calculation>, hit_cap: bool) -> LoopOutcome {
    let content = if content.trim().is_empty() && !calculations.is_empty() {
        if hit_cap { ROUND_CAP } else { SILENT_SUCCESS }.to_string()
    } else {
        content
    };
    LoopOutcome { content, calculations }
}

/// Best-effort expression for the audit trail. The calculation is recorded even
/// when the arguments were unparseable, because "the model tried to compute
/// something and it failed" is exactly what the user needs to see.
fn call_expression(arguments: &str) -> String {
    serde_json::from_str::<serde_json::Value>(arguments)
        .ok()
        .and_then(|v| v.get("expression").and_then(|e| e.as_str().map(str::to_string)))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scripted source: hands back canned turns in order, recording what
    /// conversation it was given each time.
    struct Scripted {
        turns: Vec<String>,
        seen: Vec<Vec<LoopMessage>>,
    }

    impl Scripted {
        fn new(turns: &[&str]) -> Self {
            Scripted {
                turns: turns.iter().map(|s| s.to_string()).collect(),
                seen: Vec::new(),
            }
        }
    }

    impl TurnSource for Scripted {
        fn turn(
            &mut self,
            messages: &[LoopMessage],
            sink: &mut dyn FnMut(&str),
        ) -> Result<(), String> {
            self.seen.push(messages.to_vec());
            let text = if self.turns.is_empty() {
                String::new()
            } else {
                self.turns.remove(0)
            };
            // Chunked to exercise the suppressor across boundaries, the way a
            // real token stream arrives.
            for piece in text.as_bytes().chunks(7) {
                sink(&String::from_utf8_lossy(piece));
            }
            Ok(())
        }
    }

    fn call(expr: &str) -> String {
        format!("<tool_call>{{\"name\":\"calc\",\"arguments\":{{\"expression\":\"{expr}\"}}}}</tool_call>")
    }

    fn run_script(turns: &[&str]) -> (LoopOutcome, String) {
        let mut src = Scripted::new(turns);
        let mut shown = String::new();
        let out = run(
            &mut src,
            ToolFamily::ChatMl,
            vec![LoopMessage::user("q")],
            &mut |d| shown.push_str(d),
        )
        .unwrap();
        (out, shown)
    }

    #[test]
    fn a_turn_with_no_tools_passes_straight_through() {
        let (out, shown) = run_script(&["The answer is 66."]);
        assert_eq!(out.content, "The answer is 66.");
        assert_eq!(shown, "The answer is 66.");
        assert!(out.calculations.is_empty());
    }

    #[test]
    fn a_calc_round_records_the_calculation_and_answers() {
        let (out, shown) = run_script(&[
            &format!("Let me work that out. {}", call("(3/4)*88")),
            "It comes to 66.",
        ]);
        assert_eq!(out.calculations.len(), 1);
        assert_eq!(out.calculations[0].expression, "(3/4)*88");
        assert_eq!(out.calculations[0].display.as_deref(), Some("66"));
        assert_eq!(out.calculations[0].error, None);
        assert_eq!(out.content, "Let me work that out. It comes to 66.");
        // The syntax never reached the user.
        assert!(!shown.contains("tool_call"), "leaked syntax: {shown:?}");
    }

    #[test]
    fn the_tool_result_is_appended_so_the_model_can_use_it() {
        let mut src = Scripted::new(&[&call("2+2"), "Four."]);
        let mut shown = String::new();
        run(&mut src, ToolFamily::ChatMl, vec![LoopMessage::user("q")], &mut |d| {
            shown.push_str(d)
        })
        .unwrap();

        // Second round saw: user, assistant(raw call), tool(result).
        let second = &src.seen[1];
        assert_eq!(second.len(), 3);
        assert_eq!(second[1].role, LoopRole::Assistant);
        assert!(second[1].content.contains("tool_call"), "raw turn not replayed");
        assert_eq!(second[2].role, LoopRole::Tool);
        assert_eq!(second[2].content, "4");
    }

    /// The reference implementation's "error becomes the tool result" rule: a
    /// bad expression must not fail the turn, it must let the model retry.
    #[test]
    fn a_calc_error_is_fed_back_and_the_model_self_corrects() {
        let mut src = Scripted::new(&[
            &call("sqrt(-1)"),
            &call("sqrt(1)"),
            "It's 1.",
        ]);
        let mut shown = String::new();
        let out = run(&mut src, ToolFamily::ChatMl, vec![LoopMessage::user("q")], &mut |d| {
            shown.push_str(d)
        })
        .unwrap();

        assert_eq!(out.calculations.len(), 2);
        assert!(out.calculations[0].error.is_some());
        assert_eq!(out.calculations[0].display, None);
        assert_eq!(out.calculations[1].display.as_deref(), Some("1"));
        assert_eq!(out.content, "It's 1.");
        // The error text was handed to the model as the tool turn.
        assert_eq!(src.seen[1][2].role, LoopRole::Tool);
        assert!(src.seen[1][2].content.contains("negative"));
    }

    /// The closed registry, exercised through the loop rather than in isolation.
    #[test]
    fn an_invented_tool_is_never_dispatched_and_the_loop_continues() {
        let invented =
            "<tool_call>{\"name\":\"exec\",\"arguments\":{\"cmd\":\"rm -rf /\"}}</tool_call>";
        let mut src = Scripted::new(&[invented, "Sorry, I can only calculate."]);
        let mut shown = String::new();
        let out = run(&mut src, ToolFamily::ChatMl, vec![LoopMessage::user("q")], &mut |d| {
            shown.push_str(d)
        })
        .unwrap();

        assert_eq!(out.content, "Sorry, I can only calculate.");
        let tool_turn = &src.seen[1][2];
        assert_eq!(tool_turn.role, LoopRole::Tool);
        assert!(tool_turn.content.contains("no tool named"));
    }

    #[test]
    fn a_silent_calculation_gets_the_reference_fallback_text() {
        let (out, _) = run_script(&[&call("1+1"), ""]);
        assert_eq!(out.content, SILENT_SUCCESS);
        assert_eq!(out.calculations.len(), 1);
    }

    /// Round-cap semantics: a model that never stops calling tools must still
    /// produce a bounded, syntax-free answer.
    #[test]
    fn the_round_cap_stops_the_loop_without_rendering_syntax() {
        let looping: Vec<&str> = vec![
            "<tool_call>{\"name\":\"calc\",\"arguments\":{\"expression\":\"1+1\"}}</tool_call>",
        ];
        let turns: Vec<&str> = looping.iter().cycle().take(MAX_ROUNDS + 3).copied().collect();
        let (out, shown) = run_script(&turns);

        assert_eq!(out.content, ROUND_CAP);
        assert!(!shown.contains("tool_call"), "cap round leaked syntax: {shown:?}");
        // One fewer dispatch than rounds: the final round's call is not spent,
        // because there is no round left to return its result to.
        assert_eq!(out.calculations.len(), MAX_ROUNDS - 1);
    }

    #[test]
    fn prose_alongside_a_cap_round_call_is_preferred_over_the_fallback() {
        let mut turns: Vec<String> = (0..MAX_ROUNDS)
            .map(|_| call("1+1"))
            .collect();
        let last = turns.len() - 1;
        turns[last] = format!("Here is my best answer. {}", call("1+1"));
        let refs: Vec<&str> = turns.iter().map(|s| s.as_str()).collect();
        let (out, _) = run_script(&refs);
        assert_eq!(out.content, "Here is my best answer. ");
    }

    #[test]
    fn cancellation_between_rounds_stops_the_loop() {
        struct AlwaysCancelled;
        impl TurnSource for AlwaysCancelled {
            fn turn(&mut self, _: &[LoopMessage], _: &mut dyn FnMut(&str)) -> Result<(), String> {
                panic!("must not generate when already cancelled");
            }
            fn cancelled(&self) -> bool {
                true
            }
        }
        let out = run(
            &mut AlwaysCancelled,
            ToolFamily::ChatMl,
            vec![LoopMessage::user("q")],
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(out.content, "");
    }

    #[test]
    fn a_generation_error_aborts_the_turn() {
        struct Failing;
        impl TurnSource for Failing {
            fn turn(&mut self, _: &[LoopMessage], _: &mut dyn FnMut(&str)) -> Result<(), String> {
                Err("engine exploded".to_string())
            }
        }
        let err = run(
            &mut Failing,
            ToolFamily::ChatMl,
            vec![LoopMessage::user("q")],
            &mut |_| {},
        )
        .unwrap_err();
        assert_eq!(err, "engine exploded");
    }

    /// The Llama family runs the same loop through a different wire shape.
    #[test]
    fn the_loop_works_for_the_bare_json_family_too() {
        let mut src = Scripted::new(&[
            "{\"name\":\"calc\",\"parameters\":{\"expression\":\"6*7\"}}",
            "Forty-two.",
        ]);
        let mut shown = String::new();
        let out = run(
            &mut src,
            ToolFamily::JsonFunction,
            vec![LoopMessage::user("q")],
            &mut |d| shown.push_str(d),
        )
        .unwrap();
        assert_eq!(out.calculations[0].display.as_deref(), Some("42"));
        assert_eq!(out.content, "Forty-two.");
        assert!(!shown.contains("\"name\""), "leaked syntax: {shown:?}");
    }
}
