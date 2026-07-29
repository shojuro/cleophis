//! Desktop (and every non-Android target): the OS credential store.
//!
//! **Moved verbatim from `cloud/store.rs`, not rewritten.** Phase 3.1's whole
//! desktop-side claim is that this is a relocation — the bodies, the error
//! strings, the swallow-and-log policy on the delete paths, and the two
//! round-trip tests are byte-for-byte what they were, so the desktop suite is
//! testing the same code it was testing before the seam existed. A seam that
//! also changed behaviour would be two changes wearing one commit.

use keyring::Entry;

use crate::cloud::error::CloudError;
use crate::cloud::verifier::StoredVerifier;

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

fn verifier_keyring_key(user_id: &str) -> String {
    format!("verifier:{user_id}")
}

/// One keyring entry per account, so removing one account's offline-sign-in
/// verifier (Task 6) never touches another's.
pub fn save_verifier(user_id: &str, v: &StoredVerifier) -> Result<(), CloudError> {
    let entry = Entry::new(KEYRING_SERVICE, &verifier_keyring_key(user_id)).map_err(|e| {
        eprintln!("keyring: failed to create verifier entry: {e}");
        CloudError::Internal("keyring unavailable".into())
    })?;
    let json = serde_json::to_string(v)
        .map_err(|e| CloudError::Internal(format!("failed to serialize verifier: {e}")))?;
    entry.set_password(&json).map_err(|e| {
        eprintln!("keyring: failed to save verifier: {e}");
        CloudError::Internal("keyring save failed".into())
    })
}

/// None on any error (missing entry, locked keyring, unsupported backend,
/// or corrupt JSON).
pub fn load_verifier(user_id: &str) -> Option<StoredVerifier> {
    let entry = Entry::new(KEYRING_SERVICE, &verifier_keyring_key(user_id)).ok()?;
    let json = entry.get_password().ok()?;
    serde_json::from_str(&json).ok()
}

/// Best-effort: errors swallowed. Never logs the verifier itself.
pub fn delete_verifier(user_id: &str) {
    if let Ok(entry) = Entry::new(KEYRING_SERVICE, &verifier_keyring_key(user_id)) {
        let _ = entry.delete_credential();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // Unified with every other cloud-module test that touches the real
        // OS credential store: the keyring entry is shared global state, so
        // this must serialize on the SAME lock as auth.rs/rest.rs/
        // session.rs's keyring-touching tests, not a module-local lock
        // (which would let them race).
        let _g = crate::cloud::test_support::lock();
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

    // 14. per-account verifier keyring round-trip: save -> load -> delete ->
    // load is None. Same real-OS-credential-store lock as `keyring_round_trip`
    // above — the credential store is shared global state across every
    // keyring test in the `cloud` module.
    struct VerifierKeyringCleanup<'a>(&'a str);
    impl Drop for VerifierKeyringCleanup<'_> {
        fn drop(&mut self) {
            delete_verifier(self.0);
        }
    }

    #[test]
    fn verifier_keyring_roundtrip() {
        let _g = crate::cloud::test_support::lock();
        let uid = "verifier-test-uid-t2";
        let _cleanup = VerifierKeyringCleanup(uid);
        delete_verifier(uid); // ensure a clean slate before we start

        let v = crate::cloud::verifier::derive_verifier("plan-test-pw-abcdef").unwrap();
        save_verifier(uid, &v).unwrap();
        assert!(crate::cloud::verifier::verify(
            "plan-test-pw-abcdef",
            &load_verifier(uid).unwrap()
        ));

        delete_verifier(uid);
        assert!(load_verifier(uid).is_none());
    }
}
