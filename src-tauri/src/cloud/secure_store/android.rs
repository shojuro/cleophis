//! Android: AES-256-GCM under a non-exportable AndroidKeyStore key.
//!
//! The six functions in the desktop body's shape, backed by ciphertext blobs
//! on disk and the Kotlin `SecureStore` (`gen/android/.../SecureStore.kt`)
//! reached through the bridge proven in the native chunk. Phase 3.2.
//!
//! # The division of labour, and why it is this way round
//!
//! Kotlin does crypto only. **This file and `blob.rs` own everything else** —
//! which file a secret lives in, the AAD that binds it there, the on-disk
//! envelope, and the atomic write. The pure rules live in [`super::blob`],
//! compiled on every platform and tested by the desktop suite (decision D-3),
//! because each of them fails silently: a path that escapes its directory
//! throws nothing, and a `save`/`load` pair that derive different AAD produce
//! a decrypt failure whose symptom is "you keep getting signed out" and whose
//! apparent cause is the keystore.
//!
//! The conventional Android answer — a Kotlin store owning its own
//! `SharedPreferences` — would have put every one of those rules on the far
//! side of a JNI boundary, in the one language this project cannot run a test
//! in. It would also have made the sign-out purge a Kotlin responsibility,
//! when `sign_out` is Rust and already deletes the cache file two lines away.
//!
//! # Failure is always "no credential", never a crash and never a partial read
//!
//! No bridge, no activity yet, no key, tampered blob, unknown envelope
//! version, unreadable directory: all of it resolves to `None` / `Err`, the
//! user signs in again, and a `[secure-store]` line says which. Nothing here
//! returns half a plaintext and nothing here panics.
//!
//! **A failed decrypt does NOT delete the blob**, deliberately. A destructive
//! read path would turn any transient keystore unavailability into permanent
//! credential loss; leaving the blob costs one failed read per launch until
//! the next successful `save_*` overwrites it, which is self-healing in the
//! safe direction.
//!
//! # Log hygiene (security review M3)
//!
//! No token, no verifier, no `user_id`, no ciphertext reaches a log line. The
//! Java-side detail that would identify a failure — `AEADBadTagException`
//! versus a keystore error — goes to logcat via the bridge's
//! `exception_describe`, which prints a stack trace and not our data.

use std::path::PathBuf;

use crate::android_bridge;
use crate::cloud::error::CloudError;
use crate::cloud::verifier::StoredVerifier;

use super::blob::{self, Slot};

/// Must match `SecureStore.kt`'s package and name.
const SECURE_STORE_CLASS: &str = "com.cleophis.app.SecureStore";

/// `[secure-store]` matches the `[bridge]` / `[kernels]` tag convention: a
/// device transcript carries its own explanation instead of requiring someone
/// to have believed a build log.
///
/// **Every read and every write emits exactly one line, success included**,
/// and that is a deliberate correction rather than verbosity. The first
/// version of this file logged only failures, which would have made the happy
/// path *silent* — and this milestone has already recorded, for the bridge
/// probe, that "no line at all" is the one outcome worse than a failure,
/// because it cannot be told apart from "the code never ran". Phase 3.2's
/// device checkpoint is "sign in → force-stop → relaunch → still signed in",
/// and without a success line the only evidence would be an inference from
/// app behaviour. One line per launch is what the `[kernels]` line already
/// costs, and it carries no secret.
fn trace(op: &str, outcome: &str) {
    eprintln!("[secure-store] {op}: {outcome}");
}

fn internal(op: &str, reason: &str) -> CloudError {
    trace(op, reason);
    CloudError::Internal(format!("secure_store::{op} failed"))
}

// ---------------------------------------------------------------- JNI calls

fn blob_dir() -> Result<PathBuf, String> {
    android_bridge::with_app_class(SECURE_STORE_CLASS, |env, class, activity| {
        let value = env
            .call_static_method(
                class,
                "blobDir",
                "(Landroid/content/Context;)Ljava/lang/String;",
                &[activity.into()],
            )
            .and_then(|v| v.l())
            .map_err(|e| format!("blobDir: {e}"))?;
        android_bridge::jstring_result(env, value, "blobDir")
    })
    .map(PathBuf::from)
}

/// Returns base64 of `iv ‖ ciphertext‖tag`. `plaintext` never appears in an
/// error string — `jni::Error` carries call-site detail, not our arguments.
fn encrypt(aad: &str, plaintext: &str) -> Result<String, String> {
    android_bridge::with_app_class(SECURE_STORE_CLASS, |env, class, _activity| {
        let j_aad = env.new_string(aad).map_err(|e| format!("encrypt: {e}"))?;
        let j_text = env
            .new_string(plaintext)
            .map_err(|e| format!("encrypt: {e}"))?;
        let value = env
            .call_static_method(
                class,
                "encrypt",
                "(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;",
                &[(&j_aad).into(), (&j_text).into()],
            )
            .and_then(|v| v.l())
            .map_err(|e| format!("encrypt: {e}"))?;
        android_bridge::jstring_result(env, value, "encrypt")
    })
}

fn decrypt(aad: &str, payload_b64: &str) -> Result<String, String> {
    android_bridge::with_app_class(SECURE_STORE_CLASS, |env, class, _activity| {
        let j_aad = env.new_string(aad).map_err(|e| format!("decrypt: {e}"))?;
        let j_blob = env
            .new_string(payload_b64)
            .map_err(|e| format!("decrypt: {e}"))?;
        let value = env
            .call_static_method(
                class,
                "decrypt",
                "(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;",
                &[(&j_aad).into(), (&j_blob).into()],
            )
            .and_then(|v| v.l())
            .map_err(|e| format!("decrypt: {e}"))?;
        android_bridge::jstring_result(env, value, "decrypt")
    })
}

// ------------------------------------------------------------- file storage

/// Write via temp file + rename, the pattern `store::write_cache` and
/// `store::write_auth_cache` already use — so a reader never observes a
/// half-written blob, and a crash mid-write leaves the previous credential
/// intact rather than a truncated one that would fail its tag check forever.
fn store_blob(slot: Slot<'_>, plaintext: &str, op: &str) -> Result<(), CloudError> {
    let key = slot
        .key()
        .map_err(|_| internal(op, "malformed account id rejected before any path join"))?;
    let dir = blob_dir().map_err(|e| internal(op, &e))?;
    let payload = encrypt(&key.aad, plaintext).map_err(|e| internal(op, &e))?;

    let path = dir.join(&key.file_name);
    let tmp = dir.join(format!("{}.{}.tmp", key.file_name, std::process::id()));
    std::fs::write(&tmp, blob::wrap(&payload).as_bytes())
        .map_err(|e| internal(op, &format!("write: {e}")))?;
    std::fs::rename(&tmp, &path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        internal(op, &format!("rename: {e}"))
    })?;
    trace(op, "ok (encrypted, written)");
    Ok(())
}

/// One line on every path, so a device transcript distinguishes all four
/// outcomes: `ok`, `no blob stored` (first launch, or after sign-out), a named
/// failure, and — by its absence — never called at all.
fn read_blob(slot: Slot<'_>, op: &str) -> Option<String> {
    let key = slot
        .key()
        .map_err(|_| trace(op, "malformed account id"))
        .ok()?;
    let dir = blob_dir().map_err(|e| trace(op, &e)).ok()?;

    let contents = match std::fs::read_to_string(dir.join(&key.file_name)) {
        Ok(c) => c,
        // Not an error: nothing has been stored yet. Still traced — this is
        // the expected first-launch line, and its presence is what proves the
        // read path ran at all.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            trace(op, "no blob stored");
            return None;
        }
        Err(e) => {
            trace(op, &format!("read: {e}"));
            return None;
        }
    };

    let payload = blob::unwrap(&contents)
        .map_err(|e| trace(op, &e.to_string()))
        .ok()?;
    let plaintext = decrypt(&key.aad, payload).map_err(|e| trace(op, &e)).ok()?;
    trace(op, "ok (blob decrypted, tag verified)");
    Some(plaintext)
}

/// Best-effort and idempotent, mirroring the desktop bodies' contract. A
/// missing file is success — this is what `sign_out` calls, and "there was
/// nothing to remove" is the correct outcome, not a failure.
fn delete_blob(slot: Slot<'_>, op: &str) {
    let Ok(key) = slot.key() else {
        trace(op, "malformed account id");
        return;
    };
    let dir = match blob_dir() {
        Ok(d) => d,
        Err(e) => {
            trace(op, &e);
            return;
        }
    };
    // The purge half of CP3 ("sign out → blobs purged") is checked by looking
    // at the directory, but these lines are what say the delete was attempted
    // at all — the difference between "nothing there" and "never asked".
    match std::fs::remove_file(dir.join(&key.file_name)) {
        Ok(()) => trace(op, "ok (blob removed)"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => trace(op, "no blob to remove"),
        Err(e) => trace(op, &format!("remove: {e}")),
    }
}

// -------------------------------------------------------------- the six fns

pub fn save_refresh_token(token: &str) -> Result<(), CloudError> {
    store_blob(Slot::RefreshToken, token, "save_refresh_token")
}

pub fn load_refresh_token() -> Option<String> {
    read_blob(Slot::RefreshToken, "load_refresh_token")
}

pub fn delete_refresh_token() {
    delete_blob(Slot::RefreshToken, "delete_refresh_token");
}

pub fn save_verifier(user_id: &str, v: &StoredVerifier) -> Result<(), CloudError> {
    let op = "save_verifier";
    let json = serde_json::to_string(v)
        .map_err(|e| internal(op, &format!("failed to serialize verifier: {e}")))?;
    store_blob(Slot::Verifier { user_id }, &json, op)
}

pub fn load_verifier(user_id: &str) -> Option<StoredVerifier> {
    let op = "load_verifier";
    let json = read_blob(Slot::Verifier { user_id }, op)?;
    // Corrupt JSON inside a blob that decrypted AND passed its GCM tag means
    // we wrote it wrong, not that anyone tampered with it — worth a line,
    // unlike a missing file.
    serde_json::from_str(&json)
        .map_err(|e| trace(op, &format!("verifier JSON: {e}")))
        .ok()
}

pub fn delete_verifier(user_id: &str) {
    delete_blob(Slot::Verifier { user_id }, "delete_verifier");
}
