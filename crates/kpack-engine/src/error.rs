//! The engine's error type — one enum spanning artifact resolution, the
//! load-time integrity gate, adapter composition, and the native backend.
//!
//! The variants deliberately name the same failure modes `inference.rs`
//! encodes so the mobile in-process engine is auditable against the shipped
//! sidecar path: [`EngineError::Missing`] is the `resolve_launch_paths`
//! fail-closed collapse ("never launch base-only when an adapter is
//! declared"), and [`EngineError::Integrity`] is `verify_hash_once`'s
//! fail-closed sha256 mismatch.

use std::fmt;
use std::path::PathBuf;

/// Everything the engine can fail with. `Backend` is the only variant that
/// carries a native-layer message; the rest are pure-Rust policy failures
/// that the mock backend reproduces exactly, so composition and gating are
/// testable with no native toolchain.
#[derive(Debug)]
pub enum EngineError {
    /// A declared artifact (base model or an adapter the stack names) is not
    /// present on disk. Mirrors `inference.rs::resolve_launch_paths`
    /// collapsing the whole launch to `None` when a declared file is absent —
    /// fail-closed, never a base-only or reduced-stack fallback.
    Missing { what: String, path: PathBuf },

    /// A per-artifact sha256 gate failed: the file's digest did not match the
    /// catalog-pinned hash. Fail-closed, mirrors
    /// `inference.rs::verify_hash_once`. A pinned hash of `None` is NOT this
    /// error — it skips the check (dev/fixture catalog).
    Integrity { what: String, path: PathBuf },

    /// The adapter set is not composable: two adapters claimed the same role,
    /// or a role invariant was violated. Caught before any native load.
    Composition(String),

    /// A native backend failure — model load, context creation, LoRA apply,
    /// decode, sampling, or chat-template rendering over FFI.
    Backend(String),

    /// I/O while reading or hashing an artifact.
    Io(String),
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EngineError::Missing { what, path } => {
                write!(f, "{what} not on disk: {}", path.display())
            }
            EngineError::Integrity { what, path } => {
                write!(f, "{what} integrity check failed: {}", path.display())
            }
            EngineError::Composition(msg) => write!(f, "adapter composition error: {msg}"),
            EngineError::Backend(msg) => write!(f, "engine backend error: {msg}"),
            EngineError::Io(msg) => write!(f, "engine io error: {msg}"),
        }
    }
}

impl std::error::Error for EngineError {}

impl From<std::io::Error> for EngineError {
    fn from(e: std::io::Error) -> Self {
        EngineError::Io(e.to_string())
    }
}
