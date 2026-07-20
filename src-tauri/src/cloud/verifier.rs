//! Argon2id password verifier for offline sign-in. This is NOT the account
//! password itself and never leaves the device — it's a locally-stored
//! artifact derived from the password at enrollment time so a signed-out
//! user can prove they know the password while offline (no server round
//! trip). Where it's persisted (keyring vs. store) is the caller's concern.

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use rand_core::OsRng;
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

pub const VERIFIER_VERSION: u8 = 1;

pub const ARGON2_M_KIB: u32 = 19456;
pub const ARGON2_T: u32 = 2;
pub const ARGON2_P: u32 = 1;

/// Self-describing: `phc` is the full Argon2 PHC string, which already
/// encodes the salt and the params (m/t/p) it was derived with. That's
/// what makes `version` a bump-only-on-scheme-change field rather than a
/// place to track algorithm parameters — the parameters travel with the
/// hash itself, so a future param change doesn't require migrating stored
/// verifiers, only changing what new ones are derived with.
#[derive(Clone, Serialize, Deserialize)]
pub struct StoredVerifier {
    pub version: u8,
    pub phc: String,
}

/// NFC-normalize a password before hashing/verifying so the same password
/// typed with different Unicode compositions (precomposed vs. combining
/// accents) still matches. Called by both `derive_verifier` and `verify`
/// so enrollment and offline sign-in always agree.
pub fn normalize_password(password: &str) -> String {
    password.nfc().collect()
}

fn argon2() -> Result<Argon2<'static>, String> {
    let params = Params::new(ARGON2_M_KIB, ARGON2_T, ARGON2_P, None)
        .map_err(|e| format!("invalid argon2 params: {e}"))?;
    Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
}

pub fn derive_verifier(password: &str) -> Result<StoredVerifier, String> {
    let normalized = normalize_password(password);
    let salt = SaltString::generate(&mut OsRng);
    let phc = argon2()?
        .hash_password(normalized.as_bytes(), &salt)
        .map_err(|e| format!("failed to derive verifier: {e}"))?
        .to_string();
    Ok(StoredVerifier {
        version: VERIFIER_VERSION,
        phc,
    })
}

/// Never panics — any parse or mismatch error is just "not verified".
/// Timing is constant-time inside the crate for the actual hash comparison;
/// this is offline sign-in, not a network-facing endpoint, so that's the
/// only timing guarantee that matters here.
pub fn verify(password: &str, stored: &StoredVerifier) -> bool {
    let normalized = normalize_password(password);
    let Ok(parsed) = PasswordHash::new(&stored.phc) else {
        return false;
    };
    Argon2::default()
        .verify_password(normalized.as_bytes(), &parsed)
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_then_verify_roundtrip() {
        let v = derive_verifier("correct horse battery staple").unwrap();
        assert_eq!(v.version, VERIFIER_VERSION);
        assert!(verify("correct horse battery staple", &v));
    }

    #[test]
    fn wrong_password_fails() {
        let v = derive_verifier("s3cret-passphrase-xyz").unwrap();
        assert!(!verify("s3cret-passphrase-xyj", &v));
    }

    #[test]
    fn nfc_normalization_matches_across_compositions() {
        // "é" as U+00E9 (precomposed) vs "e" + U+0301 (combining acute).
        let precomposed = "caf\u{00E9}-password-1234";
        let decomposed = "cafe\u{0301}-password-1234";
        assert_ne!(precomposed, decomposed); // different bytes...
        let v = derive_verifier(precomposed).unwrap();
        assert!(verify(decomposed, &v)); // ...but verify after NFC.
    }

    #[test]
    fn serde_roundtrip() {
        let v = derive_verifier("another-passphrase-9876").unwrap();
        let json = serde_json::to_string(&v).unwrap();
        let back: StoredVerifier = serde_json::from_str(&json).unwrap();
        assert!(verify("another-passphrase-9876", &back));
    }
}
