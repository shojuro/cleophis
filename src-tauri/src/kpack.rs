//! Tauri command seam for the on-device knowledge-pack core (K9): thin
//! `#[tauri::command]` wrappers proving the app can drive
//! `kpack_core::{build_pack, Pack::mount}` with the real, linked
//! `kpack_embed::BgeEmbedder` — not the crate's mock. This is app-
//! integration glue only: the full builder/retrieval UX (§3/§4 of the RAG
//! spec) is a later milestone, and this module adds no webview UI.
//!
//! Each command's body is a thin `spawn_blocking` shim (mirrors
//! `cloud::commands`' idiom) around a pure, `AppHandle`-free inner function
//! (`mount_pack_at` / `build_personal_pack_with_embedder`) — so the
//! `#[ignore]`d integration test at the bottom of this file can exercise the
//! real pipeline without constructing a Tauri `AppHandle`.

use std::path::{Path, PathBuf};

use kpack_core::{
    build_pack, BuildMeta, ChunkConfig, LoadContext, Manifest, Pack, PackTier, SourceInput,
};
use kpack_embed::BgeEmbedder;
use serde::Serialize;
use tauri::AppHandle;

/// Only reachable if the blocking task itself panics or the runtime is
/// shutting down — mirrors `cloud::commands::JOIN_ERROR_MESSAGE`.
const JOIN_ERROR_MESSAGE: &str = "Something went wrong on this device. Please try again.";

/// Pinned sha256 of the bundled `bge-base-en-v1.5` Q8_0 GGUF
/// (`resources/embedders/bge-base-en-v1.5-q8_0.gguf`) — the exact digest
/// `tools/fetch-embedder.mjs` recorded when it fetched that file (K4b).
/// Used two ways: as `LoadContext.available_embedder_sha256` on mount (a
/// pack whose manifest declares a different embedder hash refuses to mount
/// — `manifest.rs`'s load-time gate), and as `BuildMeta.embedder_sha256` on
/// build, so a pack this device builds always mounts back on this same app.
/// Pinned as a const, not re-hashed off disk at call time — this mirrors
/// `inference.rs`'s catalog-pinned-hash convention for the hero model,
/// scaled down: there is exactly one bundled embedder file, so there is no
/// "which catalog entry" lookup to do.
const EMBEDDER_SHA256: &str = "ad1afe72cd6654a558667a3db10878b049a75bfd72912e1dabb91310d671173c";

/// Display name recorded in a built pack's manifest (`Manifest::embedder_name`),
/// stored as-is — never derived from the GGUF file itself (see
/// `BuildMeta::embedder_name`'s doc comment in `kpack-core`).
const EMBEDDER_NAME: &str = "bge-base-en-v1.5-q8_0";

/// Where the bundled embedder GGUF ships, relative to `resources_root`.
/// Mirrors `inference.rs::model_path`'s resources-relative layout
/// (`resources/models/...`) one directory over (`resources/embedders/...`).
const EMBEDDER_RELATIVE_PATH: &str = "embedders/bge-base-en-v1.5-q8_0.gguf";

/// Resolve the bundled embedder GGUF's path: dev vs. prod resource root is
/// `inference::resources_root`'s job already (dev: `src-tauri/resources`;
/// prod: the install's resource dir) — this just joins the embedder's fixed
/// relative path onto it. Unlike the hero LLM (`inference::model_path`),
/// there is no app-data/downloaded-copy fallback: the embedder ships with
/// every install (fat and thin alike), it is never separately downloaded.
fn bundled_embedder_path(app: &AppHandle) -> PathBuf {
    crate::inference::resources_root(app).join(EMBEDDER_RELATIVE_PATH)
}

/// A camelCase, front-end-facing view of `kpack_core::Manifest` — the exact
/// fields a pack picker / builder UI needs (pack identity, embedder
/// provenance, chunk config, and the safety-gate/calibration flags), without
/// exposing the core crate's types directly across the Tauri IPC boundary.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackManifestInfo {
    pub pack_id: String,
    pub pack_version: String,
    /// `"curated"` or `"personal"` (`PackTier::as_str()`).
    pub pack_tier: String,
    pub embedder_name: String,
    pub embedder_sha256: String,
    pub embedding_dims: u32,
    pub embedding_quant: String,
    pub chunk_target_tokens: u32,
    pub chunk_overlap_pct: u32,
    pub gate_calibrated: bool,
    pub prefixes_present: bool,
    pub built_by: String,
    pub license_ref: Option<String>,
}

impl From<Manifest> for PackManifestInfo {
    fn from(m: Manifest) -> Self {
        PackManifestInfo {
            pack_id: m.pack_id,
            pack_version: m.pack_version,
            pack_tier: m.pack_tier.as_str().to_string(),
            embedder_name: m.embedder_name,
            embedder_sha256: m.embedder_sha256,
            embedding_dims: m.embedding_dims,
            embedding_quant: m.embedding_quant,
            chunk_target_tokens: m.chunk_target_tokens,
            chunk_overlap_pct: m.chunk_overlap_pct,
            gate_calibrated: m.gate_calibrated,
            prefixes_present: m.prefixes_present,
            built_by: m.built_by,
            license_ref: m.license_ref,
        }
    }
}

/// Pure mount logic behind the `mount_pack` command: mount the `.kpack` at
/// `path` against this device's pinned embedder hash and return its
/// manifest. The core's `Error` values are already user-safe plain-language
/// (`manifest.rs`'s module doc comment), so `.to_string()` is the whole
/// error-mapping story — no re-wording needed here.
fn mount_pack_at(path: &Path) -> Result<PackManifestInfo, String> {
    let available = vec![EMBEDDER_SHA256.to_string()];
    let ctx = LoadContext {
        available_embedder_sha256: &available,
        // Sourced from the pinned curator key, not hardcoded None — returns
        // None today (no key pinned yet), but when §2.6 pins CURATOR_PUBLIC_KEY
        // this app picks up curated-pack verification automatically, with no
        // forgotten swap here.
        curator_key: kpack_core::sign::curator_verifying_key(),
    };
    let (_pack, manifest) = Pack::mount(path, &ctx).map_err(|e| e.to_string())?;
    Ok(manifest.into())
}

#[tauri::command]
pub async fn mount_pack(path: String, _app: AppHandle) -> Result<PackManifestInfo, String> {
    tauri::async_runtime::spawn_blocking(move || mount_pack_at(Path::new(&path)))
        .await
        .map_err(|_| JOIN_ERROR_MESSAGE.to_string())?
}

/// Derive a `SourceInput::source_type` from `path`'s extension:
/// `.md`/`.markdown` and `.txt` map to `kpack_core::parse::parse`'s two
/// recognized dispatch strings (`"md"`/`"txt"` — `parse` itself treats `"md"`
/// and `"markdown"` as synonyms, so both normalize to `"md"` here). Anything
/// else is unsupported for v1 (PDF/EPUB/DOCX/HTML are later milestones, per
/// `parse.rs`'s module doc comment) and returns a clear error rather than
/// silently feeding binary bytes through the plain-text parser as `parse`
/// itself would if simply handed an unrecognized type — a user picking an
/// unsupported file should see a refusal, not a garbled "personal pack".
fn source_type_for(path: &Path) -> Result<String, String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("md") | Some("markdown") => Ok("md".to_string()),
        Some("txt") => Ok("txt".to_string()),
        _ => Err(format!(
            "unsupported file type: {} (only .md, .markdown, and .txt are supported)",
            path.display()
        )),
    }
}

/// A "uuid-ish" (not RFC 4122 — no `uuid` crate in this workspace, and none
/// is needed for a value that only has to be unique across one device's own
/// personal-pack builds) pack id: this process's PID plus a nanosecond
/// timestamp.
fn generate_pack_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("personal-{}-{nanos}", std::process::id())
}

/// A monotonic-enough `pack_version` for a personal build: whole seconds
/// since the Unix epoch, decimal. `build.rs`'s own RFC 3339 formatter
/// (`rfc3339_now`) is private to that module, so this is a deliberately
/// simpler stand-in — good enough for "which of my builds is newer",
/// nothing here reads it as a display-formatted date.
fn build_timestamp() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string()
}

/// Pure build logic behind the `build_personal_pack` command: load
/// `gguf_path` as a real `BgeEmbedder`, read and type every file in
/// `file_paths`, build a `Personal`-tier pack at `out_path`, then mount it
/// back (proving the round-trip, not just that the build call returned Ok)
/// and hand back its manifest. Factored out (no `AppHandle`) so the
/// `#[ignore]`d integration test below can call it directly against a temp
/// file and the bundled GGUF.
fn build_personal_pack_with_embedder(
    file_paths: &[String],
    out_path: &Path,
    gguf_path: &Path,
) -> Result<PackManifestInfo, String> {
    if file_paths.is_empty() {
        return Err("no source files given".to_string());
    }

    let embedder = BgeEmbedder::new(gguf_path).map_err(|e| e.to_string())?;

    let mut sources = Vec::with_capacity(file_paths.len());
    for file_path in file_paths {
        let path = Path::new(file_path);
        let source_type = source_type_for(path)?;
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("couldn't read {}: {e}", path.display()))?;
        let title = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| file_path.clone());
        sources.push(SourceInput {
            content,
            title,
            source_type,
        });
    }

    let meta = BuildMeta {
        pack_id: generate_pack_id(),
        pack_version: build_timestamp(),
        pack_tier: PackTier::Personal,
        embedder_name: EMBEDDER_NAME.to_string(),
        embedder_sha256: EMBEDDER_SHA256.to_string(),
        built_by: "device-builder v1".to_string(),
    };

    build_pack(&sources, &embedder, &meta, out_path, &ChunkConfig::default())
        .map_err(|e| e.to_string())?;

    mount_pack_at(out_path)
}

#[tauri::command]
pub async fn build_personal_pack(
    file_paths: Vec<String>,
    out_path: String,
    app: AppHandle,
) -> Result<PackManifestInfo, String> {
    let gguf_path = bundled_embedder_path(&app);
    tauri::async_runtime::spawn_blocking(move || {
        build_personal_pack_with_embedder(&file_paths, Path::new(&out_path), &gguf_path)
    })
    .await
    .map_err(|_| JOIN_ERROR_MESSAGE.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_type_for_maps_known_extensions_and_rejects_the_rest() {
        assert_eq!(source_type_for(Path::new("a.md")).unwrap(), "md");
        assert_eq!(source_type_for(Path::new("a.MARKDOWN")).unwrap(), "md");
        assert_eq!(source_type_for(Path::new("a.txt")).unwrap(), "txt");
        assert!(source_type_for(Path::new("a.pdf")).is_err());
        assert!(source_type_for(Path::new("no-extension")).is_err());
    }

    #[test]
    fn generate_pack_id_and_build_timestamp_are_non_empty() {
        // Not a determinism/format lock — just guards against an empty or
        // panicking implementation; the real interesting property (real
        // build -> mount round-trip) is t9_build_and_mount below.
        assert!(!generate_pack_id().is_empty());
        assert!(!build_timestamp().is_empty());
    }

    fn unique_dir(name: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "cleophis-kpack-k9-test-{}-{}-{}",
            std::process::id(),
            unique,
            name
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// End-to-end proof (K9 §"Test / prove"): build a `Personal`-tier pack
    /// from a temp `.md` file via the REAL `bge-base-en-v1.5` embedder,
    /// mount it back, and assert the manifest round-trips (`pack_tier`
    /// `"personal"`, `embedding_dims` 768 — `kpack_embed::bge::BGE_DIMS`).
    ///
    /// `#[ignore]`d: needs the bundled GGUF on disk
    /// (`src-tauri/resources/embedders/bge-base-en-v1.5-q8_0.gguf`, fetched
    /// by `tools/fetch-embedder.mjs`) and this crate's `real`-feature
    /// native build (libclang + cmake) — neither of which a routine
    /// `cargo test` should require. Run explicitly with
    /// `$env:LIBCLANG_PATH = "C:\Program Files\LLVM\bin"; cargo test -p cleophis --lib -- --ignored kpack::tests::build_and_mount_round_trips_with_real_embedder`.
    #[test]
    #[ignore]
    fn build_and_mount_round_trips_with_real_embedder() {
        let gguf_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources").join(EMBEDDER_RELATIVE_PATH);
        assert!(
            gguf_path.exists(),
            "bundled embedder GGUF missing at {} — run tools/fetch-embedder.mjs first",
            gguf_path.display()
        );

        let dir = unique_dir("roundtrip");
        let md_path = dir.join("source.md");
        std::fs::write(
            &md_path,
            "# Vitamin K\n\nVitamin K is a fat-soluble vitamin involved in blood clotting. \
             It also plays a role in bone metabolism.\n",
        )
        .unwrap();

        let out_path = dir.join("personal.kpack");
        let manifest = build_personal_pack_with_embedder(
            &[md_path.to_string_lossy().into_owned()],
            &out_path,
            &gguf_path,
        )
        .expect("build_personal_pack_with_embedder should succeed against the real embedder");

        assert_eq!(manifest.pack_tier, "personal");
        assert_eq!(manifest.embedding_dims, 768);
        assert_eq!(manifest.embedder_sha256, EMBEDDER_SHA256);
        assert_eq!(manifest.embedder_name, EMBEDDER_NAME);
        assert!(!manifest.gate_calibrated);
        assert!(!manifest.prefixes_present);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Same real-embedder gate as above: an unsupported file extension is
    /// refused with a clear error before the embedder or the pack builder
    /// ever runs — proven with the real `BgeEmbedder::new` in the loop (not
    /// just a pure-Rust unit check) so this exercises the exact refusal
    /// path `build_personal_pack` takes in production.
    #[test]
    #[ignore]
    fn unsupported_file_type_is_refused_before_building() {
        let gguf_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources").join(EMBEDDER_RELATIVE_PATH);
        let dir = unique_dir("unsupported");
        let bad_path = dir.join("source.pdf");
        std::fs::write(&bad_path, b"%PDF-not-really").unwrap();

        let out_path = dir.join("personal.kpack");
        let err = build_personal_pack_with_embedder(
            &[bad_path.to_string_lossy().into_owned()],
            &out_path,
            &gguf_path,
        )
        .unwrap_err();
        assert!(err.contains("unsupported file type"), "error was: {err}");
        assert!(!out_path.exists(), "no pack should be written on refusal");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
