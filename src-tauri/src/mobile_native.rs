//! The two Android shims 2.2 left unfinished: the download network policy's
//! fact source, and share-sheet export.
//!
//! Both reach Kotlin through `android_bridge::with_app_class`, the JNI entry
//! path proven on device in the native chunk and given a single home in 3.2.
//! Neither of these is the first thing over that bridge, which is exactly the
//! sequencing the bridge-first decision bought: a failure here has one
//! candidate cause, not two.
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
