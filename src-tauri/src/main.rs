#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod calc;
mod catalog;
mod catalog_dist;
mod cloud;
mod convstore;
mod hardware;
mod inference;
mod kpack;
mod ocr;
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

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
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
            convstore::export_chat_to_file
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
