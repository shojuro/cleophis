//! In-process inference engine (Android) — the mobile half of the
//! `inference.rs` cfg seam.
//!
//! Desktop runs llama.cpp as a `llama-server` sidecar and talks to it over
//! loopback HTTP. Android cannot spawn a sidecar at all, so the same lifecycle
//! (`start` / `start_if_no_model` / `restart` / `shutdown`) is re-implemented
//! here on top of `kpack-engine`'s in-process `EngineBackend`, and `inference`
//! re-exports these four functions under `cfg(mobile)`. Everything else in
//! `inference.rs` — `Engine`, `EngineStatus`, `EngineInfo`, catalog/tier
//! resolution, and the fail-closed sha256 integrity gate — is shared verbatim
//! between the two platforms.
//!
//! **Phase 1.1 lands the seam only.** The functions below are deliberate
//! fail-closed placeholders: they move the engine to `Failed` with a clear
//! message rather than pretending to be `Ready`, so a CP0/CP1 build reports an
//! unimplemented engine instead of hanging on a chat that can never stream.
//! Phase 1.2 replaces these bodies with the real lifecycle — a dedicated
//! inference thread owning the `!Send` `EngineHandle`, mpsc commands, and
//! `EngineBackend::load → session → stream` mapped onto the same
//! `engine-ready` / `engine-restarting` / `engine-failed` events the desktop
//! watchdog emits. The integrity gate (`verify_*_once` against the catalog's
//! pinned hashes) moves with it and stays fail-closed.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use tauri::{AppHandle, Emitter};

use crate::inference::{Engine, EngineStatus};

/// What the UI is told while the in-process engine is still unimplemented.
/// Phase 1.2 deletes this along with the placeholder bodies.
const NOT_YET_IMPLEMENTED: &str =
    "The on-device engine is not available in this build yet.";

/// Mobile counterpart of `inference::start`.
///
/// The fail-closed integrity gate is live already, ahead of the engine it
/// guards: every file a launch would feed to llama.cpp is hashed against the
/// catalog's pinned value here, through the very same
/// `inference::verify_launch` the desktop watchdog calls. That means the
/// "tampered file → engine-failed" probe is testable on-device at CP1 whether
/// or not generation works yet, and 1.2 inherits a gate that has already run on
/// real hardware instead of one written blind.
pub fn start(app: AppHandle, engine: Arc<Engine>) {
    engine.set_status(EngineStatus::Starting);

    if let Some(launch) = crate::inference::resolve_launch(&app) {
        if let Err(e) = crate::inference::verify_launch(&engine, &app, &launch) {
            eprintln!("engine_inproc::start: integrity check failed: {e}");
            engine.set_status(EngineStatus::Failed);
            let _ = app.emit(
                "engine-failed",
                "Model failed its integrity check — re-download it.".to_string(),
            );
            return;
        }
    }

    // Everything past the gate is Phase 1.2.
    eprintln!("engine_inproc::start: in-process engine lands in Phase 1.2");
    engine.set_status(EngineStatus::Failed);
    let _ = app.emit("engine-failed", NOT_YET_IMPLEMENTED.to_string());
}

/// Mobile counterpart of `inference::start_if_no_model`. Keeps the desktop
/// double-start guard — the NoModel→Starting check-and-set — so the download
/// completion path behaves identically on both platforms.
pub fn start_if_no_model(app: AppHandle, engine: Arc<Engine>) {
    if crate::inference::try_begin_start(&engine) {
        start(app, engine);
    } else {
        eprintln!("start_if_no_model: ignored, engine was not in NoModel state");
    }
}

/// Mobile counterpart of `inference::restart` (the tier-switch primitive).
///
/// Clears the per-path verify caches exactly as desktop does, so the newly
/// selected base+adapter pair is re-hashed against the catalog rather than
/// inheriting the previous selection's verdict. There is no sidecar process
/// and no watchdog thread to join yet, so the desktop `thread_alive` wait has
/// no counterpart until 1.2 introduces the inference thread.
pub fn restart(app: AppHandle, engine: Arc<Engine>) {
    if engine.closing.load(Ordering::Relaxed) {
        return;
    }
    engine.clear_verify_caches();
    start(app, engine);
}

/// Mobile counterpart of `inference::shutdown`, called from the window
/// `Destroyed` handler. Sets the same two teardown flags desktop does; 1.2 adds
/// the thread join and model unload.
pub fn shutdown(engine: &Engine) {
    engine.closing.store(true, Ordering::Relaxed);
    engine.shutting_down.store(true, Ordering::Relaxed);
}
