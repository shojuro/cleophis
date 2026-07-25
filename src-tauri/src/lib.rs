// Library crate root — the Tauri 2 lib+bin split (mobile prerequisite).
// `tauri android init` / `ios init` require the app to expose a library
// target with a `mobile_entry_point`; desktop keeps the thin `main.rs`
// binary that calls `run()`. All modules, commands, and the builder live
// here unchanged — the split moves code, it does not alter behavior.

mod calc;
mod catalog;
mod catalog_dist;
/// The mobile chat transport (`chat_stream` / `chat_complete` / `chat_cancel`).
/// Compiled on every platform so one `generate_handler!` list serves both —
/// the command bodies are cfg-gated and the desktop ones refuse; see the
/// module doc for why that beats maintaining two lists.
mod chat_cmds;
mod cloud;
mod convstore;
/// The in-process engine backing `inference`'s mobile lifecycle. Android-only:
/// desktop keeps its `llama-server` sidecar.
#[cfg(mobile)]
mod engine_inproc;
/// The inference thread's serve loop — which consecutive turns may share one
/// live `EngineSession` (task 1.5), and every way that session's life ends.
/// Declared here, and so compiled everywhere, for the same reason as
/// `engine_tools`: the routing is pure, its failure modes are dropped turns and
/// a thread that stops accepting work, and the desktop suite is the only place
/// tests run. Only the half that holds the session's borrow stays behind the
/// `cfg`.
#[path = "engine_inproc/serve.rs"]
mod engine_serve;
/// Tool-call parsing, the closed tool registry, and the per-family prompt
/// contract for the in-process engine. Lives under `engine_inproc/` because
/// that is what owns it, but is declared here — and so compiled on every
/// platform — because it is pure logic and the desktop suite is the only place
/// tests actually run. Gating it to Android would mean the closed-registry
/// security property is asserted nowhere.
#[path = "engine_inproc/tools.rs"]
mod engine_tools;
/// The calc tool-loop, transcribed from `src/calc-loop.js`. Declared here for
/// the same reason as `engine_tools`: it is written against a `TurnSource`
/// trait rather than `EngineSession` precisely so it compiles and is tested off
/// Android.
#[path = "engine_inproc/tool_loop.rs"]
mod engine_tool_loop;
mod hardware;
mod inference;
mod kpack;
mod ocr;
/// Compiled on every platform so its consistency tests run in the desktop
/// suite; the `include_bytes!` payload inside it is `cfg(mobile)`, so the
/// desktop binary carries none of it.
mod resources_embed;
mod tier_select;

use std::sync::Arc;
use std::time::{Duration, Instant};

use inference::{Engine, EngineInfo, EngineStatus};
use tauri::Manager;

#[tauri::command]
fn get_catalog(app: tauri::AppHandle) -> Result<Vec<serde_json::Value>, String> {
    let root = inference::resources_root(&app);
    let raw = std::fs::read_to_string(root.join("catalog.json")).map_err(|e| e.to_string())?;
    let entries = catalog::parse_catalog(&raw)?;
    Ok(entries
        .into_iter()
        .map(|e| {
            let cover_abs = root.join(&e.cover);
            let mut v = serde_json::to_value(&e).unwrap();
            v["coverAbs"] =
                serde_json::Value::String(cover_abs.to_string_lossy().into_owned());
            v
        })
        .collect())
}

#[tauri::command]
fn detect_hardware() -> hardware::HardwareInfo {
    hardware::detect()
}

#[tauri::command]
fn engine_info(engine: tauri::State<'_, Arc<Engine>>) -> EngineInfo {
    engine.info()
}

#[tauri::command]
async fn load_model(
    _model_id: String,
    app: tauri::AppHandle,
    engine: tauri::State<'_, Arc<Engine>>,
) -> Result<EngineInfo, String> {
    // Snapshot the status, then drop the lock before any (re)start — restart()
    // re-takes the engine's locks.
    let status = { engine.status.lock().unwrap().clone() };
    match status {
        // The dist-catalog download path (download_artifact) lands the base +
        // adapter but — unlike download_model — never starts the engine
        // itself (that wiring is this command's job, B4). If the hero is now
        // fully on disk (base AND, when declared, adapter — resolve_launch
        // enforces both), start it; otherwise it is genuinely not downloaded.
        EngineStatus::NoModel => {
            if inference::model_path(&app).is_some() {
                inference::start_if_no_model(app.clone(), engine.inner().clone());
            } else {
                return Err("Model not downloaded yet.".to_string());
            }
        }
        // A prior launch failed its load-time integrity gate — most often a
        // required file (e.g. the adapter-v2 contract adapter) was missing at
        // boot and has since been re-downloaded from the failed-engine banner.
        // `start_if_no_model` is a no-op from Failed, so recovery must go
        // through restart(), which re-runs resolve_launch + the integrity gate
        // over the now-complete set (a still-incomplete set just fails again,
        // re-surfacing engine-failed). restart() joins the old watchdog thread,
        // so it is blocking — run it off the async runtime.
        EngineStatus::Failed => {
            if inference::model_path(&app).is_some() {
                let app2 = app.clone();
                let eng = engine.inner().clone();
                tauri::async_runtime::spawn_blocking(move || inference::restart(app2, eng))
                    .await
                    .map_err(|_| "The local engine failed to restart.".to_string())?;
            } else {
                return Err("Model not downloaded yet.".to_string());
            }
        }
        _ => {}
    }
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        let status = engine.status.lock().unwrap().clone();
        match status {
            EngineStatus::Ready => return Ok(engine.info()),
            EngineStatus::Failed => {
                return Err("The local engine failed to start.".to_string())
            }
            _ => {}
        }
        if Instant::now() > deadline {
            return Err("Timed out waiting for the local engine.".to_string());
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // Android bundles no resources, so the catalog and covers have to be
            // written out of the binary before ANYTHING reads
            // `inference::resources_root` — `model_path` on the next line
            // already does. A failure here is not fatal: the app then behaves
            // exactly as it does with an absent catalog (no hero resolves, the
            // NoModel banner shows), which is the honest fail-closed outcome and
            // far better than aborting startup. It is logged loudly because in
            // that state nothing else will explain the empty library.
            #[cfg(mobile)]
            if let Err(e) = resources_embed::materialize(app.handle()) {
                eprintln!("setup: failed to materialize embedded resources: {e}");
            }

            let port = inference::free_port()?;
            let engine = Arc::new(Engine::new(port));
            app.manage(engine.clone());
            if inference::model_path(app.handle()).is_some() {
                inference::start(app.handle().clone(), engine.clone());
            } else {
                engine.set_no_model();
            }

            let cloud_dir = app
                .path()
                .app_data_dir()
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
            std::fs::create_dir_all(&cloud_dir)?;
            app.manage(Arc::new(cloud::session::Cloud::new(
                cloud_dir.join("cloud-cache.json"),
            )));
            app.manage(Arc::new(cloud::download::Downloads::new()));

            // §7 S7-1 / §7-chatiso: the local conversation store
            // (folders/chats/messages + fts5 search), managed directly
            // like `kpack::EmbedderCache`/`kpack::Builds` below — not
            // wrapped in an outer `Arc`, since its commands re-fetch
            // `app.state::<convstore::ConvStore>()` inside their own
            // `spawn_blocking` closures (see `convstore.rs`'s module doc
            // comment). `ConvStore::new` takes the conversations DIRECTORY,
            // not a single DB file — §7-chatiso (SECURITY) made the store
            // per-account (one SQLite database per signed-in account,
            // opened on demand as `<dir>/<sanitized_user_id>.db`) after a
            // cross-account privacy leak: the old single shared
            // `conversations.db` showed every account's chats to whoever
            // was currently signed in. `new` never opens a database itself
            // (no `?` needed — see `convstore.rs`), so this can't fail.
            app.manage(convstore::ConvStore::new(cloud_dir.join("conversations")));

            // §3a A1: the shared, lazily-loaded embedder (rag_query and
            // build_personal_pack both resolve it through this instead of
            // reloading the 118 MB GGUF per call) and the single-slot
            // active-build registry backing build_personal_pack's progress
            // events + cancel_build. Managed directly (not wrapped in an
            // outer Arc) — see kpack.rs's module doc comment for why that's
            // enough even though build_personal_pack/rag_query access them
            // from inside a spawn_blocking closure.
            app.manage(kpack::EmbedderCache::default());
            app.manage(kpack::Builds::default());

            // In-flight chat turns, so `chat_cancel` can reach one by request
            // id. Managed on both platforms so the command surface is uniform;
            // desktop never populates it.
            app.manage(chat_cmds::ChatCancels::default());

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            calc::calc,
            get_catalog,
            detect_hardware,
            engine_info,
            load_model,
            catalog_dist::fetch_dist_catalog,
            tier_select::get_tier_selection,
            tier_select::begin_tier_switch,
            tier_select::complete_tier_switch,
            tier_select::mark_tier_committed,
            kpack::mount_pack,
            kpack::build_personal_pack,
            kpack::cancel_build,
            kpack::rag_query,
            kpack::list_packs,
            kpack::delete_pack,
            cloud::commands::check_password_strength,
            cloud::commands::sign_up,
            cloud::commands::sign_in,
            cloud::commands::sign_out,
            cloud::commands::restore_session,
            cloud::commands::grant_entitlement,
            cloud::commands::list_entitlements,
            cloud::commands::remove_account_from_device,
            cloud::commands::remove_current_account_from_device,
            cloud::commands::start_checkout,
            cloud::commands::open_billing_portal,
            cloud::download::download_model,
            cloud::download::download_artifact,
            cloud::download::cancel_download,
            cloud::download::download_status,
            convstore::create_folder,
            convstore::list_folders,
            convstore::rename_folder,
            convstore::delete_folder,
            convstore::create_chat,
            convstore::list_chats,
            convstore::get_chat,
            convstore::rename_chat,
            convstore::auto_title_chat,
            convstore::delete_chat,
            convstore::set_chat_pinned,
            convstore::set_chat_archived,
            convstore::move_chat,
            convstore::set_chat_packs,
            convstore::append_message,
            convstore::search_chats,
            convstore::export_chat_to_file,
            chat_cmds::chat_stream,
            chat_cmds::chat_complete,
            chat_cmds::chat_cancel
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Destroyed = event {
                inference::shutdown(&window.app_handle().state::<Arc<Engine>>());
                window
                    .app_handle()
                    .state::<Arc<cloud::download::Downloads>>()
                    .request_cancel();
                // §3a A2: mirror the download cancel above for an in-flight
                // personal-pack build — `Builds` is managed directly (not
                // wrapped in an outer Arc, see kpack.rs's module doc
                // comment), so this is `state::<kpack::Builds>()`, not
                // `state::<Arc<kpack::Builds>>()`.
                window
                    .app_handle()
                    .state::<kpack::Builds>()
                    .request_cancel();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Cleophis");
}
