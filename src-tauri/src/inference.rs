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

fn spawn_server(app: &AppHandle, port: u16, ngl: u32) -> std::io::Result<Child> {
    let root = resources_root(app);
    let raw = std::fs::read_to_string(root.join("catalog.json"))
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::NotFound, format!("catalog: {e}")))?;
    let entries = crate::catalog::parse_catalog(&raw)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let hero = crate::catalog::hero(&entries).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "no real model in catalog")
    })?;
    let model = root.join(hero.model_file.as_ref().unwrap());
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
    if let Some(mut c) = engine.child.lock().unwrap().take() {
        let _ = c.kill();
        let _ = c.wait();
    }
}

pub fn shutdown(engine: &Engine) {
    engine.shutting_down.store(true, Ordering::Relaxed);
    kill_child(engine);
}

/// Spawns the engine thread: GPU first, CPU fallback, watchdog respawn.
pub fn start(app: AppHandle, engine: Arc<Engine>) {
    std::thread::spawn(move || {
        let force_cpu = std::env::var("CLEOPHIS_FORCE_CPU").is_ok();
        let mut ngl: u32 = if force_cpu { 0 } else { 99 };
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
                    continue 'restart;
                }
                std::thread::sleep(Duration::from_millis(700));
            }
        }
    });
}
