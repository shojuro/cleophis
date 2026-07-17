use std::path::PathBuf;
use std::sync::Mutex;

use serde::Serialize;

use crate::cloud::auth;
use crate::cloud::error::CloudError;
use crate::cloud::rest;
use crate::cloud::store::{self, Entitlement};
use crate::hardware;

/// In-memory only — never serialized to disk as a whole. The refresh token
/// is mirrored to the OS keyring; the access token lives nowhere else.
#[derive(Clone, Debug)]
pub struct Session {
    pub user_id: String,
    pub email: String,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: i64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub signed_in: bool,
    pub nickname: Option<String>,
    pub email: Option<String>,
    /// "online" | "offlineCached" | "signedOut"
    pub mode: String,
    pub entitlements: Vec<Entitlement>,
    pub grace_expired: bool,
}

impl SessionInfo {
    pub fn signed_out() -> Self {
        SessionInfo {
            signed_in: false,
            nickname: None,
            email: None,
            mode: "signedOut".into(),
            entitlements: vec![],
            grace_expired: false,
        }
    }
}

/// The stateful session layer. Composes `auth` (GoTrue), `rest` (PostgREST),
/// and `store` (keyring + on-disk cache) into the six operations the
/// front-end drives via `cloud::commands`.
///
/// Locking discipline (binding, see the task brief):
/// - `session` is NEVER held across network I/O — read/copy under lock,
///   drop the guard, do I/O, re-lock only to write the result back.
/// - `refresh_gate` is the ONE lock allowed to span I/O, and only across a
///   single refresh round-trip (single-flight token rotation: a second
///   caller that queues up behind the gate re-checks freshness once it
///   gets in, rather than firing a redundant refresh).
/// - Rotation discipline: after a successful `auth::refresh`, the new
///   refresh token is persisted to the keyring BEFORE the in-memory
///   session is updated or success is reported to the caller.
pub struct Cloud {
    session: Mutex<Option<Session>>,
    nickname: Mutex<Option<String>>,
    refresh_gate: Mutex<()>,
    cache_path: PathBuf,
}

impl Cloud {
    pub fn new(cache_path: PathBuf) -> Self {
        Cloud {
            session: Mutex::new(None),
            nickname: Mutex::new(None),
            refresh_gate: Mutex::new(()),
            cache_path,
        }
    }

    /// Boot-time restore. NEVER returns Err for offline — offline is a
    /// success mode (the app must still open with whatever it has cached).
    pub fn restore(&self) -> SessionInfo {
        let refresh_token = match store::load_refresh_token() {
            Some(t) => t,
            None => return SessionInfo::signed_out(),
        };
        let mut cache = store::read_cache(&self.cache_path);
        match auth::refresh(&refresh_token) {
            Ok(tok) => self.apply_and_sync(tok, &mut cache),
            // Any 4xx on refresh means the token was revoked or rotated
            // away server-side — the session cannot be re-established, so
            // purge everything and report signed out.
            Err(CloudError::SessionExpired) => {
                store::delete_refresh_token();
                self.clear_memory();
                let _ = std::fs::remove_file(&self.cache_path);
                SessionInfo::signed_out()
            }
            // Offline, server trouble (Api{..}), or anything else we don't
            // specifically recognize: never brick boot. Fall back to
            // whatever is cached.
            Err(_) => self.offline_cached_info(&cache),
        }
    }

    pub fn sign_in(&self, email: &str, password: &str) -> Result<SessionInfo, CloudError> {
        let tok = auth::sign_in_password(email, password)?;
        let mut cache = store::read_cache(&self.cache_path);
        Ok(self.apply_and_sync(tok, &mut cache))
    }

    pub fn sign_up(
        &self,
        email: &str,
        password: &str,
        nickname: &str,
    ) -> Result<SessionInfo, CloudError> {
        let tok = auth::sign_up(email, password, nickname)?;
        let mut cache = store::read_cache(&self.cache_path);
        Ok(self.apply_and_sync(tok, &mut cache))
    }

    pub fn sign_out(&self) {
        // Read/copy the access token under the session lock, then drop the
        // guard before the (best-effort) network logout call below — the
        // guard is a statement-local temporary, released at the semicolon.
        let access_token = self
            .session
            .lock()
            .unwrap()
            .as_ref()
            .map(|s| s.access_token.clone());
        if let Some(access_token) = access_token {
            auth::logout(&access_token); // best-effort, no refresh attempted
        }
        store::delete_refresh_token();
        self.clear_memory();
        let _ = std::fs::remove_file(&self.cache_path);
    }

    pub fn grant(&self, model_id: &str, source: &str) -> Result<(), CloudError> {
        if source != "trial" && source != "library" {
            return Err(CloudError::Internal("invalid source".into()));
        }

        let mut cache = store::read_cache(&self.cache_path);
        let has_session = self.session.lock().unwrap().is_some();

        if !has_session {
            // Never signed in on this device at all -> nothing to queue
            // against. Previously signed in (offline-cached) -> queue.
            if cache.user_id.is_empty() {
                return Err(CloudError::SessionExpired);
            }
            self.queue_grant(&mut cache, model_id, source);
            return Ok(());
        }

        match self.ensure_fresh() {
            Err(CloudError::Offline) | Err(CloudError::SessionExpired) => {
                self.queue_grant(&mut cache, model_id, source);
                Ok(())
            }
            Err(e) => Err(e),
            Ok((access, user_id)) => {
                self.grant_with_access(&mut cache, &access, &user_id, model_id, source)
            }
        }
    }

    pub fn entitlements(&self) -> Result<Vec<Entitlement>, CloudError> {
        let mut cache = store::read_cache(&self.cache_path);
        let has_session = self.session.lock().unwrap().is_some();
        if !has_session {
            return Ok(cache.entitlements);
        }

        match self.ensure_fresh() {
            Err(CloudError::Offline) => Ok(cache.entitlements),
            Err(e) => Err(e),
            Ok((access, _user_id)) => match rest::list_entitlements(&access) {
                Ok(list) => {
                    cache.entitlements = list.clone();
                    self.write_cache_best_effort(&cache);
                    Ok(list)
                }
                Err(CloudError::Offline) => Ok(cache.entitlements),
                Err(e) => Err(e),
            },
        }
    }

    /// The shared post-auth pass: persists the rotated refresh token,
    /// writes the in-memory session, fetches nickname + entitlements
    /// (falling back to cache on error), flushes any queued pending
    /// grants, fires off a best-effort device upsert, and rewrites the
    /// cache. Called from `restore`, `sign_in`, and `sign_up` alike.
    fn apply_and_sync(&self, tok: auth::TokenResponse, cache: &mut store::CloudCache) -> SessionInfo {
        // 1. Persist the refresh token FIRST (rotation discipline) — a
        // keyring failure degrades to a memory-only session rather than
        // aborting the sign-in.
        if let Err(e) = store::save_refresh_token(&tok.refresh_token) {
            eprintln!("cloud: failed to persist refresh token to keyring: {e}");
        }

        // 2. Write the in-memory session. Keep our own copies of the
        // fields the rest of this pass needs, since `tok`'s fields are
        // about to be moved into the `Session` — and the session lock must
        // NOT be held across the network calls that follow.
        let access_token = tok.access_token.clone();
        let user_id = tok.user_id.clone();
        let email = tok.email.clone();
        {
            let mut guard = self.session.lock().unwrap();
            *guard = Some(Session {
                user_id: tok.user_id,
                email: tok.email,
                access_token: tok.access_token,
                refresh_token: tok.refresh_token,
                expires_at: tok.expires_at,
            });
        } // guard dropped here, before any I/O below.

        // 3. Nickname: server profile row, else cached nickname, else the
        // email's local-part.
        let nickname_value = match rest::get_profile_nickname(&access_token) {
            Ok(Some(n)) => n,
            _ => {
                if !cache.nickname.is_empty() {
                    cache.nickname.clone()
                } else {
                    local_part(&email)
                }
            }
        };
        *self.nickname.lock().unwrap() = Some(nickname_value.clone());

        // 4. Entitlements: fresh list from the server, else stale-but-usable
        // cache.
        let mut entitlements = match rest::list_entitlements(&access_token) {
            Ok(list) => list,
            Err(_) => cache.entitlements.clone(),
        };

        // 5. Flush pending grants: stop on Offline (keep everything from
        // here on queued), continue past Api errors (keep just that row
        // queued), and on success make sure it's reflected in
        // `entitlements` too (synthesize an entry if the refetch above
        // predates it).
        let pending = std::mem::take(&mut cache.pending_grants);
        let mut remaining_pending = Vec::new();
        for (idx, grant) in pending.iter().enumerate() {
            match rest::insert_entitlement(&access_token, &user_id, &grant.model_id, &grant.source) {
                Ok(()) => {
                    if !entitlements.iter().any(|e| e.model_id == grant.model_id) {
                        entitlements.push(Entitlement {
                            model_id: grant.model_id.clone(),
                            source: grant.source.clone(),
                            created_at: grant.created_at.to_string(),
                            expires_at: None,
                        });
                    }
                }
                Err(CloudError::Offline) => {
                    remaining_pending.extend(pending[idx..].iter().cloned());
                    break;
                }
                Err(_) => remaining_pending.push(grant.clone()),
            }
        }
        cache.pending_grants = remaining_pending;

        // 6. Fire-and-forget device upsert — errors swallowed, never blocks
        // the caller.
        {
            let access_for_thread = access_token.clone();
            let user_id_for_thread = user_id.clone();
            std::thread::spawn(move || {
                let hw = hardware::detect();
                let fingerprint = rest::device_fingerprint();
                let _ = rest::upsert_device(&access_for_thread, &user_id_for_thread, &hw, &fingerprint);
            });
        }

        // 7. Update + write the cache.
        cache.user_id = user_id;
        cache.email = email.clone();
        cache.nickname = nickname_value;
        cache.entitlements = entitlements.clone();
        cache.last_online_auth = now();
        self.write_cache_best_effort(cache);

        // 8. Online SessionInfo. Read email back from the session record
        // (mirroring how `nickname` is handled) rather than the local
        // `email` copy, so it reflects the authoritative in-memory state.
        let session_email = self.session.lock().unwrap().as_ref().map(|s| s.email.clone());
        SessionInfo {
            signed_in: true,
            nickname: self.nickname.lock().unwrap().clone(),
            email: session_email,
            mode: "online".into(),
            entitlements,
            grace_expired: false,
        }
    }

    /// Returns fresh (access_token, user_id) copies, refreshing first if
    /// the current session is within 60s of expiring (or already expired).
    fn ensure_fresh(&self) -> Result<(String, String), CloudError> {
        let now_ts = now();
        {
            let guard = self.session.lock().unwrap();
            match guard.as_ref() {
                None => return Err(CloudError::SessionExpired),
                Some(s) if now_ts <= s.expires_at - 60 => {
                    return Ok((s.access_token.clone(), s.user_id.clone()));
                }
                _ => {}
            }
        } // guard dropped here, before taking the refresh gate.
        self.refresh_via_gate()
    }

    /// Single-flight refresh: takes `refresh_gate` and re-checks freshness
    /// under it (another caller may have already rotated the token while
    /// we were waiting), only calling `auth::refresh` if still stale. The
    /// gate stays held across the whole refresh + keyring-persist +
    /// session-update sequence by design — the one sanctioned exception to
    /// "never hold a lock across I/O", and only across this exact
    /// round-trip.
    ///
    /// Also used directly (bypassing `ensure_fresh`'s local-clock
    /// pre-check) as the "one forced refresh" `grant` performs when the
    /// server itself rejects an access token our local `expires_at`
    /// thought was still fresh.
    fn refresh_via_gate(&self) -> Result<(String, String), CloudError> {
        let _gate = self.refresh_gate.lock().unwrap();
        let now_ts = now();
        let refresh_token = {
            let guard = self.session.lock().unwrap();
            match guard.as_ref() {
                None => return Err(CloudError::SessionExpired),
                Some(s) if now_ts <= s.expires_at - 60 => {
                    return Ok((s.access_token.clone(), s.user_id.clone()));
                }
                Some(s) => s.refresh_token.clone(),
            }
        }; // guard dropped here, before the network call.

        let tok = auth::refresh(&refresh_token)?;
        if let Err(e) = store::save_refresh_token(&tok.refresh_token) {
            eprintln!("cloud: failed to persist rotated refresh token to keyring: {e}");
        }
        let access = tok.access_token.clone();
        let user_id = tok.user_id.clone();
        {
            let mut guard = self.session.lock().unwrap();
            *guard = Some(Session {
                user_id: tok.user_id,
                email: tok.email,
                access_token: tok.access_token,
                refresh_token: tok.refresh_token,
                expires_at: tok.expires_at,
            });
        }
        Ok((access, user_id))
    }

    /// `grant`'s "with session" path once `ensure_fresh` has produced a
    /// token: try the insert; on a 401 from the REST layer itself (server
    /// rejected a token our local clock thought was fresh — clock skew,
    /// server-side revocation), perform exactly one forced refresh through
    /// the shared gate and retry once before giving up and queuing.
    fn grant_with_access(
        &self,
        cache: &mut store::CloudCache,
        access: &str,
        user_id: &str,
        model_id: &str,
        source: &str,
    ) -> Result<(), CloudError> {
        match rest::insert_entitlement(access, user_id, model_id, source) {
            Ok(()) => {
                self.record_grant(cache, model_id, source);
                Ok(())
            }
            Err(CloudError::Offline) => {
                self.queue_grant(cache, model_id, source);
                Ok(())
            }
            Err(CloudError::SessionExpired) => match self.refresh_via_gate() {
                Err(CloudError::Offline) | Err(CloudError::SessionExpired) => {
                    self.queue_grant(cache, model_id, source);
                    Ok(())
                }
                Err(e) => Err(e),
                Ok((access2, user_id2)) => {
                    match rest::insert_entitlement(&access2, &user_id2, model_id, source) {
                        Ok(()) => {
                            self.record_grant(cache, model_id, source);
                            Ok(())
                        }
                        Err(CloudError::Offline) | Err(CloudError::SessionExpired) => {
                            self.queue_grant(cache, model_id, source);
                            Ok(())
                        }
                        Err(e) => Err(e), // Api errors etc.: return, do not queue.
                    }
                }
            },
            Err(e) => Err(e), // Api errors etc.: return, do not queue.
        }
    }

    /// Queues a grant for later flush and synthesizes a placeholder
    /// entitlement so the UI reflects it immediately, then writes the
    /// cache.
    fn queue_grant(&self, cache: &mut store::CloudCache, model_id: &str, source: &str) {
        let created_at = now();
        store::queue_pending(cache, model_id, source, created_at);
        if !cache.entitlements.iter().any(|e| e.model_id == model_id) {
            cache.entitlements.push(Entitlement {
                model_id: model_id.to_string(),
                source: source.to_string(),
                created_at: created_at.to_string(),
                expires_at: None,
            });
        }
        self.write_cache_best_effort(cache);
    }

    /// Records a successful grant in the cache (if not already present)
    /// and writes it.
    fn record_grant(&self, cache: &mut store::CloudCache, model_id: &str, source: &str) {
        if !cache.entitlements.iter().any(|e| e.model_id == model_id) {
            cache.entitlements.push(Entitlement {
                model_id: model_id.to_string(),
                source: source.to_string(),
                created_at: now().to_string(),
                expires_at: None,
            });
        }
        self.write_cache_best_effort(cache);
    }

    fn offline_cached_info(&self, cache: &store::CloudCache) -> SessionInfo {
        let signed_in = !cache.user_id.is_empty();
        SessionInfo {
            signed_in,
            nickname: non_empty(&cache.nickname),
            email: non_empty(&cache.email),
            mode: if signed_in { "offlineCached".into() } else { "signedOut".into() },
            entitlements: cache.entitlements.clone(),
            grace_expired: store::grace_expired(cache.last_online_auth, now()),
        }
    }

    fn clear_memory(&self) {
        *self.session.lock().unwrap() = None;
        *self.nickname.lock().unwrap() = None;
    }

    fn write_cache_best_effort(&self, cache: &store::CloudCache) {
        if let Err(e) = store::write_cache(&self.cache_path, cache) {
            eprintln!("cloud: failed to write cache: {e}");
        }
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn local_part(email: &str) -> String {
    email.split('@').next().unwrap_or(email).to_string()
}

fn non_empty(s: &str) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::error::CloudError;
    use crate::cloud::store::{self, CloudCache, PendingGrant};
    use crate::cloud::test_support::{lock, set_mock_env, start_mock_server, start_mock_server_n, unused_port};
    use std::path::PathBuf;

    /// keyring tests hit the real OS credential store; clean up our own
    /// entry via Drop so a failed assertion never leaves a stray secret
    /// behind (same pattern as A4's `store::tests::KeyringCleanup`).
    struct KeyringCleanup;
    impl Drop for KeyringCleanup {
        fn drop(&mut self) {
            store::delete_refresh_token();
        }
    }

    fn temp_cache_path(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        p.push(format!(
            "cleophis-a5-test-{}-{}-{}",
            std::process::id(),
            unique,
            name
        ));
        p
    }

    fn now() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    }

    // 1. restore with no keyring entry -> signed_out, no network needed.
    #[test]
    fn restore_no_keyring_entry_is_signed_out() {
        let _g = lock();
        let _cleanup = KeyringCleanup;
        store::delete_refresh_token();
        let cache_path = temp_cache_path("t1-cache.json");

        let cloud = Cloud::new(cache_path.clone());
        let info = cloud.restore();

        assert!(!info.signed_in);
        assert_eq!(info.mode, "signedOut");

        let _ = std::fs::remove_file(&cache_path);
    }

    // 2. restore offline (dead-port URL) with seeded keyring + cache
    // (user_id, nickname, 1 entitlement, last_online_auth = now - 5 days)
    // -> signedIn, mode offlineCached, entitlement present, grace_expired false.
    #[test]
    fn restore_offline_within_grace_is_offline_cached() {
        let _g = lock();
        let _cleanup = KeyringCleanup;
        store::delete_refresh_token();
        store::save_refresh_token("seed-refresh-token-2").expect("seed keyring");

        let cache_path = temp_cache_path("t2-cache.json");
        let mut cache = CloudCache::default();
        cache.user_id = "user-2".into();
        cache.email = "u2@example.com".into();
        cache.nickname = "Nick2".into();
        cache.entitlements.push(store::Entitlement {
            model_id: "llama-8b".into(),
            source: "purchase".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            expires_at: None,
        });
        cache.last_online_auth = now() - 5 * 86_400;
        store::write_cache(&cache_path, &cache).expect("seed cache");

        set_mock_env(unused_port());

        let cloud = Cloud::new(cache_path.clone());
        let info = cloud.restore();

        assert!(info.signed_in);
        assert_eq!(info.mode, "offlineCached");
        assert_eq!(info.entitlements.len(), 1);
        assert!(!info.grace_expired);

        let _ = std::fs::remove_file(&cache_path);
    }

    // 3. Same as #2 but last_online_auth = now - 40 days -> grace_expired true.
    #[test]
    fn restore_offline_past_grace_sets_grace_expired() {
        let _g = lock();
        let _cleanup = KeyringCleanup;
        store::delete_refresh_token();
        store::save_refresh_token("seed-refresh-token-3").expect("seed keyring");

        let cache_path = temp_cache_path("t3-cache.json");
        let mut cache = CloudCache::default();
        cache.user_id = "user-3".into();
        cache.last_online_auth = now() - 40 * 86_400;
        store::write_cache(&cache_path, &cache).expect("seed cache");

        set_mock_env(unused_port());

        let cloud = Cloud::new(cache_path.clone());
        let info = cloud.restore();

        assert!(info.signed_in);
        assert_eq!(info.mode, "offlineCached");
        assert!(info.grace_expired);

        let _ = std::fs::remove_file(&cache_path);
    }

    // 4. restore with mock refresh 400 -> keyring entry deleted, cache file
    // deleted, signed_out.
    #[test]
    fn restore_refresh_400_purges_everything() {
        let _g = lock();
        let _cleanup = KeyringCleanup;
        store::delete_refresh_token();
        store::save_refresh_token("seed-refresh-token-4").expect("seed keyring");

        let cache_path = temp_cache_path("t4-cache.json");
        let mut cache = CloudCache::default();
        cache.user_id = "user-4".into();
        store::write_cache(&cache_path, &cache).expect("seed cache");

        let port = start_mock_server(
            "400 Bad Request",
            r#"{"msg":"Invalid Refresh Token: Already Used"}"#,
        );
        set_mock_env(port);

        let cloud = Cloud::new(cache_path.clone());
        let info = cloud.restore();

        assert!(!info.signed_in);
        assert_eq!(info.mode, "signedOut");
        assert_eq!(store::load_refresh_token(), None);
        assert!(!cache_path.exists());
    }

    // 5. restore online happy path: refresh -> profile -> entitlements (3
    // canned responses) -> mode online, nickname from server, cache rewritten
    // with last_online_auth ~= now, keyring holds the ROTATED token.
    #[test]
    fn restore_online_happy_path_rotates_and_syncs() {
        let _g = lock();
        let _cleanup = KeyringCleanup;
        store::delete_refresh_token();
        store::save_refresh_token("old-refresh-token-5").expect("seed keyring");

        let cache_path = temp_cache_path("t5-cache.json");
        store::write_cache(&cache_path, &CloudCache::default()).expect("seed cache");

        let port = start_mock_server_n(vec![
            (
                "200 OK",
                r#"{"access_token":"at-new-5","token_type":"bearer","expires_in":3600,"refresh_token":"rt-rotated-5","user":{"id":"user-5","email":"u5@example.com"}}"#,
            ),
            ("200 OK", r#"[{"nickname":"ServerNick5"}]"#),
            (
                "200 OK",
                r#"[{"model_id":"llama-8b","source":"purchase","created_at":"2026-01-01T00:00:00Z","expires_at":null}]"#,
            ),
        ]);
        set_mock_env(port);

        let cloud = Cloud::new(cache_path.clone());
        let info = cloud.restore();

        assert!(info.signed_in);
        assert_eq!(info.mode, "online");
        assert_eq!(info.nickname.as_deref(), Some("ServerNick5"));
        assert_eq!(info.entitlements.len(), 1);
        assert_eq!(store::load_refresh_token(), Some("rt-rotated-5".to_string()));

        let rewritten = store::read_cache(&cache_path);
        assert_eq!(rewritten.user_id, "user-5");
        assert!(
            (rewritten.last_online_auth - now()).abs() <= 10,
            "last_online_auth was: {}",
            rewritten.last_online_auth
        );

        let _ = std::fs::remove_file(&cache_path);
    }

    // 6. Pending flush: cache seeded with 1 pending grant; refresh -> profile
    // -> entitlements -> 201 insert -> queue empty in rewritten cache,
    // entitlement present.
    #[test]
    fn restore_flushes_pending_grant_on_success() {
        let _g = lock();
        let _cleanup = KeyringCleanup;
        store::delete_refresh_token();
        store::save_refresh_token("old-refresh-token-6").expect("seed keyring");

        let cache_path = temp_cache_path("t6-cache.json");
        let mut cache = CloudCache::default();
        cache.pending_grants.push(PendingGrant {
            model_id: "phi-4".into(),
            source: "trial".into(),
            created_at: now(),
        });
        store::write_cache(&cache_path, &cache).expect("seed cache");

        let port = start_mock_server_n(vec![
            (
                "200 OK",
                r#"{"access_token":"at-new-6","token_type":"bearer","expires_in":3600,"refresh_token":"rt-rotated-6","user":{"id":"user-6","email":"u6@example.com"}}"#,
            ),
            ("200 OK", r#"[{"nickname":"Nick6"}]"#),
            ("200 OK", "[]"),
            ("201 Created", "[]"),
        ]);
        set_mock_env(port);

        let cloud = Cloud::new(cache_path.clone());
        let info = cloud.restore();

        assert!(info.signed_in);
        assert_eq!(info.mode, "online");

        let rewritten = store::read_cache(&cache_path);
        assert!(rewritten.pending_grants.is_empty());
        assert!(rewritten.entitlements.iter().any(|e| e.model_id == "phi-4"));

        let _ = std::fs::remove_file(&cache_path);
    }

    // 7. grant while offline-cached (no in-memory session, cache has a user)
    // -> Ok, queued, synthetic entitlement present in cache.
    #[test]
    fn grant_offline_cached_queues_and_synthesizes() {
        let _g = lock();
        let cache_path = temp_cache_path("t7-cache.json");
        let mut cache = CloudCache::default();
        cache.user_id = "user-7".into();
        store::write_cache(&cache_path, &cache).expect("seed cache");

        let cloud = Cloud::new(cache_path.clone()); // never restored/signed in -> no session
        let result = cloud.grant("llama-8b", "trial");
        assert!(result.is_ok(), "expected Ok(()), got {result:?}");

        let rewritten = store::read_cache(&cache_path);
        assert_eq!(rewritten.pending_grants.len(), 1);
        assert_eq!(rewritten.pending_grants[0].model_id, "llama-8b");
        assert!(rewritten.entitlements.iter().any(|e| e.model_id == "llama-8b"));

        let _ = std::fs::remove_file(&cache_path);
    }

    // 8. grant with an invalid source -> Err, nothing written.
    #[test]
    fn grant_invalid_source_errors_without_writing() {
        let _g = lock();
        let cache_path = temp_cache_path("t8-cache.json");
        let cloud = Cloud::new(cache_path.clone());

        let result = cloud.grant("llama-8b", "bogus-source");
        assert!(matches!(result, Err(CloudError::Internal(_))));
        assert!(!cache_path.exists());
    }

    // 9. sign_out purges: seed keyring + cache; sign_out -> keyring None,
    // cache file gone.
    #[test]
    fn sign_out_purges_keyring_and_cache() {
        let _g = lock();
        let _cleanup = KeyringCleanup;
        store::delete_refresh_token();
        store::save_refresh_token("seed-token-9").expect("seed keyring");

        let cache_path = temp_cache_path("t9-cache.json");
        let mut cache = CloudCache::default();
        cache.user_id = "user-9".into();
        store::write_cache(&cache_path, &cache).expect("seed cache");

        let cloud = Cloud::new(cache_path.clone()); // no in-memory session either
        cloud.sign_out();

        assert_eq!(store::load_refresh_token(), None);
        assert!(!cache_path.exists());
    }

    // 10a. ensure_fresh with a locally-fresh session returns copies WITHOUT
    // any network call (dead port proves it).
    #[test]
    fn ensure_fresh_fresh_session_skips_network() {
        let _g = lock();
        let cache_path = temp_cache_path("t10a-cache.json");
        let cloud = Cloud::new(cache_path.clone());
        {
            let mut guard = cloud.session.lock().unwrap();
            *guard = Some(Session {
                user_id: "user-10a".into(),
                email: "u10a@example.com".into(),
                access_token: "at-fresh".into(),
                refresh_token: "rt-fresh".into(),
                expires_at: now() + 3600,
            });
        }
        set_mock_env(unused_port());

        let (access, user_id) = cloud.ensure_fresh().expect("expected copies without network");
        assert_eq!(access, "at-fresh");
        assert_eq!(user_id, "user-10a");
    }

    // 10b. ensure_fresh with a stale session refreshes and persists the
    // rotated token.
    #[test]
    fn ensure_fresh_stale_session_refreshes_and_persists() {
        let _g = lock();
        let _cleanup = KeyringCleanup;
        store::delete_refresh_token();

        let cache_path = temp_cache_path("t10b-cache.json");
        let cloud = Cloud::new(cache_path.clone());
        {
            let mut guard = cloud.session.lock().unwrap();
            *guard = Some(Session {
                user_id: "user-10b".into(),
                email: "u10b@example.com".into(),
                access_token: "at-stale".into(),
                refresh_token: "rt-stale".into(),
                expires_at: now() - 10,
            });
        }

        let port = start_mock_server(
            "200 OK",
            r#"{"access_token":"at-rotated-10b","token_type":"bearer","expires_in":3600,"refresh_token":"rt-rotated-10b","user":{"id":"user-10b","email":"u10b@example.com"}}"#,
        );
        set_mock_env(port);

        let (access, user_id) = cloud.ensure_fresh().expect("expected refreshed copies");
        assert_eq!(access, "at-rotated-10b");
        assert_eq!(user_id, "user-10b");
        assert_eq!(
            store::load_refresh_token(),
            Some("rt-rotated-10b".to_string())
        );
    }
}
