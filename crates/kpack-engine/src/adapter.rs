//! Adapter composition + the load-time integrity gate — the pure-Rust half of
//! the engine that mirrors `src-tauri/src/inference.rs` byte-for-behavior
//! without depending on it or on Tauri.
//!
//! What is reproduced here, and where it lives on desktop:
//!   - **Composition order** behavioral→contract→voice — `inference.rs`'s
//!     `LaunchPaths::loras()` flattens `[behavioral, contract]` in that order;
//!     mobile adds the voice layer, so [`AdapterRole`]'s discriminant *is* the
//!     order and [`prepare_stack`] sorts by it.
//!   - **Fail-closed resolution** — a declared adapter that is not on disk
//!     collapses the whole stack (`resolve_launch_paths`'s `?`). Never load
//!     base-only when adapters are declared; never load a reduced stack
//!     silently (the §0 fallback ladder is an explicit, escalated decision,
//!     [`PreparedStack::without_voice`], not a resolution-time fallback).
//!   - **Per-adapter sha256 gate, verify-once** — `verify_model_once` /
//!     `verify_adapter_once` / `verify_contract_adapter_once` each hash their
//!     artifact against the catalog-pinned hash before a byte is parsed, cache
//!     the success for the process, and fail closed on mismatch. A pinned hash
//!     of `None` skips the check (dev/fixture catalog). [`VerifyCache`] is that
//!     mechanism, factored the way `verify_hash_once` factored it on desktop.

use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use sha2::{Digest, Sha256};

use crate::error::EngineError;

/// The three adapter roles composed onto the hero base, in the spec's
/// canonical order behavioral→contract→voice (spec §0/§8). The enum
/// discriminant IS the composition order — [`AdapterRole::order`] returns it —
/// so a stack sorted by `order()` reproduces `inference.rs::LaunchPaths::loras()`
/// (with voice appended, the layer desktop does not yet carry).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AdapterRole {
    /// The always-on behavioral adapter (Stage-5 personality/refusals).
    Behavioral,
    /// The contract / anti-hallucination adapter. Never dropped in the
    /// fallback ladder (spec §0) — [`AdapterRole::is_droppable`] is false.
    Contract,
    /// The voice adapter. The ONLY droppable layer (spec §0 fallback ladder).
    Voice,
}

impl AdapterRole {
    /// Composition rank: behavioral(0) → contract(1) → voice(2). Sorting a
    /// stack by this reproduces the sidecar's `--lora a,b,c` order.
    pub fn order(self) -> u8 {
        match self {
            AdapterRole::Behavioral => 0,
            AdapterRole::Contract => 1,
            AdapterRole::Voice => 2,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            AdapterRole::Behavioral => "behavioral",
            AdapterRole::Contract => "contract",
            AdapterRole::Voice => "voice",
        }
    }

    /// Whether the fallback ladder (spec §0) may drop this layer. Only
    /// [`AdapterRole::Voice`] is droppable; the contract adapter is the
    /// anti-hallucination half and is never the one dropped.
    pub fn is_droppable(self) -> bool {
        matches!(self, AdapterRole::Voice)
    }
}

/// One adapter to compose onto the base: its role, on-disk path, the
/// catalog-pinned sha256 (`None` = the catalog pins none, so the gate is
/// skipped exactly as `verify_hash_once` skips a `None` expected hash), and
/// the LoRA scale (1.0 reproduces the sidecar `--lora` default weight).
#[derive(Debug, Clone)]
pub struct AdapterSpec {
    pub role: AdapterRole,
    pub path: PathBuf,
    pub expected_sha256: Option<String>,
    pub scale: f32,
}

impl AdapterSpec {
    /// A spec with the default scale (1.0) and no pinned hash. Chain
    /// [`AdapterSpec::with_sha256`] / [`AdapterSpec::with_scale`] to set them.
    pub fn new(role: AdapterRole, path: impl Into<PathBuf>) -> Self {
        AdapterSpec {
            role,
            path: path.into(),
            expected_sha256: None,
            scale: 1.0,
        }
    }

    pub fn with_sha256(mut self, sha256: impl Into<String>) -> Self {
        self.expected_sha256 = Some(sha256.into());
        self
    }

    pub fn with_scale(mut self, scale: f32) -> Self {
        self.scale = scale;
        self
    }
}

/// The base model: path + the catalog-pinned sha256 (same `None`-skips
/// semantics as [`AdapterSpec`], mirroring `verify_model_once`).
#[derive(Debug, Clone)]
pub struct ModelSpec {
    pub path: PathBuf,
    pub expected_sha256: Option<String>,
}

impl ModelSpec {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        ModelSpec {
            path: path.into(),
            expected_sha256: None,
        }
    }

    pub fn with_sha256(mut self, sha256: impl Into<String>) -> Self {
        self.expected_sha256 = Some(sha256.into());
        self
    }
}

/// A validated, composition-ordered, integrity-gated stack ready to hand to a
/// backend's `load`. Producing one (via [`prepare_stack`]) guarantees:
///   - the base and every declared adapter exist on disk (fail-closed),
///   - no two adapters share a role,
///   - adapters are sorted behavioral→contract→voice,
///   - every pinned sha256 verified (fail-closed on mismatch, skipped on `None`).
/// A backend therefore never re-implements policy — it loads what it is given.
#[derive(Debug, Clone)]
pub struct PreparedStack {
    pub base: ModelSpec,
    /// Adapters in composition order (sorted by [`AdapterRole::order`]).
    pub adapters: Vec<AdapterSpec>,
}

impl PreparedStack {
    /// The mounted roles, in composition order — what a handle reports via
    /// `mounted_adapters()` and what a probe run records as the tested stack.
    pub fn roles(&self) -> Vec<AdapterRole> {
        self.adapters.iter().map(|a| a.role).collect()
    }

    /// The spec §0 fallback-ladder retreat, made explicit: drop ONLY the voice
    /// layer, keeping behavioral + contract. By construction this can never
    /// drop the contract adapter — it filters on [`AdapterRole::is_droppable`].
    /// This is a deliberate, escalated decision (a caller invokes it with fresh
    /// evidence), never a silent resolution-time fallback.
    pub fn without_voice(&self) -> PreparedStack {
        PreparedStack {
            base: self.base.clone(),
            adapters: self
                .adapters
                .iter()
                .filter(|a| !a.role.is_droppable())
                .cloned()
                .collect(),
        }
    }
}

/// Validate + integrity-gate a base and its adapter set into a
/// [`PreparedStack`]. The single choke point every backend routes `load`
/// through, so composition order, fail-closed resolution, and the sha256 gate
/// are proven once (against the mock) and inherited by the native backend.
///
/// Order of operations mirrors `inference.rs::start`: resolve/verify the base
/// first, then each adapter in composition order — every file fed to the
/// engine gets its gate before a byte is parsed.
pub fn prepare_stack(
    base: &ModelSpec,
    adapters: &[AdapterSpec],
    verify: &VerifyCache,
) -> Result<PreparedStack, EngineError> {
    // Reject duplicate roles before touching disk — a stack with two
    // behavioral adapters is a caller bug, not a composition.
    let mut seen = HashSet::new();
    for a in adapters {
        if !seen.insert(a.role) {
            return Err(EngineError::Composition(format!(
                "duplicate adapter role: {}",
                a.role.as_str()
            )));
        }
    }

    // Base first: exist-then-verify, fail-closed on either.
    require_present(&base.path, "base model")?;
    verify.verify(&base.path, base.expected_sha256.as_deref(), "model")?;

    // Sort into composition order (behavioral→contract→voice), then gate each.
    let mut ordered: Vec<AdapterSpec> = adapters.to_vec();
    ordered.sort_by_key(|a| a.role.order());
    for a in &ordered {
        require_present(&a.path, a.role.as_str())?;
        verify.verify(&a.path, a.expected_sha256.as_deref(), a.role.as_str())?;
    }

    Ok(PreparedStack {
        base: base.clone(),
        adapters: ordered,
    })
}

/// Fail-closed existence check — the `resolve_model` half of
/// `resolve_launch_paths`. A declared-but-absent artifact is `Missing`, which
/// a caller must treat as "not installed", never as "load without it".
fn require_present(path: &Path, what: &str) -> Result<(), EngineError> {
    if path.exists() {
        Ok(())
    } else {
        Err(EngineError::Missing {
            what: what.to_string(),
            path: path.to_path_buf(),
        })
    }
}

/// The load-time integrity gate with the verify-once session cache from
/// `inference.rs` (`verify_model_once` et al.): a path whose sha256 already
/// matched this process is not re-hashed — the exact optimization that keeps a
/// watchdog respawn of the same ~2 GB file from re-paying the hash. Fail-closed
/// on mismatch; a `None` expected hash skips the check (dev/fixture catalog
/// that pins nothing).
#[derive(Default)]
pub struct VerifyCache {
    verified: Mutex<HashSet<PathBuf>>,
}

impl VerifyCache {
    pub fn new() -> Self {
        VerifyCache::default()
    }

    /// Gate `path` against `expected`. Short-circuits if already verified this
    /// process; skips (with a note) when `expected` is `None`; otherwise hashes
    /// and compares case-insensitively, recording success. `what` shapes only
    /// the log/error text.
    pub fn verify(
        &self,
        path: &Path,
        expected: Option<&str>,
        what: &str,
    ) -> Result<(), EngineError> {
        if self.verified.lock().unwrap().contains(path) {
            return Ok(());
        }

        let Some(expected) = expected else {
            eprintln!(
                "VerifyCache: catalog pins no sha256 for the {what} — skipping integrity check"
            );
            return Ok(());
        };

        let actual = file_sha256(path)?;
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(EngineError::Integrity {
                what: what.to_string(),
                path: path.to_path_buf(),
            });
        }

        self.verified.lock().unwrap().insert(path.to_path_buf());
        Ok(())
    }
}

/// Streams `path` through SHA-256 in fixed 256 KiB chunks — the same shape as
/// `inference.rs::model_sha256` / `cloud::download::rehash_existing` — so a
/// multi-GB model is never read into memory whole. Returns the lowercase hex
/// digest.
pub fn file_sha256(path: &Path) -> std::io::Result<String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_dir(name: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "kpack-engine-test-{}-{}-{}",
            std::process::id(),
            unique,
            name
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sha256_hex(data: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(data);
        hasher.finalize().iter().map(|b| format!("{b:02x}")).collect()
    }

    fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, bytes).unwrap();
        p
    }

    #[test]
    fn order_is_behavioral_contract_voice() {
        assert!(AdapterRole::Behavioral.order() < AdapterRole::Contract.order());
        assert!(AdapterRole::Contract.order() < AdapterRole::Voice.order());
    }

    #[test]
    fn only_voice_is_droppable() {
        assert!(!AdapterRole::Behavioral.is_droppable());
        assert!(!AdapterRole::Contract.is_droppable());
        assert!(AdapterRole::Voice.is_droppable());
    }

    #[test]
    fn prepare_stack_sorts_into_composition_order() {
        let dir = unique_dir("order");
        let base = ModelSpec::new(write(&dir, "base.gguf", b"base"));
        // Declared out of order on purpose — prepare_stack must sort them.
        let adapters = vec![
            AdapterSpec::new(AdapterRole::Voice, write(&dir, "voice.gguf", b"v")),
            AdapterSpec::new(AdapterRole::Behavioral, write(&dir, "beh.gguf", b"b")),
            AdapterSpec::new(AdapterRole::Contract, write(&dir, "con.gguf", b"c")),
        ];
        let stack = prepare_stack(&base, &adapters, &VerifyCache::new()).unwrap();
        assert_eq!(
            stack.roles(),
            vec![
                AdapterRole::Behavioral,
                AdapterRole::Contract,
                AdapterRole::Voice
            ]
        );
    }

    #[test]
    fn prepare_stack_fails_closed_on_missing_adapter() {
        let dir = unique_dir("missing-adapter");
        let base = ModelSpec::new(write(&dir, "base.gguf", b"base"));
        // behavioral declared but never written -> whole stack collapses.
        let adapters = vec![AdapterSpec::new(
            AdapterRole::Behavioral,
            dir.join("nope.gguf"),
        )];
        let err = prepare_stack(&base, &adapters, &VerifyCache::new()).unwrap_err();
        assert!(matches!(err, EngineError::Missing { .. }), "got {err:?}");
    }

    #[test]
    fn prepare_stack_fails_closed_on_missing_base() {
        let dir = unique_dir("missing-base");
        let base = ModelSpec::new(dir.join("absent.gguf"));
        let err = prepare_stack(&base, &[], &VerifyCache::new()).unwrap_err();
        assert!(matches!(err, EngineError::Missing { .. }), "got {err:?}");
    }

    #[test]
    fn prepare_stack_rejects_duplicate_roles() {
        let dir = unique_dir("dup-role");
        let base = ModelSpec::new(write(&dir, "base.gguf", b"base"));
        let adapters = vec![
            AdapterSpec::new(AdapterRole::Behavioral, write(&dir, "a.gguf", b"a")),
            AdapterSpec::new(AdapterRole::Behavioral, write(&dir, "b.gguf", b"b")),
        ];
        let err = prepare_stack(&base, &adapters, &VerifyCache::new()).unwrap_err();
        assert!(matches!(err, EngineError::Composition(_)), "got {err:?}");
    }

    #[test]
    fn integrity_gate_accepts_matching_hash() {
        let dir = unique_dir("gate-ok");
        let bytes = b"the real adapter bytes";
        let path = write(&dir, "beh.gguf", bytes);
        let base = ModelSpec::new(write(&dir, "base.gguf", b"base"));
        let adapters = vec![
            AdapterSpec::new(AdapterRole::Behavioral, path).with_sha256(sha256_hex(bytes))
        ];
        assert!(prepare_stack(&base, &adapters, &VerifyCache::new()).is_ok());
    }

    #[test]
    fn integrity_gate_fails_closed_on_mismatch() {
        let dir = unique_dir("gate-bad");
        let path = write(&dir, "beh.gguf", b"tampered bytes");
        let base = ModelSpec::new(write(&dir, "base.gguf", b"base"));
        let adapters =
            vec![AdapterSpec::new(AdapterRole::Behavioral, path).with_sha256("00".repeat(32))];
        let err = prepare_stack(&base, &adapters, &VerifyCache::new()).unwrap_err();
        assert!(matches!(err, EngineError::Integrity { .. }), "got {err:?}");
    }

    #[test]
    fn integrity_gate_skips_when_no_hash_pinned() {
        // None expected hash = dev/fixture catalog pins nothing -> gate skipped,
        // exactly as verify_hash_once returns Ok on a None expected hash.
        let dir = unique_dir("gate-none");
        let path = write(&dir, "beh.gguf", b"whatever");
        let base = ModelSpec::new(write(&dir, "base.gguf", b"base"));
        let adapters = vec![AdapterSpec::new(AdapterRole::Behavioral, path)];
        assert!(prepare_stack(&base, &adapters, &VerifyCache::new()).is_ok());
    }

    #[test]
    fn verify_cache_short_circuits_second_call() {
        let dir = unique_dir("verify-once");
        let path = write(&dir, "m.gguf", b"content");
        let cache = VerifyCache::new();
        cache.verify(&path, Some(&sha256_hex(b"content")), "model").unwrap();
        // Overwrite the file with different bytes; the cached path must NOT be
        // re-hashed (verify-once), so the stale-but-cached path still passes —
        // the exact behavior inference.rs's session cache has.
        std::fs::write(&path, b"different now").unwrap();
        assert!(cache.verify(&path, Some(&sha256_hex(b"content")), "model").is_ok());
    }

    #[test]
    fn without_voice_keeps_behavioral_and_contract_drops_voice() {
        let dir = unique_dir("drop-voice");
        let base = ModelSpec::new(write(&dir, "base.gguf", b"base"));
        let adapters = vec![
            AdapterSpec::new(AdapterRole::Behavioral, write(&dir, "beh.gguf", b"b")),
            AdapterSpec::new(AdapterRole::Contract, write(&dir, "con.gguf", b"c")),
            AdapterSpec::new(AdapterRole::Voice, write(&dir, "voice.gguf", b"v")),
        ];
        let stack = prepare_stack(&base, &adapters, &VerifyCache::new()).unwrap();
        let reduced = stack.without_voice();
        assert_eq!(
            reduced.roles(),
            vec![AdapterRole::Behavioral, AdapterRole::Contract],
            "fallback ladder drops only voice; contract is never dropped"
        );
    }

    #[test]
    fn file_sha256_matches_known_digest() {
        let dir = unique_dir("sha");
        let content = b"kpack-engine-sha-fixture";
        let path = write(&dir, "s.bin", content);
        assert_eq!(file_sha256(&path).unwrap(), sha256_hex(content));
    }
}
