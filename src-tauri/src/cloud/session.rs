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
/// No `Debug` (mirrors `auth::TokenResponse`'s rationale): this holds live
/// access/refresh tokens and must never end up in a `{:?}`/panic message.
#[derive(Clone)]
pub struct Session {
    pub user_id: String,
    pub email: String,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: i64,
    /// Set once at session establishment from `sign_in`/`sign_up`'s
    /// `remember` flag (`ephemeral = !remember`); `restore()` always
    /// establishes a non-ephemeral session, since reaching `restore()`'s
    /// online path at all means a refresh token was already persisted to
    /// the keyring by a prior `remember=true` session. While `true`, the
    /// keyring persist sites (initial + rotated-token) and the session's
    /// on-disk cache write are skipped — the same "degrade gracefully,
    /// never error" shape as the keyring-failure-degrades-to-memory-only
    /// handling in `apply_and_sync`'s step 1, just chosen deliberately
    /// instead of hit by a keyring error. The session still works
    /// normally in memory for the rest of the process; it simply leaves
    /// no trace for the next launch.
    pub ephemeral: bool,
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

/// Result of a `Cloud::create_checkout` call. `Url` carries a validated
/// (`https://checkout.stripe.com/`-prefixed), open-in-browser Stripe
/// Checkout link. No `Debug`/`PartialEq` derive (mirrors
/// `rest::CheckoutSessionResponse`'s rationale): the URL is a payment-page
/// capability and must never end up in a `{:?}`/panic message.
pub enum CheckoutOutcome {
    AlreadyOwned,
    Url(String),
}

/// Result of a `Cloud::create_portal_session` call. `Url` carries a
/// validated (`https://billing.stripe.com/`-prefixed), open-in-browser
/// Stripe Billing Portal link. No `Debug`/`PartialEq` derive (mirrors
/// `rest::PortalSessionResponse`'s rationale): the URL is a billing-portal
/// capability and must never end up in a `{:?}`/panic message.
pub enum PortalOutcome {
    NoBillingAccount,
    Url(String),
}

/// The stateful session layer. Composes `auth` (GoTrue), `rest` (PostgREST),
/// and `store` (keyring + on-disk cache) into the six operations the
/// front-end drives via `cloud::commands`.
///
/// Locking discipline (binding, see the task brief):
/// - `session` is NEVER held across network I/O — read/copy under lock,
///   drop the guard, do I/O, re-lock only to write the result back.
/// - `refresh_gate` is the ONE lock allowed to span I/O: across a single
///   refresh round-trip (single-flight token rotation: a second caller
///   that queues up behind the gate re-checks freshness once it gets in,
///   rather than firing a redundant refresh), AND, on that round-trip's
///   `SessionExpired` failure branch, across the local purge that follows
///   (keyring delete + memory clear + cache delete under `cache_lock`).
/// - `cache_lock` is a LEAF lock: it serializes the on-disk cache file's
///   short read-modify-write cycles (queueing/recording a grant, updating
///   entitlements, purging on sign-out/session-expiry) so two callers
///   racing on the cache file can't lose each other's update. It is NEVER
///   held while taking `session` or `refresh_gate`, and NEVER held across
///   network I/O — only ever across a local read -> mutate -> write of the
///   cache file itself. The sanctioned nesting order is `refresh_gate` ->
///   `cache_lock` (the mid-session-purge case above), never the reverse.
/// - Rotation discipline: after a successful `auth::refresh`, the new
///   refresh token is persisted to the keyring BEFORE the in-memory
///   session is updated or success is reported to the caller.
pub struct Cloud {
    session: Mutex<Option<Session>>,
    nickname: Mutex<Option<String>>,
    refresh_gate: Mutex<()>,
    cache_lock: Mutex<()>,
    cache_path: PathBuf,
}

impl Cloud {
    pub fn new(cache_path: PathBuf) -> Self {
        Cloud {
            session: Mutex::new(None),
            nickname: Mutex::new(None),
            refresh_gate: Mutex::new(()),
            cache_lock: Mutex::new(()),
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
        // Early, unlocked read — used only as a read-only fallback source
        // (offline-cached info, or apply_and_sync's nickname/entitlements
        // fallback while its network calls are in flight). Not part of any
        // read-modify-write cycle, so it doesn't need `cache_lock`.
        let cache = store::read_cache(&self.cache_path);
        match auth::refresh(&refresh_token) {
            // `restore()` only reaches this Ok branch when a refresh token
            // was already found in the keyring, which only ever got there
            // via a prior `remember=true` sign_in/sign_up — so restoring
            // it is always non-ephemeral, preserving that same trail.
            Ok(tok) => self.apply_and_sync(tok, &cache, false),
            // Any 4xx on refresh means the token was revoked or rotated
            // away server-side — the session cannot be re-established, so
            // purge everything and report signed out.
            Err(CloudError::SessionExpired) => {
                store::delete_refresh_token();
                self.clear_memory();
                {
                    let _guard = self.cache_lock.lock().unwrap();
                    let _ = std::fs::remove_file(&self.cache_path);
                }
                SessionInfo::signed_out()
            }
            // Offline, server trouble (Api{..}), or anything else we don't
            // specifically recognize: never brick boot. Fall back to
            // whatever is cached.
            Err(_) => self.offline_cached_info(&cache),
        }
    }

    /// `remember` is the front-end's "Keep me signed in" opt-in — default
    /// unchecked. `ephemeral = !remember` is threaded into `apply_and_sync`,
    /// which stamps it on the resulting `Session` and skips this session's
    /// persistence sites accordingly (see `Session::ephemeral`'s doc).
    pub fn sign_in(&self, email: &str, password: &str, remember: bool) -> Result<SessionInfo, CloudError> {
        let tok = auth::sign_in_password(email, password)?;
        let cache = store::read_cache(&self.cache_path); // early, unlocked fallback read
        Ok(self.apply_and_sync(tok, &cache, !remember))
    }

    /// `remember` — see `sign_in`'s doc; identical contract.
    pub fn sign_up(
        &self,
        email: &str,
        password: &str,
        nickname: &str,
        remember: bool,
    ) -> Result<SessionInfo, CloudError> {
        let tok = auth::sign_up(email, password, nickname)?;
        let cache = store::read_cache(&self.cache_path); // early, unlocked fallback read
        Ok(self.apply_and_sync(tok, &cache, !remember))
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
        {
            let _guard = self.cache_lock.lock().unwrap();
            let _ = std::fs::remove_file(&self.cache_path);
        }
    }

    pub fn grant(&self, model_id: &str, source: &str) -> Result<(), CloudError> {
        if source != "trial" && source != "library" {
            return Err(CloudError::Internal("invalid source".into()));
        }

        let has_session = self.session.lock().unwrap().is_some();

        if !has_session {
            // Never signed in on this device at all -> nothing to queue
            // against. Previously signed in (offline-cached) -> queue.
            // Read-only decision check, not part of an RMW cycle — no lock.
            let cache = store::read_cache(&self.cache_path);
            if cache.user_id.is_empty() {
                return Err(CloudError::SessionExpired);
            }
            return self.queue_grant(model_id, source);
        }

        match self.ensure_fresh() {
            Err(CloudError::Offline) | Err(CloudError::SessionExpired) => {
                self.queue_grant(model_id, source)
            }
            Err(e) => Err(e),
            Ok((access, user_id)) => self.grant_with_access(&access, &user_id, model_id, source),
        }
    }

    pub fn entitlements(&self) -> Result<Vec<Entitlement>, CloudError> {
        let (has_session, ephemeral) = {
            let guard = self.session.lock().unwrap();
            (guard.is_some(), guard.as_ref().map(|s| s.ephemeral).unwrap_or(false))
        };
        if !has_session {
            return Ok(store::read_cache(&self.cache_path).entitlements);
        }

        match self.ensure_fresh() {
            Err(CloudError::Offline) => Ok(store::read_cache(&self.cache_path).entitlements),
            Err(e) => Err(e),
            Ok((access, user_id)) => match rest::list_entitlements(&access) {
                Ok(list) => {
                    // Read-modify-write under `cache_lock`: re-read fresh
                    // (a concurrent grant() may have queued something
                    // since we last looked) rather than reusing any
                    // earlier read, then merge in any still-pending
                    // optimistic synthetic (owned by `user_id`) that
                    // `list` doesn't know about yet — otherwise a
                    // still-queued grant's placeholder would transiently
                    // vanish every time this polls. `cache_lock` stays a
                    // leaf here: the network call already happened above,
                    // nothing under this guard does I/O.
                    let _guard = self.cache_lock.lock().unwrap();
                    let mut fresh = store::read_cache(&self.cache_path);
                    let merged = merge_pending_synthetics(&fresh, &user_id, list);
                    fresh.entitlements = merged.clone();
                    // Ephemeral session: mirror apply_and_sync's step-7
                    // skip — the merge above still runs so the RETURNED
                    // list is correct, but nothing is written to disk.
                    if !ephemeral {
                        self.write_cache_best_effort(&fresh);
                    }
                    Ok(merged)
                }
                Err(CloudError::Offline) => Ok(store::read_cache(&self.cache_path).entitlements),
                Err(e) => Err(e),
            },
        }
    }

    /// Fresh download authorization for a model. Refreshes the session token
    /// if needed; one forced refresh+retry on `SessionExpired` (mirrors
    /// `grant_with_access`'s retry shape). No cache/lock interaction beyond
    /// what `ensure_fresh`/`refresh_via_gate` already do internally — unlike
    /// `grant`, there is nothing to queue offline; a download-URL mint is
    /// only ever meaningful right before an online download attempt.
    ///
    /// Called by `cloud::download::download_model`'s worker thread as
    /// `auth_provider`, at start and on every re-mint (a retryable
    /// failure).
    pub fn download_authorization(&self, model_id: &str) -> Result<rest::DownloadAuth, CloudError> {
        let (access, _user_id) = self.ensure_fresh()?;
        match rest::mint_download_url(&access, model_id) {
            Ok(auth) => Ok(auth),
            Err(CloudError::SessionExpired) => {
                let (access2, _user_id2) = self.refresh_via_gate()?;
                rest::mint_download_url(&access2, model_id)
            }
            Err(e) => Err(e),
        }
    }

    /// Create a Stripe Checkout session for a model. Refreshes the session
    /// token if needed; one forced refresh+retry on `SessionExpired`
    /// (mirrors `download_authorization`'s retry shape). A 409 from the
    /// function (already owns the model) maps to `AlreadyOwned` rather than
    /// an error. On success, validates the returned URL actually points at
    /// Stripe's checkout host before it's ever handed to the opener plugin
    /// — see `checkout_outcome`.
    pub fn create_checkout(&self, model_id: &str) -> Result<CheckoutOutcome, CloudError> {
        let (access, _user_id) = self.ensure_fresh()?;
        match rest::create_checkout(&access, model_id) {
            Err(CloudError::SessionExpired) => {
                let (access2, _user_id2) = self.refresh_via_gate()?;
                checkout_outcome(rest::create_checkout(&access2, model_id))
            }
            result => checkout_outcome(result),
        }
    }

    /// Create a Stripe Billing Portal session for the current user.
    /// Refreshes the session token if needed; one forced refresh+retry on
    /// `SessionExpired` (IDENTICAL shape to `create_checkout`'s retry). A
    /// 404 from the function (never subscribed, so no Stripe customer yet)
    /// maps to `NoBillingAccount` rather than an error. On success,
    /// validates the returned URL actually points at Stripe's billing
    /// portal host before it's ever handed to the opener plugin — see
    /// `portal_outcome`.
    pub fn create_portal_session(&self) -> Result<PortalOutcome, CloudError> {
        let (access, _user_id) = self.ensure_fresh()?;
        match rest::create_portal_session(&access) {
            Err(CloudError::SessionExpired) => {
                let (access2, _user_id2) = self.refresh_via_gate()?;
                portal_outcome(rest::create_portal_session(&access2))
            }
            result => portal_outcome(result),
        }
    }

    /// The shared post-auth pass: persists the rotated refresh token,
    /// writes the in-memory session, fetches nickname + entitlements
    /// (falling back to cache on error), flushes any queued pending
    /// grants, fires off a best-effort device upsert, and rewrites the
    /// cache. Called from `restore`, `sign_in`, and `sign_up` alike.
    ///
    /// `cache` is an EARLY, unlocked read the caller took before any of
    /// this — used only as a best-effort fallback source while the network
    /// calls below run lock-free. It is deliberately NOT what gets written
    /// back: a concurrent `grant()` could queue a new pending row (and its
    /// synthetic entitlement) into the real cache file while this method
    /// is off doing network I/O, and blindly overwriting with this stale
    /// snapshot would silently lose it. Instead, the final write (step 7)
    /// takes `cache_lock`, re-reads the cache FRESH, and merges.
    ///
    /// `ephemeral` (`!remember` from `sign_in`/`sign_up`, always `false`
    /// from `restore`) gates the two persistence sites below (step 1 and
    /// step 7) — see `Session::ephemeral`'s doc for the shape of that skip.
    fn apply_and_sync(&self, tok: auth::TokenResponse, cache: &store::CloudCache, ephemeral: bool) -> SessionInfo {
        // 1. Persist the refresh token FIRST (rotation discipline) — a
        // keyring failure degrades to a memory-only session rather than
        // aborting the sign-in. An ephemeral session chooses that same
        // memory-only shape deliberately: skip the write, clean branch,
        // nothing lands in the keyring for this session at all.
        if !ephemeral {
            if let Err(e) = store::save_refresh_token(&tok.refresh_token) {
                eprintln!("cloud: failed to persist refresh token to keyring: {e}");
            }
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
                ephemeral,
            });
        } // guard dropped here, before any I/O below.

        // 3. Nickname: server profile row, else the early cache's nickname
        // (best-effort fallback only), else the email's local-part.
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

        // 4. Entitlements: fresh list from the server, else the early
        // cache's stale-but-usable list.
        let server_entitlements = match rest::list_entitlements(&access_token) {
            Ok(list) => list,
            Err(_) => cache.entitlements.clone(),
        };

        // 5. Flush pending grants, attempted against the early snapshot —
        // still lock-free network I/O. A row whose owner doesn't match the
        // user we just authenticated as (leftover from a different,
        // previously offline-cached user on this device) is never
        // attempted — merge_synced_cache (step 7) drops those rather than
        // flushing them under the wrong identity. Otherwise: stop on
        // Offline (leave everything from here on queued), continue past
        // Api errors (leave just that row queued). `flushed` records which
        // model_ids landed so the merge can retire them from whatever the
        // FRESH queue looks like by the time we get there.
        let mut flushed: Vec<String> = Vec::new();
        for grant in cache.pending_grants.iter() {
            if grant.user_id != user_id {
                continue;
            }
            match rest::insert_entitlement(&access_token, &user_id, &grant.model_id, &grant.source) {
                Ok(()) => flushed.push(grant.model_id.clone()),
                Err(CloudError::Offline) => break,
                Err(_) => {} // Api error etc.: leave this one queued, keep going.
            }
        }

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

        // 7. Final write: take `cache_lock` (leaf lock — never held with
        // `session`/`refresh_gate`, never across I/O), re-read the cache
        // FRESH, merge the sync's results into it, write — unless this
        // session is ephemeral, in which case the merge still runs (so
        // `merged.entitlements` below is correct either way) but the
        // actual disk write is skipped: a clean branch, not an error path,
        // leaving no on-disk cache trace for this session.
        let merged = {
            let _guard = self.cache_lock.lock().unwrap();
            let fresh = store::read_cache(&self.cache_path);
            let merged = merge_synced_cache(
                fresh,
                user_id,
                email,
                nickname_value,
                server_entitlements,
                &flushed,
                now(),
            );
            if !ephemeral {
                self.write_cache_best_effort(&merged);
            }
            merged
        };

        // 8. Online SessionInfo. Read email back from the session record
        // (mirroring how `nickname` is handled) rather than the merged
        // cache's copy, so it reflects the authoritative in-memory state.
        let session_email = self.session.lock().unwrap().as_ref().map(|s| s.email.clone());
        SessionInfo {
            signed_in: true,
            nickname: self.nickname.lock().unwrap().clone(),
            email: session_email,
            mode: "online".into(),
            entitlements: merged.entitlements,
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
        let (refresh_token, ephemeral) = {
            let guard = self.session.lock().unwrap();
            match guard.as_ref() {
                None => return Err(CloudError::SessionExpired),
                Some(s) if now_ts <= s.expires_at - 60 => {
                    return Ok((s.access_token.clone(), s.user_id.clone()));
                }
                Some(s) => (s.refresh_token.clone(), s.ephemeral),
            }
        }; // guard dropped here, before the network call.

        let tok = match auth::refresh(&refresh_token) {
            Ok(tok) => tok,
            // A revoked/rotated-away refresh token mid-session is the same
            // situation restore() handles at boot: purge everything
            // (keyring, in-memory session/nickname, cache file) before
            // propagating the error, so a subsequent restore()/sign_in()
            // doesn't trip over stale state. `cache_lock` nests safely
            // inside the still-held `refresh_gate` here (leaf lock — the
            // rule is it must never be held while TAKING session/
            // refresh_gate, not the reverse).
            Err(CloudError::SessionExpired) => {
                store::delete_refresh_token();
                self.clear_memory();
                {
                    let _guard = self.cache_lock.lock().unwrap();
                    let _ = std::fs::remove_file(&self.cache_path);
                }
                return Err(CloudError::SessionExpired);
            }
            Err(e) => return Err(e),
        };
        // Ephemeral session: skip the rotated-token persist, same clean
        // branch as apply_and_sync's step 1 — the conceptual cousin is
        // that block's "keyring failure degrades to memory-only" comment,
        // just chosen deliberately here rather than hit by an error.
        if !ephemeral {
            if let Err(e) = store::save_refresh_token(&tok.refresh_token) {
                eprintln!("cloud: failed to persist rotated refresh token to keyring: {e}");
            }
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
                ephemeral,
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
        access: &str,
        user_id: &str,
        model_id: &str,
        source: &str,
    ) -> Result<(), CloudError> {
        match rest::insert_entitlement(access, user_id, model_id, source) {
            Ok(()) => self.record_grant(model_id, source),
            Err(CloudError::Offline) => self.queue_grant(model_id, source),
            Err(CloudError::SessionExpired) => match self.refresh_via_gate() {
                Err(CloudError::Offline) | Err(CloudError::SessionExpired) => {
                    self.queue_grant(model_id, source)
                }
                Err(e) => Err(e),
                Ok((access2, user_id2)) => {
                    match rest::insert_entitlement(&access2, &user_id2, model_id, source) {
                        Ok(()) => self.record_grant(model_id, source),
                        Err(CloudError::Offline) | Err(CloudError::SessionExpired) => {
                            self.queue_grant(model_id, source)
                        }
                        Err(e) => Err(e), // Api errors etc.: return, do not queue.
                    }
                }
            },
            Err(e) => Err(e), // Api errors etc.: return, do not queue.
        }
    }

    /// Queues a grant for later flush and synthesizes a placeholder
    /// entitlement so the UI reflects it immediately. Read-modify-write is
    /// serialized under `cache_lock` (leaf lock: never held with
    /// `session`/`refresh_gate`, never across I/O) — reads its own fresh
    /// copy of the cache rather than trusting any earlier snapshot, so it
    /// can't clobber a concurrent update.
    ///
    /// Stamps the row with `cache.user_id` (this fresh read, not any
    /// earlier snapshot) and refuses to queue at all if that's empty — this
    /// closes a race with a concurrent `sign_out`/purge: without this
    /// check, a `queue_grant` that loses the race with the purge's cache
    /// delete would recreate the cache file from scratch with an anonymous
    /// (unowned) pending row instead of just failing.
    fn queue_grant(&self, model_id: &str, source: &str) -> Result<(), CloudError> {
        let _guard = self.cache_lock.lock().unwrap();
        let mut cache = store::read_cache(&self.cache_path);
        if cache.user_id.is_empty() {
            return Err(CloudError::SessionExpired);
        }
        let owner = cache.user_id.clone();
        let created_at = now();
        store::queue_pending(&mut cache, model_id, source, &owner, created_at);
        if !cache.entitlements.iter().any(|e| e.model_id == model_id) {
            cache.entitlements.push(Entitlement {
                model_id: model_id.to_string(),
                source: source.to_string(),
                created_at: created_at.to_string(),
                expires_at: None,
            });
        }
        self.write_cache_best_effort(&cache);
        Ok(())
    }

    /// Records a successful grant in the cache (if not already present).
    /// Same self-contained, `cache_lock`-serialized read-modify-write
    /// discipline as `queue_grant`, INCLUDING the same empty-owner
    /// refusal — the success-path twin of the same resurrection race:
    /// without it, a `record_grant` that loses a race with a concurrent
    /// `sign_out`/purge would recreate the cache file from scratch with an
    /// anonymous (unowned) entitlement entry instead of just failing. The
    /// server-side insert this follows already succeeded either way; the
    /// caller just needs to re-authenticate to see it again locally.
    fn record_grant(&self, model_id: &str, source: &str) -> Result<(), CloudError> {
        let _guard = self.cache_lock.lock().unwrap();
        let mut cache = store::read_cache(&self.cache_path);
        if cache.user_id.is_empty() {
            return Err(CloudError::SessionExpired);
        }
        if !cache.entitlements.iter().any(|e| e.model_id == model_id) {
            cache.entitlements.push(Entitlement {
                model_id: model_id.to_string(),
                source: source.to_string(),
                created_at: now().to_string(),
                expires_at: None,
            });
        }
        self.write_cache_best_effort(&cache);
        Ok(())
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

/// Merge the results of a completed online sync into the freshest cache
/// state. Pure function — no I/O, no locking, easy to unit-test directly.
///
/// `fresh` is the cache re-read from disk under `cache_lock` immediately
/// before this merge runs — NOT the (possibly stale) snapshot
/// `apply_and_sync` used earlier to compute `flushed`. Because the sync's
/// network calls ran lock-free, a concurrent `grant()` may have queued a
/// new pending row (with its own optimistic synthetic entitlement) into
/// the real cache file in the meantime; `fresh` reflects that, `flushed`
/// does not. Preserving that row instead of clobbering it with the stale
/// snapshot is the whole point of re-reading and merging here.
///
/// - Rows in `fresh.pending_grants` owned by a DIFFERENT user_id than the
///   one we just authenticated as (leftover from a previous, differently
///   offline-cached user on this device) are dropped entirely — never
///   flushed, never carried forward, and their entitlement (if any) is not
///   resurrected either. Only rows owned by `user_id` are considered below.
/// - `entitlements` = `server_entitlements`, plus a synthesized entry for
///   any `flushed` model_id absent from it (the entitlements fetch predates
///   the insert within the same sync), plus any synthetic entitlement
///   already in `fresh.entitlements` whose model_id is still queued (and
///   owned by `user_id`) in `fresh.pending_grants` (queued concurrently
///   mid-sync — never lose its optimistic visibility).
/// - `pending_grants` = `fresh.pending_grants` (owned rows only) minus
///   `flushed` minus any model_id already present in `server_entitlements`.
/// - identity fields (`user_id`/`email`/`nickname`) and `last_online_auth`
///   come from the sync, not from `fresh`.
fn merge_synced_cache(
    fresh: store::CloudCache,
    user_id: String,
    email: String,
    nickname: String,
    server_entitlements: Vec<Entitlement>,
    flushed: &[String],
    now: i64,
) -> store::CloudCache {
    let owned_pending: Vec<store::PendingGrant> = fresh
        .pending_grants
        .into_iter()
        .filter(|p| p.user_id == user_id)
        .collect();

    let pending_grants: Vec<store::PendingGrant> = owned_pending
        .iter()
        .filter(|p| !flushed.iter().any(|m| m == &p.model_id))
        .filter(|p| !server_entitlements.iter().any(|e| e.model_id == p.model_id))
        .cloned()
        .collect();

    let mut entitlements = server_entitlements;

    for model_id in flushed {
        if entitlements.iter().any(|e| &e.model_id == model_id) {
            continue;
        }
        if let Some(existing) = fresh.entitlements.iter().find(|e| &e.model_id == model_id) {
            entitlements.push(existing.clone());
        } else if let Some(p) = owned_pending.iter().find(|p| &p.model_id == model_id) {
            entitlements.push(Entitlement {
                model_id: p.model_id.clone(),
                source: p.source.clone(),
                created_at: p.created_at.to_string(),
                expires_at: None,
            });
        }
    }

    for entry in &fresh.entitlements {
        let still_pending = owned_pending.iter().any(|p| p.model_id == entry.model_id);
        let already_present = entitlements.iter().any(|e| e.model_id == entry.model_id);
        if still_pending && !already_present {
            entitlements.push(entry.clone());
        }
    }

    store::CloudCache {
        user_id,
        email,
        nickname,
        entitlements,
        pending_grants,
        last_online_auth: now,
    }
}

/// Merge still-pending optimistic synthetics into a fresh server list.
/// Keeps each entitlement from `fresh.entitlements` whose model_id is in
/// `fresh.pending_grants` owned by `user_id` and absent from `server`.
///
/// Pure function — no I/O, no locking, easy to unit-test directly. Owner
/// filter mirrors `merge_synced_cache`'s: only `fresh.pending_grants` rows
/// whose `user_id` matches the caller's are eligible to keep their
/// synthetic alive; a leftover row from a previously offline-cached
/// different user never resurrects its entitlement.
fn merge_pending_synthetics(
    fresh: &store::CloudCache,
    user_id: &str,
    server: Vec<Entitlement>,
) -> Vec<Entitlement> {
    let mut merged = server;
    for entry in &fresh.entitlements {
        let owned_and_pending = fresh
            .pending_grants
            .iter()
            .any(|p| p.model_id == entry.model_id && p.user_id == user_id);
        let already_present = merged.iter().any(|e| e.model_id == entry.model_id);
        if owned_and_pending && !already_present {
            merged.push(entry.clone());
        }
    }
    merged
}

/// Maps a `rest::create_checkout` result to a `CheckoutOutcome`: a 409 (the
/// function's "already owns this model" response) becomes `AlreadyOwned`
/// rather than an error; any other error passes through unchanged. On
/// success, the URL is validated here — the one place this is
/// unit-testable without a Tauri `AppHandle`/opener plugin in the loop —
/// before `Cloud::create_checkout`'s caller (`start_checkout`) ever trusts
/// it enough to hand to `tauri_plugin_opener`.
fn checkout_outcome(result: Result<rest::CheckoutSessionResponse, CloudError>) -> Result<CheckoutOutcome, CloudError> {
    match result {
        Ok(resp) => {
            if resp.url.starts_with("https://checkout.stripe.com/") {
                Ok(CheckoutOutcome::Url(resp.url))
            } else {
                Err(CloudError::Internal("unexpected checkout url".into()))
            }
        }
        Err(CloudError::Api { status: 409, .. }) => Ok(CheckoutOutcome::AlreadyOwned),
        Err(e) => Err(e),
    }
}

/// Maps a `rest::create_portal_session` result to a `PortalOutcome`: a 404
/// (the function's "no_billing_account" response — the normal answer for a
/// user who has never subscribed) becomes `NoBillingAccount` rather than an
/// error; any other error passes through unchanged. On success, the URL is
/// validated here — the one place this is unit-testable without a Tauri
/// `AppHandle`/opener plugin in the loop — before `Cloud::create_portal_session`'s
/// caller (`open_billing_portal`) ever trusts it enough to hand to
/// `tauri_plugin_opener`.
fn portal_outcome(result: Result<rest::PortalSessionResponse, CloudError>) -> Result<PortalOutcome, CloudError> {
    match result {
        Ok(resp) => {
            if resp.url.starts_with("https://billing.stripe.com/") {
                Ok(PortalOutcome::Url(resp.url))
            } else {
                Err(CloudError::Internal("unexpected portal url".into()))
            }
        }
        Err(CloudError::Api { status: 404, .. }) => Ok(PortalOutcome::NoBillingAccount),
        Err(e) => Err(e),
    }
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

    // merge_synced_cache (a): a grant queued CONCURRENTLY during the sync
    // (present in `fresh` but never seen by this sync's flush, since
    // `flushed` only reflects the early pre-sync snapshot) must survive the
    // merge — this is the reviewer's exact lost-update scenario.
    #[test]
    fn merge_synced_cache_preserves_concurrently_queued_grant() {
        let mut fresh = CloudCache::default();
        fresh.pending_grants.push(PendingGrant {
            model_id: "phi-4".into(),
            source: "trial".into(),
            user_id: "user-1".into(),
            created_at: 1000,
        });
        fresh.entitlements.push(store::Entitlement {
            model_id: "phi-4".into(),
            source: "trial".into(),
            created_at: "1000".into(),
            expires_at: None,
        });

        let flushed: Vec<String> = vec![]; // this sync's flush never attempted phi-4

        let merged = merge_synced_cache(
            fresh,
            "user-1".into(),
            "u1@example.com".into(),
            "Nick".into(),
            vec![], // server list doesn't know about phi-4 yet
            &flushed,
            5000,
        );

        assert_eq!(merged.user_id, "user-1");
        assert_eq!(merged.last_online_auth, 5000);
        assert!(
            merged.pending_grants.iter().any(|p| p.model_id == "phi-4"),
            "expected phi-4 to still be queued, got {:?}",
            merged.pending_grants
        );
        assert!(
            merged.entitlements.iter().any(|e| e.model_id == "phi-4"),
            "expected phi-4's optimistic entitlement to survive, got {:?}",
            merged.entitlements
        );
    }

    // merge_synced_cache (b): a row this sync DID flush leaves the queue
    // and appears in entitlements (synthesized, since the server list fetch
    // predates the insert within the same sync).
    #[test]
    fn merge_synced_cache_removes_flushed_and_synthesizes_entitlement() {
        let mut fresh = CloudCache::default();
        fresh.pending_grants.push(PendingGrant {
            model_id: "llama-8b".into(),
            source: "trial".into(),
            user_id: "user-2".into(),
            created_at: 1000,
        });

        let flushed = vec!["llama-8b".to_string()];

        let merged = merge_synced_cache(
            fresh,
            "user-2".into(),
            "u2@example.com".into(),
            "Nick2".into(),
            vec![], // server list fetched before the insert landed
            &flushed,
            5000,
        );

        assert!(
            !merged.pending_grants.iter().any(|p| p.model_id == "llama-8b"),
            "expected llama-8b to leave the queue, got {:?}",
            merged.pending_grants
        );
        assert!(
            merged.entitlements.iter().any(|e| e.model_id == "llama-8b"),
            "expected llama-8b's entitlement to be synthesized, got {:?}",
            merged.entitlements
        );
    }

    // merge_synced_cache (c): a row already present in the server list
    // drops from the queue without duplicating the entitlement.
    #[test]
    fn merge_synced_cache_drops_pending_already_on_server_without_duplicating() {
        let mut fresh = CloudCache::default();
        fresh.pending_grants.push(PendingGrant {
            model_id: "mistral-7b".into(),
            source: "trial".into(),
            user_id: "user-3".into(),
            created_at: 1000,
        });
        fresh.entitlements.push(store::Entitlement {
            model_id: "mistral-7b".into(),
            source: "trial".into(),
            created_at: "1000".into(),
            expires_at: None,
        });

        let server_entitlements = vec![store::Entitlement {
            model_id: "mistral-7b".into(),
            source: "trial".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            expires_at: None,
        }];
        let flushed: Vec<String> = vec![]; // this sync's flush never attempted it

        let merged = merge_synced_cache(
            fresh,
            "user-3".into(),
            "u3@example.com".into(),
            "Nick3".into(),
            server_entitlements,
            &flushed,
            5000,
        );

        assert!(
            !merged.pending_grants.iter().any(|p| p.model_id == "mistral-7b"),
            "expected mistral-7b to leave the queue, got {:?}",
            merged.pending_grants
        );
        let matches: Vec<_> = merged
            .entitlements
            .iter()
            .filter(|e| e.model_id == "mistral-7b")
            .collect();
        assert_eq!(
            matches.len(),
            1,
            "expected exactly one mistral-7b entitlement, got {matches:?}"
        );
    }

    // merge_synced_cache (d): a pending row owned by a DIFFERENT user_id
    // (leftover from a previous offline-cached user on this device) is
    // dropped entirely — never flushed under the new identity, never
    // carried forward, and its entitlement is not resurrected either.
    #[test]
    fn merge_synced_cache_drops_pending_row_owned_by_a_different_user() {
        let mut fresh = CloudCache::default();
        fresh.pending_grants.push(PendingGrant {
            model_id: "phi-4".into(),
            source: "trial".into(),
            user_id: "stale-user".into(), // NOT the user we just authenticated as
            created_at: 1000,
        });
        fresh.entitlements.push(store::Entitlement {
            model_id: "phi-4".into(),
            source: "trial".into(),
            created_at: "1000".into(),
            expires_at: None,
        });

        let flushed: Vec<String> = vec![]; // never attempted — different owner

        let merged = merge_synced_cache(
            fresh,
            "user-new".into(),
            "new@example.com".into(),
            "New".into(),
            vec![], // server (for user-new) doesn't know phi-4 either
            &flushed,
            5000,
        );

        assert!(
            !merged.pending_grants.iter().any(|p| p.model_id == "phi-4"),
            "expected the mismatched-owner row to be dropped, got {:?}",
            merged.pending_grants
        );
        assert!(
            !merged.entitlements.iter().any(|e| e.model_id == "phi-4"),
            "expected the mismatched-owner's optimistic entitlement to be dropped too, got {:?}",
            merged.entitlements
        );
    }

    // merge_pending_synthetics (S2-1): a still-pending, owned grant's
    // optimistic synthetic entitlement survives a merge against an empty
    // server list.
    #[test]
    fn merge_pending_synthetics_synthetic_survives() {
        let mut fresh = CloudCache::default();
        fresh.pending_grants.push(PendingGrant {
            model_id: "phi-4".into(),
            source: "trial".into(),
            user_id: "user-1".into(),
            created_at: 1000,
        });
        fresh.entitlements.push(store::Entitlement {
            model_id: "phi-4".into(),
            source: "trial".into(),
            created_at: "1000".into(),
            expires_at: None,
        });

        let merged = merge_pending_synthetics(&fresh, "user-1", vec![]);

        assert!(
            merged.iter().any(|e| e.model_id == "phi-4"),
            "expected phi-4's synthetic to survive, got {merged:?}"
        );
    }

    // merge_pending_synthetics (S2-2): the server already knows about the
    // model_id -> no duplicate, exactly one copy (the server's).
    #[test]
    fn merge_pending_synthetics_no_duplicate_when_server_has_it() {
        let mut fresh = CloudCache::default();
        fresh.pending_grants.push(PendingGrant {
            model_id: "llama-8b".into(),
            source: "trial".into(),
            user_id: "user-2".into(),
            created_at: 1000,
        });
        fresh.entitlements.push(store::Entitlement {
            model_id: "llama-8b".into(),
            source: "trial".into(),
            created_at: "1000".into(),
            expires_at: None,
        });

        let server = vec![store::Entitlement {
            model_id: "llama-8b".into(),
            source: "purchase".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            expires_at: None,
        }];

        let merged = merge_pending_synthetics(&fresh, "user-2", server);

        let matches: Vec<_> = merged.iter().filter(|e| e.model_id == "llama-8b").collect();
        assert_eq!(matches.len(), 1, "expected exactly one copy, got {matches:?}");
        assert_eq!(matches[0].source, "purchase", "expected the server's copy to win");
    }

    // merge_pending_synthetics (S2-3): a pending row owned by a DIFFERENT
    // user_id must not resurrect its synthetic entitlement.
    #[test]
    fn merge_pending_synthetics_foreign_owner_pending_dropped() {
        let mut fresh = CloudCache::default();
        fresh.pending_grants.push(PendingGrant {
            model_id: "phi-4".into(),
            source: "trial".into(),
            user_id: "stale-user".into(), // NOT the caller's user_id
            created_at: 1000,
        });
        fresh.entitlements.push(store::Entitlement {
            model_id: "phi-4".into(),
            source: "trial".into(),
            created_at: "1000".into(),
            expires_at: None,
        });

        let merged = merge_pending_synthetics(&fresh, "user-new", vec![]);

        assert!(
            !merged.iter().any(|e| e.model_id == "phi-4"),
            "expected the foreign-owner's synthetic to be dropped, got {merged:?}"
        );
    }

    // S2-4. entitlements() session-level: cache seeded with a pending grant
    // (owned by U) + its synthetic entitlement, a fresh in-memory session
    // for U, server returns an empty entitlements list -> entitlements()
    // returns the synthetic AND the rewritten cache file still contains
    // both the synthetic entitlement and the pending grant (poll must not
    // transiently drop the optimistic state).
    #[test]
    fn entitlements_preserves_pending_synthetic_against_empty_server_list() {
        let _g = lock();
        let cache_path = temp_cache_path("t-s2-entitlements-synthetic-cache.json");
        let mut cache = CloudCache::default();
        cache.user_id = "user-s2".into();
        cache.pending_grants.push(PendingGrant {
            model_id: "phi-4".into(),
            source: "trial".into(),
            user_id: "user-s2".into(),
            created_at: now(),
        });
        cache.entitlements.push(store::Entitlement {
            model_id: "phi-4".into(),
            source: "trial".into(),
            created_at: now().to_string(),
            expires_at: None,
        });
        store::write_cache(&cache_path, &cache).expect("seed cache");

        let cloud = Cloud::new(cache_path.clone());
        {
            let mut guard = cloud.session.lock().unwrap();
            *guard = Some(Session {
                user_id: "user-s2".into(),
                email: "s2@example.com".into(),
                access_token: "at-s2".into(),
                refresh_token: "rt-s2".into(),
                expires_at: now() + 3600, // fresh -> ensure_fresh skips network
                ephemeral: false,
            });
        }

        let port = start_mock_server("200 OK", "[]");
        set_mock_env(port);

        let result = cloud.entitlements().expect("expected Ok");
        assert!(
            result.iter().any(|e| e.model_id == "phi-4"),
            "expected the synthetic to survive in the returned list, got {result:?}"
        );

        let rewritten = store::read_cache(&cache_path);
        assert!(
            rewritten.entitlements.iter().any(|e| e.model_id == "phi-4"),
            "expected the synthetic to survive in the rewritten cache, got {:?}",
            rewritten.entitlements
        );
        assert!(
            rewritten.pending_grants.iter().any(|p| p.model_id == "phi-4"),
            "expected the pending grant to still be queued, got {:?}",
            rewritten.pending_grants
        );

        let _ = std::fs::remove_file(&cache_path);
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
            user_id: "user-6".into(), // matches the mock refresh's authenticated user below
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
        assert_eq!(rewritten.pending_grants[0].user_id, "user-7"); // owner-stamped
        assert!(rewritten.entitlements.iter().any(|e| e.model_id == "llama-8b"));

        let _ = std::fs::remove_file(&cache_path);
    }

    // item 1: queue_grant refuses to queue against an unowned (empty
    // user_id) cache instead of silently creating an anonymous pending row
    // — closes the race where a concurrent sign_out purge wipes the cache
    // out from under a grant() call that already decided to queue.
    #[test]
    fn queue_grant_refuses_when_cache_has_no_owner() {
        let _g = lock();
        let cache_path = temp_cache_path("t-queue-grant-no-owner-cache.json");
        // No cache file at all -> store::read_cache returns
        // CloudCache::default() -> user_id == "".
        let cloud = Cloud::new(cache_path.clone());

        let result = cloud.queue_grant("llama-8b", "trial");
        assert!(matches!(result, Err(CloudError::SessionExpired)));
        assert!(!cache_path.exists(), "expected nothing to be written");
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
                ephemeral: false,
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
                ephemeral: false,
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

    // item 2: a mid-session SessionExpired (refresh token revoked/rotated
    // away, not a boot-time restore) must purge exactly like restore()'s
    // boot-time purge does — keyring, in-memory session, and cache file —
    // before propagating the error.
    #[test]
    fn ensure_fresh_stale_session_refresh_400_purges_everything() {
        let _g = lock();
        let _cleanup = KeyringCleanup;
        store::delete_refresh_token();
        store::save_refresh_token("seed-token-mid-session").expect("seed keyring");

        let cache_path = temp_cache_path("t-mid-session-expired-cache.json");
        let mut cache = CloudCache::default();
        cache.user_id = "user-mid".into();
        store::write_cache(&cache_path, &cache).expect("seed cache");

        let cloud = Cloud::new(cache_path.clone());
        {
            let mut guard = cloud.session.lock().unwrap();
            *guard = Some(Session {
                user_id: "user-mid".into(),
                email: "mid@example.com".into(),
                access_token: "at-stale-mid".into(),
                refresh_token: "rt-stale-mid".into(),
                expires_at: now() - 10,
                ephemeral: false,
            });
        }

        let port = start_mock_server(
            "400 Bad Request",
            r#"{"msg":"Invalid Refresh Token: Already Used"}"#,
        );
        set_mock_env(port);

        let result = cloud.ensure_fresh();
        assert!(matches!(result, Err(CloudError::SessionExpired)));
        assert_eq!(store::load_refresh_token(), None);
        assert!(!cache_path.exists());
        assert!(cloud.session.lock().unwrap().is_none());
    }

    // item 5: models the live PostgREST RLS with-check rejection —
    // insert_entitlement returns Api{403,..} and grant() surfaces it
    // directly, without queuing (a rejected write will be rejected again).
    #[test]
    fn grant_rls_rejection_surfaces_api_403_without_queuing() {
        let _g = lock();
        let cache_path = temp_cache_path("t-grant-rls-403-cache.json");
        let cloud = Cloud::new(cache_path.clone());
        {
            let mut guard = cloud.session.lock().unwrap();
            *guard = Some(Session {
                user_id: "user-rls".into(),
                email: "rls@example.com".into(),
                access_token: "at-rls".into(),
                refresh_token: "rt-rls".into(),
                expires_at: now() + 3600, // fresh -> ensure_fresh skips network
                ephemeral: false,
            });
        }

        let port = start_mock_server(
            "403 Forbidden",
            r#"{"code":"42501","message":"new row violates row-level security policy"}"#,
        );
        set_mock_env(port);

        let result = cloud.grant("socratic-tutor", "trial");
        match result {
            Err(CloudError::Api { status, .. }) => assert_eq!(status, 403),
            other => panic!("expected Err(CloudError::Api{{403,..}}), got {other:?}"),
        }

        assert!(
            !cache_path.exists(),
            "expected the RLS rejection not to be queued"
        );
    }

    // C3a-1. download_authorization happy path: locally-fresh session skips
    // the refresh round-trip (dead port on any second connection would
    // panic the mock thread's accept, not the test — the assertions below
    // are what actually prove it) and returns the minted fields.
    #[test]
    fn download_authorization_happy_path() {
        let _g = lock();
        let cache_path = temp_cache_path("t-download-auth-happy-cache.json");
        let cloud = Cloud::new(cache_path.clone());
        {
            let mut guard = cloud.session.lock().unwrap();
            *guard = Some(Session {
                user_id: "user-dl".into(),
                email: "dl@example.com".into(),
                access_token: "at-fresh-dl".into(),
                refresh_token: "rt-fresh-dl".into(),
                expires_at: now() + 3600,
                ephemeral: false,
            });
        }

        let port = start_mock_server(
            "200 OK",
            r#"{"url":"http://x/file/b/m.gguf","authorization":"tok123","expiresAt":"2026-07-18T00:00:00Z","fileBytes":2019377696}"#,
        );
        set_mock_env(port);

        let result = cloud
            .download_authorization("socratic-tutor")
            .expect("expected success");
        assert_eq!(result.url, "http://x/file/b/m.gguf");
        assert_eq!(result.authorization, "tok123");
        assert_eq!(result.file_bytes, 2019377696);
    }

    // S7-2. create_checkout happy path: locally-fresh session, 200
    // {"url":"https://checkout.stripe.com/..."} -> Url(...) exact.
    #[test]
    fn create_checkout_happy_path_returns_url() {
        let _g = lock();
        let cache_path = temp_cache_path("t-checkout-happy-cache.json");
        let cloud = Cloud::new(cache_path.clone());
        {
            let mut guard = cloud.session.lock().unwrap();
            *guard = Some(Session {
                user_id: "user-checkout".into(),
                email: "checkout@example.com".into(),
                access_token: "at-fresh-checkout".into(),
                refresh_token: "rt-fresh-checkout".into(),
                expires_at: now() + 3600,
                ephemeral: false,
            });
        }

        let port = start_mock_server(
            "200 OK",
            r#"{"url":"https://checkout.stripe.com/c/pay/cs_test_x"}"#,
        );
        set_mock_env(port);

        let result = cloud
            .create_checkout("socratic-tutor")
            .expect("expected success");
        match result {
            CheckoutOutcome::Url(u) => {
                assert_eq!(u, "https://checkout.stripe.com/c/pay/cs_test_x")
            }
            CheckoutOutcome::AlreadyOwned => panic!("expected Url, got AlreadyOwned"),
        }

        let _ = std::fs::remove_file(&cache_path);
    }

    // S7-3. create_checkout 409 {"error":"already_purchased",...} ->
    // AlreadyOwned.
    #[test]
    fn create_checkout_409_is_already_owned() {
        let _g = lock();
        let cache_path = temp_cache_path("t-checkout-409-cache.json");
        let cloud = Cloud::new(cache_path.clone());
        {
            let mut guard = cloud.session.lock().unwrap();
            *guard = Some(Session {
                user_id: "user-409".into(),
                email: "u409@example.com".into(),
                access_token: "at-409".into(),
                refresh_token: "rt-409".into(),
                expires_at: now() + 3600,
                ephemeral: false,
            });
        }

        let port = start_mock_server(
            "409 Conflict",
            r#"{"error":"already_purchased","message":"You already own this model."}"#,
        );
        set_mock_env(port);

        let result = cloud
            .create_checkout("socratic-tutor")
            .expect("expected Ok(AlreadyOwned)");
        assert!(matches!(result, CheckoutOutcome::AlreadyOwned));

        let _ = std::fs::remove_file(&cache_path);
    }

    // S7-4. create_checkout 200 with a non-Stripe url ->
    // Err(Internal("unexpected checkout url")) -- validation lives in
    // session.rs, unit-testable here without touching the browser opener.
    #[test]
    fn create_checkout_non_stripe_url_rejected() {
        let _g = lock();
        let cache_path = temp_cache_path("t-checkout-badurl-cache.json");
        let cloud = Cloud::new(cache_path.clone());
        {
            let mut guard = cloud.session.lock().unwrap();
            *guard = Some(Session {
                user_id: "user-badurl".into(),
                email: "badurl@example.com".into(),
                access_token: "at-badurl".into(),
                refresh_token: "rt-badurl".into(),
                expires_at: now() + 3600,
                ephemeral: false,
            });
        }

        let port = start_mock_server("200 OK", r#"{"url":"https://evil.example/pay"}"#);
        set_mock_env(port);

        let err = cloud
            .create_checkout("socratic-tutor")
            .err()
            .expect("expected an error");
        match err {
            CloudError::Internal(reason) => {
                assert!(reason.contains("unexpected checkout url"), "reason was: {reason}")
            }
            other => panic!("expected Internal, got {other:?}"),
        }

        let _ = std::fs::remove_file(&cache_path);
    }

    // S7-6. create_checkout: server rejects a locally-fresh access token
    // with 401 (clock skew / out-of-band revocation -- same rationale as
    // grant_with_access's retry). `refresh_via_gate()` re-checks the SAME
    // local clock and short-circuits without a network call (the session
    // is still locally fresh), handing back the identical access token, so
    // connection 2 is the retried create-checkout call, which also 401s.
    // The mock listener serves exactly 2 responses then stops accepting --
    // a bug that retried a third time would hit a transport error
    // (Offline) here instead, so this also proves the one-retry shape (no
    // infinite retry).
    #[test]
    fn create_checkout_401_session_expired_no_retry() {
        let _g = lock();
        let cache_path = temp_cache_path("t-checkout-401-cache.json");
        let cloud = Cloud::new(cache_path.clone());
        {
            let mut guard = cloud.session.lock().unwrap();
            *guard = Some(Session {
                user_id: "user-401".into(),
                email: "u401@example.com".into(),
                access_token: "at-401".into(),
                refresh_token: "rt-401".into(),
                expires_at: now() + 3600,
                ephemeral: false,
            });
        }

        let port = start_mock_server_n(vec![
            ("401 Unauthorized", r#"{"message":"JWT expired"}"#),
            ("401 Unauthorized", r#"{"message":"JWT expired"}"#),
        ]);
        set_mock_env(port);

        let result = cloud.create_checkout("socratic-tutor");
        assert!(matches!(result, Err(CloudError::SessionExpired)));

        let _ = std::fs::remove_file(&cache_path);
    }

    // Sub7-2. create_portal_session happy path: locally-fresh session, 200
    // {"url":"https://billing.stripe.com/..."} -> Url(...) exact.
    #[test]
    fn create_portal_session_happy_path_returns_url() {
        let _g = lock();
        let cache_path = temp_cache_path("t-portal-happy-cache.json");
        let cloud = Cloud::new(cache_path.clone());
        {
            let mut guard = cloud.session.lock().unwrap();
            *guard = Some(Session {
                user_id: "user-portal".into(),
                email: "portal@example.com".into(),
                access_token: "at-fresh-portal".into(),
                refresh_token: "rt-fresh-portal".into(),
                expires_at: now() + 3600,
                ephemeral: false,
            });
        }

        let port = start_mock_server(
            "200 OK",
            r#"{"url":"https://billing.stripe.com/p/session/x"}"#,
        );
        set_mock_env(port);

        let result = cloud.create_portal_session().expect("expected success");
        match result {
            PortalOutcome::Url(u) => {
                assert_eq!(u, "https://billing.stripe.com/p/session/x")
            }
            PortalOutcome::NoBillingAccount => panic!("expected Url, got NoBillingAccount"),
        }

        let _ = std::fs::remove_file(&cache_path);
    }

    // Sub7-3. create_portal_session 404 {"error":"no_billing_account"} ->
    // NoBillingAccount.
    #[test]
    fn create_portal_session_404_is_no_billing_account() {
        let _g = lock();
        let cache_path = temp_cache_path("t-portal-404-cache.json");
        let cloud = Cloud::new(cache_path.clone());
        {
            let mut guard = cloud.session.lock().unwrap();
            *guard = Some(Session {
                user_id: "user-404".into(),
                email: "u404@example.com".into(),
                access_token: "at-404".into(),
                refresh_token: "rt-404".into(),
                expires_at: now() + 3600,
                ephemeral: false,
            });
        }

        let port = start_mock_server("404 Not Found", r#"{"error":"no_billing_account"}"#);
        set_mock_env(port);

        let result = cloud
            .create_portal_session()
            .expect("expected Ok(NoBillingAccount)");
        assert!(matches!(result, PortalOutcome::NoBillingAccount));

        let _ = std::fs::remove_file(&cache_path);
    }

    // Sub7-4. create_portal_session 200 with a non-Stripe url ->
    // Err(Internal("unexpected portal url")) -- validation lives in
    // session.rs, unit-testable here without touching the browser opener.
    #[test]
    fn create_portal_session_non_stripe_url_rejected() {
        let _g = lock();
        let cache_path = temp_cache_path("t-portal-badurl-cache.json");
        let cloud = Cloud::new(cache_path.clone());
        {
            let mut guard = cloud.session.lock().unwrap();
            *guard = Some(Session {
                user_id: "user-portal-badurl".into(),
                email: "portalbadurl@example.com".into(),
                access_token: "at-portal-badurl".into(),
                refresh_token: "rt-portal-badurl".into(),
                expires_at: now() + 3600,
                ephemeral: false,
            });
        }

        let port = start_mock_server("200 OK", r#"{"url":"https://evil.example/portal"}"#);
        set_mock_env(port);

        let err = cloud
            .create_portal_session()
            .err()
            .expect("expected an error");
        match err {
            CloudError::Internal(reason) => {
                assert!(reason.contains("unexpected portal url"), "reason was: {reason}")
            }
            other => panic!("expected Internal, got {other:?}"),
        }

        let _ = std::fs::remove_file(&cache_path);
    }

    // Remember1-1. Ephemeral sign-in (remember=false): the keyring stays
    // completely empty and no cache file is ever created, but the session
    // still works normally in memory for the rest of the process — proven
    // via ensure_fresh() succeeding purely off the local clock (mirrors
    // 10a's "dead port proves no network needed" trick): the mock listener
    // below serves exactly the 3 sign_in/apply_and_sync responses and
    // stops accepting, so a bug that fell back to a network refresh here
    // would fail this call outright.
    #[test]
    fn sign_in_ephemeral_leaves_no_keyring_or_cache_trace() {
        let _g = lock();
        let _cleanup = KeyringCleanup;
        store::delete_refresh_token();

        let cache_path = temp_cache_path("t-remember1-ephemeral-signin-cache.json");

        let port = start_mock_server_n(vec![
            (
                "200 OK",
                r#"{"access_token":"at-eph-1","token_type":"bearer","expires_in":3600,"refresh_token":"rt-eph-1","user":{"id":"user-eph-1","email":"eph1@example.com"}}"#,
            ),
            ("200 OK", r#"[{"nickname":"EphNick1"}]"#),
            ("200 OK", "[]"),
        ]);
        set_mock_env(port);

        let cloud = Cloud::new(cache_path.clone());
        let info = cloud
            .sign_in("eph1@example.com", "hunter2", false)
            .expect("expected sign_in to succeed");

        assert!(info.signed_in);
        assert_eq!(info.mode, "online");
        assert_eq!(
            store::load_refresh_token(),
            None,
            "expected no keyring entry for an ephemeral session"
        );
        assert!(!cache_path.exists(), "expected no cache file for an ephemeral session");

        let (access, user_id) = cloud
            .ensure_fresh()
            .expect("expected the session to still work purely in memory");
        assert_eq!(access, "at-eph-1");
        assert_eq!(user_id, "user-eph-1");
    }

    // Remember1-2. Ephemeral refresh: forcing a rotation (stale
    // expires_at) still updates the in-memory session and returns fresh
    // tokens, but the rotated-token persist at apply_and_sync's
    // refresh-path twin (~line 496 pre-task) is skipped exactly like
    // sign-in's ~322 site — keyring stays empty.
    #[test]
    fn ephemeral_session_refresh_skips_rotated_token_keyring_persist() {
        let _g = lock();
        let _cleanup = KeyringCleanup;
        store::delete_refresh_token();

        let cache_path = temp_cache_path("t-remember1-ephemeral-refresh-cache.json");
        let cloud = Cloud::new(cache_path.clone());
        {
            let mut guard = cloud.session.lock().unwrap();
            *guard = Some(Session {
                user_id: "user-eph-2".into(),
                email: "eph2@example.com".into(),
                access_token: "at-eph-2-stale".into(),
                refresh_token: "rt-eph-2-stale".into(),
                expires_at: now() - 10, // stale -> forces a refresh round-trip
                ephemeral: true,
            });
        }

        let port = start_mock_server(
            "200 OK",
            r#"{"access_token":"at-eph-2-rotated","token_type":"bearer","expires_in":3600,"refresh_token":"rt-eph-2-rotated","user":{"id":"user-eph-2","email":"eph2@example.com"}}"#,
        );
        set_mock_env(port);

        let (access, user_id) = cloud.ensure_fresh().expect("expected the refresh to succeed");
        assert_eq!(access, "at-eph-2-rotated");
        assert_eq!(user_id, "user-eph-2");
        assert_eq!(
            store::load_refresh_token(),
            None,
            "expected the rotated token NOT to be persisted for an ephemeral session"
        );

        let _ = std::fs::remove_file(&cache_path);
    }

    // Remember1-3. remember=true: sign_in behaves exactly as it always
    // has — the keyring holds the refresh token and the cache file holds
    // the synced session — proving the new parameter defaults through to
    // the SAME persisted path apply_and_sync already took.
    #[test]
    fn sign_in_remember_true_persists_keyring_and_cache() {
        let _g = lock();
        let _cleanup = KeyringCleanup;
        store::delete_refresh_token();

        let cache_path = temp_cache_path("t-remember1-remember-true-cache.json");

        let port = start_mock_server_n(vec![
            (
                "200 OK",
                r#"{"access_token":"at-rem-1","token_type":"bearer","expires_in":3600,"refresh_token":"rt-rem-1","user":{"id":"user-rem-1","email":"rem1@example.com"}}"#,
            ),
            ("200 OK", r#"[{"nickname":"RemNick1"}]"#),
            ("200 OK", "[]"),
        ]);
        set_mock_env(port);

        let cloud = Cloud::new(cache_path.clone());
        let info = cloud
            .sign_in("rem1@example.com", "hunter2", true)
            .expect("expected sign_in to succeed");

        assert!(info.signed_in);
        assert_eq!(info.mode, "online");
        assert_eq!(store::load_refresh_token(), Some("rt-rem-1".to_string()));

        let cached = store::read_cache(&cache_path);
        assert_eq!(cached.user_id, "user-rem-1");

        let _ = std::fs::remove_file(&cache_path);
    }
}
