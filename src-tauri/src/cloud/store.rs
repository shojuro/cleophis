use std::io::Write;
use std::path::Path;

use keyring::Entry;
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

pub const KEYRING_SERVICE: &str = "com.cleophis.desktop";
pub const KEYRING_USER: &str = "supabase-refresh-token";

pub fn save_refresh_token(token: &str) -> Result<(), CloudError> {
    let entry = Entry::new(KEYRING_SERVICE, KEYRING_USER).map_err(|e| {
        eprintln!("keyring: failed to create entry: {e}");
        CloudError::Internal("keyring unavailable".into())
    })?;
    entry.set_password(token).map_err(|e| {
        eprintln!("keyring: failed to save refresh token: {e}");
        CloudError::Internal("keyring save failed".into())
    })
}

/// None on any error (missing entry, locked keyring, unsupported backend...).
pub fn load_refresh_token() -> Option<String> {
    let entry = Entry::new(KEYRING_SERVICE, KEYRING_USER).ok()?;
    entry.get_password().ok()
}

/// Best-effort: errors swallowed. Never logs the token itself.
pub fn delete_refresh_token() {
    if let Ok(entry) = Entry::new(KEYRING_SERVICE, KEYRING_USER) {
        let _ = entry.delete_credential();
    }
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

/// Appends a pending grant unless `model_id` is already pending or already
/// entitled. Returns true iff it appended.
pub fn queue_pending(cache: &mut CloudCache, model_id: &str, source: &str, now: i64) -> bool {
    let already_pending = cache.pending_grants.iter().any(|p| p.model_id == model_id);
    let already_entitled = cache.entitlements.iter().any(|e| e.model_id == model_id);
    if already_pending || already_entitled {
        return false;
    }
    cache.pending_grants.push(PendingGrant {
        model_id: model_id.to_string(),
        source: source.to_string(),
        created_at: now,
    });
    true
}

/// True when `now - last_online_auth` exceeds `config::OFFLINE_GRACE_DAYS`.
pub fn grace_expired(last_online_auth: i64, now: i64) -> bool {
    let grace_seconds = config::OFFLINE_GRACE_DAYS * 86_400;
    now - last_online_auth > grace_seconds
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// keyring tests hit the real OS credential store; serialize them (and
    /// anything else in this module that could race) behind one lock.
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
        let appended = queue_pending(&mut cache, "llama-8b", "trial", 1000);
        assert!(appended);
        assert_eq!(cache.pending_grants.len(), 1);
        assert_eq!(cache.pending_grants[0].model_id, "llama-8b");
        assert_eq!(cache.pending_grants[0].source, "trial");
        assert_eq!(cache.pending_grants[0].created_at, 1000);
    }

    #[test]
    fn queue_pending_duplicate_in_pending_returns_false() {
        let _g = lock();
        let mut cache = CloudCache::default();
        assert!(queue_pending(&mut cache, "llama-8b", "trial", 1000));
        let appended_again = queue_pending(&mut cache, "llama-8b", "trial", 2000);
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
        let appended = queue_pending(&mut cache, "llama-8b", "trial", 1000);
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

    // 13. keyring round-trip: save → load → delete → load is None.
    // Runs against the real OS credential store (Windows Credential Manager
    // on the interop run; linux-native/keyutils under native Linux). Cleans
    // up its own entry via a Drop guard so it never leaves a stray secret
    // behind, even if an assertion above it fails.
    struct KeyringCleanup;
    impl Drop for KeyringCleanup {
        fn drop(&mut self) {
            delete_refresh_token();
        }
    }

    #[test]
    fn keyring_round_trip() {
        let _g = lock();
        let _cleanup = KeyringCleanup;
        delete_refresh_token(); // ensure a clean slate before we start

        save_refresh_token("test-refresh-token-a4").expect("save should succeed");
        assert_eq!(
            load_refresh_token(),
            Some("test-refresh-token-a4".to_string())
        );

        delete_refresh_token();
        assert_eq!(load_refresh_token(), None);
    }
}
