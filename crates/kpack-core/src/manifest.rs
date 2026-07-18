//! Pack manifest (§1.2): a typed self-description over the `manifest`
//! key/value table K1 created, plus the load-time integrity gate.
//!
//! ## Storage
//! Every field is stored as one TEXT row in the `manifest` table (numbers
//! and bools are stringified — `"true"`/`"false"` for bools, `Display` for
//! numbers). `Manifest::write` upserts every mandatory key (via
//! `Pack::manifest_set`, already an atomic upsert per key); `Manifest::read`
//! reads them all back, validating presence and type, and fails with a
//! plain-language `Error::Schema` naming the offending key on the first
//! problem found — mirroring `src-tauri/src/catalog.rs`'s
//! `parse_*() -> Result<_, String>` style, adapted to this crate's `Error`
//! type (no `String`-typed errors in this crate; see `format::Error`).
//!
//! `write` takes `&Pack` (not `&mut Pack`) per this slice's contract, so the
//! per-key upserts aren't wrapped in one SQL transaction (that would need
//! `rusqlite::Connection::transaction`'s `&mut self`, a bigger API change
//! than this slice needs) — each individual upsert is already atomic, and
//! `write` only ever runs during pack construction, not against a mounted,
//! concurrently-read pack.
//!
//! ## The load-time gate (§1.2)
//! `check_load` runs four checks, in order, each fail-closed with a reason
//! meant to be shown to a user: embedder hash → dims → schema version →
//! vec format version (the last enforces the compatibility field the plan's
//! risk note calls out; the first three are the spec-mandated ones).
//! Curated-pack signature verification is explicitly NOT here — it's
//! enforced in `Pack::mount` instead (K3; see the `// K3:` seam comment in
//! `check_load` and the doc comment on `mount` for why).

use crate::format::{Error, Pack, SCHEMA_VERSION};
use ed25519_dalek::VerifyingKey;
use std::path::{Path, PathBuf};

/// The `sqlite-vec` version whose on-disk `vec0` format this build was
/// compiled against — mirrors the pin in `Cargo.toml` (see its comment for
/// why `0.1.9`, not the newer DISKANN-broken prerelease). The `vec0`
/// on-disk format is a pack-compatibility surface exactly like the embedder
/// hash (plan risk note), so every pack self-declares which format version
/// it was built for.
pub const VEC_FORMAT_VERSION: &str = "0.1.9";

/// Pack trust tier (§1.2). `Curated` packs are signed and served from the
/// CDN (signature verification lands in K3); `Personal` packs are built
/// on-device, unsigned, and mount only from the user's local pack
/// directory, never from the CDN path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackTier {
    Curated,
    Personal,
}

impl PackTier {
    pub fn as_str(self) -> &'static str {
        match self {
            PackTier::Curated => "curated",
            PackTier::Personal => "personal",
        }
    }
}

/// A pack's typed self-description (§1.2 manifest keys), all mandatory
/// unless noted. Plus two this project adds: `schema_version` (K1 already
/// writes it under this same key) and `vec_format_version` (the risk-note
/// field above).
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    pub pack_id: String,
    pub pack_version: String,
    pub pack_tier: PackTier,
    pub embedder_name: String,
    pub embedder_sha256: String,
    pub embedding_dims: u32,
    pub embedding_quant: String,
    pub chunk_target_tokens: u32,
    pub chunk_overlap_pct: u32,
    pub gate_abs_floor: f64,
    pub gate_rel_margin: f64,
    pub gate_calibrated: bool,
    pub prefixes_present: bool,
    pub built_by: String,
    /// Curated packs only; `None` for personal packs (and simply absent
    /// from the manifest table — there is no empty-string convention here).
    pub license_ref: Option<String>,
    pub schema_version: i64,
    pub vec_format_version: String,
}

impl Manifest {
    /// The mandatory key/value pairs this manifest serializes to (excludes
    /// `license_ref`, which `write` appends separately only when present).
    /// Shared by `write` and this module's tests (so a test simulating a
    /// missing key can write "every key except one" without duplicating
    /// this field/key mapping).
    fn mandatory_pairs(&self) -> Vec<(&'static str, String)> {
        vec![
            ("pack_id", self.pack_id.clone()),
            ("pack_version", self.pack_version.clone()),
            ("pack_tier", self.pack_tier.as_str().to_string()),
            ("embedder_name", self.embedder_name.clone()),
            ("embedder_sha256", self.embedder_sha256.clone()),
            ("embedding_dims", self.embedding_dims.to_string()),
            ("embedding_quant", self.embedding_quant.clone()),
            ("chunk_target_tokens", self.chunk_target_tokens.to_string()),
            ("chunk_overlap_pct", self.chunk_overlap_pct.to_string()),
            ("gate_abs_floor", self.gate_abs_floor.to_string()),
            ("gate_rel_margin", self.gate_rel_margin.to_string()),
            ("gate_calibrated", bool_str(self.gate_calibrated).to_string()),
            (
                "prefixes_present",
                bool_str(self.prefixes_present).to_string(),
            ),
            ("built_by", self.built_by.clone()),
            ("schema_version", self.schema_version.to_string()),
            ("vec_format_version", self.vec_format_version.clone()),
        ]
    }

    /// Upsert every manifest key into `pack` (each `manifest_set` call is
    /// itself an atomic upsert; see the module doc for why this isn't
    /// wrapped in one SQL transaction).
    pub fn write(&self, pack: &Pack) -> Result<(), Error> {
        for (key, value) in self.mandatory_pairs() {
            pack.manifest_set(key, &value)?;
        }
        if let Some(license_ref) = &self.license_ref {
            pack.manifest_set("license_ref", license_ref)?;
        }
        Ok(())
    }

    /// Read and validate every mandatory manifest key from `pack`. Fails on
    /// the first missing or malformed key with `Error::Schema("manifest:
    /// missing/invalid key <k>: <why>")`.
    pub fn read(pack: &Pack) -> Result<Manifest, Error> {
        Ok(Manifest {
            pack_id: require(pack, "pack_id")?,
            pack_version: require(pack, "pack_version")?,
            pack_tier: require_pack_tier(pack, "pack_tier")?,
            embedder_name: require(pack, "embedder_name")?,
            embedder_sha256: require(pack, "embedder_sha256")?,
            embedding_dims: require_u32(pack, "embedding_dims")?,
            embedding_quant: require(pack, "embedding_quant")?,
            chunk_target_tokens: require_u32(pack, "chunk_target_tokens")?,
            chunk_overlap_pct: require_u32(pack, "chunk_overlap_pct")?,
            gate_abs_floor: require_gate_threshold(pack, "gate_abs_floor")?,
            gate_rel_margin: require_gate_threshold(pack, "gate_rel_margin")?,
            gate_calibrated: require_bool(pack, "gate_calibrated")?,
            prefixes_present: require_bool(pack, "prefixes_present")?,
            built_by: require(pack, "built_by")?,
            license_ref: pack.manifest_get("license_ref")?,
            schema_version: require_i64(pack, "schema_version")?,
            vec_format_version: require(pack, "vec_format_version")?,
        })
    }
}

fn bool_str(b: bool) -> &'static str {
    if b {
        "true"
    } else {
        "false"
    }
}

/// Read a mandatory manifest key as a raw string, or a plain-language error
/// naming it. An empty string is treated the same as missing (an empty
/// `embedder_sha256` or `pack_id` must never read as valid).
fn require(pack: &Pack, key: &str) -> Result<String, Error> {
    let raw = pack.manifest_get(key)?.ok_or_else(|| {
        Error::Schema(format!("manifest: missing/invalid key {key}: not present"))
    })?;
    if raw.is_empty() {
        return Err(Error::Schema(format!(
            "manifest: mandatory key {key} is empty"
        )));
    }
    Ok(raw)
}

fn require_u32(pack: &Pack, key: &str) -> Result<u32, Error> {
    let raw = require(pack, key)?;
    raw.parse::<u32>().map_err(|e| {
        Error::Schema(format!(
            "manifest: missing/invalid key {key}: not a valid whole number ({e}, got {raw:?})"
        ))
    })
}

fn require_i64(pack: &Pack, key: &str) -> Result<i64, Error> {
    let raw = require(pack, key)?;
    raw.parse::<i64>().map_err(|e| {
        Error::Schema(format!(
            "manifest: missing/invalid key {key}: not a valid integer ({e}, got {raw:?})"
        ))
    })
}

fn require_f64(pack: &Pack, key: &str) -> Result<f64, Error> {
    let raw = require(pack, key)?;
    raw.parse::<f64>().map_err(|e| {
        Error::Schema(format!(
            "manifest: missing/invalid key {key}: not a valid number ({e}, got {raw:?})"
        ))
    })
}

/// Read a mandatory f64 that also feeds the §4 safety gate as a threshold
/// (`gate_abs_floor`, `gate_rel_margin`) — on top of `require_f64`'s parse,
/// reject non-finite (`NaN`/`inf`) values and negatives. A negative
/// `gate_abs_floor` in particular would fail OPEN in the safety gate, so
/// this is stricter than the generic f64 reader on purpose; other f64
/// manifest fields have no such constraint and keep using `require_f64`.
fn require_gate_threshold(pack: &Pack, key: &str) -> Result<f64, Error> {
    let value = require_f64(pack, key)?;
    if !value.is_finite() || value < 0.0 {
        return Err(Error::Schema(format!(
            "manifest: missing/invalid key {key}: must be a finite, non-negative number (got {value})"
        )));
    }
    Ok(value)
}

fn require_bool(pack: &Pack, key: &str) -> Result<bool, Error> {
    let raw = require(pack, key)?;
    match raw.as_str() {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(Error::Schema(format!(
            "manifest: missing/invalid key {key}: expected \"true\" or \"false\", got {raw:?}"
        ))),
    }
}

fn require_pack_tier(pack: &Pack, key: &str) -> Result<PackTier, Error> {
    let raw = require(pack, key)?;
    match raw.as_str() {
        "curated" => Ok(PackTier::Curated),
        "personal" => Ok(PackTier::Personal),
        _ => Err(Error::Schema(format!(
            "manifest: missing/invalid key {key}: expected \"curated\" or \"personal\", got {raw:?}"
        ))),
    }
}

/// What the wrapper (the Tauri app today; the future server pipeline too)
/// knows about this device/session at pack-mount time. K2's tests pass a
/// fixture for `available_embedder_sha256`; K4 wires it to the real set of
/// installed embedder GGUFs, hashed with the same convention as
/// `inference.rs`'s `model_sha256`.
///
/// `curator_key` is the pinned curator public key (K3), injected here
/// exactly like `available_embedder_sha256` above — the wrapper passes
/// `sign::curator_verifying_key()` in production (currently always `None`;
/// see that function's doc comment), and this crate's own tests pass a
/// fixed-seed test key. This is deliberate: the key is never read from the
/// pack itself (see the `sign` module doc comment for why a pack-supplied
/// key would be trivially forgeable).
pub struct LoadContext<'a> {
    pub available_embedder_sha256: &'a [String],
    pub curator_key: Option<VerifyingKey>,
}

/// The load-time integrity gate (§1.2) — the checks that must pass before
/// ANY query touches a mounted pack, run in order: embedder hash → dims →
/// schema version → vec format version. Each failure is fail-closed with a
/// plain-language reason (never a raw SQLite/parse error surfaced to the
/// user).
///
/// Takes `pack` in addition to `manifest` because the dims check needs the
/// pack's ACTUAL `vec0` column width via `Pack::vec_dims` — an independent
/// read of the real schema, not a comparison of the manifest against
/// itself.
///
/// Signature verification (curated packs only) is explicitly NOT here.
pub fn check_load(manifest: &Manifest, ctx: &LoadContext, pack: &Pack) -> Result<(), Error> {
    // 1. Embedder hash: the pack refuses to mount if this device lacks an
    // embedder with this exact hash (spec §1.2).
    if !ctx
        .available_embedder_sha256
        .iter()
        .any(|h| h.eq_ignore_ascii_case(&manifest.embedder_sha256))
    {
        let hash_prefix: String = manifest.embedder_sha256.chars().take(8).collect();
        return Err(Error::Schema(format!(
            "this pack needs embedder {} ({}…), which isn't installed",
            manifest.embedder_name, hash_prefix
        )));
    }

    // 2. Dims: the manifest's declared width must match the pack's actual
    // vec0 column width.
    let actual_dims = pack.vec_dims()?;
    if manifest.embedding_dims != actual_dims {
        return Err(Error::Schema(format!(
            "this pack's manifest declares {} embedding dimensions but its \
             vector table is built for {} — the pack is corrupt or was tampered with",
            manifest.embedding_dims, actual_dims
        )));
    }

    // 3. Schema version: v1 requires an exact match. Migrations don't exist
    // yet, so "older" isn't safe to accept either (that also let a tampered
    // `0`/negative schema_version through); distinguish the two directions
    // since the fix differs (update the app vs. rebuild the pack).
    if manifest.schema_version != SCHEMA_VERSION {
        let reason = if manifest.schema_version > SCHEMA_VERSION {
            format!(
                "this pack was built by a newer version of the app (pack schema {}, \
                 this build supports {}) — please update",
                manifest.schema_version, SCHEMA_VERSION
            )
        } else {
            format!(
                "this pack was built by an older schema (pack schema {}, this build \
                 supports {}) — rebuild the pack",
                manifest.schema_version, SCHEMA_VERSION
            )
        };
        return Err(Error::Schema(reason));
    }

    // 4. Vector format version: the on-disk `vec0` format is a
    // pack-compatibility surface exactly like the embedder hash (plan risk
    // note) — v1 requires an exact match. A future format-compatible
    // sqlite-vec bump would extend this to an accepted-set as a
    // coordinated, deliberate event, not silently accepted here.
    if manifest.vec_format_version != VEC_FORMAT_VERSION {
        return Err(Error::Schema(format!(
            "this pack uses vector format {} but this app builds/reads {} — update the app",
            manifest.vec_format_version, VEC_FORMAT_VERSION
        )));
    }

    // K3: curated-only ed25519 signature verification is enforced in
    // `Pack::mount`, immediately after this gate passes — not here. It
    // needs the file's path/bytes (the detached signature lives alongside
    // the `.kpack` file on disk, as `<path>.sig`), and this function is
    // deliberately a pure metadata gate over what's already inside the
    // pack. See `Pack::mount`'s doc comment for the check itself and its
    // residual note on origin-based tier enforcement (K9's job, not the
    // core's).

    Ok(())
}

impl Pack {
    /// Open a `.kpack`, read and validate its manifest, run the load-time
    /// integrity gate, and — for curated packs only — verify the detached
    /// ed25519 signature (K3) — the single entry point the Tauri `mount`
    /// command (K9) will call. Any failure at any stage (open, a
    /// missing/malformed manifest key, a gate check, or signature
    /// verification) refuses with a plain-language reason; nothing
    /// partially mounts.
    ///
    /// ## Curated-pack signature verification
    /// Runs AFTER `check_load`'s four metadata checks pass, because it
    /// needs the file `path` (`check_load` only ever sees the already-open
    /// `pack` and its parsed `manifest`):
    /// 1. `Curated` packs: refuse if `ctx.curator_key` is `None` (this
    ///    build can't verify curated packs yet — see `sign`'s module doc
    ///    comment for why that's the correct v1 state, fail-closed).
    ///    Otherwise, read the detached signature from `<path>.sig`
    ///    (refusing if it's missing) and the pack file's raw bytes, then
    ///    call `sign::verify_detached`; refuse on any error it returns.
    /// 2. `Personal` packs: skip signature verification entirely — they're
    ///    unsigned by design (spec §1.2).
    ///
    /// Residual: this function trusts whatever `manifest.pack_tier` says
    /// and only ever decides whether THAT tier's signature requirement is
    /// satisfied. It does NOT check that a curated pack actually came from
    /// the CDN, or that a personal pack actually came from the local pack
    /// directory — origin-based tier enforcement is the wrapper's job (K9),
    /// not the core's.
    pub fn mount<P: AsRef<Path>>(path: P, ctx: &LoadContext) -> Result<(Pack, Manifest), Error> {
        let path = path.as_ref();
        let pack = Pack::open(path)?;
        let manifest = Manifest::read(&pack)?;
        check_load(&manifest, ctx, &pack)?;

        if manifest.pack_tier == PackTier::Curated {
            let key = ctx.curator_key.ok_or_else(|| {
                Error::Schema("this build can't verify curated packs yet".to_string())
            })?;
            let sig_bytes = std::fs::read(sig_path_for(path)).map_err(|_| {
                Error::Schema("curated pack is missing its signature".to_string())
            })?;
            let pack_bytes = std::fs::read(path).map_err(|e| {
                Error::Schema(format!(
                    "could not read the pack file to verify its signature: {e}"
                ))
            })?;
            crate::sign::verify_detached(&pack_bytes, &sig_bytes, &key)?;
        }

        Ok((pack, manifest))
    }
}

/// `<path>.sig` — same path, `.sig` appended to the whole file name (so
/// `foo.kpack` gets `foo.kpack.sig`, not `foo.sig`). Mirrors
/// `src-tauri/src/cloud/download.rs`'s `part_path_for` convention.
fn sig_path_for(path: &Path) -> PathBuf {
    let mut os = path.as_os_str().to_os_string();
    os.push(".sig");
    PathBuf::from(os)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use std::path::PathBuf;

    /// Mirrors `format.rs`'s own test helper (this repo hand-rolls temp
    /// dirs instead of depending on `tempfile`); duplicated per-module since
    /// each module's `#[cfg(test)]` helper is private to it.
    fn unique_dir(name: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "cleophis-kpack-manifest-test-{}-{}-{}",
            std::process::id(),
            unique,
            name
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const TEST_DIMS: usize = 8;

    fn sample_manifest() -> Manifest {
        Manifest {
            pack_id: "neuro-foundations".to_string(),
            pack_version: "2026.07.1".to_string(),
            pack_tier: PackTier::Curated,
            embedder_name: "bge-base-en-v1.5-q8_0.gguf".to_string(),
            embedder_sha256: "ab3f".repeat(16),
            embedding_dims: TEST_DIMS as u32,
            embedding_quant: "int8".to_string(),
            chunk_target_tokens: 400,
            chunk_overlap_pct: 18,
            gate_abs_floor: 0.462,
            gate_rel_margin: 0.07,
            gate_calibrated: true,
            prefixes_present: true,
            built_by: "server-pipeline v1.4".to_string(),
            license_ref: Some("elsevier-2026-xyz".to_string()),
            schema_version: SCHEMA_VERSION,
            vec_format_version: VEC_FORMAT_VERSION.to_string(),
        }
    }

    /// Writes every mandatory key from `m` except `skip` (plus `license_ref`
    /// when present), for testing the missing-key path. Uses the same
    /// `mandatory_pairs` mapping `write` uses, so it can't silently drift
    /// from production key names.
    fn write_all_except(pack: &Pack, m: &Manifest, skip: &str) {
        for (key, value) in m.mandatory_pairs() {
            if key != skip {
                pack.manifest_set(key, &value).unwrap();
            }
        }
        if skip != "license_ref" {
            if let Some(license_ref) = &m.license_ref {
                pack.manifest_set("license_ref", license_ref).unwrap();
            }
        }
    }

    // 1. Round-trip: write a full valid Manifest, read it back equal.
    #[test]
    fn t1_manifest_roundtrip() {
        let dir = unique_dir("t1");
        let pack = Pack::open_or_create(dir.join("t1.kpack"), TEST_DIMS).unwrap();
        let m = sample_manifest();
        m.write(&pack).unwrap();
        let got = Manifest::read(&pack).unwrap();
        assert_eq!(got, m);
    }

    // 2. Missing mandatory key -> Error::Schema naming the key.
    #[test]
    fn t2_missing_mandatory_key_names_it() {
        let dir = unique_dir("t2");
        let pack = Pack::open_or_create(dir.join("t2.kpack"), TEST_DIMS).unwrap();
        write_all_except(&pack, &sample_manifest(), "built_by");

        let err = Manifest::read(&pack).unwrap_err();
        let msg = err.to_string();
        assert!(matches!(err, Error::Schema(_)));
        assert!(msg.contains("built_by"), "error should name the key: {msg}");
    }

    // 3. Malformed value (embedding_dims = "not-a-number") -> clear error.
    #[test]
    fn t3_malformed_embedding_dims_errors() {
        let dir = unique_dir("t3");
        let pack = Pack::open_or_create(dir.join("t3.kpack"), TEST_DIMS).unwrap();
        sample_manifest().write(&pack).unwrap();
        pack.manifest_set("embedding_dims", "not-a-number").unwrap();

        let err = Manifest::read(&pack).unwrap_err();
        assert!(err.to_string().contains("embedding_dims"));
    }

    // 3b. Malformed pack_tier -> clear error (same item, second bad-value
    // case, mirroring format.rs's t2/t2b sub-numbering convention).
    #[test]
    fn t3b_bad_pack_tier_errors() {
        let dir = unique_dir("t3b");
        let pack = Pack::open_or_create(dir.join("t3b.kpack"), TEST_DIMS).unwrap();
        sample_manifest().write(&pack).unwrap();
        pack.manifest_set("pack_tier", "bogus").unwrap();

        let err = Manifest::read(&pack).unwrap_err();
        assert!(err.to_string().contains("pack_tier"));
    }

    // 4. Gate happy path: valid manifest + matching embedder hash +
    // matching dims + current schema_version -> Ok.
    #[test]
    fn t4_gate_happy_path() {
        let dir = unique_dir("t4");
        let pack = Pack::open_or_create(dir.join("t4.kpack"), TEST_DIMS).unwrap();
        let m = sample_manifest();
        m.write(&pack).unwrap();

        let ctx = LoadContext {
            available_embedder_sha256: std::slice::from_ref(&m.embedder_sha256),
            curator_key: None,
        };
        check_load(&m, &ctx, &pack).unwrap();
    }

    // 5. Gate refuses: embedder hash NOT in available set -> refuse
    // (message mentions the embedder).
    #[test]
    fn t5_gate_refuses_missing_embedder_hash() {
        let dir = unique_dir("t5");
        let pack = Pack::open_or_create(dir.join("t5.kpack"), TEST_DIMS).unwrap();
        let m = sample_manifest();
        m.write(&pack).unwrap();

        let available = vec!["deadbeefdeadbeef".to_string()];
        let ctx = LoadContext {
            available_embedder_sha256: &available,
            curator_key: None,
        };
        let err = check_load(&m, &ctx, &pack).unwrap_err();
        assert!(matches!(err, Error::Schema(_)));
        assert!(err.to_string().contains(&m.embedder_name));
    }

    // 6. Gate refuses: dims mismatch -> refuse.
    #[test]
    fn t6_gate_refuses_dims_mismatch() {
        let dir = unique_dir("t6");
        let pack = Pack::open_or_create(dir.join("t6.kpack"), TEST_DIMS).unwrap();
        let mut m = sample_manifest();
        m.embedding_dims = 999; // pack's real vec0 width is TEST_DIMS (8)
        m.write(&pack).unwrap();

        let ctx = LoadContext {
            available_embedder_sha256: std::slice::from_ref(&m.embedder_sha256),
            curator_key: None,
        };
        let err = check_load(&m, &ctx, &pack).unwrap_err();
        assert!(matches!(err, Error::Schema(_)));
    }

    // 7. Gate refuses: schema_version newer than SCHEMA_VERSION -> refuse.
    #[test]
    fn t7_gate_refuses_newer_schema_version() {
        let dir = unique_dir("t7");
        let pack = Pack::open_or_create(dir.join("t7.kpack"), TEST_DIMS).unwrap();
        let mut m = sample_manifest();
        m.schema_version = SCHEMA_VERSION + 1;
        m.write(&pack).unwrap();

        let ctx = LoadContext {
            available_embedder_sha256: std::slice::from_ref(&m.embedder_sha256),
            curator_key: None,
        };
        let err = check_load(&m, &ctx, &pack).unwrap_err();
        assert!(matches!(err, Error::Schema(_)));
        assert!(err.to_string().contains("newer"));
    }

    // 8. Pack::mount end-to-end: a fully-built valid pack mounts; a
    // tampered one (drop a key) refuses. Uses a Personal-tier manifest
    // deliberately — this test is about the metadata gate chaining
    // correctly through `mount` (open -> read -> check_load), not about
    // signatures (K3's curated-only signature tests live below, t15+).
    #[test]
    fn t8_mount_end_to_end() {
        let dir = unique_dir("t8");
        let mut m = sample_manifest();
        m.pack_tier = PackTier::Personal;
        m.license_ref = None;

        let good_path = dir.join("good.kpack");
        {
            let pack = Pack::open_or_create(&good_path, TEST_DIMS).unwrap();
            m.write(&pack).unwrap();
        }
        let ctx = LoadContext {
            available_embedder_sha256: std::slice::from_ref(&m.embedder_sha256),
            curator_key: None,
        };
        let (_pack, mounted) = Pack::mount(&good_path, &ctx).unwrap();
        assert_eq!(mounted, m);

        let bad_path = dir.join("bad.kpack");
        {
            let pack = Pack::open_or_create(&bad_path, TEST_DIMS).unwrap();
            write_all_except(&pack, &m, "built_by");
        }
        let result = Pack::mount(&bad_path, &ctx);
        assert!(result.is_err());
    }

    // 9. Gate happy path: embedder hash match is case-insensitive (the
    // available set has the hash in a different case than the manifest) ->
    // still passes. This is the security-relevant branch (t5's mirror) and
    // was previously untested.
    #[test]
    fn t9_gate_embedder_hash_match_is_case_insensitive() {
        let dir = unique_dir("t9");
        let pack = Pack::open_or_create(dir.join("t9.kpack"), TEST_DIMS).unwrap();
        let m = sample_manifest();
        m.write(&pack).unwrap();

        let available = vec![m.embedder_sha256.to_uppercase()];
        let ctx = LoadContext {
            available_embedder_sha256: &available,
            curator_key: None,
        };
        check_load(&m, &ctx, &pack).unwrap();
    }

    // 10. Gate refuses: an empty available_embedder_sha256 set can never
    // match -> refuse (not a panic, not an accidental pass).
    #[test]
    fn t10_gate_refuses_empty_available_embedder_set() {
        let dir = unique_dir("t10");
        let pack = Pack::open_or_create(dir.join("t10.kpack"), TEST_DIMS).unwrap();
        let m = sample_manifest();
        m.write(&pack).unwrap();

        let available: Vec<String> = Vec::new();
        let ctx = LoadContext {
            available_embedder_sha256: &available,
            curator_key: None,
        };
        let err = check_load(&m, &ctx, &pack).unwrap_err();
        assert!(matches!(err, Error::Schema(_)));
    }

    // 11. license_ref = None round-trips: write a manifest with no license
    // (personal pack), read it back, still None.
    #[test]
    fn t11_license_ref_none_roundtrips() {
        let dir = unique_dir("t11");
        let pack = Pack::open_or_create(dir.join("t11.kpack"), TEST_DIMS).unwrap();
        let mut m = sample_manifest();
        m.pack_tier = PackTier::Personal;
        m.license_ref = None;
        m.write(&pack).unwrap();

        let got = Manifest::read(&pack).unwrap();
        assert_eq!(got.license_ref, None);
        assert_eq!(got, m);
    }

    // 12. Gate refuses: vec_format_version mismatch -> refuse (Fix 1: the
    // previously-inert compatibility field).
    #[test]
    fn t12_gate_refuses_vec_format_version_mismatch() {
        let dir = unique_dir("t12");
        let pack = Pack::open_or_create(dir.join("t12.kpack"), TEST_DIMS).unwrap();
        let mut m = sample_manifest();
        m.vec_format_version = "0.0.1-not-what-we-build".to_string();
        m.write(&pack).unwrap();

        let ctx = LoadContext {
            available_embedder_sha256: std::slice::from_ref(&m.embedder_sha256),
            curator_key: None,
        };
        let err = check_load(&m, &ctx, &pack).unwrap_err();
        assert!(matches!(err, Error::Schema(_)));
        assert!(err.to_string().contains("vector format"));
    }

    // 13. Empty mandatory string (pack_id="") reads as missing, not valid.
    #[test]
    fn t13_empty_mandatory_string_refuses() {
        let dir = unique_dir("t13");
        let pack = Pack::open_or_create(dir.join("t13.kpack"), TEST_DIMS).unwrap();
        sample_manifest().write(&pack).unwrap();
        pack.manifest_set("pack_id", "").unwrap();

        let err = Manifest::read(&pack).unwrap_err();
        assert!(matches!(err, Error::Schema(_)));
        let msg = err.to_string();
        assert!(msg.contains("pack_id"), "error should name the key: {msg}");
    }

    // 14. Negative gate_abs_floor refuses: a negative absolute floor would
    // fail OPEN in the §4 safety gate, so it must never parse as valid.
    #[test]
    fn t14_negative_gate_abs_floor_refuses() {
        let dir = unique_dir("t14");
        let pack = Pack::open_or_create(dir.join("t14.kpack"), TEST_DIMS).unwrap();
        sample_manifest().write(&pack).unwrap();
        pack.manifest_set("gate_abs_floor", "-0.1").unwrap();

        let err = Manifest::read(&pack).unwrap_err();
        assert!(matches!(err, Error::Schema(_)));
        let msg = err.to_string();
        assert!(
            msg.contains("gate_abs_floor"),
            "error should name the key: {msg}"
        );
    }

    // ---- K3: curated-pack signature verification, exercised end-to-end
    // through Pack::mount (unit-level verify_detached coverage lives in
    // sign.rs's own test module). ----

    /// Fixed-seed test curator keypairs, constructed here inside
    /// `#[cfg(test)]` — never at crate scope (mirrors `sign`'s own
    /// test-key discipline; see that module's doc comment and its t9
    /// guardrail test).
    fn test_curator_signing_key() -> SigningKey {
        SigningKey::from_bytes(&[42u8; 32])
    }

    fn other_curator_signing_key() -> SigningKey {
        SigningKey::from_bytes(&[99u8; 32])
    }

    /// Signs the `.kpack` file already written at `path` with `sk` and
    /// writes the detached signature to `<path>.sig` — the exact layout
    /// `Pack::mount` reads back.
    fn sign_pack_file(path: &Path, sk: &SigningKey) {
        let bytes = std::fs::read(path).unwrap();
        let sig = sk.sign(&bytes);
        std::fs::write(sig_path_for(path), sig.to_bytes()).unwrap();
    }

    // 15. Curated pack + valid signature + correct curator key -> mounts OK.
    #[test]
    fn t15_mount_curated_pack_valid_signature_succeeds() {
        let dir = unique_dir("t15");
        let m = sample_manifest(); // Curated by default
        let path = dir.join("curated.kpack");
        {
            let pack = Pack::open_or_create(&path, TEST_DIMS).unwrap();
            m.write(&pack).unwrap();
        }
        let sk = test_curator_signing_key();
        sign_pack_file(&path, &sk);

        let ctx = LoadContext {
            available_embedder_sha256: std::slice::from_ref(&m.embedder_sha256),
            curator_key: Some(sk.verifying_key()),
        };
        let (_pack, mounted) = Pack::mount(&path, &ctx).unwrap();
        assert_eq!(mounted, m);
    }

    // 16. Curated pack signed by a DIFFERENT key than ctx.curator_key ->
    // refuses.
    #[test]
    fn t16_mount_curated_pack_wrong_signing_key_refuses() {
        let dir = unique_dir("t16");
        let m = sample_manifest();
        let path = dir.join("curated.kpack");
        {
            let pack = Pack::open_or_create(&path, TEST_DIMS).unwrap();
            m.write(&pack).unwrap();
        }
        sign_pack_file(&path, &other_curator_signing_key());

        let ctx = LoadContext {
            available_embedder_sha256: std::slice::from_ref(&m.embedder_sha256),
            curator_key: Some(test_curator_signing_key().verifying_key()),
        };
        // `.map(|_| ())` sidesteps `unwrap_err`'s `T: Debug` bound — `Pack`
        // (holds a `rusqlite::Connection`, which isn't `Debug`) is the Ok
        // side of this Result, but we only need the Err side here.
        let err = Pack::mount(&path, &ctx).map(|_| ()).unwrap_err();
        assert!(matches!(err, Error::Schema(_)));
    }

    // 17. Truncated (63 bytes) and oversized (65 bytes) .sig file -> refuse
    // with the malformed message, no panic on the short/long slice.
    #[test]
    fn t17_mount_curated_pack_wrong_length_signature_refuses_malformed() {
        let dir = unique_dir("t17");
        let m = sample_manifest();
        let sk = test_curator_signing_key();
        let ctx = LoadContext {
            available_embedder_sha256: std::slice::from_ref(&m.embedder_sha256),
            curator_key: Some(sk.verifying_key()),
        };

        let short_path = dir.join("short.kpack");
        {
            let pack = Pack::open_or_create(&short_path, TEST_DIMS).unwrap();
            m.write(&pack).unwrap();
        }
        sign_pack_file(&short_path, &sk);
        let sig_bytes = std::fs::read(sig_path_for(&short_path)).unwrap();
        std::fs::write(sig_path_for(&short_path), &sig_bytes[..63]).unwrap();
        let err = Pack::mount(&short_path, &ctx).map(|_| ()).unwrap_err();
        assert!(matches!(err, Error::Schema(_)));
        assert!(err.to_string().contains("malformed"), "error was: {err}");

        let long_path = dir.join("long.kpack");
        {
            let pack = Pack::open_or_create(&long_path, TEST_DIMS).unwrap();
            m.write(&pack).unwrap();
        }
        sign_pack_file(&long_path, &sk);
        let mut long_sig = std::fs::read(sig_path_for(&long_path)).unwrap();
        long_sig.push(0);
        std::fs::write(sig_path_for(&long_path), &long_sig).unwrap();
        let err = Pack::mount(&long_path, &ctx).map(|_| ()).unwrap_err();
        assert!(matches!(err, Error::Schema(_)));
        assert!(err.to_string().contains("malformed"), "error was: {err}");
    }

    // 18. Tampered pack bytes after signing (rewrite one manifest value
    // through the normal Pack API, keeping the now-stale old signature) ->
    // refuses. This is the core guarantee, exercised end-to-end through
    // Pack::mount (sign.rs's own t4 covers verify_detached directly).
    // Tampers via `manifest_set` rather than flipping a raw byte in the
    // file so the SQLite file stays structurally valid — this isolates
    // "the signature no longer matches the bytes" from "corrupt SQLite
    // file", which would fail for an unrelated reason.
    #[test]
    fn t18_mount_curated_pack_tampered_bytes_refuses() {
        let dir = unique_dir("t18");
        let m = sample_manifest();
        let path = dir.join("curated.kpack");
        {
            let pack = Pack::open_or_create(&path, TEST_DIMS).unwrap();
            m.write(&pack).unwrap();
        }
        let sk = test_curator_signing_key();
        sign_pack_file(&path, &sk);

        {
            let pack = Pack::open(&path).unwrap();
            pack.manifest_set("built_by", "attacker-modified-pipeline")
                .unwrap();
        }

        let ctx = LoadContext {
            available_embedder_sha256: std::slice::from_ref(&m.embedder_sha256),
            curator_key: Some(sk.verifying_key()),
        };
        // `.map(|_| ())` sidesteps `unwrap_err`'s `T: Debug` bound — `Pack`
        // (holds a `rusqlite::Connection`, which isn't `Debug`) is the Ok
        // side of this Result, but we only need the Err side here.
        let err = Pack::mount(&path, &ctx).map(|_| ()).unwrap_err();
        assert!(matches!(err, Error::Schema(_)));
        assert!(
            err.to_string().contains("does not verify"),
            "expected a signature-verification refusal, got: {err}"
        );
    }

    // 19. Curated pack with NO .sig file -> refuses ("missing its
    // signature").
    #[test]
    fn t19_mount_curated_pack_missing_sig_file_refuses() {
        let dir = unique_dir("t19");
        let m = sample_manifest();
        let path = dir.join("curated.kpack");
        {
            let pack = Pack::open_or_create(&path, TEST_DIMS).unwrap();
            m.write(&pack).unwrap();
        }
        // Deliberately no .sig file written.

        let ctx = LoadContext {
            available_embedder_sha256: std::slice::from_ref(&m.embedder_sha256),
            curator_key: Some(test_curator_signing_key().verifying_key()),
        };
        // `.map(|_| ())` sidesteps `unwrap_err`'s `T: Debug` bound — `Pack`
        // (holds a `rusqlite::Connection`, which isn't `Debug`) is the Ok
        // side of this Result, but we only need the Err side here.
        let err = Pack::mount(&path, &ctx).map(|_| ()).unwrap_err();
        assert!(matches!(err, Error::Schema(_)));
        assert!(
            err.to_string().contains("missing its signature"),
            "error was: {err}"
        );
    }

    // 20. Curated pack but ctx.curator_key = None -> refuses ("can't
    // verify curated packs yet").
    #[test]
    fn t20_mount_curated_pack_no_curator_key_refuses() {
        let dir = unique_dir("t20");
        let m = sample_manifest();
        let path = dir.join("curated.kpack");
        {
            let pack = Pack::open_or_create(&path, TEST_DIMS).unwrap();
            m.write(&pack).unwrap();
        }
        sign_pack_file(&path, &test_curator_signing_key());

        let ctx = LoadContext {
            available_embedder_sha256: std::slice::from_ref(&m.embedder_sha256),
            curator_key: None,
        };
        // `.map(|_| ())` sidesteps `unwrap_err`'s `T: Debug` bound — `Pack`
        // (holds a `rusqlite::Connection`, which isn't `Debug`) is the Ok
        // side of this Result, but we only need the Err side here.
        let err = Pack::mount(&path, &ctx).map(|_| ()).unwrap_err();
        assert!(matches!(err, Error::Schema(_)));
        assert!(
            err.to_string().contains("can't verify curated packs yet"),
            "error was: {err}"
        );
    }

    // 21. Personal pack with no signature and curator_key = None -> mounts
    // OK (personal packs skip signature verification entirely).
    #[test]
    fn t21_mount_personal_pack_skips_signature_verification() {
        let dir = unique_dir("t21");
        let mut m = sample_manifest();
        m.pack_tier = PackTier::Personal;
        m.license_ref = None;
        let path = dir.join("personal.kpack");
        {
            let pack = Pack::open_or_create(&path, TEST_DIMS).unwrap();
            m.write(&pack).unwrap();
        }
        // Deliberately no .sig file, and no curator key available.

        let ctx = LoadContext {
            available_embedder_sha256: std::slice::from_ref(&m.embedder_sha256),
            curator_key: None,
        };
        let (_pack, mounted) = Pack::mount(&path, &ctx).unwrap();
        assert_eq!(mounted, m);
    }
}
