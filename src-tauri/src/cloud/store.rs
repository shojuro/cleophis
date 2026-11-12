//! On-disk cloud state: the single-account `CloudCache`, the per-account
//! `auth-cache/` entries, and the pure helpers over both.
//!
//! **Secrets do not live here.** The refresh token and the offline-sign-in
//! verifiers moved to [`crate::cloud::secure_store`] in Phase 3.1, because
//! their storage is platform-dependent (OS keyring on desktop, AndroidKeyStore
//! on Android) while everything remaining in this file is plain JSON that
//! behaves identically on every target. The split is along that line and not
//! along module tidiness: what is left needs no `cfg` at all.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::cloud::config;
use crate::cloud::error::CloudError;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Entitlement {
    pub model_id: String,
    pub source: String,
    pub created_at: String,
    #[serde(default)]
    pub expires_at: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PendingGrant {
    pub model_id: String,
    pub source: String,
    /// Owner stamp: the `user_id` the cache belonged to when this grant was
    /// queued. `#[serde(default)]` for backward compat with cache files
    /// written before this field existed (deserializes to `""`, which never
    /// matches a real authenticated user_id, so old unowned rows are simply
    /// dropped at the next flush rather than misattributed).
    #[serde(default)]
    pub user_id: String,
    pub created_at: i64,
}

#[derive(Serialize, Deserialize, Default, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct CloudCache {
    pub user_id: String,
    pub email: String,
    pub nickname: String,
    pub entitlements: Vec<Entitlement>,
    pub pending_grants: Vec<PendingGrant>,
    pub last_online_auth: i64,
}

/// One snapshot per account, keyed by `user_id`, under `auth-cache/`. Unlike
/// `CloudCache` (which tracks the single currently-signed-in account), these
/// persist per-account so a signed-out user can be recognized (and their
/// offline sign-in throttled) by `find_user_by_email` without another
/// online round trip. `failed_attempts`/`last_failed_at` are the offline
/// sign-in throttle counters (Task 5) — they live here, not in the keyring
/// verifier entry, since they change on every attempt and the verifier
/// itself should stay untouched.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AuthCacheEntry {
    pub user_id: String,
    pub email: String,
    pub nickname: String,
    pub entitlements: Vec<Entitlement>,
    pub last_online_auth: i64,
    pub failed_attempts: u32,
    pub last_failed_at: i64,
}

/// Missing or corrupt file → `CloudCache::default()`.
pub fn read_cache(path: &Path) -> CloudCache {
    match std::fs::read_to_string(path) {
        Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
        Err(_) => CloudCache::default(),
    }
}

/// Creates parent dirs as needed; writes via temp file + rename so a reader
/// never observes a partially-written cache file.
pub fn write_cache(path: &Path, cache: &CloudCache) -> Result<(), CloudError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| CloudError::Internal(format!("failed to create cache dir: {e}")))?;
        }
    }
    let json = serde_json::to_string_pretty(cache)
        .map_err(|e| CloudError::Internal(format!("failed to serialize cache: {e}")))?;

    let tmp_path = temp_write_path(path);
    {
        let mut file = std::fs::File::create(&tmp_path)
            .map_err(|e| CloudError::Internal(format!("failed to write cache: {e}")))?;
        file.write_all(json.as_bytes())
            .map_err(|e| CloudError::Internal(format!("failed to write cache: {e}")))?;
    }
    std::fs::rename(&tmp_path, path)
        .map_err(|e| CloudError::Internal(format!("failed to finalize cache: {e}")))?;
    Ok(())
}

/// A temp path unique per writer, alongside `path`: `<name>.<pid>.<nanos>.tmp`
/// in the same directory. Uniqueness matters if two processes (or two racing
/// writes in tests) target the same cache file concurrently — each gets its
/// own temp file, so neither clobbers the other before the atomic rename.
fn temp_write_path(path: &Path) -> std::path::PathBuf {
    let file_name = path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    path.with_file_name(format!("{file_name}.{}.{nanos}.tmp", std::process::id()))
}

/// Appends a pending grant, owner-stamped with `user_id`, unless `model_id`
/// is already pending or already entitled. Returns true iff it appended.
pub fn queue_pending(
    cache: &mut CloudCache,
    model_id: &str,
    source: &str,
    user_id: &str,
    now: i64,
) -> bool {
    let already_pending = cache.pending_grants.iter().any(|p| p.model_id == model_id);
    let already_entitled = cache.entitlements.iter().any(|e| e.model_id == model_id);
    if already_pending || already_entitled {
        return false;
    }
    cache.pending_grants.push(PendingGrant {
        model_id: model_id.to_string(),
        source: source.to_string(),
        user_id: user_id.to_string(),
        created_at: now,
    });
    true
}

/// True when `now - last_online_auth` exceeds `config::OFFLINE_GRACE_DAYS`.
pub fn grace_expired(last_online_auth: i64, now: i64) -> bool {
    let grace_seconds = config::OFFLINE_GRACE_DAYS * 86_400;
    now - last_online_auth > grace_seconds
}

/// `<app_data>/auth-cache` — one JSON file per account lives here, named
/// `<user_id>.json`.
pub fn auth_cache_dir(app_data: &Path) -> PathBuf {
    app_data.join("auth-cache")
}

fn auth_cache_path(dir: &Path, user_id: &str) -> PathBuf {
    dir.join(format!("{user_id}.json"))
}

/// Missing or corrupt file → `None` (never a default entry — unlike
/// `CloudCache`, there's no sensible default for a specific account that
/// doesn't have a cache entry yet).
pub fn read_auth_cache(dir: &Path, user_id: &str) -> Option<AuthCacheEntry> {
    let contents = std::fs::read_to_string(auth_cache_path(dir, user_id)).ok()?;
    serde_json::from_str(&contents).ok()
}

/// Creates `dir` as needed; writes via temp file + rename (same pattern as
/// `write_cache`) so a reader never observes a partially-written entry.
pub fn write_auth_cache(dir: &Path, entry: &AuthCacheEntry) -> Result<(), CloudError> {
    std::fs::create_dir_all(dir)
        .map_err(|e| CloudError::Internal(format!("failed to create auth-cache dir: {e}")))?;
    let path = auth_cache_path(dir, &entry.user_id);
    let json = serde_json::to_string_pretty(entry)
        .map_err(|e| CloudError::Internal(format!("failed to serialize auth-cache entry: {e}")))?;

    let tmp_path = temp_write_path(&path);
    {
        let mut file = std::fs::File::create(&tmp_path)
            .map_err(|e| CloudError::Internal(format!("failed to write auth-cache entry: {e}")))?;
        file.write_all(json.as_bytes())
            .map_err(|e| CloudError::Internal(format!("failed to write auth-cache entry: {e}")))?;
    }
    std::fs::rename(&tmp_path, &path)
        .map_err(|e| CloudError::Internal(format!("failed to finalize auth-cache entry: {e}")))?;
    Ok(())
}

/// Scans `dir`'s `*.json` entries for one whose email matches (trim +
/// lowercase compare), returning the first match. Used to recognize a
/// signed-out user by the email they type, before they've proven they know
/// the password.
pub fn find_user_by_email(dir: &Path, email: &str) -> Option<AuthCacheEntry> {
    let target = normalize_email(email);
    let read_dir = std::fs::read_dir(dir).ok()?;
    for entry in read_dir.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(cached) = serde_json::from_str::<AuthCacheEntry>(&contents) else {
            continue;
        };
        if normalize_email(&cached.email) == target {
            return Some(cached);
        }
    }
    None
}

/// Trim + lowercase, so `find_user_by_email` matches regardless of
/// surrounding whitespace or case the user typed.
pub fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

/// Sanitizes a session user id into an `auth-cache` filename segment — a
/// VERBATIM MIRROR of `kpack.rs`'s/`convstore.rs`'s module-private
/// `account_dir_segment` (see either's doc comment for the full "reject,
/// don't strip" rationale; neither is `pub`/reachable from here, hence a
/// third copy rather than a shared import — keep all three in sync if any
/// ever changes). Accepts `user_id` unchanged if — and only if — it's
/// already clean: non-empty, every char `[A-Za-z0-9_-]`. Anything else is a
/// hard `None`, never a stripped-down remainder.
///
/// Task 6 ("remove account from this device") is the reason this exists
/// here: unlike `read_auth_cache`/`write_auth_cache` above (only ever
/// called with a real Supabase-issued UUID `enroll_verifier` itself
/// produced), [`read_known_account`]/[`delete_auth_cache_entry`] below are
/// reachable with a raw, front-end-supplied `user_id` — this is what stops
/// `../evil` (or any other traversal/separator string) from ever reaching
/// a `dir.join(...)` call.
///
/// `pub(crate)` since Phase 3.2 so `secure_store::blob` can reuse it rather
/// than add a **fourth** copy of this rule. A visibility widen, no behaviour
/// change — the same move 1.1 made with `verify_launch`. The Android port
/// turns a `user_id` into a filename for the first time (on desktop it was
/// only ever a keyring entry name, where `../` is an ordinary character), and
/// a sanitizer that disagreed with its siblings would be worse than a strict
/// one.
pub(crate) fn account_dir_segment(user_id: &str) -> Option<String> {
    let is_clean = !user_id.is_empty()
        && user_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if is_clean {
        Some(user_id.to_string())
    } else {
        None
    }
}

/// Task 6's KNOWN-ACCOUNT gate: `Some` only if `user_id` is both a
/// well-formed segment ([`account_dir_segment`]) AND has an existing,
/// parseable `auth-cache/<user_id>.json` entry in `dir`. A malformed id
/// (traversal dots, separators, empty) is a `None` immediately, before any
/// path join or filesystem access — the enforcement point that keeps
/// `Cloud::remove_account_auth` from ever touching an arbitrary/foreign id.
pub fn read_known_account(dir: &Path, user_id: &str) -> Option<AuthCacheEntry> {
    let segment = account_dir_segment(user_id)?;
    let contents = std::fs::read_to_string(dir.join(format!("{segment}.json"))).ok()?;
    serde_json::from_str(&contents).ok()
}

/// Deletes `user_id`'s `auth-cache/<user_id>.json` entry, if present.
/// Sanitized via [`account_dir_segment`] — the same as [`read_known_account`]
/// — so a malformed id is a hard `Err`, never a best-effort/traversal-prone
/// path join. Missing file is not an error (idempotent).
pub fn delete_auth_cache_entry(dir: &Path, user_id: &str) -> Result<(), CloudError> {
    let segment = account_dir_segment(user_id)
        .ok_or_else(|| CloudError::Internal("invalid account id".into()))?;
    match std::fs::remove_file(dir.join(format!("{segment}.json"))) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(CloudError::Internal(format!(
            "failed to delete auth-cache entry: {e}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serializes this module's tests against each other. Deliberately a
    /// LOCAL lock, not `cloud::test_support::lock()`: nothing left in this
    /// file touches the OS credential store (Phase 3.1 moved those to
    /// `secure_store`), so these tests share no global state with the rest
    /// of the `cloud` module — only temp paths with each other.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn temp_path(name: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        p.push(format!(
            "cleophis-a4-test-{}-{}-{}",
            std::process::id(),
            unique,
            name
        ));
        p
    }

    // 10. cache round-trip: write then read → equal; missing path → default;
    // corrupt JSON → default.
    #[test]
    fn cache_round_trip() {
        let _g = lock();
        let path = temp_path("cache-roundtrip.json");
        let mut cache = CloudCache::default();
        cache.user_id = "user-1".into();
        cache.email = "a@example.com".into();
        cache.nickname = "Nick".into();
        cache.last_online_auth = 12345;
        cache.entitlements.push(Entitlement {
            model_id: "llama-8b".into(),
            source: "purchase".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            expires_at: None,
        });
        cache.pending_grants.push(PendingGrant {
            model_id: "phi-4".into(),
            source: "trial".into(),
            user_id: "user-1".into(),
            created_at: 999,
        });

        write_cache(&path, &cache).expect("write_cache should succeed");
        let read_back = read_cache(&path);
        assert_eq!(read_back, cache);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn cache_read_missing_path_returns_default() {
        let _g = lock();
        let path = temp_path("does-not-exist.json");
        let cache = read_cache(&path);
        assert_eq!(cache, CloudCache::default());
    }

    #[test]
    fn cache_read_corrupt_json_returns_default() {
        let _g = lock();
        let path = temp_path("corrupt.json");
        std::fs::write(&path, b"{ not valid json ").unwrap();
        let cache = read_cache(&path);
        assert_eq!(cache, CloudCache::default());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn write_cache_creates_parent_dirs() {
        let _g = lock();
        let mut path = temp_path("nested-dir");
        path.push("sub");
        path.push("cache.json");
        let cache = CloudCache::default();
        write_cache(&path, &cache).expect("should create parent dirs and write");
        assert!(path.exists());
        let _ = std::fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
    }

    // 11. queue_pending: appends once; duplicate pending model_id → false;
    // model_id already in entitlements → false.
    #[test]
    fn queue_pending_appends_once() {
        let _g = lock();
        let mut cache = CloudCache::default();
        let appended = queue_pending(&mut cache, "llama-8b", "trial", "user-1", 1000);
        assert!(appended);
        assert_eq!(cache.pending_grants.len(), 1);
        assert_eq!(cache.pending_grants[0].model_id, "llama-8b");
        assert_eq!(cache.pending_grants[0].source, "trial");
        assert_eq!(cache.pending_grants[0].user_id, "user-1");
        assert_eq!(cache.pending_grants[0].created_at, 1000);
    }

    #[test]
    fn queue_pending_duplicate_in_pending_returns_false() {
        let _g = lock();
        let mut cache = CloudCache::default();
        assert!(queue_pending(&mut cache, "llama-8b", "trial", "user-1", 1000));
        let appended_again = queue_pending(&mut cache, "llama-8b", "trial", "user-1", 2000);
        assert!(!appended_again);
        assert_eq!(cache.pending_grants.len(), 1);
    }

    #[test]
    fn queue_pending_already_entitled_returns_false() {
        let _g = lock();
        let mut cache = CloudCache::default();
        cache.entitlements.push(Entitlement {
            model_id: "llama-8b".into(),
            source: "purchase".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            expires_at: None,
        });
        let appended = queue_pending(&mut cache, "llama-8b", "trial", "user-1", 1000);
        assert!(!appended);
        assert!(cache.pending_grants.is_empty());
    }

    // 12. grace_expired: boundary math for OFFLINE_GRACE_DAYS=30.
    #[test]
    fn grace_expired_boundaries() {
        let day = 86_400;
        let last_online_auth = 0;
        let now_29 = last_online_auth + 29 * day;
        let now_31 = last_online_auth + 31 * day;
        assert!(!grace_expired(last_online_auth, now_29));
        assert!(grace_expired(last_online_auth, now_31));
    }

    // Tests 13 (`keyring_round_trip`) and 14 (`verifier_keyring_roundtrip`)
    // moved with their subjects to `secure_store/desktop.rs` in Phase 3.1.
    // They still run in the desktop suite, unchanged, and are now excluded
    // from the Android build by construction rather than by remembering to
    // exclude them: the module that holds them is itself
    // `cfg(not(target_os = "android"))`.

    // 15. auth-cache round-trip + case/space-insensitive email lookup.
    #[test]
    fn auth_cache_roundtrip_and_email_lookup() {
        let _g = lock();
        let dir = temp_path("auth-cache-t2");
        let entry = AuthCacheEntry {
            user_id: "uid-A".into(),
            email: "Alice@Example.com".into(),
            nickname: "Alice".into(),
            entitlements: vec![],
            last_online_auth: 1000,
            failed_attempts: 0,
            last_failed_at: 0,
        };
        write_auth_cache(&dir, &entry).unwrap();
        assert_eq!(
            read_auth_cache(&dir, "uid-A").unwrap().email,
            "Alice@Example.com"
        );
        // case/space-insensitive email lookup:
        assert_eq!(
            find_user_by_email(&dir, "  alice@example.com ")
                .unwrap()
                .user_id,
            "uid-A"
        );
        assert!(find_user_by_email(&dir, "bob@example.com").is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    // 16. Task 6's known-account gate: an enrolled entry resolves; an
    // unenrolled id (no `.json` in `dir` at all) is `None`.
    #[test]
    fn read_known_account_finds_an_enrolled_entry_and_none_for_an_unknown_one() {
        let _g = lock();
        let dir = temp_path("auth-cache-t6-known");
        let entry = AuthCacheEntry {
            user_id: "uid-oa6-known".into(),
            email: "oa6-known@example.com".into(),
            nickname: "OA6Known".into(),
            entitlements: vec![],
            last_online_auth: 1000,
            failed_attempts: 0,
            last_failed_at: 0,
        };
        write_auth_cache(&dir, &entry).unwrap();

        let found = read_known_account(&dir, "uid-oa6-known").expect("should find the enrolled entry");
        assert_eq!(found.email, "oa6-known@example.com");

        assert!(
            read_known_account(&dir, "uid-oa6-never-enrolled").is_none(),
            "an id with no auth-cache entry must never resolve"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // 17. THE traversal-rejection assertion (Task 6 — SECURITY): a
    // malformed/traversal id is `None` — never a path join outside `dir` —
    // even when a file exists at exactly the location a naive
    // `dir.join(format!("{user_id}.json"))` would resolve to.
    #[test]
    fn read_known_account_rejects_traversal_and_separator_ids() {
        let _g = lock();
        let dir = temp_path("auth-cache-t6-traversal");
        std::fs::create_dir_all(&dir).unwrap();
        // The file a naive, unsanitized join of "../evil" would resolve to
        // — one directory above `dir`, named `evil.json`.
        let decoy = dir.parent().unwrap().join("evil.json");
        std::fs::write(&decoy, br#"{"userId":"not-a-real-account"}"#).unwrap();

        assert!(read_known_account(&dir, "../evil").is_none());
        assert!(read_known_account(&dir, "a/b").is_none());
        assert!(read_known_account(&dir, "a\\b").is_none());
        assert!(read_known_account(&dir, "").is_none());
        assert!(
            decoy.exists(),
            "a rejected traversal id must never even reach the filesystem"
        );

        let _ = std::fs::remove_file(&decoy);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // 18. delete_auth_cache_entry: deletes an existing entry; a missing
    // file is a clean no-op (idempotent); a malformed id is a hard `Err`,
    // never a best-effort delete.
    #[test]
    fn delete_auth_cache_entry_deletes_missing_is_noop_malformed_is_err() {
        let _g = lock();
        let dir = temp_path("auth-cache-t6-delete");
        let entry = AuthCacheEntry {
            user_id: "uid-oa6-delete".into(),
            email: "oa6-delete@example.com".into(),
            nickname: "OA6Delete".into(),
            entitlements: vec![],
            last_online_auth: 1000,
            failed_attempts: 0,
            last_failed_at: 0,
        };
        write_auth_cache(&dir, &entry).unwrap();
        assert!(read_auth_cache(&dir, "uid-oa6-delete").is_some());

        delete_auth_cache_entry(&dir, "uid-oa6-delete").expect("delete should succeed");
        assert!(read_auth_cache(&dir, "uid-oa6-delete").is_none());

        // Deleting again (already gone) is a clean no-op, not an error.
        delete_auth_cache_entry(&dir, "uid-oa6-delete").expect("missing entry should be a no-op");

        let err = delete_auth_cache_entry(&dir, "../evil").unwrap_err();
        assert!(matches!(err, CloudError::Internal(_)));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
