//! In-process inference engine (Android) — the mobile half of the
//! `inference.rs` cfg seam.
//!
//! Desktop runs llama.cpp as a `llama-server` sidecar and talks to it over
//! loopback HTTP. Android cannot spawn a sidecar at all, so the same lifecycle
//! (`start` / `start_if_no_model` / `restart` / `shutdown`) is re-implemented
//! here on top of `kpack-engine`'s in-process `EngineBackend`, and `inference`
//! re-exports these four functions under `cfg(mobile)`. Everything else in
//! `inference.rs` — `Engine`, `EngineStatus`, `EngineInfo`, catalog/tier
//! resolution — is shared verbatim between the two platforms.
//!
//! ## Why a dedicated thread
//!
//! `EngineHandle` is deliberately **not `Send`**: `llama-cpp-2`'s
//! `LlamaLoraAdapter` holds a raw `NonNull` and a `LlamaContext` is
//! thread-bound, so a loaded stack must live entirely on the thread that
//! created it. This module therefore owns one long-lived inference thread; the
//! handle never leaves it, and callers communicate by mpsc command. That mirrors
//! desktop, where the same isolation comes free from the sidecar being a
//! separate process.
//!
//! ## Integrity
//!
//! The fail-closed sha256 gate is the backend's own `prepare_stack`, fed the
//! catalog's pinned hashes through `ModelSpec`/`AdapterSpec`. Nothing native is
//! touched until every artifact has passed, and a missing or mismatched file
//! errors rather than loading a reduced stack. Desktop's separate
//! `inference::verify_launch` does the same job for the sidecar; mobile uses the
//! engine's gate instead of stacking a second full hash of a multi-gigabyte
//! file on top of it.
//!
//! ## Scope
//!
//! Phase 1.2 is the **load** lifecycle: thread, commands, status/event mapping,
//! integrity. Generation (`chat_stream` / `chat_complete` / `chat_cancel`, the
//! calc tool-loop, the partial-turn flush) is 1.3–1.4, served by this same
//! thread from the handle it already owns.
//!
//! ## Two loops, one thread (task 1.5)
//!
//! Consecutive turns of the same conversation share one live `EngineSession` so
//! the KV cache is reused instead of rebuilt — without it, first-token latency
//! grows with the conversation and reaches 25–40 s mid-chat on the floor device.
//! A session **borrows** the handle, so it cannot be stored beside it (that is a
//! self-referential struct) and cannot outlive a function scope. The shape that
//! follows: an outer loop owning the handle, and an inner loop holding one
//! session for as long as turns keep arriving for the same chat.
//!
//! The inner loop's *routing* — which command may reuse the session, which must
//! close it — lives in [`crate::engine_serve`], where the desktop suite can test
//! it. Only [`serve_one_session`], which holds the borrow, stays here.

use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use std::ops::ControlFlow;
use std::sync::atomic::AtomicBool;

use kpack_engine::{
    AdapterRole, AdapterSpec, ChatMessage, ChatTemplate, EngineBackend, EngineHandle,
    EngineSession, LlamaEngine, LoadRequest, ModelSpec, Role, SessionConfig,
};
use tauri::{AppHandle, Emitter};

use crate::engine_serve::{ChatTurn, Command, SessionEnd, Waited};
use crate::engine_tool_loop::{LoopMessage, LoopRole, TurnSource};

use crate::inference::{Engine, EngineStatus};

/// Map the loaded model's template family onto the tool wire format it emits.
/// `Auto` means the template came from the GGUF and we have not been told the
/// family; the JSON-function shape is the safer assumption because the parser
/// recovers a fenced call anyway, whereas a ChatML preamble asks a Llama model
/// for syntax it does not produce.
pub(crate) fn tool_family(template: ChatTemplate) -> crate::engine_tools::ToolFamily {
    match template {
        ChatTemplate::ChatMl => crate::engine_tools::ToolFamily::ChatMl,
        ChatTemplate::Llama3 | ChatTemplate::Auto => {
            crate::engine_tools::ToolFamily::JsonFunction
        }
    }
}

/// The engine's mobile counterpart to desktop's `child: Mutex<Option<Child>>` —
/// the live inference thread, or `None` before first start / after shutdown.
#[derive(Default)]
pub(crate) struct ThreadSlot {
    inner: Mutex<Option<Running>>,
}

struct Running {
    tx: Sender<Command>,
    join: JoinHandle<()>,
}

/// Poison-tolerant lock: this slot is touched from `Drop`/teardown paths where
/// panicking a second time would abort the process.
fn lock_slot(slot: &ThreadSlot) -> std::sync::MutexGuard<'_, Option<Running>> {
    slot.inner.lock().unwrap_or_else(|p| p.into_inner())
}

/// Mobile counterpart of `inference::start`.
///
/// Idempotent with respect to the thread: if one is already running it is
/// re-tasked with a fresh `Load` rather than a second thread being spawned —
/// two threads would each hold a multi-gigabyte model.
pub fn start(app: AppHandle, engine: Arc<Engine>) {
    engine.set_status(EngineStatus::Starting);

    let mut slot = lock_slot(&engine.inproc);
    if let Some(running) = slot.as_ref() {
        if running.tx.send(Command::Load).is_ok() {
            return;
        }
        // The thread died (panic, or a shutdown that raced us). Drop the stale
        // handle and fall through to spawning a replacement.
        *slot = None;
    }

    let (tx, rx) = mpsc::channel();
    // Mark the thread alive BEFORE the spawn, exactly as desktop does, so a
    // concurrent restart waiting on this flag cannot observe `false` for an
    // about-to-run thread.
    engine.thread_alive.store(true, Ordering::Relaxed);
    engine.shutting_down.store(false, Ordering::Relaxed);

    let thread_app = app.clone();
    let thread_engine = engine.clone();
    match std::thread::Builder::new()
        .name("cleophis-inference".to_string())
        .spawn(move || run(thread_app, thread_engine, rx))
    {
        Ok(join) => {
            let _ = tx.send(Command::Load);
            *slot = Some(Running { tx, join });
        }
        Err(e) => {
            engine.thread_alive.store(false, Ordering::Relaxed);
            engine.set_status(EngineStatus::Failed);
            let _ = app.emit("engine-failed", format!("could not start inference thread: {e}"));
        }
    }
}

/// Mobile counterpart of `inference::start_if_no_model`. Keeps the desktop
/// double-start guard — the NoModel→Starting check-and-set — so the
/// download-completion path behaves identically on both platforms.
pub fn start_if_no_model(app: AppHandle, engine: Arc<Engine>) {
    if crate::inference::try_begin_start(&engine) {
        start(app, engine);
    } else {
        eprintln!("start_if_no_model: ignored, engine was not in NoModel state");
    }
}

/// Mobile counterpart of `inference::restart` (the tier-switch primitive).
///
/// Serialized against other restarts by the same `restart_lock` desktop uses.
/// The thread is stopped and joined before a new one starts, so two loaded
/// models never coexist — the mobile equivalent of desktop's "two llama-servers
/// would fight over the port", except here the scarce resource is RAM, and on
/// the 8 GB reference device a doubled 4B stack is fatal rather than merely
/// wasteful. Blocking; call it off the async runtime.
pub fn restart(app: AppHandle, engine: Arc<Engine>) {
    let _guard = engine.restart_lock.lock().unwrap_or_else(|p| p.into_inner());

    stop_thread(&engine);

    // If the app began tearing down while we were stopping the old thread, do
    // not relaunch — teardown has already run.
    if engine.closing.load(Ordering::Relaxed) {
        return;
    }
    // No cache to clear: the backend's `VerifyCache` is keyed by path, and a
    // tier switch resolves different files, so the new stack re-verifies on its
    // own. (Desktop's `clear_verify_caches` exists because its caches are keyed
    // per *slot* — model/adapter/contract — not per path.)
    start(app, engine.clone());
}

/// Mobile counterpart of `inference::shutdown`, called from the window
/// `Destroyed` handler. Unloads the model and joins the thread.
pub fn shutdown(engine: &Engine) {
    engine.closing.store(true, Ordering::Relaxed);
    stop_thread(engine);
}

/// Ask the inference thread to unload and exit, and wait for it. Waiting is the
/// point: the native model must be released before the process tears down or a
/// new one loads.
fn stop_thread(engine: &Engine) {
    engine.shutting_down.store(true, Ordering::Relaxed);

    let running = lock_slot(&engine.inproc).take();
    if let Some(running) = running {
        // A send error just means the thread is already gone; join either way.
        let _ = running.tx.send(Command::Shutdown);
        let _ = running.join.join();
    }
    engine.thread_alive.store(false, Ordering::Relaxed);
}

/// The inference thread body. Owns the `!Send` handle for its whole life.
fn run(app: AppHandle, engine: Arc<Engine>, rx: Receiver<Command>) {
    let _exit = ExitGuard {
        app: app.clone(),
        engine: engine.clone(),
    };

    let backend = LlamaEngine::new();

    // The self-evidencing kernels line (spec H3). llama.cpp reports the CPU
    // features it actually compiled and detected, so a device transcript
    // carries its own proof that the `armv8.2-a+dotprod` build is the one
    // running: **DOTPROD = 1, or every tok/s number measured on this build is
    // invalid** — the brief's rule, and until now nothing in the app printed
    // the evidence for it. CPU feature flags only; no user or token material,
    // so this is safe in a release log (security review M3).
    eprintln!("[kernels] {}", kpack_engine::backend_system_info());

    let mut handle: Option<Box<dyn EngineHandle>> = None;
    // A command the inner serve loop received but could not serve. It must be
    // processed here before anything is read from the channel, or the turn (or
    // the shutdown) it carries is lost.
    let mut pending: Option<Command> = None;

    loop {
        let cmd = match pending.take() {
            Some(cmd) => cmd,
            None => match rx.recv() {
                Ok(cmd) => cmd,
                // Every sender is gone; nothing further can arrive.
                Err(_) => break,
            },
        };

        match cmd {
            Command::Shutdown => break,
            Command::Chat(turn) => {
                let Some(h) = handle.as_mut() else {
                    // Fail-closed and legible: a chat arriving before the model
                    // is loaded is a UI-state bug, not something to paper over
                    // by silently loading here (which would block the caller on
                    // a multi-second load it never asked for).
                    let _ = turn
                        .reply
                        .send(Err("The on-device engine is not loaded.".to_string()));
                    continue;
                };
                let tier = crate::tier_select::effective_tier(&app);
                // The session borrows `handle`, and `Load` has to `unload()` it,
                // so the borrow is confined to this expression while the
                // decision it produces outlives it.
                let end = serve_one_session(&mut **h, session_config(&tier), turn, &rx);
                match end {
                    SessionEnd::Idle => {}
                    SessionEnd::Yield(next) => pending = Some(next),
                    SessionEnd::Closed => break,
                }
            }
            Command::Load => {
                // Release the previous model FIRST. Loading the new one while
                // the old is still resident would momentarily need both, which
                // the reference device does not have the RAM for.
                if let Some(old) = handle.take() {
                    old.unload();
                }
                engine.set_status(EngineStatus::Starting);
                match load_stack(&app, &backend) {
                    Ok(loaded) => {
                        handle = Some(loaded);
                        // No GPU offload on Android: CPU-first is policy
                        // (spec §1/H3), not a fallback.
                        engine.gpu_offload.store(false, Ordering::Relaxed);
                        engine.set_status(EngineStatus::Ready);
                        let _ = app.emit("engine-ready", engine.info());
                    }
                    Err(e) => {
                        eprintln!("engine_inproc: load failed: {e}");
                        engine.set_status(EngineStatus::Failed);
                        let _ = app.emit("engine-failed", e);
                    }
                }
            }
        }
    }

    if let Some(h) = handle {
        h.unload();
    }
}

/// How long an open session waits for the conversation's next turn before
/// releasing itself.
///
/// This bounds the one steady-state cost 1.5 adds. A live `LlamaContext` holds
/// its KV cache — on the order of 60 MB at the floor tier's 2048-token window —
/// and before 1.5 no context outlived a turn. Five minutes is chosen to be
/// longer than any pause inside a live conversation (read the answer, think,
/// type) and shorter than "the user has gone"; the penalty for guessing low is
/// one slow turn, and for guessing high it is memory held on the device with
/// the least of it, on an OS that resolves the argument by killing the app.
///
/// A deadline is a proxy. The right signal is the Android lifecycle, which
/// arrives with the §8 backgrounding work in Phase 5; this should become
/// "release on pause" then, and the timer should become the backstop.
const IDLE_SESSION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// Open one session and serve turns on it until [`SessionEnd`] says to stop.
///
/// This is the half of the serve loop that cannot leave `cfg(mobile)`: the
/// session borrows the handle, so it exists only on the inference thread. It
/// supplies the two effects [`crate::engine_serve::serve_loop`] cannot have —
/// running a turn, and blocking for the next command — and defers every
/// decision to the routing tested there.
///
/// Blocking for the next command *while holding a session* is the deliberate
/// part: that is what keeps the KV cache warm between a user's messages. The
/// thread was already idle-blocked between commands before 1.5; what is new is
/// that it now holds a live context while idle, which is the memory cost the
/// latency buys.
fn serve_one_session(
    handle: &mut dyn EngineHandle,
    cfg: SessionConfig,
    first: ChatTurn,
    rx: &Receiver<Command>,
) -> SessionEnd {
    let mut session = match handle.session(cfg) {
        Ok(session) => session,
        Err(e) => {
            let _ = first
                .reply
                .send(Err(format!("could not open a generation session: {e}")));
            return SessionEnd::Idle;
        }
    };

    let mut run_turn = |turn: ChatTurn| {
        let ChatTurn {
            convo,
            family,
            cancel,
            mut on_delta,
            reply,
            ..
        } = turn;
        // Borrowed, not owned: the session outlives the turn, which is the
        // whole mechanism. `SessionTurns` used to own its session, and that
        // ownership was what forced a fresh one per turn.
        let mut turns = SessionTurns {
            session: &mut *session,
            cancel,
        };
        let result = crate::engine_tool_loop::run(&mut turns, family, convo, &mut on_delta);
        // A closed reply channel just means the caller gave up; the turn still
        // ran to completion and the session is clean.
        let _ = reply.send(result);
    };
    let mut wait = || match rx.recv_timeout(IDLE_SESSION_TIMEOUT) {
        Ok(cmd) => Waited::Cmd(cmd),
        Err(mpsc::RecvTimeoutError::Timeout) => Waited::Timeout,
        Err(mpsc::RecvTimeoutError::Disconnected) => Waited::Closed,
    };

    crate::engine_serve::serve_loop(first, &mut run_turn, &mut wait)
}

/// Resolve the hero for the effective tier and load it, with the catalog's
/// pinned hashes attached so the backend's `prepare_stack` gates every artifact
/// before anything native is touched.
fn load_stack(
    app: &AppHandle,
    backend: &LlamaEngine,
) -> Result<Box<dyn EngineHandle>, String> {
    let launch = crate::inference::resolve_launch(app)
        .ok_or_else(|| "Model not downloaded yet.".to_string())?;
    let pinned = crate::inference::launch_hashes(app)?;

    let mut base = ModelSpec::new(&launch.model);
    if let Some(h) = pinned.model {
        base = base.with_sha256(h);
    }

    let mut adapters = Vec::new();
    if let Some(path) = &launch.behavioral_lora {
        let mut spec = AdapterSpec::new(AdapterRole::Behavioral, path);
        if let Some(h) = pinned.behavioral {
            spec = spec.with_sha256(h);
        }
        adapters.push(spec);
    }
    if let Some(path) = &launch.contract_lora {
        let mut spec = AdapterSpec::new(AdapterRole::Contract, path);
        if let Some(h) = pinned.contract {
            spec = spec.with_sha256(h);
        }
        adapters.push(spec);
    }

    let req = LoadRequest::new(base, adapters).with_template(template_for(app));
    backend.load(req).map_err(|e| {
        // The backend's own message distinguishes a hash mismatch from a
        // missing file; surface it, since "re-download" is the user's fix for
        // both and the distinction matters in a bug report.
        format!("The on-device engine failed to load the model: {e}")
    })
}

/// The chat template family for the current hero. `Auto` reads the template
/// embedded in the GGUF (the sidecar's `--jinja` behavior) and is the correct
/// default; the catalog only overrides it when a model needs a named family —
/// notably ChatML, whose start-of-turn `<think>` block the runtime strips.
/// Hand a command to the running inference thread. Returns false when there is
/// no thread — the engine was never started, or has been shut down.
pub(crate) fn send_command(engine: &Engine, cmd: Command) -> bool {
    lock_slot(&engine.inproc)
        .as_ref()
        .map(|running| running.tx.send(cmd).is_ok())
        .unwrap_or(false)
}

pub(crate) fn current_template(app: &AppHandle) -> ChatTemplate {
    template_for(app)
}

fn template_for(app: &AppHandle) -> ChatTemplate {
    match crate::inference::hero_chat_template(app).as_deref() {
        Some("llama3") | Some("llama-3") | Some("llama") => ChatTemplate::Llama3,
        Some("chatml") | Some("qwen") => ChatTemplate::ChatMl,
        _ => ChatTemplate::Auto,
    }
}

/// Bridges the tool loop's [`TurnSource`] onto a live `EngineSession`.
///
/// This is the adapter that lets the loop be written against a trait — and so
/// be tested on desktop — while still driving the real llama.cpp session here.
/// It lives on the inference thread and never crosses it, because the session
/// borrows the `!Send` handle.
///
/// Two lifetimes, and both are load-bearing. `'a` is the session's own borrow
/// of the engine handle; `'s` is this struct's shorter borrow of the session.
/// They cannot be collapsed into one: `&'s mut (dyn EngineSession + 'a)` is
/// **invariant** in its referent, so the compiler will not quietly shorten `'a`
/// to `'s` for us. The shape is what lets one session serve many turns — the
/// session is created once per conversation, each turn borrows it, and no turn
/// can take it with it.
struct SessionTurns<'a, 's> {
    session: &'s mut (dyn EngineSession + 'a),
    /// Set by `chat_cancel`. Checked per token, which is what makes cancel feel
    /// immediate rather than arriving at the end of a turn.
    cancel: Arc<AtomicBool>,
}

impl TurnSource for SessionTurns<'_, '_> {
    fn turn(
        &mut self,
        messages: &[LoopMessage],
        sink: &mut dyn FnMut(&str),
    ) -> Result<(), String> {
        let rendered: Vec<ChatMessage> = messages.iter().map(to_chat_message).collect();
        let cancel = self.cancel.clone();
        // `ControlFlow::Break` is kpack-engine's cooperative cancel: it stops
        // generation at the next token rather than tearing the session down, so
        // the handle stays reusable for the next turn.
        let mut tokens = |text: &str| -> ControlFlow<()> {
            sink(text);
            if cancel.load(Ordering::Relaxed) {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        };
        self.session
            .stream(&rendered, &mut tokens)
            .map(|_stats| ())
            .map_err(|e| e.to_string())
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

fn to_chat_message(m: &LoopMessage) -> ChatMessage {
    ChatMessage {
        role: match m.role {
            LoopRole::System => Role::System,
            LoopRole::User => Role::User,
            LoopRole::Assistant => Role::Assistant,
            LoopRole::Tool => Role::Tool,
        },
        content: m.content.clone(),
    }
}

/// Context window per tier (spec §2): 2048 on the floor, 4096 above. The A22 is
/// a floor device, so 2048 is the shipping default and the larger window is
/// opt-in by tier rather than by hope.
fn session_config(tier: &str) -> SessionConfig {
    SessionConfig {
        n_ctx: if tier == "low" { 2048 } else { 4096 },
        ..SessionConfig::default()
    }
}

/// Guarantees the engine never sits in a non-terminal state after the inference
/// thread goes away — including when it goes away by panicking, which would
/// otherwise leave the UI on a `Starting` spinner forever (brief: engine-thread
/// panics → `Failed`, never hung).
struct ExitGuard {
    app: AppHandle,
    engine: Arc<Engine>,
}

impl Drop for ExitGuard {
    fn drop(&mut self) {
        self.engine.thread_alive.store(false, Ordering::Relaxed);

        // A deliberate shutdown has already set the status it wants.
        if self.engine.shutting_down.load(Ordering::Relaxed) {
            return;
        }

        // Poison-tolerant: we may be unwinding from a panic that held this very
        // lock, and panicking again inside Drop aborts the process.
        let mut status = self
            .engine
            .status
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if *status == EngineStatus::Failed {
            return;
        }
        *status = EngineStatus::Failed;
        drop(status);

        let _ = self.app.emit(
            "engine-failed",
            "The on-device engine stopped unexpectedly.".to_string(),
        );
    }
}
