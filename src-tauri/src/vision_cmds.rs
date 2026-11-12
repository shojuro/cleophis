//! Image-transcription commands for the in-process engine (`transcribe_image` /
//! `prewarm_vision` / `vision_state`).
//!
//! A photographed line of maths goes in as base64; a verdict, the transcribed
//! blocks and the costs come back over a Tauri `Channel`. The work itself runs
//! on the inference thread (see `engine_inproc`), because that thread owns both
//! the tutor and the transcriber handles.
//!
//! # Why these are compiled on desktop too
//!
//! Same reason as `chat_cmds`: one `generate_handler!` list serves both
//! platforms, so the commands exist everywhere and their **bodies** are
//! cfg-gated. The desktop bodies refuse before touching any state. The wire
//! types and the pure helpers below are compiled everywhere so the desktop
//! suite can test them; nothing here may name a `kpack_engine` type, because
//! that crate is not a desktop dependency (the crate-local mirrors in
//! `engine_serve` stand in for it).
//!
//! # Mitigation M1
//!
//! At most one transcription is queued or running per process
//! ([`VisionInFlight`]). Without it a looping caller could stack decoded image
//! buffers in the inference thread's command channel.

use serde::Serialize;
use tauri::ipc::Channel;
use tauri::AppHandle;

use crate::engine_serve::{OutcomeKind, OutcomeVerdict, OutcomeWarning, TranscribeOutcome};
use crate::inference::VisionInfo;

/// What the desktop stubs return. Names the platform and the command family so
/// a misrouted invoke is diagnosable from one line.
#[cfg_attr(mobile, allow(dead_code))]
const DESKTOP_REFUSAL: &str = "in-process vision commands are mobile-only; this is a desktop build";

/// The largest encoded image accepted. Mirrors `kpack_engine::MAX_IMAGE_BYTES`,
/// which the engine re-checks; `kpack_engine` is not a desktop dependency, so
/// the value cannot be imported here.
const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;

/// One transcribed block, as the frontend receives it.
#[cfg_attr(desktop, allow(dead_code))]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WireBlock {
    /// `"latex"` or `"text"`.
    pub kind: &'static str,
    pub content: String,
    pub confidence: f64,
    /// The weakest operator token's probability, when the block had operators.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_operator_prob: Option<f64>,
}

/// Which model produced the transcription.
#[cfg_attr(desktop, allow(dead_code))]
#[derive(Debug, Clone, Serialize)]
pub struct WireModel {
    pub id: String,
    pub family: String,
}

/// What the transcription cost, in milliseconds.
#[cfg_attr(desktop, allow(dead_code))]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WireTimings {
    pub prep_ms: u64,
    pub prefill_ms: u64,
    pub decode_ms: u64,
    /// Present only on the sequential path, where the tutor was reloaded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reload_ms: Option<u64>,
}

/// A finished transcription as the frontend receives it.
#[cfg_attr(desktop, allow(dead_code))]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscribeResult {
    pub blocks: Vec<WireBlock>,
    /// `"lowConfidence"`, `"operatorUncertain"`, `"noMathFound"`,
    /// `"invalidLatex"` or `"truncated"`.
    pub warnings: Vec<&'static str>,
    /// `"accept"`, `"review"` or `"refuse"`.
    pub verdict: &'static str,
    pub model: WireModel,
    pub timings: WireTimings,
    pub image_tokens: usize,
    pub peak_rss_kb: u64,
    pub coresident: bool,
}

/// What `transcribe_image` emits on its channel: exactly one event per call
/// that reaches the engine's command path. The early refusals (a body that is
/// not valid base64, one over the size cap, a transcription already in flight)
/// reject the promise without sending an event.
#[cfg_attr(desktop, allow(dead_code))]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "event", content = "data")]
pub enum VisionEvent {
    Done(TranscribeResult),
    Error { message: String },
}

/// Widen an `f32` probability to the `f64` JSON carries WITHOUT the widening
/// noise: `0.995f32 as f64` serialises as `0.9950000047683716`. Going through
/// the shortest decimal that round-trips the `f32` gives the frontend `0.995`.
#[cfg_attr(desktop, allow(dead_code))]
fn f32_wire(x: f32) -> f64 {
    x.to_string().parse().unwrap_or(f64::from(x))
}

/// Map the engine's outcome onto the wire shape.
#[cfg_attr(desktop, allow(dead_code))]
pub(crate) fn to_wire(outcome: TranscribeOutcome, model_id: &str, family: &str) -> TranscribeResult {
    TranscribeResult {
        blocks: outcome
            .blocks
            .into_iter()
            .map(|b| WireBlock {
                kind: match b.kind {
                    OutcomeKind::Latex => "latex",
                    OutcomeKind::Text => "text",
                },
                content: b.content,
                confidence: f32_wire(b.confidence),
                min_operator_prob: b.min_operator_prob.map(f32_wire),
            })
            .collect(),
        warnings: outcome
            .warnings
            .into_iter()
            .map(|w| match w {
                OutcomeWarning::LowConfidence => "lowConfidence",
                OutcomeWarning::OperatorUncertain => "operatorUncertain",
                OutcomeWarning::NoMathFound => "noMathFound",
                OutcomeWarning::InvalidLatex => "invalidLatex",
                OutcomeWarning::Truncated => "truncated",
            })
            .collect(),
        verdict: match outcome.verdict {
            OutcomeVerdict::Accept => "accept",
            OutcomeVerdict::Review => "review",
            OutcomeVerdict::Refuse => "refuse",
        },
        model: WireModel { id: model_id.to_string(), family: family.to_string() },
        timings: WireTimings {
            prep_ms: outcome.prep_ms,
            prefill_ms: outcome.prefill_ms,
            decode_ms: outcome.decode_ms,
            reload_ms: outcome.reload_ms,
        },
        image_tokens: outcome.n_image_tokens,
        peak_rss_kb: outcome.peak_rss_kb,
        coresident: outcome.coresident,
    }
}

/// Decode the frontend's base64 image body.
///
/// The length check comes BEFORE the decode: an over-long body is refused
/// without allocating the buffer it would have produced.
#[cfg_attr(desktop, allow(dead_code))]
pub(crate) fn decode_image_base64(s: &str) -> Result<Vec<u8>, String> {
    use base64::Engine as _;
    if s.len() > MAX_IMAGE_BYTES * 4 / 3 + 4 {
        return Err("The image is too large.".to_string());
    }
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|_| "The image data is not valid base64.".to_string())
}

/// Mitigation M1: at most one transcription queued or running per process.
#[derive(Default)]
pub struct VisionInFlight(std::sync::atomic::AtomicBool);

impl VisionInFlight {
    /// Claim the slot; false when a transcription already holds it.
    #[cfg_attr(desktop, allow(dead_code))]
    pub fn try_begin(&self) -> bool {
        use std::sync::atomic::Ordering;
        self.0
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// Release the slot. Called on every exit path after a successful
    /// [`try_begin`](Self::try_begin).
    #[cfg_attr(desktop, allow(dead_code))]
    pub fn end(&self) {
        self.0.store(false, std::sync::atomic::Ordering::Release);
    }
}

#[cfg(mobile)]
mod imp {
    use super::*;
    use std::sync::{mpsc, Arc};

    use tauri::Manager;

    use crate::engine_serve::{Command, TranscribeJob};
    use crate::inference::Engine;

    pub(super) async fn transcribe_image(
        request_id: String,
        image_base64: String,
        on_event: Channel<VisionEvent>,
        app: AppHandle,
    ) -> Result<(), String> {
        // Claim the slot BEFORE decoding (final review M9): otherwise every
        // concurrent caller allocates its decoded image, up to 20 MiB, only to
        // be refused. (The base64 string itself is already allocated by the IPC
        // layer before this body runs.) The length check still runs before the
        // decode, inside `decode_image_base64`.
        let in_flight = app.state::<VisionInFlight>();
        if !in_flight.try_begin() {
            return Err("A transcription is already in progress.".to_string());
        }
        // From here every exit path runs `in_flight.end()`. The decode refusal
        // is the one early return, and nothing else has begun when it runs;
        // after it there is no `?` or early return before the teardown below,
        // the same "always" argument `chat_stream` makes for the cancel
        // registry.
        let image = match decode_image_base64(&image_base64) {
            Ok(bytes) => bytes,
            Err(message) => {
                in_flight.end();
                return Err(message);
            }
        };
        drop(image_base64);
        let cancel = app.state::<crate::chat_cmds::ChatCancels>().begin(&request_id);
        // Foreground service for the duration, as for a chat turn, so Android
        // does not kill a multi-second transcription under memory pressure.
        crate::mobile_native::inference_service(true);
        let engine = app.state::<Arc<Engine>>().inner().clone();
        let (reply_tx, reply_rx) = mpsc::channel();
        let sent = crate::engine_inproc::send_command(
            &engine,
            Command::Transcribe(TranscribeJob { image, cancel, reply: reply_tx }),
        );
        let result = if !sent {
            Err("The on-device engine is not running.".to_string())
        } else {
            match tauri::async_runtime::spawn_blocking(move || {
                reply_rx
                    .recv()
                    .map_err(|_| "The on-device engine stopped during transcription.".to_string())?
            })
            .await
            {
                Ok(r) => r,
                Err(_) => Err("The transcription task failed to run.".to_string()),
            }
        };
        app.state::<crate::chat_cmds::ChatCancels>().end(&request_id);
        crate::mobile_native::inference_service(false);
        in_flight.end();
        let (model_id, family) = crate::inference::hero_vision(&app)
            .map(|v| (v.base_model, v.family))
            .unwrap_or_default();
        match result {
            Ok(outcome) => {
                let _ = on_event.send(VisionEvent::Done(to_wire(outcome, &model_id, &family)));
                Ok(())
            }
            Err(message) => {
                let _ = on_event.send(VisionEvent::Error { message: message.clone() });
                Err(message)
            }
        }
    }

    /// The transcriber's state NOW, not as stamped at the last tutor load
    /// (`EngineInfo.vision`), which misses a later download and is unset while
    /// no tutor has loaded (final review I3). Each flag is cheap: a catalog
    /// read, two `exists()` calls, and a memory-size read.
    ///
    /// `coresident` is recomputed from the device's RAM rather than read from
    /// `Engine::vision_coresident`, because that atomic is written only when the
    /// engine thread starts and is still `false` before then (no model yet). It
    /// is the same expression the thread evaluates, over a total that does not
    /// change while the process lives, so the two agree once the thread runs;
    /// the thread's own value stays authoritative for what it loads.
    fn live_vision_info(app: &AppHandle) -> Option<VisionInfo> {
        VisionInfo::from_flags(
            crate::inference::hero_vision(app).is_some(),
            crate::inference::resolve_vision(app).is_some(),
            crate::vision_policy::coresident(crate::hardware::detect().ram_gb),
        )
    }

    pub(super) fn prewarm_vision(app: AppHandle) -> Result<(), String> {
        // A prewarm that cannot keep anything resident would only end an open
        // chat session and drop its KV cache, so send it only when the files
        // are installed AND the device is co-resident. Advisory either way:
        // never an error.
        if live_vision_info(&app).is_some_and(|v| v.can_prewarm()) {
            let engine = app.state::<Arc<Engine>>().inner().clone();
            let _ = crate::engine_inproc::send_command(&engine, Command::PrewarmVision);
        }
        Ok(())
    }

    pub(super) fn vision_state(app: AppHandle) -> Result<Option<VisionInfo>, String> {
        Ok(live_vision_info(&app))
    }
}

/// Transcribe one image (base64 body), emitting exactly one [`VisionEvent`] per
/// call that reaches the engine's command path; the early refusals reject
/// without an event (see [`VisionEvent`]). Cancel through the existing
/// `chat_cancel` with the same `request_id`.
#[tauri::command]
pub async fn transcribe_image(
    request_id: String,
    image_base64: String,
    on_event: Channel<VisionEvent>,
    app: AppHandle,
) -> Result<(), String> {
    #[cfg(mobile)]
    {
        return imp::transcribe_image(request_id, image_base64, on_event, app).await;
    }
    #[cfg(desktop)]
    {
        let _ = (request_id, image_base64, on_event, app);
        Err(DESKTOP_REFUSAL.to_string())
    }
}

/// Load the transcriber ahead of the first image (the camera just opened).
/// Sent to the engine only when the transcriber is installed and may stay
/// resident beside the tutor. Advisory: `Ok(())` even when nothing is running
/// or nothing can be kept resident.
#[tauri::command]
pub fn prewarm_vision(app: AppHandle) -> Result<(), String> {
    #[cfg(mobile)]
    {
        return imp::prewarm_vision(app);
    }
    #[cfg(desktop)]
    {
        let _ = app;
        Err(DESKTOP_REFUSAL.to_string())
    }
}

/// What the frontend may know about the transcriber on this tier, computed at
/// call time (unlike `EngineInfo.vision`, which is stamped at the last tutor
/// load), or `None` when the tier neither declares a transcriber nor has its
/// files on disk. Independent of the engine's status.
#[tauri::command]
pub fn vision_state(app: AppHandle) -> Result<Option<VisionInfo>, String> {
    #[cfg(mobile)]
    {
        return imp::vision_state(app);
    }
    #[cfg(desktop)]
    {
        let _ = app;
        Err(DESKTOP_REFUSAL.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wire_shape_is_camel_case_and_names_the_verdict_in_lower_case() {
        use crate::engine_serve::{OutcomeBlock, OutcomeKind, OutcomeVerdict, OutcomeWarning};
        let outcome = crate::engine_serve::TranscribeOutcome {
            blocks: vec![OutcomeBlock { kind: OutcomeKind::Latex, content: "3x-7=2x+5".into(), confidence: 0.99, min_operator_prob: Some(0.995) }],
            warnings: vec![OutcomeWarning::LowConfidence],
            verdict: OutcomeVerdict::Review,
            n_image_tokens: 64, prefill_ms: 7600, decode_ms: 1000, prep_ms: 12, reload_ms: None, peak_rss_kb: 389_000, coresident: true,
        };
        let v = serde_json::to_value(to_wire(outcome, "granite-docling-258M", "docling")).unwrap();
        assert_eq!(v["verdict"], "review");
        assert_eq!(v["blocks"][0]["kind"], "latex");
        assert_eq!(v["blocks"][0]["minOperatorProb"], 0.995);
        assert_eq!(v["warnings"][0], "lowConfidence");
        assert_eq!(v["timings"]["prefillMs"], 7600);
        assert!(v["timings"].get("reloadMs").is_none());
        assert_eq!(v["model"]["id"], "granite-docling-258M");
        assert_eq!(v["imageTokens"], 64);
    }

    #[test]
    fn only_one_transcription_may_be_in_flight() {
        // (mitigation M1)
        let g = VisionInFlight::default();
        assert!(g.try_begin());
        assert!(!g.try_begin(), "a second caller is refused while the first runs");
        g.end();
        assert!(g.try_begin(), "the slot is free again after end()");
    }

    #[test]
    fn a_bad_base64_body_is_refused_before_anything_is_decoded() {
        assert!(decode_image_base64("not base64!!").is_err());
        assert_eq!(decode_image_base64("AQID").unwrap(), vec![1, 2, 3]);
    }
}
