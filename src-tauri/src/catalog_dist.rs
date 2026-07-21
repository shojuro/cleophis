//! The signed **distribution** catalog (`catalog.json` + detached
//! `catalog.json.sig`, fetched from the public `cleophis-dist` bucket) —
//! distinct from the bundled product catalog in `catalog.rs`, which ships
//! inside the app and is never signed.
//!
//! This module is pure logic: given catalog bytes, a detached signature, and
//! a trusted `VerifyingKey`, [`parse_and_verify`] verifies the signature
//! against the EXACT bytes (sign-then-parse — the signature covers the raw
//! wire bytes, not any re-serialized form) and only then parses JSON, so a
//! signature failure never even reaches the parser. [`check_not_downgrade`]
//! is the separate monotonic-version guard: the app persists the highest
//! `catalog_version` it has ever verified (Task B2 wires that persistence)
//! and refuses to accept a catalog whose version is lower, closing off a
//! rollback-to-a-vulnerable-catalog attack even though the bucket is public
//! and every artifact there is individually integrity-checked.
//!
//! Fetch and persistence are out of scope here (Task B2); this module never
//! touches the network or the filesystem.

use ed25519_dalek::VerifyingKey;
use serde::{Deserialize, Serialize};

/// The signed distribution catalog's top-level shape. Field names are
/// already snake_case in the wire JSON (produced by the Python signing
/// pipeline) — plain derive, no rename attribute, unlike the camelCase
/// bundled `catalog.rs::CatalogEntry`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DistCatalog {
    pub catalog_version: u64,
    pub generated_at: String,
    pub artifacts: Vec<Artifact>,
}

/// One downloadable artifact (base model or LoRA adapter) listed in the
/// catalog. `path` is relative to the single configured artifact base URL;
/// `sha256` is the integrity check performed after download (Task B3).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Artifact {
    pub path: String,
    pub sha256: String,
    pub size: u64,
    pub kind: String,
    pub base_model: String,
    pub version: String,
    pub license: String,
}

/// Verifies `sig` as a detached ed25519 signature over `catalog_bytes`
/// (exactly as received — the signature covers the raw wire bytes) against
/// `key`, and only on success parses those same bytes as JSON. Reuses
/// `kpack_core::sign::verify_detached` (`verify_strict` under the hood) so
/// there is exactly one ed25519 verification path in the app; production
/// callers pass `kpack_core::sign::curator_verifying_key()`'s key (this
/// function does not decide which key to trust — that stays the caller's
/// call, so a `None` production key fails closed at the caller, not here).
pub fn parse_and_verify(
    catalog_bytes: &[u8],
    sig: &[u8],
    key: &VerifyingKey,
) -> Result<DistCatalog, String> {
    kpack_core::sign::verify_detached(catalog_bytes, sig, key).map_err(|e| e.to_string())?;
    serde_json::from_slice(catalog_bytes).map_err(|e| format!("catalog.json invalid: {e}"))
}

/// The downgrade guard: refuses a `new_version` lower than `highest_seen`
/// (the highest `catalog_version` ever successfully verified, persisted by
/// the caller — Task B2). Equal or newer is `Ok`; the caller is responsible
/// for then updating its persisted `highest_seen`.
pub fn check_not_downgrade(new_version: u64, highest_seen: u64) -> Result<(), String> {
    if new_version < highest_seen {
        return Err(format!(
            "catalog_version {new_version} is older than the highest previously verified version {highest_seen} — refusing a downgrade"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    /// Fixed-seed test keypair, constructed here inside `#[cfg(test)]` —
    /// mirrors kpack-core's own `sign.rs` test pattern exactly (never at
    /// crate/module scope, so a release binary is compile-time incapable of
    /// linking a test key in). `SigningKey::from_bytes` is deterministic
    /// given a 32-byte seed, so no RNG/`rand` dependency is needed.
    fn test_signing_key() -> SigningKey {
        SigningKey::from_bytes(&[7u8; 32])
    }

    /// A minimal valid catalog (`catalog_version: 0`) in the exact wire
    /// shape the Python signing pipeline produces.
    const CATALOG_JSON: &[u8] = br#"{
        "catalog_version": 0,
        "generated_at": "2026-07-21T00:00:00Z",
        "artifacts": [
            {
                "path": "base/qwen3-4b-q4.gguf",
                "sha256": "abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234",
                "size": 123456,
                "kind": "base",
                "base_model": "qwen3-4b",
                "version": "1",
                "license": "apache-2.0"
            }
        ]
    }"#;

    #[test]
    fn verify_rejects_bad_signature() {
        let sk = test_signing_key();
        let mut sig = sk.sign(CATALOG_JSON).to_bytes();
        sig[0] ^= 0x01; // flip a byte -> signature no longer matches
        let err = parse_and_verify(CATALOG_JSON, &sig, &sk.verifying_key()).unwrap_err();
        assert!(!err.is_empty());
    }

    #[test]
    fn verify_accepts_good_signature_and_parses() {
        let sk = test_signing_key();
        let sig = sk.sign(CATALOG_JSON);
        let catalog = parse_and_verify(CATALOG_JSON, &sig.to_bytes(), &sk.verifying_key()).unwrap();

        assert_eq!(catalog.catalog_version, 0);
        assert_eq!(catalog.generated_at, "2026-07-21T00:00:00Z");
        assert_eq!(catalog.artifacts.len(), 1);
        let a = &catalog.artifacts[0];
        assert_eq!(a.path, "base/qwen3-4b-q4.gguf");
        assert_eq!(a.kind, "base");
        assert_eq!(a.base_model, "qwen3-4b");
        assert_eq!(a.version, "1");
        assert_eq!(a.license, "apache-2.0");
        assert_eq!(a.size, 123456);
    }

    #[test]
    fn downgrade_refused() {
        assert!(check_not_downgrade(3, 5).is_err());
    }

    #[test]
    fn same_or_newer_ok() {
        assert!(check_not_downgrade(5, 5).is_ok());
        assert!(check_not_downgrade(6, 5).is_ok());
    }
}
