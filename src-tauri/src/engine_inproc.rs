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
//! integrity. Generation commands (`chat_stream` / `chat_complete` /
//! `chat_cancel`, the calc tool-loop and the partial-turn flush) are 1.3–1.4 and
//! will be served by this same thread — the session is opened from the handle it
//! already owns.

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

use crate::engine_tool_loop::{LoopMessage, LoopRole, TurnSource};

use crate::inference::{Engine, EngineStatus};

/// Map the loaded model's template family onto the tool wire format it emits.
/// `Auto` means the template came from the GGUF and we have not been told the
/// family; the JSON-function shape is the safer assumption because the parser
/// recovers a fenced call anyway, whereas a ChatML preamble asks a Llama model
/// for syntax it does not produce.
#[allow(dead_code)] // wired into the generation loop in 1.4
pub(crate) fn tool_family(template: ChatTemplate) -> crate::engine_tools::ToolFamily {
    match template {
        ChatTemplate::ChatMl => crate::engine_tools::ToolFamily::ChatMl,
        ChatTemplate::Llama3 | ChatTemplate::Auto => {
            crate::engine_tools::ToolFamily::JsonFunction
        }
    }
}

/// Commands accepted by the inference thread. Generation variants join this
/// enum in 1.4; the thread already owns the loaded handle they need.
pub(crate) enum Command {
    /// (Re)resolve the hero and load it, replacing whatever is loaded now.
    Load,
    /// Run one chat turn — the whole tool loop — against the loaded model.
    ///
    /// The loop runs *on the inference thread* because every round needs the
    /// session, and the session borrows the `!Send` handle. Only the streamed
    /// text and the final outcome cross back.
    ///
    // Constructed by the `chat_stream` / `chat_complete` commands, which are
    // the rest of 1.4. Remove this allow when they land.
    #[allow(dead_code)]
    Chat {
        convo: Vec<LoopMessage>,
        family: crate::engine_tools::ToolFamily,
        cancel: Arc<AtomicBool>,
        on_delta: Box<dyn FnMut(&str) + Send>,
        reply: mpsc::Sender<Result<crate::engine_tool_loop::LoopOutcome, String>>,
    },
    /// Unload and exit the thread.
    Shutdown,
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
    let mut handle: Option<Box<dyn EngineHandle>> = None;

    while let Ok(cmd) = rx.recv() {
        match cmd {
            Command::Shutdown => break,
            Command::Chat {
                convo,
                family,
                cancel,
                mut on_delta,
                reply,
            } => {
                let tier = crate::tier_select::effective_tier(&app);
                let result = match handle.as_mut() {
                    // Fail-closed and legible: a chat arriving before the model
                    // is loaded is a UI-state bug, not something to paper over
                    // by silently loading here (which would block the caller on
                    // a multi-second load it never asked for).
                    None => Err("The on-device engine is not loaded.".to_string()),
                    Some(h) => match h.session(session_config(&tier)) {
                        Ok(session) => {
                            let mut turns = SessionTurns { session, cancel };
                            crate::engine_tool_loop::run(
                                &mut turns,
                                family,
                                convo,
                                &mut on_delta,
                            )
                        }
                        Err(e) => Err(format!("could not open a generation session: {e}")),
                    },
                };
                // A closed reply channel just means the caller gave up; the
                // turn still ran to completion and the session is clean.
                let _ = reply.send(result);
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
struct SessionTurns<'a> {
    session: Box<dyn EngineSession + 'a>,
    /// Set by `chat_cancel`. Checked per token, which is what makes cancel feel
    /// immediate rather than arriving at the end of a turn.
    cancel: Arc<AtomicBool>,
}

impl TurnSource for SessionTurns<'_> {
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
