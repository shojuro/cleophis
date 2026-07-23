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
    /// True while a `start` watchdog thread is alive. A tier switch waits on
    /// this (via [`restart`]) so the old thread fully exits before a new one
    /// spawns — otherwise the two would fight over `child`/VRAM. Set
    /// SYNCHRONOUSLY in `start` (before the thread is spawned, so the wait can
    /// never miss an about-to-run thread) and cleared by the thread's own
    /// drop-guard on every exit path.
    pub thread_alive: AtomicBool,
    /// Set once the app is tearing down (window Destroyed → [`shutdown`]). A
    /// [`restart`] in flight checks this after stopping the old thread and
    /// bails out instead of relaunching, so a tier switch racing app-close
    /// can't spawn a llama-server the teardown already finished reaping.
    pub closing: AtomicBool,
    /// Session cache for the load-time integrity check in
    /// `verify_model_once`: once a model path's sha256 has been checked
    /// against the catalog's pinned hash, it's recorded here so watchdog
    /// respawns of the SAME ~2GB file don't re-hash it every time — only
    /// the first successful verification per process pays the hashing
    /// cost.
    verified_model: Mutex<Option<PathBuf>>,
    /// The adapter counterpart of `verified_model` (B4): the LoRA adapter is
    /// hashed against the catalog's `adapter_sha256` at load time with the
    /// same first-time-only session caching, so a watchdog respawn of the
    /// same base+adapter pair doesn't re-hash either file.
    verified_adapter: Mutex<Option<PathBuf>>,
    /// The contract adapter's (adapter v2) counterpart, checked against
    /// `contract_adapter_sha256` — its own cache slot so both composed
    /// adapters can be verified-once independently.
    verified_contract_adapter: Mutex<Option<PathBuf>>,
    /// Serializes [`restart`] across all callers (tier switch, and now
    /// `load_model`'s Failed→restart recovery). `restart`'s own `thread_alive`
    /// wait only guards against a *previously running* watchdog, not a *second
    /// concurrent restart*: two restarts that both observe `thread_alive ==
    /// false` before either calls `start` would each spawn a watchdog thread,
    /// and the two llama-servers would fight over the fixed port + `child`
    /// (orphaning one, holding VRAM). Holding this for the whole restart makes
    /// concurrent restarts run one-at-a-time instead.
    restart_lock: Mutex<()>,
}

impl Engine {
    pub fn new(port: u16) -> Self {
        Engine {
            port,
            status: Mutex::new(EngineStatus::Starting),
            child: Mutex::new(None),
            shutting_down: AtomicBool::new(false),
            gpu_offload: AtomicBool::new(false),
            thread_alive: AtomicBool::new(false),
            closing: AtomicBool::new(false),
            verified_model: Mutex::new(None),
            verified_adapter: Mutex::new(None),
            verified_contract_adapter: Mutex::new(None),
            restart_lock: Mutex::new(()),
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

/// The resolved on-disk paths a launch needs: the base model, plus the LoRA
/// adapters composed onto it (static composition) — the always-on behavioral
/// adapter and, once adapter v2 ships, the contract-grounding adapter. Each is
/// `None` when the hero declares none — NOT when a declared one is merely
/// missing (that case fails the whole resolution; see [`resolve_launch`]).
pub struct LaunchPaths {
    pub model: PathBuf,
    pub behavioral_lora: Option<PathBuf>,
    pub contract_lora: Option<PathBuf>,
}

impl LaunchPaths {
    /// The adapters to hand llama.cpp, in composition order (behavioral first,
    /// then contract), skipping any the hero doesn't declare. Emitted by
    /// `build_server_args` as a single comma-separated `--lora`.
    pub fn loras(&self) -> Vec<&Path> {
        [self.behavioral_lora.as_deref(), self.contract_lora.as_deref()]
            .into_iter()
            .flatten()
            .collect()
    }
}

/// Resolve the hero model on disk: downloaded copy first (app data),
/// bundled copy second (dev / fat installs). None = thin install, not yet
/// downloaded.
///
/// Since B4, "on disk" means *launchable*: if the hero declares an adapter
/// (`adapter_file`) that is not present, this returns `None` exactly as if
/// the base were missing — the hero must never launch base-only when an
/// adapter is declared. Callers that only need "is the hero launchable?"
/// (main.rs setup, `start`) keep working unchanged; callers that need both
/// paths for the launch use [`resolve_launch`].
pub fn model_path(app: &AppHandle) -> Option<PathBuf> {
    resolve_launch(app).map(|l| l.model)
}

/// Resolves the full launch path set for the hero: the base model and, when
/// the hero declares one, the adapter. Fails closed (`None`) if the base is
/// missing OR if a declared adapter is missing — the same "not installed"
/// signal `model_path` has always returned, so a declared-but-undownloaded
/// adapter routes through the identical NoModel banner + download flow.
pub fn resolve_launch(app: &AppHandle) -> Option<LaunchPaths> {
    let root = resources_root(app);
    let raw = std::fs::read_to_string(root.join("catalog.json"))
        .map_err(|e| eprintln!("resolve_launch: catalog read: {e}"))
        .ok()?;
    let entries = crate::catalog::parse_catalog(&raw)
        .map_err(|e| eprintln!("resolve_launch: catalog parse: {e}"))
        .ok()?;
    let hero = crate::catalog::hero(&entries)?;
    // Tier-selection: resolve the base+adapter for the EFFECTIVE device tier
    // (override, else `hardware::detect`), not the flat 4B fields. `hero_hash`
    // keys off the same `effective_tier`, so the file resolved here and the
    // hash checked at load always come from the same variant.
    let tier = crate::tier_select::effective_tier(app);
    let variant = crate::catalog::hero_variant(hero, &tier);
    let model_file = variant.model_file.clone()?;
    let app_data = app.path().app_data_dir().ok();
    resolve_launch_paths(
        app_data,
        root,
        &model_file,
        variant.adapter_file.as_deref(),
        variant.contract_adapter_file.as_deref(),
    )
}

/// Pure resolution logic behind [`resolve_launch`]: resolves the base
/// `model_file` (app-data copy wins, else bundled, else `None`), then — iff
/// an `adapter_file` is declared — resolves it the SAME way. A declared
/// adapter that resolves to nothing collapses the whole result to `None` (`?`
/// on the inner `resolve_model`), which is what enforces "never launch
/// base-only when an adapter is declared". No `AppHandle`, so it is directly
/// unit-testable.
fn resolve_launch_paths(
    app_data: Option<PathBuf>,
    resources: PathBuf,
    model_file: &str,
    adapter_file: Option<&str>,
    contract_adapter_file: Option<&str>,
) -> Option<LaunchPaths> {
    let model = resolve_model(app_data.clone(), resources.clone(), model_file)?;
    // Each DECLARED adapter must resolve on disk, else the whole launch
    // collapses to `None` (fail-closed via `?`) — never launch base-only, or
    // behavioral-only when a contract adapter is declared.
    let behavioral_lora = match adapter_file {
        Some(f) => Some(resolve_model(app_data.clone(), resources.clone(), f)?),
        None => None,
    };
    let contract_lora = match contract_adapter_file {
        Some(f) => Some(resolve_model(app_data, resources, f)?),
        None => None,
    };
    Some(LaunchPaths { model, behavioral_lora, contract_lora })
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
    // Cache-first, exactly as before the B4 refactor: a watchdog respawn of an
    // already-verified path returns without even reading the catalog.
    if engine.verified_model.lock().unwrap().as_deref() == Some(model) {
        return Ok(());
    }
    let expected = hero_hash(app, |v| v.sha256.clone())?;
    verify_hash_once(&engine.verified_model, model, expected.as_deref(), "model")
}

/// The adapter counterpart of [`verify_model_once`] (B4): re-hashes the LoRA
/// adapter against the catalog's `adapter_sha256` before it is passed to
/// llama-server via `--lora`, with the same session cache
/// (`engine.verified_adapter`). Same fail-closed semantics: a pinned hash
/// that doesn't match is a hard error; a catalog that pins none skips the
/// check. Only ever called when [`resolve_launch`] produced a `lora` path,
/// which itself only happens when the hero declares an `adapter_file`.
fn verify_adapter_once(engine: &Engine, app: &AppHandle, adapter: &Path) -> Result<(), String> {
    if engine.verified_adapter.lock().unwrap().as_deref() == Some(adapter) {
        return Ok(());
    }
    let expected = hero_hash(app, |v| v.adapter_sha256.clone())?;
    verify_hash_once(&engine.verified_adapter, adapter, expected.as_deref(), "adapter")
}

/// The contract adapter's (adapter v2) counterpart of [`verify_adapter_once`]:
/// re-hashes it against the catalog's `contract_adapter_sha256` with its own
/// session cache. Only called when [`resolve_launch`] produced a
/// `contract_lora` path (i.e. the tier declares a `contract_adapter_file`).
fn verify_contract_adapter_once(engine: &Engine, app: &AppHandle, adapter: &Path) -> Result<(), String> {
    if engine.verified_contract_adapter.lock().unwrap().as_deref() == Some(adapter) {
        return Ok(());
    }
    let expected = hero_hash(app, |v| v.contract_adapter_sha256.clone())?;
    verify_hash_once(
        &engine.verified_contract_adapter,
        adapter,
        expected.as_deref(),
        "contract adapter",
    )
}

/// Reads the hero entry and projects one of its pinned hashes out of it —
/// the shared catalog read behind `verify_model_once`/`verify_adapter_once`.
/// Returns the (owned) hash string, or `None` when the catalog pins none.
fn hero_hash(
    app: &AppHandle,
    pick: impl Fn(&crate::catalog::ResolvedHero) -> Option<String>,
) -> Result<Option<String>, String> {
    let root = resources_root(app);
    let raw = std::fs::read_to_string(root.join("catalog.json")).map_err(|e| e.to_string())?;
    let entries = crate::catalog::parse_catalog(&raw)?;
    let hero = crate::catalog::hero(&entries)
        .ok_or_else(|| "catalog missing integrity data".to_string())?;
    // Same effective tier `resolve_launch` used, so the pinned hash matches
    // the file that was resolved onto disk.
    let tier = crate::tier_select::effective_tier(app);
    let variant = crate::catalog::hero_variant(hero, &tier);
    Ok(pick(&variant))
}

/// The shared load-time integrity check (B4 factored this out of
/// `verify_model_once` so the adapter path reuses it byte-for-byte): if
/// `cache` already records `path` this process, short-circuit; if `expected`
/// is `None` the catalog pins nothing, so there's nothing to check; otherwise
/// re-hash `path` and compare case-insensitively, recording success in
/// `cache`. `what` ("model" / "adapter") only shapes the log + error text.
fn verify_hash_once(
    cache: &Mutex<Option<PathBuf>>,
    path: &Path,
    expected: Option<&str>,
    what: &str,
) -> Result<(), String> {
    if cache.lock().unwrap().as_deref() == Some(path) {
        return Ok(());
    }

    let Some(expected) = expected else {
        eprintln!("verify_hash_once: catalog pins no sha256 for the {what} — skipping integrity check");
        return Ok(());
    };

    let actual = model_sha256(path).map_err(|e| e.to_string())?;
    if !actual.eq_ignore_ascii_case(expected) {
        eprintln!("verify_hash_once: integrity check failed for {} ({what})", path.display());
        return Err(format!("{what} integrity check failed"));
    }

    *cache.lock().unwrap() = Some(path.to_path_buf());
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

/// Builds the exact llama-server argument vector (B4's testable unit). With no
/// adapters it is byte-identical to the args this app has always passed; with
/// one or more it appends a single `--lora a,b` — llama.cpp b10042 takes
/// comma-separated adapters and COMPOSES them on the base at load (never
/// merged), verified against the bundled binary's `--help`. Pure — no
/// `AppHandle`, no process — so every branch is covered by the unit tests below.
fn build_server_args(model: &Path, port: u16, ngl: u32, loras: &[&Path]) -> Vec<String> {
    let mut args = vec![
        "-m".to_string(),
        model.to_string_lossy().into_owned(),
        "--host".to_string(),
        "127.0.0.1".to_string(),
        "--port".to_string(),
        port.to_string(),
        "-ngl".to_string(),
        ngl.to_string(),
        "-c".to_string(),
        "4096".to_string(),
        "--no-webui".to_string(),
        "--jinja".to_string(),
    ];
    if !loras.is_empty() {
        args.push("--lora".to_string());
        args.push(
            loras
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(","),
        );
    }
    args
}

fn spawn_server(app: &AppHandle, port: u16, ngl: u32) -> std::io::Result<Child> {
    let root = resources_root(app);
    let launch = resolve_launch(app)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "model not on disk"))?;
    let exe = root.join("llama").join("llama-server.exe");

    let log_path = std::env::temp_dir().join("cleophis-llama.log");
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;

    // Path args (`exe`, `-m <model>`, `--lora a,b`) may be `\\?\`-verbatim on
    // Windows — `resource_dir()` returns them that way. Unlike tesseract's
    // `--tessdata-dir`, which BREAKS on a verbatim path because tesseract
    // appends "/eng.traineddata" with a forward slash (see `ocr::strip_verbatim`
    // for that root cause), llama.cpp/CreateProcess OPEN these paths directly
    // with no forward-slash concatenation, so verbatim is safe here and is left
    // intact ON PURPOSE: de-verbatim'ing would drop long-path (>260 char)
    // support that the prefix exists to provide. Do not "harden" this by
    // stripping — the stripping fix is tesseract-specific.
    let args = build_server_args(&launch.model, port, ngl, &launch.loras());
    let mut cmd = Command::new(exe);
    cmd.args(&args)
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
    engine.closing.store(true, Ordering::Relaxed);
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
    // Mark the watchdog thread alive SYNCHRONOUSLY, before the spawn — so a
    // concurrent [`restart`] waiting on this flag can never observe `false` for
    // an about-to-run thread and race a second one into existence. The thread's
    // drop-guard clears it on every exit path.
    engine.thread_alive.store(true, Ordering::Relaxed);
    std::thread::spawn(move || {
        struct AliveGuard(Arc<Engine>);
        impl Drop for AliveGuard {
            fn drop(&mut self) {
                self.0.thread_alive.store(false, Ordering::Relaxed);
            }
        }
        let _alive = AliveGuard(engine.clone());

        engine.set_status(EngineStatus::Starting);
        sweep_stray_servers();
        let force_cpu = std::env::var("CLEOPHIS_FORCE_CPU").is_ok();
        let mut ngl: u32 = if force_cpu { 0 } else { 99 };
        let mut crashes: u32 = 0;
        'restart: loop {
            if engine.shutting_down.load(Ordering::Relaxed) {
                break;
            }
            if let Some(launch) = resolve_launch(&app) {
                // Verify the base, then (when declared) the adapter — both are
                // fed to llama.cpp, so both get the same load-time integrity
                // gate before a single byte is parsed.
                // Verify the base, the behavioral adapter, and (when declared)
                // the contract adapter — every file fed to `--lora` gets its
                // load-time integrity gate before a byte is parsed.
                let integrity = verify_model_once(&engine, &app, &launch.model)
                    .and_then(|()| match &launch.behavioral_lora {
                        Some(lora) => verify_adapter_once(&engine, &app, lora),
                        None => Ok(()),
                    })
                    .and_then(|()| match &launch.contract_lora {
                        Some(lora) => verify_contract_adapter_once(&engine, &app, lora),
                        None => Ok(()),
                    });
                if let Err(e) = integrity {
                    eprintln!("start: integrity check failed: {e}");
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

/// Stop the running engine and relaunch it on whatever [`resolve_launch`] now
/// resolves — the tier-switch primitive. The selection/catalog have changed,
/// so the engine must reload the new base+adapter. Blocks until the old
/// watchdog thread has fully exited (so the two never overlap on `child`/VRAM)
/// before spawning the fresh one; clears the per-path verify caches so the new
/// pair re-verifies. The exit wait is unbounded by design — `shutting_down` +
/// the killed child guarantee the old thread returns within ~1s, and a cap
/// could expire mid-load and spawn a second thread (the race this prevents).
/// Blocking — call it off the async runtime (`spawn_blocking`).
pub fn restart(app: AppHandle, engine: Arc<Engine>) {
    // Serialize with any other restart in flight (a concurrent tier switch, or a
    // rapid double of load_model's Failed→restart recovery). Without this, two
    // callers could both pass the `thread_alive` wait below before either spawns
    // and end up with two watchdog threads racing on the port + `child`. Held
    // for the whole stop→wait→relaunch so the second caller's sequence only
    // begins once the first has fully completed. Blocking is fine — restart is
    // always called off the async runtime.
    let _restart_guard = engine.restart_lock.lock().unwrap();
    // Stop the running engine WITHOUT setting `closing` (that flag is reserved
    // for real app teardown): flag shutdown so the old watchdog thread breaks
    // at its next checkpoint, and kill its child now.
    engine.shutting_down.store(true, Ordering::Relaxed);
    kill_child(&engine);
    // Wait for the old thread to actually exit before spawning a new one — else
    // the two would both write `engine.child`, orphaning a llama-server that
    // holds VRAM. `shutting_down` + the killed child guarantee the thread hits
    // a checkpoint and exits within ~1s, so this poll is bounded in practice;
    // we intentionally do NOT cap it (a cap could expire mid-model-load and let
    // us spawn a second thread — the exact race we're preventing).
    while engine.thread_alive.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(25));
    }
    // If the app began tearing down while we were stopping the old thread, do
    // NOT relaunch — the teardown's kill_child has already run and would not
    // reap a freshly spawned server.
    if engine.closing.load(Ordering::Relaxed) {
        return;
    }
    engine.shutting_down.store(false, Ordering::Relaxed);
    *engine.verified_model.lock().unwrap() = None;
    *engine.verified_adapter.lock().unwrap() = None;
    *engine.verified_contract_adapter.lock().unwrap() = None;
    engine.set_status(EngineStatus::Starting);
    // Clone so `engine` (and thus `_restart_guard`, which borrows it) stays
    // alive through `start`: the lock must be held until `start` has set
    // `thread_alive = true`, or a second restart could still slip past the
    // wait above. The guard drops at function end, just after `start` returns.
    start(app, engine.clone());
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

    // ---- B4 Step 1: the launch-arg builder (the testable unit) ----

    #[test]
    fn build_server_args_without_lora_is_the_historical_arg_vec() {
        let model = PathBuf::from("/models/base.gguf");
        let got = build_server_args(&model, 8080, 99, &[]);
        assert_eq!(
            got,
            vec![
                "-m", "/models/base.gguf",
                "--host", "127.0.0.1",
                "--port", "8080",
                "-ngl", "99",
                "-c", "4096",
                "--no-webui",
                "--jinja",
            ]
        );
        // The historical arg vec carries NO `--lora` when no adapter is given.
        assert!(!got.iter().any(|a| a == "--lora"));
    }

    #[test]
    fn build_server_args_with_lora_appends_lora_flag_and_path() {
        let model = PathBuf::from("/models/base.gguf");
        let lora = PathBuf::from("/models/adapter.gguf");
        let got = build_server_args(&model, 8080, 0, &[lora.as_path()]);
        assert_eq!(
            got,
            vec![
                "-m", "/models/base.gguf",
                "--host", "127.0.0.1",
                "--port", "8080",
                "-ngl", "0",
                "-c", "4096",
                "--no-webui",
                "--jinja",
                "--lora", "/models/adapter.gguf",
            ]
        );
        // The only difference vs. the no-lora branch is the trailing pair —
        // every leading arg is identical, byte for byte.
        let base = build_server_args(&model, 8080, 0, &[]);
        assert_eq!(&got[..base.len()], base.as_slice());
        assert_eq!(&got[base.len()..], &["--lora".to_string(), "/models/adapter.gguf".to_string()]);
    }

    #[test]
    fn build_server_args_with_two_loras_comma_joins_a_single_flag() {
        // Static composition: behavioral + contract adapters go in ONE
        // comma-separated `--lora`, in order (llama.cpp b10042 composes them).
        let model = PathBuf::from("/models/base.gguf");
        let behavioral = PathBuf::from("/models/behavioral.gguf");
        let contract = PathBuf::from("/models/contract.gguf");
        let got = build_server_args(&model, 8080, 0, &[behavioral.as_path(), contract.as_path()]);
        assert_eq!(
            &got[got.len() - 2..],
            &[
                "--lora".to_string(),
                "/models/behavioral.gguf,/models/contract.gguf".to_string(),
            ]
        );
        // Exactly one `--lora` flag, never two.
        assert_eq!(got.iter().filter(|a| *a == "--lora").count(), 1);
    }

    #[test]
    fn build_server_args_enables_jinja_for_tool_calling() {
        let args = build_server_args(Path::new("m.gguf"), 8080, 99, &[]);
        assert!(args.iter().any(|a| a == "--jinja"),
            "llama-server must launch with --jinja so the calc tool template is active");
    }

    // ---- B4 Step 1: adapter-aware launch resolution ----

    #[test]
    fn resolve_launch_paths_both_present_when_adapter_declared() {
        let app_data = unique_dir("launch-both-ad");
        let resources = unique_dir("launch-both-res");
        std::fs::create_dir_all(app_data.join("models")).unwrap();
        std::fs::write(app_data.join("models/base.gguf"), b"base").unwrap();
        std::fs::write(app_data.join("models/adapter.gguf"), b"adapter").unwrap();

        let got = resolve_launch_paths(
            Some(app_data.clone()),
            resources,
            "models/base.gguf",
            Some("models/adapter.gguf"),
            None,
        )
        .expect("both files present -> Some");
        assert_eq!(got.model, app_data.join("models/base.gguf"));
        assert_eq!(got.behavioral_lora, Some(app_data.join("models/adapter.gguf")));
        assert_eq!(got.contract_lora, None);
        assert_eq!(got.loras(), vec![app_data.join("models/adapter.gguf").as_path()]);
    }

    #[test]
    fn resolve_launch_paths_composes_behavioral_and_contract_when_both_declared() {
        let app_data = unique_dir("launch-compose-ad");
        let resources = unique_dir("launch-compose-res");
        std::fs::create_dir_all(app_data.join("models")).unwrap();
        for f in ["models/base.gguf", "models/behavioral.gguf", "models/contract.gguf"] {
            std::fs::write(app_data.join(f), b"x").unwrap();
        }
        let got = resolve_launch_paths(
            Some(app_data.clone()),
            resources,
            "models/base.gguf",
            Some("models/behavioral.gguf"),
            Some("models/contract.gguf"),
        )
        .expect("all three present -> Some");
        // Composition order: behavioral first, contract second.
        assert_eq!(
            got.loras(),
            vec![
                app_data.join("models/behavioral.gguf").as_path(),
                app_data.join("models/contract.gguf").as_path(),
            ]
        );
    }

    #[test]
    fn resolve_launch_paths_declared_contract_adapter_missing_is_none() {
        // Fail-closed: a declared contract adapter that isn't on disk collapses
        // the launch even though base + behavioral are present.
        let app_data = unique_dir("launch-nocontract-ad");
        let resources = unique_dir("launch-nocontract-res");
        std::fs::create_dir_all(app_data.join("models")).unwrap();
        std::fs::write(app_data.join("models/base.gguf"), b"x").unwrap();
        std::fs::write(app_data.join("models/behavioral.gguf"), b"x").unwrap();
        // contract.gguf deliberately NOT written.
        let got = resolve_launch_paths(
            Some(app_data),
            resources,
            "models/base.gguf",
            Some("models/behavioral.gguf"),
            Some("models/contract.gguf"),
        );
        assert!(got.is_none(), "declared-but-missing contract adapter must resolve to None");
    }

    #[test]
    fn resolve_launch_paths_declared_adapter_missing_is_none() {
        // The contract: a hero that DECLARES an adapter but whose adapter file
        // is absent resolves to None (treated as not-installed) even though
        // the base is present — never launch base-only.
        let app_data = unique_dir("launch-noadapter-ad");
        let resources = unique_dir("launch-noadapter-res");
        std::fs::create_dir_all(app_data.join("models")).unwrap();
        std::fs::write(app_data.join("models/base.gguf"), b"base").unwrap();
        // adapter.gguf deliberately NOT written.

        let got = resolve_launch_paths(
            Some(app_data),
            resources,
            "models/base.gguf",
            Some("models/adapter.gguf"),
            None,
        );
        assert!(got.is_none(), "declared-but-missing adapter must resolve to None");
    }

    #[test]
    fn resolve_launch_paths_no_adapter_declared_returns_base_only() {
        let app_data = unique_dir("launch-plain-ad");
        let resources = unique_dir("launch-plain-res");
        std::fs::create_dir_all(app_data.join("models")).unwrap();
        std::fs::write(app_data.join("models/base.gguf"), b"base").unwrap();

        let got = resolve_launch_paths(Some(app_data.clone()), resources, "models/base.gguf", None, None)
            .expect("base present, no adapter declared -> Some");
        assert_eq!(got.model, app_data.join("models/base.gguf"));
        assert_eq!(got.behavioral_lora, None);
        assert!(got.loras().is_empty());
    }

    #[test]
    fn resolve_launch_paths_missing_base_is_none_regardless_of_adapter() {
        let app_data = unique_dir("launch-nobase-ad");
        let resources = unique_dir("launch-nobase-res");
        // Neither file written.
        let got = resolve_launch_paths(
            Some(app_data),
            resources,
            "models/base.gguf",
            Some("models/adapter.gguf"),
            None,
        );
        assert!(got.is_none(), "missing base must resolve to None");
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
