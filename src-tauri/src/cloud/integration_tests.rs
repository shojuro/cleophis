//! Task A8: one live integration test against the REAL Supabase project
//! (`isltexsxpysxqewjsryv`). `#[ignore]`d — runs only when explicitly asked
//! for, since it hits the network and creates a real (never deleted here —
//! deleting needs service_role) auth user.
//!
//! Hard safety requirements (non-negotiable — see the task brief):
//! - Never sets `CLEOPHIS_SUPABASE_*` env vars: the committed consts in
//!   `config.rs` already point at the real project.
//! - Still takes the shared `test_support` lock: `config::supabase_url/key`
//!   read those env vars, so this test must serialize against any other
//!   `cloud` module test that sets/unsets them.
//! - `KeyringGuard` captures whatever refresh-token keyring state already
//!   exists (a developer's real session) as its FIRST action, before this
//!   module's `Cloud`/`auth`/`store` calls touch the keyring at all, and
//!   restores that exact state on drop — pass or fail.
//! - `CacheGuard` deletes only the unique temp-path cache file this test
//!   created, never the real app-data cache.

#![cfg(test)]

use std::path::PathBuf;

use crate::cloud::auth;
use crate::cloud::error::CloudError;
use crate::cloud::rest;
use crate::cloud::session::Cloud;
use crate::cloud::store;
use crate::cloud::test_support::lock;

/// Restores whatever refresh-token keyring state existed before this test
/// touched it (a developer's real session, or nothing) — pass or fail.
/// Captured as the test's very first action.
struct KeyringGuard {
    original: Option<String>,
}

impl KeyringGuard {
    fn capture() -> Self {
        KeyringGuard {
            original: store::load_refresh_token(),
        }
    }
}

impl Drop for KeyringGuard {
    fn drop(&mut self) {
        match &self.original {
            Some(token) => {
                if let Err(e) = store::save_refresh_token(token) {
                    eprintln!("a8: failed to restore developer's refresh token: {e}");
                }
            }
            None => store::delete_refresh_token(),
        }
    }
}

/// Deletes the unique temp cache file this test used — pass or fail. Never
/// touches the real app-data cache path.
struct CacheGuard {
    path: PathBuf,
}

impl Drop for CacheGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn unique_temp_cache_path() -> PathBuf {
    let mut p = std::env::temp_dir();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    p.push(format!(
        "cleophis-a8-integration-{}-{}.json",
        std::process::id(),
        nanos
    ));
    p
}

#[test]
#[ignore]
fn live_auth_and_entitlements_roundtrip() {
    // env-reading code (config::supabase_url/key) runs during this test even
    // though we never set CLEOPHIS_SUPABASE_* ourselves — keep serialization
    // with every other cloud-module test that does.
    let _g = lock();

    // FIRST action, before anything below touches the keyring: capture
    // whatever state (a developer's real session, or none) already lives in
    // the real Windows Credential Manager entry.
    let _keyring_guard = KeyringGuard::capture();

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let email = format!("shojuro.sb+a8-{nanos}@gmail.com");
    let password = format!("A8-integration-{nanos}!");
    let nickname = "A8 Test";

    let cache_path = unique_temp_cache_path();
    let _cache_guard = CacheGuard {
        path: cache_path.clone(),
    };

    // 1. A dedicated Cloud instance pointed at a unique temp cache path —
    // never the real app-data cache.
    let cloud = Cloud::new(cache_path.clone());

    // 2. sign_up against the live project. Email autoconfirm is ON, so this
    // returns a session directly (no EmailNotConfirmed detour). A successful
    // `nickname == "A8 Test"` proves the DB trigger (handle_new_user) ran
    // and the profile readback (rest::get_profile_nickname) round-tripped.
    let info = cloud
        .sign_up(&email, &password, nickname)
        .expect("sign_up should succeed against the live project");
    assert!(info.signed_in, "expected signed_in after sign_up");
    assert_eq!(info.mode, "online");
    assert_eq!(info.nickname.as_deref(), Some(nickname));
    assert!(
        info.entitlements.is_empty(),
        "expected no entitlements immediately after sign_up, got {:?}",
        info.entitlements
    );

    // 3. Self-grant a trial entitlement (client-insertable source).
    cloud
        .grant("socratic-tutor", "trial")
        .expect("grant(\"socratic-tutor\", \"trial\") should succeed");

    // 4. Exactly one entitlement now exists server-side.
    let entitlements = cloud.entitlements().expect("entitlements() should succeed");
    assert_eq!(
        entitlements.len(),
        1,
        "expected exactly one entitlement, got {entitlements:?}"
    );
    assert_eq!(entitlements[0].model_id, "socratic-tutor");
    assert_eq!(entitlements[0].source, "trial");

    // 5. Duplicate grant is idempotent (on_conflict + ignore-duplicates) —
    // still Ok, still exactly one entitlement.
    cloud
        .grant("socratic-tutor", "trial")
        .expect("duplicate grant should still be Ok (idempotent)");
    let entitlements = cloud.entitlements().expect("entitlements() should succeed");
    assert_eq!(
        entitlements.len(),
        1,
        "expected the duplicate grant not to duplicate the entitlement, got {entitlements:?}"
    );

    // 6. RLS fence: the session lives privately inside `cloud`, so get a
    // fresh access token the same way any external caller would — a plain
    // password sign-in — and hit `rest::insert_entitlement` directly with
    // source "purchase". The insert policy's with-check restricts clients to
    // source in ('trial','library'); "purchase" must be rejected.
    let fresh = auth::sign_in_password(&email, &password)
        .expect("sign_in_password should succeed for the just-created user");
    let rls_result = rest::insert_entitlement(
        &fresh.access_token,
        &fresh.user_id,
        "socratic-tutor",
        "purchase",
    );
    match rls_result {
        Err(CloudError::Api { status, msg }) => {
            eprintln!(
                "a8: RLS fence rejected the purchase-source insert as expected (status {status}, msg: {msg})"
            );
        }
        other => panic!(
            "expected Err(CloudError::Api{{..}}) from the RLS with-check fence, got {other:?}"
        ),
    }

    // 7. Sign out: keyring restoration is deferred to the guard (which
    // restores the ORIGINAL developer token, not whatever sign_out left
    // behind); here we only assert the cache file was deleted.
    cloud.sign_out();
    assert!(
        !cache_path.exists(),
        "expected sign_out to delete the cache file at {cache_path:?}"
    );

    // 8. A fresh Cloud instance at the SAME temp cache path. sign_out just
    // deleted the keyring entry this test created, so restore() here finds
    // nothing in the keyring and reports signed out — no network call is
    // even needed for that outcome. (The guard restores the developer's
    // original token afterward, once this whole test function returns.)
    let cloud2 = Cloud::new(cache_path.clone());
    let info2 = cloud2.restore();
    assert!(
        !info2.signed_in,
        "expected signed_out on restore() with no keyring entry present"
    );
    assert_eq!(info2.mode, "signedOut");

    // Left behind: deleting the auth user needs service_role, which this
    // test does not have. Print the email so the coordinator can clean it
    // up out-of-band.
    eprintln!("a8: leftover live test user email: {email}");
}
