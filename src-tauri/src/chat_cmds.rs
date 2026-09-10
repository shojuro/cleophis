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

/// What the desktop stubs return. Names the platform and the command family
/// explicitly: a misrouted invoke should be diagnosable from a single line in a
/// bug report, never a mystery about which half of the transport seam fired.
#[cfg_attr(mobile, allow(dead_code))]
const DESKTOP_REFUSAL: &str =
    "in-process chat commands are mobile-only; this is a desktop build, which \
     reaches the engine through the llama-server sidecar instead";

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
    /// `rename_all` on the enum above renames the VARIANTS, not their fields —
    /// which nothing had noticed while every field was one lower-case word.
    /// `message_id` is the first that would wire as snake_case, so the variant
    /// carries its own rename.
    #[serde(rename_all = "camelCase")]
    Done {
        content: String,
        calculations: Vec<CalcView>,
        /// The id of the assistant row [`settle`] just finalized for this
        /// turn, or `None` when there was no checkpoint to finalize (no
        /// signed-in account, no chat, or the finalize itself failed).
        ///
        /// It exists because the row is written BEFORE this event: the front
        /// end's guard verdict for a supervised reply has to be ATTACHED to
        /// that row (`attach_guard`), since appending would file a second
        /// assistant row for one turn. A `None` here is the front end's
        /// signal to fall back to `append_message`, which upgrades a row
        /// still marked partial and inserts otherwise.
        message_id: Option<i64>,
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

/// The latest thermal verdict, so a webview that reloaded can re-ask for it
/// (hazard H6, acceptance A7). Managed on both platforms so the command
/// surface stays uniform (D-1); desktop never populates it.
///
/// **Why a re-assert path is needed at all, given the event.**
/// `ThermalWatch` emits only on *transitions*, and it lives on the inference
/// thread — which outlives the webview. If the WebView renderer is killed
/// under memory pressure, the frontend loses `state.thermal` while the watch
/// keeps `notified = true`. `on_token`'s onset branch is guarded by
/// `!self.notified`, so **no further onset can ever be emitted**: the notice
/// stays withdrawn for the rest of a hot session. Silent, and permanent until
/// the device happens to cool.
///
/// That is not a hypothetical on the hardware this targets. Rotation is
/// already covered (`configChanges` includes `orientation|screenSize`), so the
/// remaining path is renderer death under memory pressure — which is ordinary
/// on a 4 GB floor device running a local model, and which *correlates with
/// the very thermal load this feature detects*. It would also corrupt the A7
/// soak run itself: a founder device session, the scarcest resource on this
/// track, silently recording a false negative.
#[derive(Default)]
pub struct ThermalState {
    #[cfg(mobile)]
    inner: std::sync::Mutex<Option<crate::engine_thermal::ThermalNotice>>,
}

#[cfg(mobile)]
impl ThermalState {
    /// Called by the inference thread on every transition, so the stored
    /// verdict is whatever was last emitted.
    pub(crate) fn set(&self, notice: crate::engine_thermal::ThermalNotice) {
        *self.inner.lock().unwrap_or_else(|p| p.into_inner()) = Some(notice);
    }

    fn get(&self) -> Option<crate::engine_thermal::ThermalNotice> {
        *self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }
}

#[cfg(mobile)]
mod imp {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{mpsc, Arc};

    use tauri::Manager;

    use crate::engine_inproc::tool_family;
    use crate::engine_serve::{ChatTurn, Command};

    /// How much generated text may accumulate between checkpoints. A write per
    /// token would put SQLite inside the token loop; this bounds what a kill
    /// can lose to roughly a sentence.
    const CHECKPOINT_EVERY: usize = 256;

    /// Shared between the streaming closure and the settle step.
    #[derive(Default)]
    struct Progress {
        text: String,
        /// Length already checkpointed.
        written: usize,
        /// The partial row's id, once one exists.
        row: Option<i64>,
    }
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

    /// The wire turns as the inference loop wants them, with the calc tool
    /// contract appended to the system turn.
    ///
    /// P2.9: an entry whose catalog declares `"tools": false` gets NO preamble
    /// and no synthesised system turn — its system content is the frontend's,
    /// verbatim. That is not a tidiness preference. The supervised triage
    /// entry was gated with exactly the catalog's `systemPrompt` as its only
    /// system content, so a tool contract silently appended on device would
    /// make the shipped context a different one from the gated context.
    /// `tools` unset means the historical behaviour, so the tutor is
    /// unaffected.
    fn to_loop_messages(app: &AppHandle, wire: Vec<WireMessage>) -> Vec<LoopMessage> {
        let mut out: Vec<LoopMessage> = Vec::with_capacity(wire.len() + 1);
        // Both arms are `&'static str` — `tools_preamble` returns one, and the
        // off arm must too, or the `if` fails to unify (E0308). Every use below
        // (`format!`, `is_empty`, `trim_start`) works on `&str` unchanged.
        let preamble: &'static str = if crate::inference::hero_tools_enabled(app) {
            crate::engine_tools::tools_preamble(tool_family(
                crate::engine_inproc::current_template(app),
            ))
        } else {
            ""
        };

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
        // With tools off there is nothing to carry, so a conversation that
        // arrived without a system turn keeps arriving without one — inserting
        // an empty system message would add a turn the gate never saw.
        if !injected && !preamble.is_empty() {
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
    ///
    /// `chat_key` is the conversation this turn belongs to, and it is what lets
    /// the inference thread keep one session — and so one warm KV cache — alive
    /// across consecutive turns (task 1.5). `None` means the turn belongs to no
    /// conversation and must neither inherit nor leave a prefix.
    fn run_turn(
        app: &AppHandle,
        chat_key: Option<i64>,
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
            Command::Chat(ChatTurn {
                convo,
                family,
                chat_key,
                cancel,
                on_delta,
                reply: reply_tx,
            }),
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
        chat_id: Option<i64>,
        messages: Vec<WireMessage>,
        on_event: Channel<ChatEvent>,
        app: AppHandle,
    ) -> Result<(), String> {
        let cancel = app.state::<ChatCancels>().begin(&request_id);
        let id = request_id.clone();

        // Foreground service up for the duration of the turn (spec §8, H4/H5),
        // so Android does not kill generation mid-reply under memory pressure.
        // Paired with the `false` beside the cancel-registry teardown below —
        // the one place that already runs on every exit path.
        //
        // `chat_complete` is deliberately NOT wrapped. Its callers are
        // auto-title and analysis: short, not user-visible, and capable of
        // firing just after the user backgrounds the app — which is exactly
        // when Android 12+ forbids starting a foreground service. A crash there
        // would trade a real restriction for a title nobody is waiting for.
        crate::mobile_native::inference_service(true);

        // Checkpointing needs a signed-in account and a chat to write into.
        // Without both, generation still streams — it just is not recoverable,
        // which is the honest degradation rather than a refusal.
        let checkpoint_to =
            chat_id.and_then(|c| crate::convstore::current_user_id(&app).map(|u| (u, c)));
        let progress = Arc::new(std::sync::Mutex::new(Progress::default()));

        let app_for_turn = app.clone();
        let stream_channel = on_event.clone();
        let delta_progress = progress.clone();
        let delta_target = checkpoint_to.clone();
        let result = tauri::async_runtime::spawn_blocking(move || {
            let delta_channel = stream_channel.clone();
            let store_app = app_for_turn.clone();
            let on_delta = Box::new(move |text: &str| {
                // A failed send means the webview went away; generation is
                // cancelled by the next `cancelled()` check rather than here,
                // so the session unwinds cleanly.
                let _ = delta_channel.send(ChatEvent::Delta {
                    text: text.to_string(),
                });
                let Some((user, chat)) = delta_target.as_ref() else {
                    return;
                };
                let mut p = delta_progress.lock().unwrap_or_else(|e| e.into_inner());
                p.text.push_str(text);
                // Throttled: a write per token would put SQLite in the token
                // loop. Losing at most CHECKPOINT_EVERY characters to a kill is
                // the deliberate trade.
                if p.text.len() - p.written >= CHECKPOINT_EVERY {
                    p.written = p.text.len();
                    let store = store_app.state::<crate::convstore::ConvStore>();
                    match store.checkpoint_partial(user, *chat, &p.text) {
                        Ok(row) => p.row = Some(row),
                        // A checkpoint failure must not kill a turn the user is
                        // watching; recoverability degrades, generation does not.
                        Err(e) => eprintln!("chat_stream: checkpoint failed: {e}"),
                    }
                }
            }) as Box<dyn FnMut(&str) + Send>;
            run_turn(&app_for_turn, chat_id, messages, cancel, on_delta)
        })
        .await
        .map_err(|_| "The generation task failed to run.".to_string())?;

        // Always deregister, whether the turn succeeded, errored, or was
        // cancelled — a stale entry would let a later `chat_cancel` with a
        // recycled id flip a flag nobody is watching.
        app.state::<ChatCancels>().end(&id);
        // Same "always" argument, which is why the demotion lives here rather
        // than in the success arm: a foreground service left running after a
        // cancelled or failed turn is a permanent notification for work that
        // stopped, and the user has no way to clear it.
        crate::mobile_native::inference_service(false);

        match result {
            Ok(outcome) => {
                let message_id = settle(&app, &checkpoint_to, &progress, Some(&outcome));
                let _ = on_event.send(ChatEvent::Done {
                    content: outcome.content.clone(),
                    calculations: calc_views(&outcome),
                    message_id,
                });
                Ok(())
            }
            Err(message) => {
                // Deliberately NOT finalized: the row stays marked partial,
                // which is what makes it recoverable as truncated rather than
                // indistinguishable from a short finished answer. The returned
                // id is `None` for that same reason — a failed turn has no
                // finished reply for a verdict to be attached to.
                settle(&app, &checkpoint_to, &progress, None);
                let _ = on_event.send(ChatEvent::Error {
                    message: message.clone(),
                });
                Err(message)
            }
        }
    }

    /// Close out the checkpoint row: finalize on success (a success whose
    /// content is EMPTY — Stop before the first token, or a think-only reply —
    /// is discarded instead, so no blank row is finalized), and on failure
    /// either discard it (nothing was produced) or leave it marked partial.
    ///
    /// Returns the id of the row it FINALIZED, which the caller puts on
    /// [`ChatEvent::Done`] so the front end can attach a guard verdict to that
    /// row rather than append a second one. `None` on every path where no
    /// finalized row exists — no account or chat to write into, no row, or a
    /// finalize that failed. In that last case the row is still marked partial,
    /// so the front end's `append_message` fallback upgrades it in place; a
    /// `Some` returned for a failed finalize would instead point `attach_guard`
    /// at a row still holding the last checkpoint's text.
    fn settle(
        app: &AppHandle,
        target: &Option<(String, i64)>,
        progress: &Arc<std::sync::Mutex<Progress>>,
        outcome: Option<&LoopOutcome>,
    ) -> Option<i64> {
        let Some((user, chat)) = target.as_ref() else {
            return None;
        };
        let p = progress.lock().unwrap_or_else(|e| e.into_inner());
        let store = app.state::<crate::convstore::ConvStore>();

        let Some(outcome) = outcome else {
            if p.text.trim().is_empty() {
                // Nothing was generated, so there is no truncated turn to
                // recover — an empty partial row would just be litter.
                let _ = store.discard_partial(user, *chat);
            }
            return None;
        };

        // A SUCCESSFUL turn that produced no text is not a reply either, and
        // finalizing one is worse here than the litter it is on the tutor's
        // side. Stop before the first token returns `Ok` with empty content;
        // finalizing wrote a blank, non-partial assistant row, and the front
        // end persists nothing for an empty reply — so the row kept no verdict,
        // and on the next open `replayMessage` withheld it behind "Unverified
        // reply". A health worker who pressed Stop was shown a safety notice
        // about a reply that never existed.
        //
        // So it takes the same path the `None` branch above takes: discard the
        // checkpoint row, and return `None` for the id — there is no finalized
        // reply for `attach_guard` to be pointed at. `ChatEvent::Done` still
        // fires with the empty content it always did, and the front end's
        // `finishStream` removes the bubble and persists nothing, exactly as
        // before. The tool loop is unaffected: it has already returned by the
        // time `settle` runs, and a turn that calculated but never spoke is
        // given fallback copy by `tool_loop::finish`, so it is not empty here.
        if outcome.content.trim().is_empty() {
            let _ = store.discard_partial(user, *chat);
            return None;
        }

        // The final text may differ from the streamed text (the loop
        // substitutes fallback copy for a silent or capped turn), so finalize
        // with the outcome rather than the accumulator.
        let calcs = serde_json::to_value(calc_views(outcome)).ok();
        let row = match p.row {
            Some(row) => Some(row),
            // Short turns can finish before the first checkpoint fires.
            None => store.checkpoint_partial(user, *chat, &outcome.content).ok(),
        };
        let row = row?;
        match store.finalize_partial(user, row, &outcome.content, None, calcs) {
            Ok(()) => Some(row),
            Err(e) => {
                eprintln!("chat_stream: finalize failed: {e}");
                None
            }
        }
    }

    pub(super) async fn chat_complete(
        messages: Vec<WireMessage>,
        app: AppHandle,
    ) -> Result<String, String> {
        let cancel = Arc::new(AtomicBool::new(false));
        let outcome = tauri::async_runtime::spawn_blocking(move || {
            // Deliberately unkeyed. These prompts (auto-title, analysis) are not
            // continuations of the conversation they are about — keying them to
            // its chat id would hand them a prefix they do not extend, and evict
            // the one the next real turn wants.
            run_turn(&app, None, messages, cancel, Box::new(|_: &str| {}))
        })
        .await
        .map_err(|_| "The generation task failed to run.".to_string())??;
        Ok(outcome.content)
    }

    pub(super) fn chat_cancel(request_id: String, app: AppHandle) {
        app.state::<ChatCancels>().cancel(&request_id);
    }

    /// A3 — run the Stage-5 probe set through the **shipped engine path** and
    /// adjudicate it.
    ///
    /// # Why this is here and not in `examples/probe.rs`
    ///
    /// §11 A3 asks for the probes *in-app*. The CLI harness links its own
    /// `LlamaEngine`, sends its own system prompt and never sees the tools
    /// preamble, the tool loop or `ThinkStripper` — so a green from it says
    /// nothing about what a user gets. This calls [`chat_complete`], the same
    /// entry point auto-title uses, so every arm goes through the real
    /// composed stack, the real template resolution and the real suppressor.
    ///
    /// One consequence, stated because it makes transcripts differ from the
    /// CLI's: `to_loop_messages` appends the tool contract to the system turn.
    /// That is what the shipped path does, so it is what the gate must measure.
    ///
    /// Each arm is `chat_key: None` — a transient turn that neither inherits a
    /// prefix nor leaves one — which is the in-app equivalent of the CLI's
    /// fresh session per probe. Six arms in sequence, no interleaving, because
    /// the inference thread serves one turn at a time anyway.
    #[cfg(debug_assertions)]
    pub(super) async fn stage5_probe(
        app: AppHandle,
    ) -> Result<crate::engine_probes::Verdict, String> {
        use crate::engine_probes::adjudicate::{
            probe_verdict, render, Transcript, STAGE5_ARMS,
        };

        // `?`, not a default. A probe run whose system prompt was invented by
        // the harness measures a configuration no user runs, and it would look
        // exactly like a real result.
        let system = crate::inference::hero_system_prompt(&app).ok_or_else(|| {
            "the catalog hero declares no systemPrompt, so there is nothing to \
             probe the behavioural adapter WITH — this would measure a \
             configuration no user ever runs"
                .to_string()
        })?;
        let stack = stack_label(&app);
        eprintln!("[stage5] running {} arms against {stack}", STAGE5_ARMS.len());

        let mut transcripts = Vec::with_capacity(STAGE5_ARMS.len());
        for spec in STAGE5_ARMS {
            let messages = vec![
                WireMessage {
                    role: "system".to_string(),
                    content: system.clone(),
                },
                WireMessage {
                    role: "user".to_string(),
                    content: spec.prompt.to_string(),
                },
            ];
            let reply = chat_complete(messages, app.clone()).await?;
            // Printed BEFORE adjudication, and in full. The verdicts below are
            // this module's opinion; the transcript is the evidence, and the
            // founder session exists partly to calibrate the former against
            // the latter.
            eprintln!("[stage5] ===== {} =====", spec.label);
            eprintln!("[stage5] Q: {}", spec.prompt);
            eprintln!("[stage5] A: {}", reply.trim());
            transcripts.push(Transcript {
                probe: spec.probe,
                arm: spec.arm,
                reply,
            });
        }

        let verdict = probe_verdict(&transcripts, &stack);
        eprint!("{}", render(&verdict));
        Ok(verdict)
    }

    /// What the probes actually ran against, for [`Verdict::stack`].
    ///
    /// §11 A3: "Run against the full composed stack (§0); a probe run against
    /// a reduced stack is a different gate and is labeled as such." So a
    /// missing adapter is named in the label rather than being invisible —
    /// the transcript of a base-only run looks like any other transcript, and
    /// nothing else in the output would say which gate it was.
    #[cfg(debug_assertions)]
    fn stack_label(app: &AppHandle) -> String {
        let Some(launch) = crate::inference::resolve_launch(app) else {
            return "UNRESOLVED (no launchable hero on disk)".to_string();
        };
        let base = |p: &std::path::Path| {
            p.file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "?".to_string())
        };
        let label = |slot: &Option<std::path::PathBuf>| match slot {
            Some(p) => base(p),
            None => "ABSENT".to_string(),
        };
        let reduced = launch.behavioral_lora.is_none() || launch.contract_lora.is_none();
        format!(
            "base={} + behavioral={} + contract={}{}",
            base(&launch.model),
            label(&launch.behavioral_lora),
            label(&launch.contract_lora),
            if reduced {
                "  [REDUCED STACK — §11 A3: a different gate from the full \
                 composed stack, and must not be reported as the A3 result]"
            } else {
                ""
            }
        )
    }
}

/// Stream one chat turn, emitting [`ChatEvent`]s on `on_event`.
#[tauri::command]
pub async fn chat_stream(
    request_id: String,
    chat_id: Option<i64>,
    messages: Vec<WireMessage>,
    on_event: Channel<ChatEvent>,
    app: AppHandle,
) -> Result<(), String> {
    #[cfg(mobile)]
    {
        return imp::chat_stream(request_id, chat_id, messages, on_event, app).await;
    }
    #[cfg(desktop)]
    {
        let _ = (request_id, chat_id, messages, on_event, app);
        Err(DESKTOP_REFUSAL.to_string())
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
        Err(DESKTOP_REFUSAL.to_string())
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

/// The current thermal verdict, or `null` if nothing has been decided yet.
///
/// A *re-assert* path rather than an event: the frontend calls this once at
/// boot so a reloaded webview recovers a notice it would otherwise never be
/// told about again. See [`ThermalState`] for why the event alone is not
/// enough.
#[tauri::command]
pub fn chat_thermal_state(
    app: AppHandle,
) -> Result<Option<crate::engine_thermal::ThermalNotice>, String> {
    #[cfg(mobile)]
    {
        use tauri::Manager;
        return Ok(app.state::<ThermalState>().get());
    }
    #[cfg(desktop)]
    {
        let _ = app;
        Err(DESKTOP_REFUSAL.to_string())
    }
}

/// **Q1 — the debug-only throttle-notice pipeline check** (hazard H6,
/// acceptance A7).
///
/// `run: false` is an availability probe: it answers whether this build has the
/// affordance, changes nothing and emits nothing. The frontend calls it once at
/// boot to decide whether to show the button, so a release APK never presents a
/// control that would only refuse.
///
/// `run: true` **toggles**: it raises the notice if none is up and withdraws it
/// if one is. That is what keeps the affordance from falsifying the UI it
/// verifies — see [`crate::engine_thermal::synthetic_recovery`] — and it means
/// two taps demonstrate both edges of §8, including the withdrawal, which a
/// soak cannot produce on demand.
///
/// # Why this exists
///
/// The A7 soak is a ~20-minute session on a hot phone, and the detector is
/// deliberately biased toward false negatives — so a soak that produces no
/// notice is a *possible correct result*. Run on its own it cannot distinguish
/// "working, and correctly quiet" from "broken, and silent", and the outcome is
/// uninterpretable. Steering's split makes the two unknowns fail differently:
/// this answers *does the notice pipeline work at all?* before the soak starts,
/// on a cold phone, in about a second.
///
/// # ⚠ EVIDENCE SCOPE — decided deliberately, and it is narrower than it looks
///
/// Two implementations were available. This is the one that **builds its own
/// [`crate::engine_thermal::ThermalWatch`]** and feeds it a scripted cadence on
/// a virtual clock. The rejected alternative was a shared flag offsetting
/// `ThermalProbe`'s clock during a live turn, which would also have exercised
/// the production probe.
///
/// **What a green here proves:** the detector's arithmetic, the
/// [`crate::engine_thermal::ThermalNotice`] payload and its rates, the
/// [`ThermalState`] store, the `"thermal-notice"` event, the frontend listener,
/// §8's copy, and the prominent-row-then-pill decay — end to end, on the real
/// device, in the real app.
///
/// **What it does NOT prove, and must never be read as proving:** that
/// `ThermalProbe::on_token` is called from the generation loop at all, or that
/// its `Instant` clock behaves. This command supplies its own timestamps
/// precisely so it needs no turn in flight, and that is the same reason it
/// cannot speak for the wiring.
///
/// **Why that gap is acceptable here.** The rejected alternative would have
/// covered the wiring only weakly — offsetting the clock *overwrites* the real
/// cadence, so it proves the sink is called some number of times and destroys
/// the evidence of how often. The `[thermal]` trace covers it properly instead,
/// by logging `sink_calls` against the engine's independent
/// `GenStats::generated_tokens` during the soak itself, which is where the
/// answer is actually needed. It also costs the founder nothing: option (a)
/// needed a live turn plus ~38 tokens of generation after a baseline existed,
/// which is not a five-second check on a cold phone.
#[tauri::command]
pub fn chat_thermal_selftest(
    run: bool,
    app: AppHandle,
) -> Result<Option<crate::engine_thermal::ThermalNotice>, String> {
    #[cfg(all(mobile, debug_assertions))]
    {
        use tauri::{Emitter, Manager};
        if !run {
            return Ok(None);
        }
        let state = app.state::<ThermalState>();
        // TOGGLE, and the direction comes from the BACKEND's stored verdict
        // rather than from an argument. The frontend's copy is the thing that
        // gets lost when the renderer dies — which is the entire reason
        // `ThermalState` exists — so letting the caller say which edge to emit
        // would reintroduce the desynchronisation this is meant to survive.
        //
        // `?` on purpose: a script that cannot fire is a red, not a silence.
        // "No notice appeared" is the one reading Q1 exists to disambiguate, so
        // a failure must arrive as a message rather than as nothing happening.
        let currently_throttled = state.get().is_some_and(|n| n.throttled);
        let notice = if currently_throttled {
            crate::engine_thermal::selftest::synthetic_recovery()?
        } else {
            crate::engine_thermal::selftest::synthetic_collapse()?
        };
        // Stored before emitted, in the production order — see `ThermalState`.
        state.set(notice);
        let _ = app.emit("thermal-notice", notice);
        return Ok(Some(notice));
    }
    #[cfg(not(all(mobile, debug_assertions)))]
    {
        let _ = (run, app);
        // Two refusals, not one, because they mean different things to whoever
        // reads the bug report: the wrong platform, versus the right platform
        // and a release build. Inlined rather than named as consts — a const is
        // live in exactly one of these configurations and dead in the other,
        // which is a dead-code allow to get wrong for no benefit.
        #[cfg(desktop)]
        return Err(DESKTOP_REFUSAL.to_string());
        #[cfg(not(desktop))]
        return Err("the thermal selftest is a debug-build affordance and is \
                    compiled out of release builds"
            .to_string());
    }
}

/// **A3 — the Stage-5 behavioural probes, in-app** (spec §11 A3;
/// `adapter-distribution-design.md` §8).
///
/// `run: false` is the availability probe, in Q1's mould: it changes nothing,
/// generates nothing, and answers only whether this build has the affordance.
/// A release APK refuses it, so no control is ever presented that would only
/// fail.
///
/// `run: true` sends six prompts through the shipped engine path and returns
/// the adjudicated [`crate::engine_probes::Verdict`], having also written the
/// full transcripts and the verdicts to logcat under `[stage5]`.
///
/// # ⚠ EVIDENCE SCOPE — what a green here does and does not mean
///
/// **It means:** on this device, through the real composed stack, the real
/// template and the real suppressor, the adapter declined a mathematician who
/// does not exist *while describing one who does*, disputed `5 + 5 = 9`,
/// conceded a correct correction, and refused to dose a cardiac emergency
/// *while answering a benign health question*. The paired arms are what make
/// each of those a statement about behaviour rather than about a disposition
/// to refuse.
///
/// **It does not mean `4/4`.** Three probes are machine-decided; probe 3 is
/// machine-screened and the founder still owes it an answer — see
/// [`crate::engine_probes::adjudicate::CONCESSION_HUMAN_QUESTION`]. Any probe
/// whose evidence is contradictory comes back `UNDECIDED`, which is neither a
/// pass nor a fail and must be reported as its own thing.
///
/// **It says nothing about a stack it did not run on.** [`Verdict::stack`]
/// carries what was actually loaded, and labels a reduced stack as the
/// different gate §11 says it is.
///
/// # Cost
///
/// Six prompts. At the A22's measured ~8 tok/s that is under a minute of
/// device time — less than connecting the phone. The percentage this was first
/// quoted as ("+75%") was true and misleading, which is now a Convention.
///
/// # Why the return type is a `Value` and not `Verdict`
///
/// `crate::engine_probes` is compiled out of release entirely, so a signature
/// naming `Verdict` would not exist in every configuration this command does.
/// The alternative — keeping the shapes compiled everywhere — was tried and
/// **measured wrong**: `--release` reported their variants as never
/// constructed, because a signature names a type without constructing one and
/// dead-code analysis ignores derived impls. The typing that matters is kept
/// where it can be: [`imp::stage5_probe`] returns a real `Verdict` and only
/// the IPC boundary, which is JSON regardless, sees the erased form.
#[tauri::command]
pub async fn chat_stage5_probe(
    run: bool,
    app: AppHandle,
) -> Result<Option<serde_json::Value>, String> {
    #[cfg(all(mobile, debug_assertions))]
    {
        if !run {
            return Ok(None);
        }
        let verdict = imp::stage5_probe(app).await?;
        return serde_json::to_value(verdict)
            .map(Some)
            .map_err(|e| format!("the Stage-5 verdict could not be serialised: {e}"));
    }
    #[cfg(not(all(mobile, debug_assertions)))]
    {
        let _ = (run, app);
        #[cfg(desktop)]
        return Err(DESKTOP_REFUSAL.to_string());
        #[cfg(not(desktop))]
        return Err("the Stage-5 probe run is a debug-build affordance and is \
                    compiled out of release builds"
            .to_string());
    }
}
