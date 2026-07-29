//! Android: no secure storage yet. Every call says so, out loud.
//!
//! Phase 3.2 replaces this file with the Kotlin `SecureStore`
//! (AndroidKeyStore AES-256-GCM, non-exportable key, per-encryption 96-bit IV)
//! reached through the JNI bridge proven in the native chunk. Until then these
//! six bodies exist to make the gap **legible**, which is the entire point of
//! 3.1 — the thing they replace was not a missing feature, it was a working
//! feature that lied.
//!
//! # Why a loud stub beats the `keyring` fallback it replaces
//!
//! On Android `keyring` v3 resolves to its `mock` store (see the module doc on
//! `secure_store.rs` for the source-level check). `MockCredentialBuilder`
//! declares `CredentialPersistence::EntryOnly` and builds a fresh
//! `MockCredential` per `Entry::new`, so `save_refresh_token` returned **`Ok`**
//! after writing into a value dropped on the very next line. The failure was
//! not that nothing persisted — it is that the call reported success. Every
//! caller downstream was entitled to believe a secret was on disk.
//!
//! So the shape here is chosen by what each signature is *able* to report:
//!
//! - The two `save_*` functions return `Result`, so they return `Err`. Both
//!   call sites already treat a persist failure as "degrade to a memory-only
//!   session" and log it (`session.rs`'s `apply_and_sync` step 1 and
//!   `refresh_via_gate`), so this is a path the code was written for — the
//!   session still works, it simply does not outlive the process.
//! - The two `load_*` functions return `Option`, where `None` is
//!   indistinguishable from "nothing was ever stored" — which on this platform
//!   is permanently true. A returned value cannot carry the reason, so the log
//!   line is the only channel there is, and without it a founder reading
//!   logcat after a force-stop sees a sign-out with no explanation anywhere.
//! - The two `delete_*` functions return `()`. They are honest no-ops (there
//!   is nothing to delete), and they log because sign-out's purge is a
//!   security-relevant promise: when 3.2 lands, a purge that silently skipped
//!   a real blob is exactly the defect worth having a breadcrumb for.
//!
//! # Log hygiene (security review M3)
//!
//! No token, no verifier, no `user_id` reaches these lines — matching the
//! desktop bodies, which log the keyring's own error and never the secret.
//! The parameters are `_`-prefixed to say deliberately-unused rather than
//! accidentally-dropped.

use crate::cloud::error::CloudError;
use crate::cloud::verifier::StoredVerifier;

/// One home for the message, so the `Err` a caller propagates and the line a
/// founder reads in logcat cannot drift apart. `[secure-store]` matches the
/// `[bridge]` / `[kernels]` tag convention: a device transcript carries its
/// own explanation rather than needing someone to have believed a build log.
const UNAVAILABLE: &str =
    "secure storage is not implemented on Android yet (Phase 3.2 — AndroidKeyStore); \
     this session is memory-only and will not survive a force-stop";

fn unavailable(op: &str) -> CloudError {
    eprintln!("[secure-store] {op}: {UNAVAILABLE}");
    CloudError::Internal(format!("secure_store::{op} unavailable on Android"))
}

fn note(op: &str) {
    eprintln!("[secure-store] {op}: {UNAVAILABLE}");
}

pub fn save_refresh_token(_token: &str) -> Result<(), CloudError> {
    Err(unavailable("save_refresh_token"))
}

/// Always `None` on Android — nothing is ever stored, so nothing can be read.
/// This is why a force-stop signs the user out, and the log line is the only
/// place that fact is stated.
pub fn load_refresh_token() -> Option<String> {
    note("load_refresh_token");
    None
}

/// A truthful no-op: there is nothing persisted to remove.
pub fn delete_refresh_token() {
    note("delete_refresh_token");
}

pub fn save_verifier(_user_id: &str, _v: &StoredVerifier) -> Result<(), CloudError> {
    Err(unavailable("save_verifier"))
}

/// Always `None`, so offline sign-in reports `OfflineNoVerifier` ("Sign in
/// online once on this device first") — which is the truth on Android until
/// 3.2, since enrollment's `save_verifier` above cannot have succeeded.
pub fn load_verifier(_user_id: &str) -> Option<StoredVerifier> {
    note("load_verifier");
    None
}

/// A truthful no-op: there is nothing persisted to remove.
pub fn delete_verifier(_user_id: &str) {
    note("delete_verifier");
}
