#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod catalog;
mod cloud;
mod hardware;
mod inference;

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
    engine: tauri::State<'_, Arc<Engine>>,
) -> Result<EngineInfo, String> {
    if *engine.status.lock().unwrap() == EngineStatus::NoModel {
        return Err("Model not downloaded yet.".to_string());
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

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_catalog,
            detect_hardware,
            engine_info,
            load_model,
            cloud::commands::sign_up,
            cloud::commands::sign_in,
            cloud::commands::sign_out,
            cloud::commands::restore_session,
            cloud::commands::grant_entitlement,
            cloud::commands::list_entitlements,
            cloud::download::download_model,
            cloud::download::cancel_download,
            cloud::download::download_status
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Destroyed = event {
                inference::shutdown(&window.app_handle().state::<Arc<Engine>>());
                window
                    .app_handle()
                    .state::<Arc<cloud::download::Downloads>>()
                    .request_cancel();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Cleophis");
}
