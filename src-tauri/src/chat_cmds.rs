//! Chat commands for the in-process engine (`chat_stream` / `chat_complete` /
//! `chat_cancel`).
//!
//! Desktop reaches the engine over loopback HTTP and runs its tool loop in JS
//! (`src/calc-loop.js`). Mobile has no server, so the loop runs Rust-side on the
//! inference thread and streamed text comes back over a Tauri `Channel` instead
//! of an SSE body.
//!
//! # Why these are compiled on desktop too
//!
//! The alternative was a second `generate_handler!` list under `cfg`, and two
//! fifty-entry lists drift — one gets a command the other doesn't, and nothing
//! catches it until a feature is silently missing on one platform. Instead the
//! commands exist everywhere and their **bodies** are cfg-gated; the desktop
//! bodies refuse immediately.
//!
//! Desktop behaviour is unchanged by this: its frontend never invokes these
//! (it uses the fetch transport), and a command that always returns `Err`
//! before touching any state cannot alter an existing flow. What it costs is
//! three inert names on the desktop invoke surface, which is the cheaper of the
//! two risks.

use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::AppHandle;

/// A conversation turn as the frontend sends it.
// On desktop these types exist only to give the refusing command stubs their
// signatures, so nothing reads the fields there.
#[cfg_attr(desktop, allow(dead_code))]
#[derive(Debug, Clone, Deserialize)]
pub struct WireMessage {
    pub role: String,
    pub content: String,
}

/// One calculation, shaped for the UI's audit strip.
#[cfg_attr(desktop, allow(dead_code))]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalcView {
    pub expression: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What a streaming turn emits. Mirrors the desktop loop's return shape so the
/// frontend's transport seam (2.1) can hand both to the same renderer.
#[cfg_attr(desktop, allow(dead_code))]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "event", content = "data")]
pub enum ChatEvent {
    /// Display text only — tool syntax has already been suppressed.
    Delta { text: String },
    Done {
        content: String,
        calculations: Vec<CalcView>,
    },
    Error { message: String },
}

/// Registry of in-flight turns, so `chat_cancel` can reach one by id.
#[derive(Default)]
pub struct ChatCancels {
    #[cfg(mobile)]
    inner: std::sync::Mutex<
        std::collections::HashMap<String, std::sync::Arc<std::sync::atomic::AtomicBool>>,
    >,
}

#[cfg(mobile)]
mod imp {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{mpsc, Arc};

    use tauri::Manager;

    use crate::engine_inproc::{tool_family, Command};
    use crate::engine_tool_loop::{LoopMessage, LoopRole, LoopOutcome};
    use crate::inference::Engine;

    impl ChatCancels {
        fn begin(&self, request_id: &str) -> Arc<AtomicBool> {
            let flag = Arc::new(AtomicBool::new(false));
            self.inner
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(request_id.to_string(), flag.clone());
            flag
        }

        fn end(&self, request_id: &str) {
            self.inner
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(request_id);
        }

        pub(super) fn cancel(&self, request_id: &str) {
            if let Some(flag) = self
                .inner
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get(request_id)
            {
                flag.store(true, Ordering::Relaxed);
            }
        }
    }

    fn to_loop_messages(app: &AppHandle, wire: Vec<WireMessage>) -> Vec<LoopMessage> {
        let mut out: Vec<LoopMessage> = Vec::with_capacity(wire.len() + 1);
        let preamble =
            crate::engine_tools::tools_preamble(tool_family(
                crate::engine_inproc::current_template(app),
            ));

        let mut injected = false;
        for m in wire {
            let role = match m.role.as_str() {
                "system" => LoopRole::System,
                "assistant" => LoopRole::Assistant,
                "tool" => LoopRole::Tool,
                _ => LoopRole::User,
            };
            // The tool contract rides on the system turn, because
            // `llama_chat_apply_template` has no tools parameter to put it in.
            let content = if role == LoopRole::System && !injected {
                injected = true;
                format!("{}{}", m.content, preamble)
            } else {
                m.content
            };
            out.push(LoopMessage { role, content });
        }
        if !injected {
            out.insert(
                0,
                LoopMessage {
                    role: LoopRole::System,
                    content: preamble.trim_start().to_string(),
                },
            );
        }
        out
    }

    fn calc_views(outcome: &LoopOutcome) -> Vec<CalcView> {
        outcome
            .calculations
            .iter()
            .map(|c| CalcView {
                expression: c.expression.clone(),
                display: c.display.clone(),
                error: c.error.clone(),
            })
            .collect()
    }

    /// Run one turn on the inference thread, forwarding deltas through `on_delta`.
    fn run_turn(
        app: &AppHandle,
        wire: Vec<WireMessage>,
        cancel: Arc<AtomicBool>,
        on_delta: Box<dyn FnMut(&str) + Send>,
    ) -> Result<LoopOutcome, String> {
        let engine = app.state::<Arc<Engine>>();
        let convo = to_loop_messages(app, wire);
        let family = tool_family(crate::engine_inproc::current_template(app));

        let (reply_tx, reply_rx) = mpsc::channel();
        let sent = crate::engine_inproc::send_command(
            &engine,
            Command::Chat {
                convo,
                family,
                cancel,
                on_delta,
                reply: reply_tx,
            },
        );
        if !sent {
            return Err("The on-device engine is not running.".to_string());
        }
        // The inference thread always replies, including on error; a recv error
        // means the thread died mid-turn, which its exit guard has already
        // surfaced as `engine-failed`.
        reply_rx
            .recv()
            .map_err(|_| "The on-device engine stopped during generation.".to_string())?
    }

    pub(super) async fn chat_stream(
        request_id: String,
        messages: Vec<WireMessage>,
        on_event: Channel<ChatEvent>,
        app: AppHandle,
    ) -> Result<(), String> {
        let cancel = app.state::<ChatCancels>().begin(&request_id);
        let id = request_id.clone();

        let app_for_turn = app.clone();
        let stream_channel = on_event.clone();
        let result = tauri::async_runtime::spawn_blocking(move || {
            let delta_channel = stream_channel.clone();
            let on_delta = Box::new(move |text: &str| {
                // A failed send means the webview went away; generation is
                // cancelled by the next `cancelled()` check rather than here,
                // so the session unwinds cleanly.
                let _ = delta_channel.send(ChatEvent::Delta {
                    text: text.to_string(),
                });
            }) as Box<dyn FnMut(&str) + Send>;
            run_turn(&app_for_turn, messages, cancel, on_delta)
        })
        .await
        .map_err(|_| "The generation task failed to run.".to_string())?;

        // Always deregister, whether the turn succeeded, errored, or was
        // cancelled — a stale entry would let a later `chat_cancel` with a
        // recycled id flip a flag nobody is watching.
        app.state::<ChatCancels>().end(&id);

        match result {
            Ok(outcome) => {
                let _ = on_event.send(ChatEvent::Done {
                    content: outcome.content.clone(),
                    calculations: calc_views(&outcome),
                });
                Ok(())
            }
            Err(message) => {
                let _ = on_event.send(ChatEvent::Error {
                    message: message.clone(),
                });
                Err(message)
            }
        }
    }

    pub(super) async fn chat_complete(
        messages: Vec<WireMessage>,
        app: AppHandle,
    ) -> Result<String, String> {
        let cancel = Arc::new(AtomicBool::new(false));
        let outcome = tauri::async_runtime::spawn_blocking(move || {
            run_turn(&app, messages, cancel, Box::new(|_: &str| {}))
        })
        .await
        .map_err(|_| "The generation task failed to run.".to_string())??;
        Ok(outcome.content)
    }

    pub(super) fn chat_cancel(request_id: String, app: AppHandle) {
        app.state::<ChatCancels>().cancel(&request_id);
    }
}

/// Stream one chat turn, emitting [`ChatEvent`]s on `on_event`.
#[tauri::command]
pub async fn chat_stream(
    request_id: String,
    messages: Vec<WireMessage>,
    on_event: Channel<ChatEvent>,
    app: AppHandle,
) -> Result<(), String> {
    #[cfg(mobile)]
    {
        return imp::chat_stream(request_id, messages, on_event, app).await;
    }
    #[cfg(desktop)]
    {
        let _ = (request_id, messages, on_event, app);
        Err("chat_stream is the mobile transport; desktop uses the sidecar.".to_string())
    }
}

/// Non-streaming completion — the two call sites (auto-title, analysis) that
/// want an answer rather than a stream. Runs the same loop; no suppressor
/// output is forwarded because nobody is watching it.
#[tauri::command]
pub async fn chat_complete(
    messages: Vec<WireMessage>,
    app: AppHandle,
) -> Result<String, String> {
    #[cfg(mobile)]
    {
        return imp::chat_complete(messages, app).await;
    }
    #[cfg(desktop)]
    {
        let _ = (messages, app);
        Err("chat_complete is the mobile transport; desktop uses the sidecar.".to_string())
    }
}

/// Cooperatively cancel an in-flight [`chat_stream`]. Idempotent, and harmless
/// for an id that has already finished.
#[tauri::command]
pub fn chat_cancel(request_id: String, app: AppHandle) {
    #[cfg(mobile)]
    imp::chat_cancel(request_id, app);
    #[cfg(desktop)]
    let _ = (request_id, app);
}
