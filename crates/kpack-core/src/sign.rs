//! Curated-pack signature verification (spec §1.2, §2.6): a detached
//! ed25519 signature over the whole `.kpack` file's raw bytes, checked
//! against a pinned curator public key. Personal packs are unsigned and
//! never reach this module (see `manifest::Pack::mount`).
//!
//! ## Trust model — mirrors K2's injection pattern exactly
//! The trusted key is never read from the pack itself: a pack-supplied key
//! would be trivially forgeable (anyone could sign their own pack and ship
//! their own "trusted" key alongside it, same failure mode K2 avoided for
//! the embedder-hash allowlist). It is injected via
//! `manifest::LoadContext::curator_key` — sourced in production from
//! `curator_verifying_key()` below (a compiled-in constant), and from a
//! test-only fixed-seed key in this crate's tests. Nothing in this module
//! ever reads a key out of the `.kpack` file.
//!
//! ## `verify_strict`, not `verify`
//! [`VerifyingKey::verify_strict`] additionally rejects small-order
//! (malleable) signatures/keys that the plain, RFC 8032-only `verify`
//! accepts — see `tests::t8_malleable_signature_rejected_by_verify_strict`
//! for a concrete signature that demonstrates the difference. A curated
//! pack's signature is a trust boundary; using the stricter check is
//! deliberate, not incidental.
//!
//! ## The `CURATOR_PUBLIC_KEY` release guardrail
//! `CURATOR_PUBLIC_KEY` is `None` until the real company signing key is
//! pinned at the §2.6 sign-and-publish milestone. Until then, EVERY curated
//! pack refuses to mount (fail-closed — see `manifest::Pack::mount`'s
//! curated-tier branch) — the honest v1 state, since no signing
//! infrastructure exists yet and therefore no curated pack could have been
//! legitimately signed. There is deliberately NO test key anywhere at crate
//! scope: the fixed-seed keypairs this module's tests use are constructed
//! *inside* `#[cfg(test)] mod tests`, so a release binary is
//! compile-time incapable of linking one in. Only a deliberate, reviewed
//! edit to `CURATOR_PUBLIC_KEY` itself — at §2.6 — can ever make a curated
//! pack verify.
//!
//! ## Two entry points: bytes vs. file, pre-open vs. in-mount
//! [`verify_detached`] takes already-in-memory bytes and is the shared
//! primitive both other functions in this module are built on. [`verify_file`]
//! wraps it with the actual disk I/O (raw pack bytes + a capped read of the
//! detached `.sig`) and is the **pre-open** gate — no `Pack`, no SQLite
//! touched anywhere on its call path — meant for a caller (K9) that has a
//! pack's bytes on disk but hasn't opened it yet. `manifest::Pack::mount`'s
//! curated-tier branch calls `verify_file` too, but only after it has
//! already opened the file as SQLite to read the manifest; see that
//! function's doc comment for the residual that ordering leaves open for
//! curated/CDN packs and why `verify_file` exists to let a wrapper close it
//! earlier.

use crate::format::Error;
use ed25519_dalek::{Signature, VerifyingKey};
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// The pinned production curator public key. `None` until the §2.6
/// sign-and-publish milestone mints the real company signing key and this
/// constant is deliberately replaced with it — see the module doc comment
/// for why `None` is the correct, fail-closed v1 state, and for the
/// guardrail this const's test-scope-only companion key enforces.
pub const CURATOR_PUBLIC_KEY: Option<[u8; 32]> = None;

/// Decodes [`CURATOR_PUBLIC_KEY`] into a [`VerifyingKey`], or `None` if it
/// isn't pinned yet (always `None` in this build — see the module doc
/// comment). Panics only if a future, deliberately-pinned
/// `CURATOR_PUBLIC_KEY` is malformed (not a valid ed25519 public key) —
/// impossible today since the constant is `None`, and a compile-time
/// invariant (caught immediately in CI/tests) once it isn't.
pub fn curator_verifying_key() -> Option<VerifyingKey> {
    CURATOR_PUBLIC_KEY.map(|bytes| {
        VerifyingKey::from_bytes(&bytes).expect(
            "CURATOR_PUBLIC_KEY is a compiled-in constant and must decode as a valid ed25519 public key",
        )
    })
}

/// Verifies a detached ed25519 `signature` over `pack_bytes` (the whole
/// `.kpack` file, byte for byte) against the pinned `key`, using
/// `verify_strict` (see the module doc comment for why). `signature` must
/// be exactly 64 bytes — any other length is a plain `Error::Schema`, never
/// a panic.
pub fn verify_detached(pack_bytes: &[u8], signature: &[u8], key: &VerifyingKey) -> Result<(), Error> {
    let sig = Signature::try_from(signature)
        .map_err(|_| Error::Schema("curated pack signature is malformed".to_string()))?;
    key.verify_strict(pack_bytes, &sig).map_err(|_| {
        Error::Schema(
            "curated pack signature does not verify against the pinned curator key".to_string(),
        )
    })
}

/// Reads a detached signature from `sig_path`, capped at 65 bytes — one more
/// than the exact 64 bytes a valid ed25519 signature always is. An uncapped
/// `std::fs::read` on this path would slurp an arbitrarily large `.sig` file
/// in full before any length check ran (a local OOM/DoS, and `.sig` files
/// travel alongside attacker-reachable CDN pack downloads); reading through
/// a 65-byte `Take` means at most 65 bytes are ever materialized regardless
/// of the file's real size on disk. Anything other than exactly 64 bytes —
/// including the file not existing — is reported as one of the same plain
/// errors `Pack::mount` already surfaced, never a panic.
fn read_sig_capped(sig_path: &Path) -> Result<Vec<u8>, Error> {
    let file = File::open(sig_path)
        .map_err(|_| Error::Schema("curated pack is missing its signature".to_string()))?;
    let mut buf = Vec::with_capacity(65);
    file.take(65)
        .read_to_end(&mut buf)
        .map_err(|_| Error::Schema("curated pack signature is malformed".to_string()))?;
    if buf.len() != 64 {
        return Err(Error::Schema(
            "curated pack signature is malformed".to_string(),
        ));
    }
    Ok(buf)
}

/// The **pre-open** verification gate: verifies a detached ed25519 signature
/// over a `.kpack` file's raw bytes given only the pack file's path and its
/// companion `<pack>.sig` path — no `Pack`, no SQLite involved anywhere in
/// this call. That matters because `manifest::Pack::mount` (this crate's
/// other signature-checking caller) necessarily opens the file as SQLite —
/// running sqlite-vec's C `xConnect` against still-unverified, untrusted
/// bytes — before its own in-mount signature check ever runs (see that
/// function's doc comment for the residual this leaves for curated/CDN
/// packs). `verify_file` is how a caller closes that gap: read the raw bytes
/// (`std::fs::read`) and the capped detached signature (`read_sig_capped`,
/// Fix 2 above), then delegate to `verify_detached` — one crypto path, no
/// SQLite anywhere on it. K9 (the CDN pack-download wrapper) should call
/// this on a freshly-downloaded curated pack's bytes BEFORE the pack is ever
/// handed to `Pack::mount`.
pub fn verify_file(pack_path: &Path, sig_path: &Path, key: &VerifyingKey) -> Result<(), Error> {
    let sig_bytes = read_sig_capped(sig_path)?;
    let pack_bytes = std::fs::read(pack_path).map_err(|e| {
        Error::Schema(format!(
            "could not read the pack file to verify its signature: {e}"
        ))
    })?;
    verify_detached(&pack_bytes, &sig_bytes, key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey, Verifier};

    /// Fixed-seed test keypairs, constructed here inside `#[cfg(test)]` —
    /// never at crate scope (see the module doc comment / t9's guardrail
    /// test). `SigningKey::from_bytes` is deterministic given a 32-byte
    /// seed, so no RNG dependency is needed for reproducible tests.
    fn test_signing_key() -> SigningKey {
        SigningKey::from_bytes(&[7u8; 32])
    }

    fn other_signing_key() -> SigningKey {
        SigningKey::from_bytes(&[9u8; 32])
    }

    const PACK_BYTES: &[u8] = b"pretend .kpack file bytes, byte for byte";

    // 1. Valid signature + correct key -> verifies.
    #[test]
    fn t1_valid_signature_verifies() {
        let sk = test_signing_key();
        let sig = sk.sign(PACK_BYTES);
        verify_detached(PACK_BYTES, &sig.to_bytes(), &sk.verifying_key()).unwrap();
    }

    // 2. Signature made by a DIFFERENT key -> refuses.
    #[test]
    fn t2_signature_from_wrong_key_refuses() {
        let sk = test_signing_key();
        let wrong = other_signing_key();
        let sig = sk.sign(PACK_BYTES);
        let err =
            verify_detached(PACK_BYTES, &sig.to_bytes(), &wrong.verifying_key()).unwrap_err();
        assert!(matches!(err, Error::Schema(_)));
    }

    // 3. Truncated (63 bytes) and oversized (65 bytes) signature -> refuse
    // with the malformed message, no panic on the short/long slice.
    #[test]
    fn t3_wrong_length_signature_refuses_malformed_no_panic() {
        let sk = test_signing_key();
        let sig = sk.sign(PACK_BYTES).to_bytes();

        let short = &sig[..63];
        let err = verify_detached(PACK_BYTES, short, &sk.verifying_key()).unwrap_err();
        assert!(matches!(err, Error::Schema(_)));
        assert!(err.to_string().contains("malformed"), "error was: {err}");

        let mut long = sig.to_vec();
        long.push(0);
        let err = verify_detached(PACK_BYTES, &long, &sk.verifying_key()).unwrap_err();
        assert!(matches!(err, Error::Schema(_)));
        assert!(err.to_string().contains("malformed"), "error was: {err}");
    }

    // 4. Tampered pack bytes after signing (flip one byte, keep the old
    // valid signature) -> refuses. This is the core guarantee.
    #[test]
    fn t4_tampered_pack_bytes_refuses() {
        let sk = test_signing_key();
        let sig = sk.sign(PACK_BYTES);

        let mut tampered = PACK_BYTES.to_vec();
        tampered[0] ^= 0x01;
        let err =
            verify_detached(&tampered, &sig.to_bytes(), &sk.verifying_key()).unwrap_err();
        assert!(matches!(err, Error::Schema(_)));
    }

    // 8. Malleability: a small-order/weak-pubkey signature that plain
    // `verify()` accepts must be rejected by `verify_strict()` (what
    // `verify_detached` actually calls). This uses a concrete, fixed test
    // vector rather than a live construction: a weak (order-2) public key
    // and ONE signature that verifies against TWO distinct messages
    // simultaneously — the textbook ed25519 "repudiation" malleability
    // case. Generated once, offline, using ed25519-dalek 2.2.0's own
    // documented construction (`tests/ed25519.rs::repudiation` in the
    // ed25519-dalek source, itself built on
    // `curve25519_dalek::constants::EIGHT_TORSION[4]`) and pinned here as
    // literal bytes — self-verified at generation time (`.verify()` must
    // accept both messages, `.verify_strict()` must reject both) — so this
    // test needs no extra runtime dependency (no `curve25519-dalek`, no
    // `rand`) beyond what's already in this crate's `Cargo.toml`.
    #[test]
    fn t8_malleable_signature_rejected_by_verify_strict() {
        const WEAK_PUBKEY_BYTES: [u8; 32] = [
            236, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
            255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 127,
        ];
        const MSG1: &[u8] = b"curated-pack-bytes-v1";
        const MSG2: &[u8] = b"curated-pack-bytes-v2-tampered";
        const SIG_BYTES: [u8; 64] = [
            54, 243, 8, 88, 76, 21, 212, 115, 84, 220, 165, 106, 116, 245, 41, 188, 169, 135, 42,
            230, 64, 120, 199, 8, 130, 103, 27, 122, 121, 220, 3, 82, 116, 180, 138, 246, 133, 36,
            11, 102, 55, 63, 246, 112, 233, 255, 224, 232, 61, 116, 39, 62, 68, 211, 69, 37, 22,
            78, 74, 94, 95, 156, 184, 13,
        ];

        let weak_key = VerifyingKey::from_bytes(&WEAK_PUBKEY_BYTES).unwrap();
        let sig = Signature::try_from(&SIG_BYTES[..]).unwrap();

        // Proof the vector is genuinely malleable (not just malformed): the
        // plain, non-strict `Verifier::verify` accepts it against BOTH
        // messages.
        assert!(weak_key.verify(MSG1, &sig).is_ok());
        assert!(weak_key.verify(MSG2, &sig).is_ok());

        // `verify_detached` — which calls `verify_strict` internally — must
        // refuse both.
        assert!(matches!(
            verify_detached(MSG1, &SIG_BYTES, &weak_key),
            Err(Error::Schema(_))
        ));
        assert!(matches!(
            verify_detached(MSG2, &SIG_BYTES, &weak_key),
            Err(Error::Schema(_))
        ));
    }

    // 9. Release guardrail: CURATOR_PUBLIC_KEY is None in this build — no
    // test key ever sits at crate scope; a real key only ever replaces it
    // deliberately at §2.6.
    #[test]
    fn t9_curator_public_key_is_none_in_this_build() {
        assert_eq!(CURATOR_PUBLIC_KEY, None);
        assert!(curator_verifying_key().is_none());
    }

    // ---- verify_file: the pre-open, bytes-on-disk gate (Fix 1/Fix 2). ----

    /// Mirrors `manifest.rs`'s own `unique_dir` test helper (this repo
    /// hand-rolls temp dirs instead of depending on `tempfile`); duplicated
    /// per-module since each module's `#[cfg(test)]` helper is private to
    /// it.
    fn unique_dir(name: &str) -> std::path::PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "cleophis-kpack-sign-test-{}-{}-{}",
            std::process::id(),
            unique,
            name
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Writes `bytes` to `<dir>/pack` and a valid detached signature (from
    /// `sk`) to `<dir>/pack.sig`, returning the pack path — the exact layout
    /// `verify_file` reads back.
    fn write_pack_and_sig(dir: &std::path::Path, bytes: &[u8], sk: &SigningKey) -> std::path::PathBuf {
        let pack_path = dir.join("pack");
        std::fs::write(&pack_path, bytes).unwrap();
        let sig = sk.sign(bytes);
        std::fs::write(sig_path(&pack_path), sig.to_bytes()).unwrap();
        pack_path
    }

    fn sig_path(pack_path: &std::path::Path) -> std::path::PathBuf {
        let mut os = pack_path.as_os_str().to_os_string();
        os.push(".sig");
        std::path::PathBuf::from(os)
    }

    // 10. Happy path: sign a temp file, verify_file confirms it OK.
    #[test]
    fn t10_verify_file_valid_signature_verifies() {
        let dir = unique_dir("t10");
        let sk = test_signing_key();
        let pack_path = write_pack_and_sig(&dir, PACK_BYTES, &sk);
        verify_file(&pack_path, &sig_path(&pack_path), &sk.verifying_key()).unwrap();
    }

    // 11. Signed by a DIFFERENT key than the one verify_file is given ->
    // refuses.
    #[test]
    fn t11_verify_file_wrong_key_refuses() {
        let dir = unique_dir("t11");
        let sk = test_signing_key();
        let pack_path = write_pack_and_sig(&dir, PACK_BYTES, &sk);
        let err = verify_file(&pack_path, &sig_path(&pack_path), &other_signing_key().verifying_key())
            .unwrap_err();
        assert!(matches!(err, Error::Schema(_)));
    }

    // 12. Pack bytes tampered after signing (rewrite the file, keep the
    // now-stale signature) -> refuses.
    #[test]
    fn t12_verify_file_tampered_pack_refuses() {
        let dir = unique_dir("t12");
        let sk = test_signing_key();
        let pack_path = write_pack_and_sig(&dir, PACK_BYTES, &sk);
        let mut tampered = PACK_BYTES.to_vec();
        tampered[0] ^= 0x01;
        std::fs::write(&pack_path, &tampered).unwrap();
        let err = verify_file(&pack_path, &sig_path(&pack_path), &sk.verifying_key()).unwrap_err();
        assert!(matches!(err, Error::Schema(_)));
    }

    // 13. Oversized (10 KB) .sig file -> rejected as malformed, no OOM/panic
    // (Fix 2's cap, exercised through verify_file specifically).
    #[test]
    fn t13_verify_file_oversized_sig_refuses_malformed_no_panic() {
        let dir = unique_dir("t13");
        let sk = test_signing_key();
        let pack_path = write_pack_and_sig(&dir, PACK_BYTES, &sk);
        std::fs::write(sig_path(&pack_path), vec![0u8; 10 * 1024]).unwrap();
        let err = verify_file(&pack_path, &sig_path(&pack_path), &sk.verifying_key()).unwrap_err();
        assert!(matches!(err, Error::Schema(_)));
        assert!(err.to_string().contains("malformed"), "error was: {err}");
    }
}
