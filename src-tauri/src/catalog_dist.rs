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
//! Task B2 wires fetch + persistence in this same module:
//! [`fetch_and_verify_catalog`] is the testable core — GET `catalog.json` +
//! `catalog.json.sig` from a caller-supplied base URL, `parse_and_verify`,
//! `check_not_downgrade` against the highest version persisted at a
//! caller-supplied path, then (only if the version grew) persist the new
//! highest via [`write_highest_version`] — and `#[tauri::command]
//! fetch_dist_catalog` is the thin production wrapper that resolves the
//! compiled-in [`ARTIFACT_BASE_URL`], the pinned curator key (Task B5
//! pinned it; [`resolve_curator_key`] still fails closed if a build were
//! ever compiled with it unset), and the app-data state path.

use ed25519_dalek::VerifyingKey;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

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

// ---------------------------------------------------------------------
// Fetch + persistence (Task B2)
// ---------------------------------------------------------------------

/// Compiled-in artifact bucket base URL — the single config value the task
/// brief calls for: `cleophis-dist`'s live production S3-style B2 endpoint
/// (the friendly `f005.backblazeb2.com` alias does NOT resolve for the
/// `us-east-005` region — NXDOMAIN; this host is confirmed live by
/// `verify_published.py`'s public-path self-check and passes
/// `download_host_allowed`'s `*.backblazeb2.com` suffix rule), Cloudflare
/// in front of the same bucket later. `catalog.json` and its detached
/// signature live at `<ARTIFACT_BASE_URL>/catalog.json` and
/// `<ARTIFACT_BASE_URL>/catalog.json.sig` respectively; every catalog
/// `Artifact::path` (Task B3) is relative to this same base.
pub const ARTIFACT_BASE_URL: &str = "https://cleophis-dist.s3.us-east-005.backblazeb2.com";

/// The app-data file name the persisted highest-ever-verified
/// `catalog_version` lives in — see [`read_highest_version`] /
/// [`write_highest_version`].
const STATE_FILE_NAME: &str = "dist_catalog_state.json";

/// `catalog.json` is capped well above any real catalog's expected size but
/// far below "arbitrary" — defense against a compromised/misconfigured
/// bucket trying to make the app read an unbounded response into memory,
/// the same defensive-cap rationale as `kpack_core::sign::verify_file`'s
/// capped `.sig` read.
const MAX_CATALOG_BYTES: u64 = 1024 * 1024; // 1 MiB
/// A valid detached ed25519 signature is exactly 64 bytes —
/// `verify_detached` itself enforces that exactly; this cap only bounds how
/// much a misbehaving server can make this module read into memory before
/// that check ever runs.
const MAX_SIG_BYTES: u64 = 64 * 1024; // 64 KiB

/// On-disk shape of the persisted downgrade-guard state — deliberately just
/// the one field; nothing else needs to survive a restart for this task.
#[derive(Serialize, Deserialize, Default)]
struct PersistedState {
    highest_catalog_version: u64,
}

/// Reads the persisted highest-ever-verified `catalog_version` from `path`.
/// Missing file, unreadable file, and corrupt/malformed JSON all read as
/// `0` (mirrors `cloud::store::read_cache`'s "any failure -> default"
/// stance) — the correct "nothing verified yet" state, since `0` never
/// causes [`check_not_downgrade`] to reject a first-ever fetch.
pub fn read_highest_version(path: &Path) -> u64 {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<PersistedState>(&s).ok())
        .map(|s| s.highest_catalog_version)
        .unwrap_or(0)
}

/// Atomically persists `version` as the new highest-ever-verified
/// `catalog_version`: temp file + rename, the same house pattern
/// `cloud::store::write_cache` uses, so a concurrent reader never observes
/// a partially-written state file. Creates `path`'s parent directory as
/// needed.
pub fn write_highest_version(path: &Path, version: u64) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create dist-catalog state dir: {e}"))?;
        }
    }
    let json = serde_json::to_string(&PersistedState {
        highest_catalog_version: version,
    })
    .map_err(|e| format!("failed to serialize dist-catalog state: {e}"))?;

    let tmp_path = temp_write_path(path);
    {
        let mut file = std::fs::File::create(&tmp_path)
            .map_err(|e| format!("failed to write dist-catalog state: {e}"))?;
        file.write_all(json.as_bytes())
            .map_err(|e| format!("failed to write dist-catalog state: {e}"))?;
    }
    std::fs::rename(&tmp_path, path)
        .map_err(|e| format!("failed to finalize dist-catalog state: {e}"))?;
    Ok(())
}

/// A temp path unique per writer, alongside `path` — the same scheme as
/// `cloud::store::temp_write_path` (a third independent copy of this small
/// helper; there's no common module both sides already depend on, so
/// duplicating it here beats introducing a new shared one for four lines).
fn temp_write_path(path: &Path) -> PathBuf {
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

/// GETs `url` with a short timeout, reading at most `max_bytes + 1` bytes
/// (via `Read::take`) so an oversized response is detected as a plain error
/// rather than read to completion. Used for BOTH `catalog.json` and
/// `catalog.json.sig` — deliberately no ranged/resume machinery (unlike
/// `download.rs`'s `streaming_agent`/`run_download`): these are small
/// files, and on failure the whole fetch is simply retried by the caller
/// re-invoking `fetch_dist_catalog`, not resumed byte-for-byte.
fn get_capped(url: &str, max_bytes: u64) -> Result<Vec<u8>, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(crate::cloud::config::CONNECT_TIMEOUT)
        .timeout(crate::cloud::config::OVERALL_TIMEOUT)
        .build();
    let resp = agent.get(url).call().map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    resp.into_reader()
        .take(max_bytes + 1)
        .read_to_end(&mut buf)
        .map_err(|e| e.to_string())?;
    if buf.len() as u64 > max_bytes {
        return Err(format!(
            "response from {url} exceeded the {max_bytes}-byte cap"
        ));
    }
    Ok(buf)
}

/// Process-wide single-flight guard over [`fetch_and_verify_catalog`]'s
/// read-check-write section (below). Without it, two concurrent fetches
/// (e.g. a manual retry racing a background refresh) could both
/// `read_highest_version` before either `write_highest_version`, both pass
/// `check_not_downgrade` against the same stale `highest_seen`, and then
/// race to write — not a security hole (both catalogs were independently
/// signature-verified), but a lost-update on the persisted highest-version
/// state. Plain `Mutex<()>`, held only across that section — never across
/// the network fetch above it, so concurrent fetches still overlap on the
/// slow part. Poisoned-lock handling mirrors `download.rs`/`kpack.rs`'s
/// production locks: `.unwrap()`, not a recovery path — a poisoned lock
/// here means a prior holder panicked mid-write, and propagating that
/// panic is preferable to silently proceeding over a possibly-torn state
/// file.
static FETCH_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The testable core of catalog fetch (Task B2): GET `<base_url>/catalog.json`
/// + `<base_url>/catalog.json.sig`, [`parse_and_verify`] against `key`,
/// apply [`check_not_downgrade`] against the highest version persisted at
/// `state_path`, and — ONLY on success, and only if the version actually
/// grew — persist the new highest via [`write_highest_version`]. Every
/// failure path (network, signature, downgrade) returns before any state
/// change: `state_path` is written to only after `check_not_downgrade` has
/// already accepted `catalog.catalog_version`, so a refused downgrade
/// always leaves the persisted highest exactly as it was.
///
/// Deliberately takes no Tauri types (`base_url`, `key`, `state_path` are
/// all plain values) — mirrors `download.rs`'s `run_download`, and for the
/// same reason: the test suite below drives this directly against a local
/// mock server instead of standing up an `AppHandle`.
pub fn fetch_and_verify_catalog(
    base_url: &str,
    key: &VerifyingKey,
    state_path: &Path,
) -> Result<DistCatalog, String> {
    let catalog_bytes = get_capped(&format!("{base_url}/catalog.json"), MAX_CATALOG_BYTES)
        .map_err(|e| format!("failed to fetch catalog.json: {e}"))?;
    let sig_bytes = get_capped(&format!("{base_url}/catalog.json.sig"), MAX_SIG_BYTES)
        .map_err(|e| format!("failed to fetch catalog.json.sig: {e}"))?;

    let catalog = parse_and_verify(&catalog_bytes, &sig_bytes, key)?;

    // See `FETCH_LOCK`'s doc comment: single-flight the check+write section
    // so two concurrent fetches can't both observe the same `highest_seen`
    // and race to write it.
    let _guard = FETCH_LOCK.lock().unwrap();

    let highest_seen = read_highest_version(state_path);
    check_not_downgrade(catalog.catalog_version, highest_seen)?;

    if catalog.catalog_version > highest_seen {
        write_highest_version(state_path, catalog.catalog_version)?;
    }

    Ok(catalog)
}

/// Resolves the pinned production curator key, or a clear, fail-closed
/// error if none is pinned. `curator_verifying_key()` has returned `Some`
/// in every production build since Task B5 pinned the real curator key (see
/// `kpack_core::sign`'s module doc comment); this function's `None` branch
/// is now a belt-and-suspenders guard rather than the expected v1 path —
/// if `CURATOR_PUBLIC_KEY` were ever reverted to `None`, this command must
/// refuse rather than silently skip verification, so `None` is a hard
/// `Err` here, never a "trust anyway" fallback. Factored out of
/// `fetch_dist_catalog` so this fail-closed behavior is directly unit
/// testable without an `AppHandle`.
fn resolve_curator_key() -> Result<VerifyingKey, String> {
    kpack_core::sign::curator_verifying_key()
        .ok_or_else(|| "no curator key pinned in this build".to_string())
}

/// Production wrapper around [`fetch_and_verify_catalog`]: resolves the
/// compiled-in [`ARTIFACT_BASE_URL`], the pinned curator key (fail-closed —
/// see [`resolve_curator_key`]), and the app-data state path
/// (`<app_data>/dist_catalog_state.json`), then runs the fetch on a
/// `spawn_blocking` thread — mirrors every other blocking-network Tauri
/// command in `cloud::commands`, since this is a real network call and must
/// not stall the async runtime.
#[tauri::command]
pub async fn fetch_dist_catalog(app: tauri::AppHandle) -> Result<DistCatalog, String> {
    tauri::async_runtime::spawn_blocking(move || {
        use tauri::Manager;
        let key = resolve_curator_key()?;
        let app_data = app.path().app_data_dir().map_err(|e| e.to_string())?;
        let state_path = app_data.join(STATE_FILE_NAME);
        fetch_and_verify_catalog(ARTIFACT_BASE_URL, &key, &state_path)
    })
    .await
    .map_err(|_| "Something went wrong on this device. Please try again.".to_string())?
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
        assert!(err.contains("signature"), "error was: {err}");
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

    // ---------------------------------------------------------------
    // Task B2: persistence + fetch tests
    // ---------------------------------------------------------------

    fn unique_test_dir(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "cleophis-b2-test-{}-{}-{}",
            std::process::id(),
            nanos,
            name
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A minimal valid catalog at an arbitrary `catalog_version`, empty
    /// `artifacts` — B2's fetch/downgrade tests only care about the version
    /// number, not artifact shape (that's `parse_and_verify`'s own coverage
    /// above, via `CATALOG_JSON`).
    fn catalog_json_with_version(version: u64) -> Vec<u8> {
        format!(
            r#"{{"catalog_version": {version}, "generated_at": "2026-07-21T00:00:00Z", "artifacts": []}}"#
        )
        .into_bytes()
    }

    // 1. Persistence round-trip.
    #[test]
    fn persisted_highest_version_round_trips() {
        let dir = unique_test_dir("persist-roundtrip");
        let path = dir.join(STATE_FILE_NAME);

        write_highest_version(&path, 42).expect("write should succeed");
        assert_eq!(read_highest_version(&path), 42);

        let _ = std::fs::remove_dir_all(&dir);
    }

    // 1b. Missing file -> 0 (the brief's explicit case).
    #[test]
    fn persisted_highest_version_missing_file_is_zero() {
        let dir = unique_test_dir("persist-missing");
        let path = dir.join("does-not-exist.json");
        assert_eq!(read_highest_version(&path), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // Corrupt JSON -> 0 too (mirrors cloud::store::read_cache's own
    // "any failure -> default" test coverage).
    #[test]
    fn persisted_highest_version_corrupt_file_is_zero() {
        let dir = unique_test_dir("persist-corrupt");
        let path = dir.join(STATE_FILE_NAME);
        std::fs::write(&path, b"{ not valid json ").unwrap();
        assert_eq!(read_highest_version(&path), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_highest_version_creates_parent_dirs() {
        let mut dir = unique_test_dir("persist-nested");
        dir.push("sub");
        let path = dir.join(STATE_FILE_NAME);
        write_highest_version(&path, 7).expect("should create parent dirs and write");
        assert_eq!(read_highest_version(&path), 7);
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }

    // 2. Mock-server fetch test: signed catalog -> parsed, and the
    // persisted highest grows to match (version > the missing-file default
    // of 0).
    #[test]
    fn fetch_core_happy_path_verifies_and_persists_when_version_grows() {
        let sk = test_signing_key();
        let catalog_bytes = catalog_json_with_version(5);
        let sig = sk.sign(&catalog_bytes).to_bytes().to_vec();

        let (base_url, _handle) = crate::cloud::test_support::start_path_server(vec![
            ("/catalog.json".to_string(), catalog_bytes),
            ("/catalog.json.sig".to_string(), sig),
        ]);

        let dir = unique_test_dir("fetch-happy");
        let state_path = dir.join(STATE_FILE_NAME);

        let catalog = fetch_and_verify_catalog(&base_url, &sk.verifying_key(), &state_path)
            .expect("fetch should succeed");
        assert_eq!(catalog.catalog_version, 5);
        assert_eq!(read_highest_version(&state_path), 5);

        let _ = std::fs::remove_dir_all(&dir);
    }

    // 3. Downgrade after newer -> Err, and the persisted state is left
    // exactly as it was (the brief's explicit acceptance case).
    #[test]
    fn fetch_core_refuses_downgrade_after_newer_and_leaves_state_untouched() {
        let sk = test_signing_key();
        let dir = unique_test_dir("fetch-downgrade");
        let state_path = dir.join(STATE_FILE_NAME);

        let catalog5 = catalog_json_with_version(5);
        let sig5 = sk.sign(&catalog5).to_bytes().to_vec();
        let (base_url_5, _h5) = crate::cloud::test_support::start_path_server(vec![
            ("/catalog.json".to_string(), catalog5),
            ("/catalog.json.sig".to_string(), sig5),
        ]);
        fetch_and_verify_catalog(&base_url_5, &sk.verifying_key(), &state_path)
            .expect("first fetch (v5) should succeed");
        assert_eq!(read_highest_version(&state_path), 5);

        let catalog3 = catalog_json_with_version(3);
        let sig3 = sk.sign(&catalog3).to_bytes().to_vec();
        let (base_url_3, _h3) = crate::cloud::test_support::start_path_server(vec![
            ("/catalog.json".to_string(), catalog3),
            ("/catalog.json.sig".to_string(), sig3),
        ]);
        let err = fetch_and_verify_catalog(&base_url_3, &sk.verifying_key(), &state_path)
            .expect_err("an older catalog after a newer one must be refused");
        assert!(err.contains("downgrade"), "error was: {err}");
        assert_eq!(
            read_highest_version(&state_path),
            5,
            "a refused downgrade must never touch the persisted highest"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // A bad signature during fetch must also refuse without touching state
    // (belt-and-suspenders on top of `verify_rejects_bad_signature` above,
    // this time through the full fetch path against a mock server).
    #[test]
    fn fetch_core_refuses_bad_signature_and_leaves_state_untouched() {
        let sk = test_signing_key();
        let wrong_key = SigningKey::from_bytes(&[9u8; 32]);
        let catalog_bytes = catalog_json_with_version(5);
        let sig = sk.sign(&catalog_bytes).to_bytes().to_vec();

        let (base_url, _handle) = crate::cloud::test_support::start_path_server(vec![
            ("/catalog.json".to_string(), catalog_bytes),
            ("/catalog.json.sig".to_string(), sig),
        ]);

        let dir = unique_test_dir("fetch-bad-sig");
        let state_path = dir.join(STATE_FILE_NAME);

        // Verify against the WRONG key's VerifyingKey -> signature check
        // must fail.
        let err = fetch_and_verify_catalog(&base_url, &wrong_key.verifying_key(), &state_path)
            .expect_err("a signature that doesn't match the trusted key must be refused");
        assert!(err.contains("signature"), "error was: {err}");
        assert_eq!(read_highest_version(&state_path), 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    // 4. The production command path's key resolution: Task B5 pinned
    // `CURATOR_PUBLIC_KEY`, so `resolve_curator_key()` must now resolve
    // successfully and return exactly the pinned production key — not
    // silently substitute a different one. Deliberately asserted against
    // the literal expected bytes (not re-derived from
    // `kpack_core::sign::curator_verifying_key()`, which would make this
    // tautological — one is defined via the other, so it could never
    // catch a wrong key): a real, deliberate key rotation must consciously
    // edit BOTH `sign.rs`'s `CURATOR_PUBLIC_KEY` and this literal, or this
    // test fails. `resolve_curator_key`'s `None` branch (fail closed,
    // never "trust anyway") is exercised structurally by inspection —
    // `curator_verifying_key().ok_or_else(...)` — since a real build can
    // no longer put the production constant back to `None` without
    // editing `sign.rs` directly.
    #[test]
    fn resolve_curator_key_resolves_the_pinned_production_key() {
        let key = resolve_curator_key().expect("pinned production key must resolve to Ok");
        assert_eq!(
            key.to_bytes(),
            [
                0x15, 0x8c, 0xb9, 0x9e, 0x97, 0x56, 0xe2, 0xe4, 0xd0, 0x1d, 0x88, 0xb7, 0xec,
                0xfe, 0xb9, 0x9a, 0x76, 0x54, 0x78, 0x21, 0xff, 0xe9, 0xf9, 0xf1, 0x98, 0x53,
                0x16, 0xc5, 0x45, 0x1c, 0xd0, 0xc0,
            ]
        );
    }
}
