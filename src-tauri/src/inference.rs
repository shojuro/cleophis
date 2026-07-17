use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

#[derive(Clone, Serialize, Debug, PartialEq)]
pub enum EngineStatus {
    Starting,
    Ready,
    Restarting,
    Failed,
    NoModel,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineInfo {
    pub port: u16,
    pub status: EngineStatus,
    pub gpu_offload: bool,
}

pub struct Engine {
    pub port: u16,
    pub status: Mutex<EngineStatus>,
    pub child: Mutex<Option<Child>>,
    pub shutting_down: AtomicBool,
    pub gpu_offload: AtomicBool,
}

impl Engine {
    pub fn new(port: u16) -> Self {
        Engine {
            port,
            status: Mutex::new(EngineStatus::Starting),
            child: Mutex::new(None),
            shutting_down: AtomicBool::new(false),
            gpu_offload: AtomicBool::new(false),
        }
    }

    pub fn info(&self) -> EngineInfo {
        EngineInfo {
            port: self.port,
            status: self.status.lock().unwrap().clone(),
            gpu_offload: self.gpu_offload.load(Ordering::Relaxed),
        }
    }

    fn set_status(&self, s: EngineStatus) {
        *self.status.lock().unwrap() = s;
    }

    /// Marks the engine as having no model on disk yet (thin install, not
    /// downloaded). Cleared by `start_if_no_model` once the download lands.
    pub fn set_no_model(&self) {
        self.set_status(EngineStatus::NoModel);
    }
}

pub fn free_port() -> std::io::Result<u16> {
    let l = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    Ok(l.local_addr()?.port())
}

/// In dev, resources live in src-tauri/resources; in prod, under the install's resource dir.
pub fn resources_root(app: &AppHandle) -> PathBuf {
    #[cfg(debug_assertions)]
    {
        let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources");
        if dev.exists() {
            return dev;
        }
    }
    app.path()
        .resource_dir()
        .expect("no resource dir")
        .join("resources")
}

/// Resolve the hero model on disk: downloaded copy first (app data),
/// bundled copy second (dev / fat installs). None = thin install, not yet downloaded.
pub fn model_path(app: &AppHandle) -> Option<PathBuf> {
    let root = resources_root(app);
    let raw = std::fs::read_to_string(root.join("catalog.json"))
        .map_err(|e| eprintln!("model_path: catalog read: {e}"))
        .ok()?;
    let entries = crate::catalog::parse_catalog(&raw)
        .map_err(|e| eprintln!("model_path: catalog parse: {e}"))
        .ok()?;
    let hero = crate::catalog::hero(&entries)?;
    let model_file = hero.model_file.as_ref()?;
    let app_data = app.path().app_data_dir().ok();
    resolve_model(app_data, root, model_file)
}

/// Pure resolution logic behind `model_path`: app-data copy wins if present,
/// else the bundled resources copy, else None.
fn resolve_model(app_data: Option<PathBuf>, resources: PathBuf, model_file: &str) -> Option<PathBuf> {
    if let Some(dir) = app_data {
        let candidate = dir.join(model_file);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    let candidate = resources.join(model_file);
    if candidate.exists() {
        return Some(candidate);
    }
    None
}

fn spawn_server(app: &AppHandle, port: u16, ngl: u32) -> std::io::Result<Child> {
    let root = resources_root(app);
    let model = model_path(app)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "model not on disk"))?;
    let exe = root.join("llama").join("llama-server.exe");

    let log_path = std::env::temp_dir().join("cleophis-llama.log");
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;

    let mut cmd = Command::new(exe);
    cmd.args([
        "-m",
        model.to_str().unwrap(),
        "--host",
        "127.0.0.1",
        "--port",
        &port.to_string(),
        "-ngl",
        &ngl.to_string(),
        "-c",
        "4096",
        "--no-webui",
    ])
    .stdout(Stdio::from(log.try_clone()?))
    .stderr(Stdio::from(log));

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd.spawn()
}

fn healthy(port: u16) -> bool {
    ureq::get(&format!("http://127.0.0.1:{port}/health"))
        .timeout(Duration::from_millis(800))
        .call()
        .map(|r| r.status() == 200)
        .unwrap_or(false)
}

fn child_exited(engine: &Engine) -> bool {
    let mut guard = engine.child.lock().unwrap();
    match guard.as_mut() {
        Some(c) => matches!(c.try_wait(), Ok(Some(_))),
        None => true,
    }
}

fn kill_child(engine: &Engine) {
    let child = engine.child.lock().unwrap().take();
    if let Some(mut c) = child {
        let _ = c.kill();
        let _ = c.wait();
    }
}

pub fn shutdown(engine: &Engine) {
    engine.shutting_down.store(true, Ordering::Relaxed);
    kill_child(engine);
}

/// A llama-server left over from an abnormal exit holds VRAM and would
/// silently force this launch onto the CPU — clear it before spawning.
#[cfg(windows)]
fn sweep_stray_servers() {
    use std::os::windows::process::CommandExt;
    let _ = Command::new("taskkill")
        .args(["/F", "/IM", "llama-server.exe"])
        .creation_flags(0x0800_0000)
        .output();
}
#[cfg(not(windows))]
fn sweep_stray_servers() {}

/// Spawns the engine thread: GPU first, CPU fallback, watchdog respawn.
pub fn start(app: AppHandle, engine: Arc<Engine>) {
    std::thread::spawn(move || {
        engine.set_status(EngineStatus::Starting);
        sweep_stray_servers();
        let force_cpu = std::env::var("CLEOPHIS_FORCE_CPU").is_ok();
        let mut ngl: u32 = if force_cpu { 0 } else { 99 };
        let mut crashes: u32 = 0;
        'restart: loop {
            if engine.shutting_down.load(Ordering::Relaxed) {
                break;
            }
            match spawn_server(&app, engine.port, ngl) {
                Ok(c) => *engine.child.lock().unwrap() = Some(c),
                Err(e) => {
                    engine.set_status(EngineStatus::Failed);
                    let _ = app.emit("engine-failed", format!("spawn error: {e}"));
                    break;
                }
            }
            if engine.shutting_down.load(Ordering::Relaxed) {
                kill_child(&engine);
                break 'restart;
            }
            // Wait for health (model load can take a while on CPU).
            let deadline = Instant::now() + Duration::from_secs(180);
            let mut healthy_now = false;
            while Instant::now() < deadline {
                if engine.shutting_down.load(Ordering::Relaxed) {
                    break 'restart;
                }
                if child_exited(&engine) {
                    break;
                }
                if healthy(engine.port) {
                    healthy_now = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(400));
            }
            if !healthy_now {
                kill_child(&engine);
                if ngl > 0 {
                    // GPU path failed — drop to CPU.
                    ngl = 0;
                    engine.set_status(EngineStatus::Restarting);
                    continue 'restart;
                }
                engine.set_status(EngineStatus::Failed);
                let _ = app.emit(
                    "engine-failed",
                    format!(
                        "engine failed to start — log: {}",
                        std::env::temp_dir().join("cleophis-llama.log").display()
                    ),
                );
                break;
            }
            engine.gpu_offload.store(ngl > 0, Ordering::Relaxed);
            engine.set_status(EngineStatus::Ready);
            let _ = app.emit("engine-ready", engine.info());
            // Watchdog: poll for unexpected exit.
            loop {
                if engine.shutting_down.load(Ordering::Relaxed) {
                    break 'restart;
                }
                if child_exited(&engine) {
                    engine.set_status(EngineStatus::Restarting);
                    let _ = app.emit("engine-restarting", ());
                    if ngl > 0 {
                        ngl = 0; // be conservative after a crash
                    }
                    crashes += 1;
                    if crashes >= 3 {
                        engine.set_status(EngineStatus::Failed);
                        let _ = app.emit(
                            "engine-failed",
                            "engine crashed repeatedly — giving up".to_string(),
                        );
                        break 'restart;
                    }
                    std::thread::sleep(Duration::from_secs(2));
                    continue 'restart;
                }
                std::thread::sleep(Duration::from_millis(700));
            }
        }
    });
}

/// Check-and-set: NoModel -> Starting under the status lock. Returns whether
/// the transition happened (true) or the engine was in some other state
/// (false) — the double-start guard for `start_if_no_model`.
///
/// Not yet called from production code: the download-completion event that
/// invokes `start_if_no_model` lands in a later task (thin-installer C3b/C4).
/// Directly unit-tested below in the meantime.
#[allow(dead_code)]
fn try_begin_start(engine: &Engine) -> bool {
    let mut status = engine.status.lock().unwrap();
    if *status == EngineStatus::NoModel {
        *status = EngineStatus::Starting;
        true
    } else {
        false
    }
}

/// Start the engine after a download completes. No-op unless current status is
/// NoModel (atomic check-and-set under the status lock — double-start guard).
///
/// Not yet wired to a caller: the download-completion handler that invokes
/// this lands in a later task (thin-installer C3b/C4).
#[allow(dead_code)]
pub fn start_if_no_model(app: AppHandle, engine: Arc<Engine>) {
    if try_begin_start(&engine) {
        start(app, engine);
    } else {
        eprintln!("start_if_no_model: ignored, engine was not in NoModel state");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_dir(name: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "cleophis-c2-test-{}-{}-{}",
            std::process::id(),
            unique,
            name
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn resolve_model_prefers_app_data_when_both_exist() {
        let app_data = unique_dir("appdata-both");
        let resources = unique_dir("resources-both");
        std::fs::create_dir_all(app_data.join("models")).unwrap();
        std::fs::create_dir_all(resources.join("models")).unwrap();
        std::fs::write(app_data.join("models/hero.gguf"), b"a").unwrap();
        std::fs::write(resources.join("models/hero.gguf"), b"b").unwrap();

        let got = resolve_model(Some(app_data.clone()), resources, "models/hero.gguf");
        assert_eq!(got, Some(app_data.join("models/hero.gguf")));
    }

    #[test]
    fn resolve_model_falls_back_to_resources_when_app_data_missing_file() {
        let app_data = unique_dir("appdata-fallback");
        let resources = unique_dir("resources-fallback");
        std::fs::create_dir_all(resources.join("models")).unwrap();
        std::fs::write(resources.join("models/hero.gguf"), b"b").unwrap();
        // app_data dir exists but does not contain the model file.

        let got = resolve_model(Some(app_data), resources.clone(), "models/hero.gguf");
        assert_eq!(got, Some(resources.join("models/hero.gguf")));
    }

    #[test]
    fn resolve_model_falls_back_to_resources_when_app_data_absent() {
        let resources = unique_dir("resources-optnone");
        std::fs::create_dir_all(resources.join("models")).unwrap();
        std::fs::write(resources.join("models/hero.gguf"), b"b").unwrap();

        let got = resolve_model(None, resources.clone(), "models/hero.gguf");
        assert_eq!(got, Some(resources.join("models/hero.gguf")));
    }

    #[test]
    fn resolve_model_none_when_neither_has_the_file() {
        let app_data = unique_dir("appdata-none");
        let resources = unique_dir("resources-none");

        let got = resolve_model(Some(app_data), resources, "models/hero.gguf");
        assert_eq!(got, None);
    }

    #[test]
    fn try_begin_start_transitions_no_model_to_starting() {
        let engine = Engine::new(0);
        engine.set_no_model();
        assert!(try_begin_start(&engine));
        assert_eq!(*engine.status.lock().unwrap(), EngineStatus::Starting);
    }

    #[test]
    fn try_begin_start_is_a_noop_for_other_statuses() {
        let engine = Engine::new(0); // Engine::new starts in Starting.
        assert!(!try_begin_start(&engine));
        assert_eq!(*engine.status.lock().unwrap(), EngineStatus::Starting);

        engine.set_status(EngineStatus::Ready);
        assert!(!try_begin_start(&engine));
        assert_eq!(*engine.status.lock().unwrap(), EngineStatus::Ready);
    }
}
