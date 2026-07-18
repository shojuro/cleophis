use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use sha2::{Digest, Sha256};
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
    /// Session cache for the load-time integrity check in
    /// `verify_model_once`: once a model path's sha256 has been checked
    /// against the catalog's pinned hash, it's recorded here so watchdog
    /// respawns of the SAME ~2GB file don't re-hash it every time — only
    /// the first successful verification per process pays the hashing
    /// cost.
    verified_model: Mutex<Option<PathBuf>>,
}

impl Engine {
    pub fn new(port: u16) -> Self {
        Engine {
            port,
            status: Mutex::new(EngineStatus::Starting),
            child: Mutex::new(None),
            shutting_down: AtomicBool::new(false),
            gpu_offload: AtomicBool::new(false),
            verified_model: Mutex::new(None),
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

/// Streams `path` through SHA-256 in fixed-size chunks — mirrors
/// `cloud::download::rehash_existing`'s pattern — rather than reading the
/// whole ~2GB model file into memory at once. Returns the lowercase hex
/// digest.
fn model_sha256(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 256 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Re-verifies `model`'s sha256 against the catalog's pinned hash before it
/// is fed into llama-server (audit sec-rust #5): integrity was previously
/// checked only at download time, so a local attacker overwriting the
/// app-data file after that check — or a TOCTOU window between
/// download-verify and load — got an arbitrary GGUF parsed by llama.cpp
/// with no re-check at load. Hashing costs real time on a ~2GB file, so the
/// result is cached in `engine.verified_model` for the life of the
/// process: only the FIRST successful verification of a given path
/// re-hashes it; every watchdog respawn of the same file after that is
/// free.
///
/// A catalog that pins no hash for the hero model (`sha256: None`) is
/// treated as "nothing to check" rather than a hard failure — the download
/// path already gates real models, so this only affects dev/fixture
/// catalogs missing integrity data.
fn verify_model_once(engine: &Engine, app: &AppHandle, model: &Path) -> Result<(), String> {
    if engine.verified_model.lock().unwrap().as_deref() == Some(model) {
        return Ok(());
    }

    let root = resources_root(app);
    let raw = std::fs::read_to_string(root.join("catalog.json")).map_err(|e| e.to_string())?;
    let entries = crate::catalog::parse_catalog(&raw)?;
    let hero = crate::catalog::hero(&entries)
        .ok_or_else(|| "catalog missing integrity data".to_string())?;

    let Some(expected) = hero.sha256.as_deref() else {
        eprintln!(
            "verify_model_once: catalog pins no sha256 for the hero model — skipping integrity check"
        );
        return Ok(());
    };

    let actual = model_sha256(model).map_err(|e| e.to_string())?;
    if !actual.eq_ignore_ascii_case(expected) {
        eprintln!(
            "verify_model_once: integrity check failed for {}",
            model.display()
        );
        return Err("model integrity check failed".to_string());
    }

    *engine.verified_model.lock().unwrap() = Some(model.to_path_buf());
    Ok(())
}

/// Where the currently-spawned llama-server's PID is recorded (symmetry
/// with `cleophis-llama.log`) — read back by `sweep_stray_servers` on the
/// NEXT process start so a leftover from an abnormal exit can be killed by
/// the exact PID we spawned, instead of by image name (which would kill
/// every llama-server.exe on the box, including ones from other apps or
/// another Cleophis instance).
fn pid_file_path() -> PathBuf {
    std::env::temp_dir().join("cleophis-llama.pid")
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
    let child = cmd.spawn()?;
    // Best-effort: record our PID so `sweep_stray_servers` can target
    // exactly this instance on a future sweep instead of every
    // llama-server.exe on the box. Never fails the spawn itself.
    let _ = std::fs::write(pid_file_path(), child.id().to_string());
    Ok(child)
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
    let _ = std::fs::remove_file(pid_file_path());
}

pub fn shutdown(engine: &Engine) {
    engine.shutting_down.store(true, Ordering::Relaxed);
    kill_child(engine);
}

/// A llama-server left over from an abnormal exit holds VRAM and would
/// silently force this launch onto the CPU — clear it before spawning.
///
/// Targets ONLY the PID recorded in `pid_file_path()` by a previous
/// `spawn_server` call, never every `llama-server.exe` on the box (audit
/// sec-rust #3): a bare `/IM llama-server.exe` kill would also nuke any
/// llama-server run by another app, or by a second Cleophis instance. The
/// double `/FI PID eq <pid> /FI IMAGENAME eq llama-server.exe` filter
/// additionally guards against PID reuse — taskkill only fires when BOTH
/// filters match the same process, so if the OS has since handed that PID
/// to an unrelated program, it's left alone.
#[cfg(windows)]
fn sweep_stray_servers() {
    use std::os::windows::process::CommandExt;

    let Ok(raw_pid) = std::fs::read_to_string(pid_file_path()) else {
        return;
    };
    let Ok(pid) = raw_pid.trim().parse::<u32>() else {
        return;
    };

    // Absolute path, not a bare "taskkill": Windows' CreateProcess search
    // order checks the current working directory before PATH, so a bare
    // name would let a planted taskkill.exe in an attacker-writable CWD run
    // instead of the real system binary (binary planting).
    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
    let taskkill = format!(r"{system_root}\System32\taskkill.exe");
    let _ = Command::new(&taskkill)
        .args([
            "/F",
            "/FI",
            &format!("PID eq {pid}"),
            "/FI",
            "IMAGENAME eq llama-server.exe",
        ])
        .creation_flags(0x0800_0000)
        .output();

    let _ = std::fs::remove_file(pid_file_path());
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
            if let Some(model) = model_path(&app) {
                if let Err(e) = verify_model_once(&engine, &app, &model) {
                    eprintln!("start: verify_model_once failed: {e}");
                    engine.set_status(EngineStatus::Failed);
                    let _ = app.emit(
                        "engine-failed",
                        "Model failed its integrity check — re-download it.".to_string(),
                    );
                    break;
                }
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
/// Called by `cloud::download::download_model`'s worker thread once a
/// download finishes and the file lands at its final path.
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

    fn sha256_hex(data: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(data);
        hasher.finalize().iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn model_sha256_matches_a_known_digest() {
        let dir = unique_dir("sha256-known");
        let path = dir.join("sample.bin");
        let content = b"cleophis-model-sha256-fixture";
        std::fs::write(&path, content).unwrap();

        let expected = sha256_hex(content);
        let got = model_sha256(&path).unwrap();
        assert_eq!(got, expected);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn model_sha256_streams_content_larger_than_one_chunk() {
        // Content bigger than the 256 KiB read buffer, to exercise the
        // chunked-read loop across more than one iteration rather than
        // reading the whole file in a single `read` call.
        let dir = unique_dir("sha256-chunked");
        let path = dir.join("sample.bin");
        let content: Vec<u8> = (0..600_000u32).map(|i| (i % 256) as u8).collect();
        std::fs::write(&path, &content).unwrap();

        let expected = sha256_hex(&content);
        let got = model_sha256(&path).unwrap();
        assert_eq!(got, expected);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn model_sha256_mismatch_against_a_wrong_hash_does_not_match() {
        // Mirrors the comparison `verify_model_once` makes after hashing:
        // a real file's digest must NOT case-insensitively match an
        // unrelated hash. `verify_model_once` itself takes an `AppHandle`,
        // which (like `model_path` above it) isn't constructible outside a
        // running Tauri app, so this exercises the same comparison logic
        // directly rather than wiring a full catalog + AppHandle for a
        // unit test.
        let dir = unique_dir("sha256-mismatch");
        let path = dir.join("sample.bin");
        std::fs::write(&path, b"the real model bytes").unwrap();

        let actual = model_sha256(&path).unwrap();
        let wrong_hash = "0".repeat(64);
        assert!(!actual.eq_ignore_ascii_case(&wrong_hash));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn verified_model_session_cache_round_trips_a_path() {
        // Exercises the exact field + comparison `verify_model_once` uses
        // for its session-cache short-circuit (`engine.verified_model.lock()
        // .unwrap().as_deref() == Some(model)`), without needing an
        // `AppHandle` to call `verify_model_once` itself.
        let engine = Engine::new(0);
        assert_eq!(*engine.verified_model.lock().unwrap(), None);

        let dir = unique_dir("verified-cache");
        let model = dir.join("hero.gguf");
        *engine.verified_model.lock().unwrap() = Some(model.clone());

        assert_eq!(
            engine.verified_model.lock().unwrap().as_deref(),
            Some(model.as_path())
        );

        let other = dir.join("other.gguf");
        assert_ne!(engine.verified_model.lock().unwrap().as_deref(), Some(other.as_path()));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pid_file_round_trips_a_written_pid() {
        let path = pid_file_path();
        let pid: u32 = 424242; // arbitrary, unlikely to collide with a real PID.
        std::fs::write(&path, pid.to_string()).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        let parsed: u32 = raw.trim().parse().unwrap();
        assert_eq!(parsed, pid);

        let _ = std::fs::remove_file(&path);
    }
}
