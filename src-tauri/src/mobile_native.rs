//! The two Android shims 2.2 left unfinished — the download network policy's
//! fact source and share-sheet export — plus, from 5.1, the inference
//! foreground service's start/stop.
//!
//! All three reach Kotlin through `android_bridge::with_app_class`, the JNI
//! entry path proven on device in the native chunk and given a single home in
//! 3.2. None of them is the first thing over that bridge, which is exactly the
//! sequencing the bridge-first decision bought: a failure here has one
//! candidate cause, not two.
//!
//! The 5.1 addition is deliberately **not** a command. Nothing in the frontend
//! decides when a turn is running, so exposing it to the invoke surface would
//! create a second, wrong source of truth for a lifetime `chat_stream` already
//! owns. It is called from there and nowhere else, which is why the D-1
//! discussion below covers only the two commands.
//!
//! # Both commands live on the SHARED invoke surface (decision D-1)
//!
//! Compiled on every platform, registered once in `generate_handler!`, bodies
//! `cfg`-gated, desktop refusing with a message that names the platform and
//! the reason. The rejected alternative — a second `generate_handler!` list
//! under `cfg` — is a silent-drift failure class: one list gains a command the
//! other doesn't, nothing fails, and a feature is quietly missing on one
//! platform until someone notices in the field.
//!
//! # What is NOT here, deliberately
//!
//! **The policy decision.** This module reports what Android knows; whether a
//! given download may proceed on a given connection is
//! `src/download-policy.js`, where `npm test` runs. That is not an arbitrary
//! split: the decision is a *user-facing* rule the user can always override,
//! its failure mode is silent (a wrong verdict shows no error, it just spends
//! someone's data), and the frontend already owns the prompt it produces. The
//! parse, whose failure mode is equally silent but which has no business in
//! JavaScript, is `net_state.rs` — compiled and tested on every platform.

use tauri::AppHandle;

/// Named for the same reason `chat_cmds`'s does: a misrouted invoke should be
/// diagnosable from one line of a bug report rather than a mystery about which
/// half of a seam fired.
#[cfg_attr(target_os = "android", allow(dead_code))]
const DESKTOP_REFUSAL: &str =
    "network-policy and share-sheet commands are Android-only; this is a \
     desktop build, which uses the OS save dialog and has no metered-network \
     concept";

/// What Android reports about the active connection and the battery.
///
/// Desktop refuses rather than returning a plausible default. A desktop build
/// that answered `metered: false` would be *inventing* a fact about a platform
/// that has no such concept, and the frontend only calls this on Android — so
/// a desktop call is a bug, and it should say so instead of being absorbed.
#[tauri::command]
pub async fn network_state(app: AppHandle) -> Result<crate::net_state::NetState, String> {
    #[cfg(target_os = "android")]
    {
        let _ = app;
        let raw = crate::android_bridge::with_app_class(
            "com.cleophis.app.NetworkPolicy",
            |env, class, activity| {
                let value = env
                    .call_static_method(
                        class,
                        "describe",
                        "(Landroid/content/Context;)Ljava/lang/String;",
                        &[activity.into()],
                    )
                    .and_then(|v| v.l())
                    .map_err(|e| format!("describe: {e}"))?;
                crate::android_bridge::jstring_result(env, value, "describe")
            },
        );
        match raw {
            Ok(raw) => Ok(crate::net_state::parse(&raw)),
            Err(e) => {
                // Not an error to the caller. A bridge failure is exactly the
                // case the safe default exists for: report "metered" and let
                // the user decide, rather than failing the download outright
                // or — far worse — assuming unmetered.
                eprintln!("[net-policy] describe failed, assuming metered: {e}");
                Ok(crate::net_state::NetState::default())
            }
        }
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Err(DESKTOP_REFUSAL.to_string())
    }
}

/// Hide or show app content in the app switcher (`FLAG_SECURE`) — spec §5.3.
///
/// **Default OFF**, decided in the frontend where the preference lives.
/// Screenshots are the user's right, so this is opt-in; §5.3's immigration-
/// paperwork user turns it on with one tap. Nothing here defaults anything —
/// this applies whatever it is told, and a build that never calls it is a build
/// with screenshots allowed, which is the correct behaviour.
///
/// Idempotent, and safe to call on every boot to re-apply a stored preference:
/// window flags do not accumulate.
#[tauri::command]
pub async fn set_screen_privacy(secure: bool, app: AppHandle) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let _ = app;
        crate::android_bridge::with_app_class(
            "com.cleophis.app.ScreenPrivacy",
            |env, class, activity| {
                env.call_static_method(
                    class,
                    "apply",
                    "(Landroid/content/Context;Z)V",
                    &[activity.into(), secure.into()],
                )
                .map(|_| ())
                .map_err(|e| format!("apply: {e}"))
            },
        )
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (secure, app);
        Err(DESKTOP_REFUSAL.to_string())
    }
}

/// Export a chat and hand it to the Android share sheet (`ACTION_SEND`).
///
/// Desktop keeps `dialog.save` untouched — `exportChat` in `app.js` branches
/// on the platform, so the desktop path is the same code it has always been.
/// `export_chat_to_file` already produces the bytes; what mobile needs is a
/// destination, not a formatter, so this reuses that command's formatting via
/// `convstore::export_chat` rather than reimplementing it.
#[tauri::command]
pub async fn share_chat(id: i64, format: String, title: String, app: AppHandle) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        imp::share_chat(id, format, title, app).await
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (id, format, title, app);
        Err(DESKTOP_REFUSAL.to_string())
    }
}

/// The pure half of the share path, in its own file so it can be compiled and
/// run outside the app crate (the `#[path]` pattern `engine_tools` and
/// `engine_serve` already use). Compiled on **every** platform so its tests
/// execute in the desktop suite — decision D-3.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
#[path = "mobile_native/pure.rs"]
mod pure;

#[cfg(target_os = "android")]
mod imp {
    use tauri::{AppHandle, Manager};

    use super::pure::{ext_and_mime, safe_file_stem};
    use crate::android_bridge;
    use crate::convstore::{current_user_id, ConvStore, ExportFormat};

    fn export_format(format: &str) -> Option<ExportFormat> {
        match format {
            "markdown" => Some(ExportFormat::Markdown),
            "json" => Some(ExportFormat::Json),
            "txt" => Some(ExportFormat::Txt),
            _ => None,
        }
    }

    pub(super) async fn share_chat(
        id: i64,
        format: String,
        title: String,
        app: AppHandle,
    ) -> Result<(), String> {
        let user_id = current_user_id(&app).ok_or_else(|| "Sign in to export.".to_string())?;
        let fmt =
            export_format(&format).ok_or_else(|| format!("Unknown export format: {format}"))?;
        let (ext, mime) =
            ext_and_mime(&format).ok_or_else(|| format!("Unknown export format: {format}"))?;

        let stem = safe_file_stem(&title);
        let content = {
            let app = app.clone();
            let user_id = user_id.clone();
            tauri::async_runtime::spawn_blocking(move || {
                app.state::<ConvStore>().export_chat(&user_id, id, fmt)
            })
            .await
            .map_err(|_| "export task failed".to_string())??
        };

        // The cache dir, not app-data: a share file is transient, the receiving
        // app copies what it needs, and the framework may reclaim it. It is
        // also already covered by the FileProvider's cache-path root, so no new
        // provider wiring is needed.
        let dir = app
            .path()
            .app_cache_dir()
            .map_err(|e| format!("no cache dir: {e}"))?
            .join("exports");
        std::fs::create_dir_all(&dir).map_err(|e| format!("cache dir: {e}"))?;

        // One file per format, overwritten each share. Deliberate: a growing
        // pile of exports in the cache is the user's storage, and the previous
        // share has already been consumed by whatever received it.
        let path = dir.join(format!("{stem}.{ext}"));
        std::fs::write(&path, content).map_err(|e| format!("write: {e}"))?;
        let path = path.to_string_lossy().to_string();

        android_bridge::with_app_class("com.cleophis.app.ShareSheet", |env, class, activity| {
            let j_path = env.new_string(&path).map_err(|e| format!("share: {e}"))?;
            let j_mime = env.new_string(mime).map_err(|e| format!("share: {e}"))?;
            let j_title = env.new_string(&title).map_err(|e| format!("share: {e}"))?;
            env.call_static_method(
                class,
                "shareFile",
                "(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)V",
                &[
                    activity.into(),
                    (&j_path).into(),
                    (&j_mime).into(),
                    (&j_title).into(),
                ],
            )
            .map(|_| ())
            .map_err(|e| format!("shareFile: {e}"))
        })
    }

    // No tests here on purpose. Everything in this module is either a JNI
    // call against a live JVM (which D-3 exempts) or plumbing around it; the
    // logic that CAN be got wrong silently lives in `super::pure`, where the
    // desktop suite executes it.
}

/// Promote or demote the inference foreground service around a turn (spec §8,
/// hazards H4/H5).
///
/// Called only from `chat_stream`, whose lifetime *is* the condition being
/// signalled. `true` on the way in, `false` on the way out, on every exit path
/// including cancel and error.
///
/// # Failure is logged and swallowed, deliberately
///
/// If the promotion fails — the bridge is not up, the notification cannot be
/// posted, the OS refuses the start — the turn still generates. It is merely
/// killable under memory pressure, which is precisely the behaviour every build
/// before 5.1 had. Aborting a reply the user is watching because a *status-bar
/// notification* could not be posted would trade a real answer for a
/// housekeeping detail. So this reports and continues, and the log line names
/// the method so a device log distinguishes "never promoted" from "promoted and
/// then killed anyway" — two very different bugs that look identical from the
/// transcript.
#[cfg(all(mobile, target_os = "android"))]
pub(crate) fn inference_service(running: bool) {
    let method = if running { "start" } else { "stop" };
    let outcome = crate::android_bridge::with_app_class(
        "com.cleophis.app.InferenceService",
        |env, class, activity| {
            env.call_static_method(
                class,
                method,
                "(Landroid/content/Context;)V",
                &[activity.into()],
            )
            .map(|_| ())
            .map_err(|e| format!("{method}: {e}"))
        },
    );
    if let Err(e) = outcome {
        eprintln!("[fgs] InferenceService.{method} failed: {e}");
    }
}

/// iOS keeps generation alive while the app is foregrounded (spec §8), so there
/// is no service to start. Present so `chat_stream` has one unconditional call
/// rather than a `cfg` at the call site, where the risk is forgetting one of a
/// matched pair.
#[cfg(all(mobile, not(target_os = "android")))]
pub(crate) fn inference_service(_running: bool) {}
