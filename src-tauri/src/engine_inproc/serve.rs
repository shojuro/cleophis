//! The inference thread's serve loop: which turns may share one live
//! `EngineSession`, and every way that session's life ends.
//!
//! # Why this is a module and not ten lines inside `engine_inproc`
//!
//! Task 1.5 keeps one `EngineSession` alive across consecutive turns of the
//! same conversation so the KV cache is reused instead of rebuilt. The session
//! **borrows** the `!Send` engine handle (`EngineHandle::session` returns
//! `Box<dyn EngineSession + '_>`), so the code holding it can only live on the
//! inference thread, inside `engine_inproc`, which is `cfg(mobile)` — the
//! desktop suite cannot see it, and the desktop suite is the only place tests
//! actually run.
//!
//! What is risky here, though, is not the borrow. It is the **routing**: which
//! arriving command may reuse the session, which must close it, and whether
//! anything can be silently dropped on the way. Those failures are deadlocks
//! and missing turns, not compile errors. So the routing is written here —
//! platform-neutral, closure-generic over "run a turn" and "get the next
//! command" — and `engine_inproc` supplies the two effects it cannot. Same
//! reasoning as [`crate::engine_tools`] and [`crate::engine_tool_loop`]: put
//! the logic where it can be tested, and leave only the FFI-bound remainder
//! behind the cfg.
//!
//! # The cache belongs to one conversation
//!
//! A live session's KV cache holds *one specific chat's* prefix. Serving
//! another conversation from it would have the model attend to a different
//! conversation's tokens — the same silent-corruption class as reusing a cache
//! without trimming it (see `LlamaSession::stream`'s invariants). So reuse is
//! keyed, and an unkeyed turn — `chat_key: None`, meaning a transient
//! completion like auto-title that belongs to no conversation — never reuses
//! and never leaves a cache behind for anyone else to reuse.

// On desktop nothing constructs these: the only production caller is
// `chat_cmds::imp`, which is `cfg(mobile)`. They are compiled here anyway so
// the tests below run in the desktop suite. This states a permanent platform
// fact rather than deferring a question — on Android the dead-code check stays
// live, which is what the 5.3 `mobile-check` CI job is for.
// (The dead-code allow for this module lives on its `mod` declaration in
// lib.rs — canonical placement per D-3's amendment, beside the rationale.)

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::sync::mpsc;

use crate::engine_tool_loop::{LoopMessage, LoopOutcome};
use crate::engine_tools::ToolFamily;

/// One chat turn, as it crosses onto the inference thread.
pub(crate) struct ChatTurn {
    /// The full conversation to render. `LlamaSession::stream` computes the
    /// shared prefix against what the cache already holds, so passing the whole
    /// history every turn is what *enables* reuse rather than defeating it.
    pub convo: Vec<LoopMessage>,
    pub family: ToolFamily,
    /// Which conversation this turn belongs to — the key a live session's KV
    /// cache is valid for. `None` is a transient turn that belongs to no chat
    /// (auto-title, analysis); it neither reuses nor is reused.
    pub chat_key: Option<i64>,
    pub cancel: Arc<AtomicBool>,
    pub on_delta: Box<dyn FnMut(&str) + Send>,
    pub reply: mpsc::Sender<Result<LoopOutcome, String>>,
}

/// Commands accepted by the inference thread.
pub(crate) enum Command {
    /// (Re)resolve the hero and load it, replacing whatever is loaded now.
    Load,
    /// Run one chat turn — the whole tool loop — against the loaded model.
    ///
    /// The loop runs *on the inference thread* because every round needs the
    /// session, and the session borrows the `!Send` handle. Only the streamed
    /// text and the final outcome cross back.
    Chat(ChatTurn),
    /// Unload and exit the thread.
    Shutdown,
}

/// The result of waiting for the next command while a session is open.
pub(crate) enum Waited {
    Cmd(Command),
    /// Nothing arrived before the caller's deadline.
    ///
    /// Holding a session open is what keeps the cache warm, and it is also the
    /// one steady-state cost 1.5 adds: a live `LlamaContext` on the device with
    /// the least RAM, held for as long as the conversation is merely *open*
    /// rather than active. Before 1.5 no context outlived a turn. A deadline
    /// bounds that without any lifecycle signal, which we do not have until the
    /// §8 backgrounding work.
    Timeout,
    /// Every sender is gone.
    Closed,
}

/// Why a live session stopped serving turns.
pub(crate) enum SessionEnd {
    /// Closed with nothing outstanding; the caller goes back to waiting for a
    /// command.
    Idle,
    /// A command arrived that this session cannot serve.
    ///
    /// **The caller must process it.** Discarding it here is precisely how a
    /// turn goes missing: the turn's `reply` sender would drop, and the awaiting
    /// command would surface "the engine stopped during generation" for a turn
    /// that was never attempted. (That it degrades to a legible error rather
    /// than a hang is luck, not design.)
    Yield(Command),
    /// Every sender is gone, so no further work can arrive: the inference thread
    /// should unload and exit.
    Closed,
}

/// Whether a session opened for conversation `live` may serve a turn belonging
/// to `next`.
///
/// Both `None` cases are deliberate, and neither is an oversight:
/// - a transient turn (`next` is `None`) must not inherit a conversation's
///   prefix, because its prompt is not a continuation of that conversation;
/// - a session opened for a transient turn (`live` is `None`) must not be
///   handed a conversation's turn, for the mirror-image reason.
///
/// `None == None` is false for the same reason: two transient completions are
/// unrelated prompts that happen to share the absence of a chat id.
pub(crate) fn reusable(live: Option<i64>, next: Option<i64>) -> bool {
    match (live, next) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// What a session open for `live` does with a command that just arrived.
pub(crate) enum Route {
    /// Serve this turn on the session already open — same conversation, and so
    /// a warm prefix.
    Reuse(ChatTurn),
    /// Close the session and hand this command back to the outer loop.
    Yield(Command),
}

/// Decide [`Route`] for one arriving command. The whole reuse policy is this
/// function; `serve_loop` only carries it out.
pub(crate) fn route(live: Option<i64>, cmd: Command) -> Route {
    match cmd {
        Command::Chat(turn) if reusable(live, turn.chat_key) => Route::Reuse(turn),
        other => Route::Yield(other),
    }
}

/// Serve turns on one open session for as long as they keep belonging to the
/// same conversation.
///
/// The caller holds the session (it borrows the `!Send` handle) and supplies:
/// - `run` — execute one turn on it, replying to the caller of that turn;
/// - `wait` — block for the next command, up to the caller's idle deadline.
///
/// Returns the reason the session should now be dropped. Every exit is
/// enumerated in [`SessionEnd`], and each has a test below, because the failure
/// modes here are a thread that stops accepting work and a turn nobody answers.
pub(crate) fn serve_loop(
    first: ChatTurn,
    run: &mut dyn FnMut(ChatTurn),
    wait: &mut dyn FnMut() -> Waited,
) -> SessionEnd {
    let live = first.chat_key;
    let mut turn = first;

    loop {
        run(turn);

        // A transient turn leaves nothing worth keeping open, and holding the
        // session would only block the next conversation's turn behind a
        // needless close-and-reopen.
        if live.is_none() {
            return SessionEnd::Idle;
        }

        let cmd = match wait() {
            Waited::Cmd(cmd) => cmd,
            // Both release the session; they differ in whether the thread has
            // anything left to do afterwards.
            Waited::Timeout => return SessionEnd::Idle,
            Waited::Closed => return SessionEnd::Closed,
        };

        match route(live, cmd) {
            Route::Reuse(next) => turn = next,
            Route::Yield(cmd) => return SessionEnd::Yield(cmd),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::sync::mpsc::Receiver;

    /// A turn carrying `tag` as its single user message, so a test can prove
    /// *which* turn was run or yielded rather than just how many.
    fn turn(chat_key: Option<i64>, tag: &str) -> (ChatTurn, Receiver<Result<LoopOutcome, String>>) {
        let (tx, rx) = mpsc::channel();
        (
            ChatTurn {
                convo: vec![LoopMessage::user(tag)],
                family: ToolFamily::JsonFunction,
                chat_key,
                cancel: Arc::new(AtomicBool::new(false)),
                on_delta: Box::new(|_| {}),
                reply: tx,
            },
            rx,
        )
    }

    fn tag_of(t: &ChatTurn) -> String {
        t.convo[0].content.clone()
    }

    /// Drives `serve_loop` with a scripted wait queue, recording the tag of
    /// every turn that reached `run` — and replying to each, as the real caller
    /// does, so a test can also assert nothing was left unanswered.
    fn drive_waits(first: ChatTurn, script: Vec<Waited>) -> (SessionEnd, Vec<String>) {
        let ran = RefCell::new(Vec::new());
        let queue = RefCell::new(script.into_iter());

        let end = {
            let mut run = |t: ChatTurn| {
                ran.borrow_mut().push(tag_of(&t));
                let _ = t.reply.send(Ok(LoopOutcome {
                    content: String::new(),
                    calculations: Vec::new(),
                }));
            };
            // A queue that runs dry stands in for every sender being gone.
            let mut wait = || queue.borrow_mut().next().unwrap_or(Waited::Closed);
            serve_loop(first, &mut run, &mut wait)
        };

        (end, ran.into_inner())
    }

    fn drive(first: ChatTurn, script: Vec<Command>) -> (SessionEnd, Vec<String>) {
        drive_waits(first, script.into_iter().map(Waited::Cmd).collect())
    }

    #[test]
    fn consecutive_turns_of_one_chat_share_the_session() {
        let (first, _r) = turn(Some(7), "a");
        let (second, _r2) = turn(Some(7), "b");
        let (third, _r3) = turn(Some(7), "c");

        let (end, ran) = drive(
            first,
            vec![
                Command::Chat(second),
                Command::Chat(third),
                Command::Shutdown,
            ],
        );

        // All three ran without the session closing between them — the whole
        // point of 1.5.
        assert_eq!(ran, vec!["a", "b", "c"]);
        assert!(matches!(end, SessionEnd::Yield(Command::Shutdown)));
    }

    #[test]
    fn a_turn_for_another_chat_ends_the_session_and_is_handed_back() {
        let (first, _r) = turn(Some(7), "a");
        let (other, _r2) = turn(Some(8), "b");

        let (end, ran) = drive(first, vec![Command::Chat(other)]);

        assert_eq!(ran, vec!["a"]);
        // Handed back intact, not dropped: the outer loop reopens a session for
        // chat 8 and serves it. A dropped turn here is a message the user sent
        // that never gets an answer.
        match end {
            SessionEnd::Yield(Command::Chat(t)) => {
                assert_eq!(t.chat_key, Some(8));
                assert_eq!(tag_of(&t), "b");
            }
            _ => panic!("expected the switched turn to be yielded"),
        }
    }

    #[test]
    fn a_transient_turn_closes_the_session_immediately() {
        let (first, _r) = turn(None, "title");
        // The script would be served if the loop ever consulted it — it must
        // not, because a transient session has no prefix worth holding.
        let (next, _r2) = turn(None, "another");

        let (end, ran) = drive(first, vec![Command::Chat(next)]);

        assert_eq!(ran, vec!["title"]);
        assert!(matches!(end, SessionEnd::Idle));
    }

    #[test]
    fn a_transient_turn_may_not_be_served_by_a_chats_session() {
        let (first, _r) = turn(Some(7), "a");
        let (transient, _r2) = turn(None, "title");

        let (end, ran) = drive(first, vec![Command::Chat(transient)]);

        assert_eq!(ran, vec!["a"]);
        match end {
            SessionEnd::Yield(Command::Chat(t)) => assert_eq!(t.chat_key, None),
            _ => panic!("expected the transient turn to be yielded"),
        }
    }

    #[test]
    fn load_ends_the_session() {
        let (first, _r) = turn(Some(7), "a");
        let (end, ran) = drive(first, vec![Command::Load]);

        assert_eq!(ran, vec!["a"]);
        // Load must reach the outer loop: it has to `unload()` the handle, which
        // is impossible while a session borrows it.
        assert!(matches!(end, SessionEnd::Yield(Command::Load)));
    }

    #[test]
    fn shutdown_ends_the_session() {
        let (first, _r) = turn(Some(7), "a");
        let (end, ran) = drive(first, vec![Command::Shutdown]);

        assert_eq!(ran, vec!["a"]);
        // If this were ever swallowed, `stop_thread`'s join would block forever
        // and take app teardown with it.
        assert!(matches!(end, SessionEnd::Yield(Command::Shutdown)));
    }

    #[test]
    fn a_closed_channel_ends_the_thread() {
        let (first, _r) = turn(Some(7), "a");
        // Empty script: `recv` returns None, standing in for every sender gone.
        let (end, ran) = drive(first, vec![]);

        assert_eq!(ran, vec!["a"]);
        assert!(matches!(end, SessionEnd::Closed));
    }

    #[test]
    fn every_turn_the_loop_accepts_is_answered() {
        let (first, r1) = turn(Some(7), "a");
        let (second, r2) = turn(Some(7), "b");

        let (_end, _ran) = drive(first, vec![Command::Chat(second), Command::Shutdown]);

        // The reply channels outlive the loop here, so a missing reply is a
        // missing reply rather than a closed channel.
        assert!(r1.try_recv().is_ok());
        assert!(r2.try_recv().is_ok());
    }

    #[test]
    fn an_idle_session_is_released_rather_than_held() {
        let (first, _r) = turn(Some(7), "a");
        let (end, ran) = drive_waits(first, vec![Waited::Timeout]);

        assert_eq!(ran, vec!["a"]);
        // Idle, not Closed: the thread is still live and still wants commands.
        // Only the context — and the memory it holds on the floor device — goes.
        assert!(matches!(end, SessionEnd::Idle));
    }

    #[test]
    fn the_deadline_only_applies_to_an_idle_session() {
        let (first, _r) = turn(Some(7), "a");
        let (second, _r2) = turn(Some(7), "b");

        // A turn arrives, then the user goes quiet. The deadline must not cut
        // the conversation short while turns are still coming.
        let (end, ran) = drive_waits(
            first,
            vec![Waited::Cmd(Command::Chat(second)), Waited::Timeout],
        );

        assert_eq!(ran, vec!["a", "b"]);
        assert!(matches!(end, SessionEnd::Idle));
    }

    #[test]
    fn reuse_is_keyed_and_never_accidental() {
        assert!(reusable(Some(7), Some(7)));
        assert!(!reusable(Some(7), Some(8)));
        assert!(!reusable(Some(7), None));
        assert!(!reusable(None, Some(7)));
        // Two transient completions are unrelated prompts, not one session.
        assert!(!reusable(None, None));
    }
}
