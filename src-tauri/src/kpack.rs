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
//!
//! ## Cached embedder + build progress/cancel (spec §3a A1)
//! `BgeEmbedder::new` loads a 118 MB GGUF via `llama.cpp` — reloading it on
//! every `rag_query`/`build_personal_pack` call (K9's original behavior)
//! would make grounded chat unusable. [`EmbedderCache`] loads it once,
//! lazily, behind a `Mutex`-guarded `Option<Arc<BgeEmbedder>>`; every
//! subsequent call clones the `Arc` (cheap — `BgeEmbedder` wraps a
//! `Send + Sync` `LlamaModel` and makes a fresh context per embed call, so a
//! shared `Arc` is safe for concurrent embeds). It's managed directly
//! (`.manage(EmbedderCache::default())` in `main.rs`, not wrapped in an
//! outer `Arc`) — a command needing it inside a `spawn_blocking` closure
//! clones the `AppHandle` (already `Clone + Send + 'static`) into the
//! closure and re-fetches `app.state::<EmbedderCache>()` there, the same
//! `main.rs:114/117` idiom this app already uses for `Arc<Engine>`/
//! `Arc<Downloads>` from `on_window_event`; that avoids requiring
//! `EmbedderCache: Clone` just to cross the `spawn_blocking` boundary.
//!
//! [`Builds`] is `build_personal_pack`'s single-slot active-build registry
//! (an `Arc<AtomicBool>` cancel flag, cleared by a `ClearActiveOnDrop`-style
//! guard), mirroring `cloud::download::Downloads`. Unlike a download, a
//! build's command doesn't return until the build finishes (no detached
//! worker thread), so the guard lives in the command's own async body
//! rather than a spawned thread's.
//!
//! ## App-data pack store (spec §3a A2)
//! Personal packs no longer land wherever the caller names: `build_personal_pack`
//! now computes its own out-path under [`packs_dir`], an app-data `packs/`
//! directory it owns. [`list_packs`]/[`delete_pack`] are the read/delete
//! halves of that store — both `spawn_blocking` around pure, `AppHandle`-free
//! inner functions (`list_packs_in`/`delete_pack_in`, mirroring this file's
//! existing `*_at`/`*_with_embedder` pattern) so they're unit-testable
//! without a real Tauri app. `delete_pack_in`'s path-safety check (now
//! [`resolve_pack_in_dir`], shared with `rag_query`/`mount_pack` below) is
//! what keeps a path from the front end from ever reaching anything outside
//! that one managed directory.
//!
//! ## Per-account pack isolation (spec §3a A6 — SECURITY)
//! `packs_dir` is scoped to `packs/<account>/`, where `<account>` is the
//! AUTHORITATIVE current user id read from `Cloud::current_user_id()` — the
//! Rust session state, never the front end. Signed-out has no pack store
//! (`Err`); the Packs UI is already sign-in-gated, this is the server-side
//! backstop. Because `build_personal_pack`/`list_packs`/`delete_pack` all
//! resolve through this one function, scoping it here scopes all three at
//! once — WITHOUT this fix, any signed-in account on a shared device could
//! see and query every other account's packs, which is exactly the bug this
//! closes. `resolve_pack_in_dir`'s parent-must-equal-the-managed-dir check
//! is what turns that scoping into an enforced boundary on the READ path
//! too (`rag_query`, `mount_pack`): a crafted call naming another account's
//! pack path is a hard `Err`, not a silent skip — a path resolving into a
//! different account's subdirectory has a different canonical parent and is
//! refused exactly like an outside-the-store path.
//!
//! Honest scope boundary: this isolates pack VISIBILITY and in-app ACCESS
//! per cloud account, under the same OS user/device-data directory. Pack
//! files remain plaintext under that OS user's app-data dir — a DIFFERENT OS
//! user, or raw filesystem access, is a separate and deeper boundary (OS
//! ACLs / at-rest encryption) this does NOT provide; a possible follow-up,
//! not oversold here as at-rest isolation.
//!
//! Migration note: packs built before this fix sit flat in `packs/*.kpack`
//! and are now invisible (`list_packs` only reads `packs/<account>/`).
//! They are NOT auto-migrated to whichever account signs in first — ownership
//! of a pre-fix pack is unknown, and guessing would recreate the exact leak
//! this closes. They become inert; rebuild them per account.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use kpack_core::retrieve::{retrieve, Citation, RetrievalResult, Tier};
use kpack_core::{
    build_pack_with_progress, BuildMeta, BuildProgress, ChunkConfig, LoadContext, Manifest, Pack,
    PackTier, SourceContent, SourceInput,
};
use kpack_embed::BgeEmbedder;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::cloud::session::Cloud;

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

/// Where the bundled pdfium dynamic library ships, relative to
/// `resources_root` (§3b B3) — mirrors `EMBEDDER_RELATIVE_PATH` one
/// directory over (`resources/pdfium/...`).
const PDFIUM_RELATIVE_PATH: &str = "pdfium/pdfium.dll";

/// Resolve the bundled pdfium dynamic library's path — an exact mirror of
/// `bundled_embedder_path` above, just for `kpack_pdf::extract_pages`'s
/// `pdfium_lib_path` param instead of the embedder's GGUF. Same dev/prod
/// resource-root resolution via `inference::resources_root`, same "ships
/// with every install, never separately downloaded" story as the embedder.
fn bundled_pdfium_path(app: &AppHandle) -> PathBuf {
    crate::inference::resources_root(app).join(PDFIUM_RELATIVE_PATH)
}

/// Accepts `user_id` unchanged as the account's directory segment if — and
/// only if — it's ALREADY clean: non-empty and every char is
/// `[A-Za-z0-9_-]`. Anything else (empty, or containing so much as one
/// `.`/`/`/`\`/`:`/NUL/unicode/whitespace char) is a hard `None`, not a
/// stripped-down remainder. Reject, don't strip: stripping is non-injective
/// (`"a.b"` and `"ab"` would both collapse to the same segment `"ab"`), so
/// two distinct account ids could theoretically alias onto the same
/// directory — silently reopening the exact cross-account leak this module
/// exists to close. Rejecting keeps the mapping trivially injective (the
/// segment IS the id, verbatim) at the cost of refusing anything that isn't
/// already clean — the only shape a Supabase-issued UUID ever has, so real
/// accounts are unaffected; a malformed/foreign id just gets a hard `Err`
/// via `user_packs_dir` instead of a best-effort, alias-prone directory.
fn account_dir_segment(user_id: &str) -> Option<String> {
    let is_clean = !user_id.is_empty()
        && user_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if is_clean {
        Some(user_id.to_string())
    } else {
        None
    }
}

/// Pure join behind `packs_dir`: `<app_data>/packs/<sanitized user_id>`.
/// Never returns the bare `packs/` root — a malformed/empty `user_id` is a
/// hard `Err` here, not a silent fallback to the shared directory. Takes
/// `app_data`/`user_id` explicitly (no `AppHandle`) so it's unit-testable
/// directly, without a Tauri app or a `Cloud` session in the loop.
fn user_packs_dir(app_data: &Path, user_id: &str) -> Result<PathBuf, String> {
    let segment = account_dir_segment(user_id).ok_or_else(|| "invalid account id".to_string())?;
    Ok(app_data.join("packs").join(segment))
}

/// App-data `packs/<account>/` — the one managed home for personal `.kpack`
/// files (spec §3a A2), scoped per signed-in cloud account (spec §3a A6 —
/// SECURITY; see the module doc comment). The account segment comes from
/// `Cloud::current_user_id()` — the AUTHORITATIVE session state, never the
/// front end. `None` (signed out) is a hard `Err`: there is no pack store
/// to hand back. Created on demand; every personal pack the builder writes
/// and every pack `list_packs`/`delete_pack`/`rag_query`/`mount_pack` sees
/// lives directly under this one account's directory.
fn packs_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let user_id = app
        .state::<Arc<Cloud>>()
        .current_user_id()
        .ok_or_else(|| "Sign in to use knowledge packs.".to_string())?;
    let app_data = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let dir = user_packs_dir(&app_data, &user_id)?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

/// A lazily-loaded, shared `BgeEmbedder` (spec §3a A1's load-bearing fix —
/// see the module doc comment). `get_or_load` locks only for the check +
/// (on a miss) the load + store; once populated, every subsequent call is a
/// lock + `Arc::clone` — cheap, and safe to call concurrently from multiple
/// `rag_query`/`build_personal_pack` invocations (they'll serialize briefly
/// on the `Mutex`, then share the same `Arc<BgeEmbedder>`).
#[derive(Default)]
pub struct EmbedderCache {
    inner: Mutex<Option<Arc<BgeEmbedder>>>,
}

impl EmbedderCache {
    /// Returns the cached embedder for `gguf_path`, loading it first if this
    /// is the first call. Does NOT check whether a previously cached
    /// embedder was loaded from a DIFFERENT path — this app has exactly one
    /// bundled embedder GGUF (`EMBEDDER_RELATIVE_PATH`, fixed per install),
    /// so every real call site passes the same `gguf_path` every time; a
    /// path-keyed cache would be solving a problem this app doesn't have.
    pub fn get_or_load(&self, gguf_path: &Path) -> Result<Arc<BgeEmbedder>, String> {
        let mut guard = self.inner.lock().unwrap();
        if let Some(embedder) = guard.as_ref() {
            return Ok(embedder.clone());
        }
        let embedder = Arc::new(BgeEmbedder::new(gguf_path).map_err(|e| e.to_string())?);
        *guard = Some(embedder.clone());
        Ok(embedder)
    }
}

/// The single-slot active-build registry (spec §3a A1), mirroring
/// `cloud::download::Downloads` — only one personal-pack build runs at a
/// time app-wide. Holds just the cancel flag: `build_personal_pack` reports
/// progress via `app.emit` directly (no separate poll-able byte/phase state
/// the way `Downloads` tracks for `download_status`), so there's nothing
/// else to register here.
#[derive(Default)]
pub struct Builds {
    active: Mutex<Option<Arc<AtomicBool>>>,
}

impl Builds {
    /// Sets the active build's cancel flag if one is running; no-op
    /// otherwise. Used by the `cancel_build` command.
    pub fn request_cancel(&self) {
        if let Some(cancel) = self.active.lock().unwrap().as_ref() {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

/// The `build-progress` event payload (spec §3a A1): a camelCase,
/// `Serialize`-deriving mirror of `kpack_core::BuildProgress` — the same
/// "core type stays Tauri-free; this file wraps it for IPC" pattern already
/// used for `Manifest`/`Citation` (see `PackManifestInfo`/`CitationInfo`
/// below), and the same shape as `cloud::download::DownloadProgress`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildProgressEvent {
    pub phase: String,
    pub done: usize,
    pub total: usize,
}

impl From<BuildProgress> for BuildProgressEvent {
    fn from(p: BuildProgress) -> Self {
        BuildProgressEvent {
            phase: p.phase,
            done: p.done,
            total: p.total,
        }
    }
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

/// A personal pack on disk: its absolute path (the FE attaches packs to a
/// chat BY path in A4) plus its manifest view. Returned by both
/// `build_personal_pack` (the just-built pack) and `list_packs`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackEntry {
    pub path: String,
    pub manifest: PackManifestInfo,
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

/// `mount_pack` is currently FE-unused, but registered and reachable from
/// the front end regardless — apply the same `resolve_pack_in_dir` gate
/// `delete_pack`/`rag_query` use before ever mounting `path`, for
/// consistency/defense (spec §3a A6): a crafted call naming another
/// account's pack (or anything outside the current user's `packs_dir`) is
/// refused here too, not just silently allowed because nothing calls it yet.
#[tauri::command]
pub async fn mount_pack(path: String, app: AppHandle) -> Result<PackManifestInfo, String> {
    let user_packs_dir = packs_dir(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        let canonical = resolve_pack_in_dir(&user_packs_dir, &path)?;
        mount_pack_at(&canonical)
    })
    .await
    .map_err(|_| JOIN_ERROR_MESSAGE.to_string())?
}

/// Derive a `SourceInput::source_type` from `path`'s extension:
/// `.md`/`.markdown` and `.txt` map to `kpack_core::parse::parse`'s
/// recognized dispatch strings (`"md"`/`"txt"` — `parse` itself treats `"md"`
/// and `"markdown"` as synonyms, so both normalize to `"md"` here). `.pdf`
/// (§3b B3) maps to `"pdf"` — not one of `parse`'s dispatch strings, since a
/// PDF never reaches `parse`: `build_personal_pack_with_embedder` branches
/// on this string before that call and hands PDFs to `kpack_pdf::extract_pages`
/// + `kpack_core::document_from_pages` instead, wrapping the result in
/// `SourceContent::Prebuilt`. `.html`/`.htm` (formats slice) map to
/// `"html"` — UNLIKE `.pdf`, this IS one of `parse`'s dispatch strings
/// (`parse.rs`'s `"html"` arm → `kpack_core::html::html_to_document`), so
/// HTML needs no special-cased branch in `build_personal_pack_with_embedder`
/// at all: it's text, so it takes the existing `read_to_string` →
/// `SourceContent::Raw` path the same way `"md"`/`"txt"` do. Anything else
/// is unsupported for v1 (EPUB/DOCX are later milestones, per `parse.rs`'s
/// module doc comment) and returns a clear error rather than silently
/// feeding binary bytes through the plain-text parser as `parse` itself
/// would if simply handed an unrecognized type — a user picking an
/// unsupported file should see a refusal, not a garbled "personal pack".
fn source_type_for(path: &Path) -> Result<String, String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("md") | Some("markdown") => Ok("md".to_string()),
        Some("txt") => Ok("txt".to_string()),
        Some("pdf") => Ok("pdf".to_string()),
        Some("html") | Some("htm") => Ok("html".to_string()),
        _ => Err(format!(
            "unsupported file type: {} (only .md, .markdown, .txt, .pdf, .html, and .htm are supported)",
            path.display()
        )),
    }
}

/// True if `pages` carries no usable text at all — every page's extracted
/// text is empty or whitespace-only (a scanned/image-only PDF: pdfium found
/// pages but no text layer). Pure and unit-testable without pdfium (§3b B3,
/// B1-review): the emptiness check is total-across-all-pages, not
/// per-page — a PARTIALLY-scanned PDF (some text pages, some image pages)
/// has non-empty total text and is NOT flagged here, since it still builds
/// a useful pack from whichever pages do have text.
fn pdf_has_no_text(pages: &[(u32, String)]) -> bool {
    pages.iter().all(|(_, text)| text.trim().is_empty())
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

/// A unique on-disk filename for a newly built personal pack — independent
/// of its (possibly user-chosen, possibly duplicate) display name, so two
/// packs built back-to-back, or two packs sharing a display name, never
/// collide under `packs_dir`. Same "pid + nanos" idiom as `generate_pack_id`,
/// factored out separately since the filename and the `pack_id` are no
/// longer always the same value (spec §3a A2: `name` becomes the `pack_id`
/// when given).
fn unique_pack_filename() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("personal-{}-{nanos}.kpack", std::process::id())
}

/// Sanitizes a user-supplied display `name` into a manifest-safe `pack_id`
/// (spec §3a A2): trim, lowercase, collapse whitespace runs to a single
/// `-`, drop everything outside `[a-z0-9_-]`, collapse repeated `-`, cap at
/// 64 chars. Returns `None` if nothing survives (e.g. `"   "` or `"!!!"`) —
/// the caller then falls back to `generate_pack_id()`.
fn sanitize_pack_name(name: &str) -> Option<String> {
    let lower = name.trim().to_lowercase();

    // Pass 1: collapse whitespace runs to a single '-', drop anything that
    // isn't ascii-alphanumeric/'_'/'-'.
    let mut filtered = String::with_capacity(lower.len());
    let mut last_was_space = false;
    for ch in lower.chars() {
        if ch.is_whitespace() {
            if !filtered.is_empty() && !last_was_space {
                filtered.push('-');
            }
            last_was_space = true;
        } else if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            filtered.push(ch);
            last_was_space = false;
        }
        // Anything else (punctuation, non-ascii) is silently dropped.
    }

    // Pass 2: collapse literal repeated '-' runs that pass 1's whitespace
    // handling doesn't catch (e.g. a source string with "--" already in it).
    let mut collapsed = String::with_capacity(filtered.len());
    let mut last_was_dash = false;
    for ch in filtered.chars() {
        if ch == '-' {
            if !last_was_dash {
                collapsed.push('-');
            }
            last_was_dash = true;
        } else {
            collapsed.push(ch);
            last_was_dash = false;
        }
    }

    collapsed.truncate(64);
    while collapsed.ends_with('-') {
        collapsed.pop();
    }

    if collapsed.is_empty() {
        None
    } else {
        Some(collapsed)
    }
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

/// Pure build logic behind the `build_personal_pack` command: resolve
/// `gguf_path` through `cache` (a real `BgeEmbedder`, loaded at most once —
/// see [`EmbedderCache`]'s doc comment), read and type every file in
/// `file_paths`, build a `Personal`-tier pack at `out_path` reporting
/// progress through `progress` and honoring `cancel` (spec §3a A1's
/// `kpack_core::build_pack_with_progress`), then mount it back (proving the
/// round-trip, not just that the build call returned Ok) and hand back its
/// manifest. `pack_id` is caller-supplied (spec §3a A2: the sanitized `name`
/// or a `generate_pack_id()` fallback — the command's job, not this fn's) —
/// this fn just stamps it into `BuildMeta` unchanged. Factored out (no
/// `AppHandle`) so the `#[ignore]`d integration tests below can call it
/// directly against a temp file, the bundled GGUF, and a plain
/// `EmbedderCache::default()` — no Tauri `AppHandle` needed.
fn build_personal_pack_with_embedder(
    file_paths: &[String],
    pack_id: &str,
    out_path: &Path,
    gguf_path: &Path,
    pdfium_path: &Path,
    cache: &EmbedderCache,
    progress: &dyn Fn(BuildProgress),
    cancel: &AtomicBool,
) -> Result<PackManifestInfo, String> {
    if file_paths.is_empty() {
        return Err("no source files given".to_string());
    }

    let embedder = cache.get_or_load(gguf_path)?;

    let mut sources = Vec::with_capacity(file_paths.len());
    for file_path in file_paths {
        let path = Path::new(file_path);
        let source_type = source_type_for(path)?;
        let title = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| file_path.clone());

        let content = if source_type == "pdf" {
            let bytes = std::fs::read(path)
                .map_err(|e| format!("couldn't read {}: {e}", path.display()))?;
            // Extraction runs a native, FFI-bound PDF library (pdfium) over
            // caller-controlled bytes. A maliciously malformed PDF could in
            // theory crash pdfium (segfault/UB) — inherent to any FFI PDF
            // parser, uncatchable from Rust. Low in-scope risk: pack PDFs
            // are LOCAL files the user explicitly picks via the file
            // picker, not untrusted network input; subprocess sandboxing is
            // a possible future hardening, out of scope here (B1-review).
            let pages = kpack_pdf::extract_pages(&bytes, pdfium_path).map_err(|e| {
                format!(
                    "Couldn't read the PDF \"{title}\" — it may be corrupted, \
                     password-protected, or not a valid PDF. ({e})"
                )
            })?;
            let pages: Vec<(u32, String)> =
                pages.into_iter().map(|p| (p.page, p.text)).collect();
            if pdf_has_no_text(&pages) {
                return Err(format!(
                    "\"{title}\" looks scanned or image-only — no text could be \
                     extracted from it. Scanned-PDF support (OCR) isn't available \
                     yet; try a PDF with selectable text."
                ));
            }
            let doc = kpack_core::document_from_pages(&title, &pages);
            SourceContent::Prebuilt(doc)
        } else {
            let text = std::fs::read_to_string(path)
                .map_err(|e| format!("couldn't read {}: {e}", path.display()))?;
            SourceContent::Raw(text)
        };

        sources.push(SourceInput {
            content,
            title,
            source_type,
        });
    }

    let meta = BuildMeta {
        pack_id: pack_id.to_string(),
        pack_version: build_timestamp(),
        pack_tier: PackTier::Personal,
        embedder_name: EMBEDDER_NAME.to_string(),
        embedder_sha256: EMBEDDER_SHA256.to_string(),
        built_by: "device-builder v1".to_string(),
    };

    build_pack_with_progress(
        &sources,
        embedder.as_ref(),
        &meta,
        out_path,
        &ChunkConfig::default(),
        progress,
        cancel,
    )
    .map_err(|e| e.to_string())?;

    mount_pack_at(out_path)
}

/// How often a throttled `"embedding"` progress tick is allowed to reach
/// `app.emit` — mirrors `cloud::download::stream_response`'s `>=500ms`
/// byte-progress gate (same rationale: a build's chunk count can run into
/// the hundreds, and the webview doesn't need — or want — an IPC message
/// per chunk).
const BUILD_PROGRESS_THROTTLE: Duration = Duration::from_millis(500);

#[tauri::command]
pub async fn build_personal_pack(
    file_paths: Vec<String>,
    name: Option<String>,
    app: AppHandle,
    builds: State<'_, Builds>,
) -> Result<PackEntry, String> {
    let gguf_path = bundled_embedder_path(&app);
    let pdfium_path = bundled_pdfium_path(&app);
    // Computed here, before the spawn_blocking build, per spec §3a A2: the
    // out-path is this command's to decide now, not the caller's — `name`
    // (when it sanitizes to something non-empty) becomes the pack_id shown
    // in the picker/citations, but the on-disk filename is always the
    // unique pid+nanos idiom, so two packs sharing a display name never
    // collide on disk.
    let out_path = packs_dir(&app)?.join(unique_pack_filename());
    let pack_id = name
        .as_deref()
        .and_then(sanitize_pack_name)
        .unwrap_or_else(generate_pack_id);

    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut guard = builds.active.lock().unwrap();
        if guard.is_some() {
            return Err("A build is already in progress.".to_string());
        }
        *guard = Some(cancel.clone());
    }

    // A Drop guard so an early return (a mapped error below) or a panic
    // unwinding out of the `spawn_blocking` join can't wedge the active
    // slot forever — mirrors `cloud::download::download_model`'s
    // `ClearActiveOnDrop`, just scoped to this command's own async body
    // rather than a detached worker thread's: a build's command doesn't
    // return until the build finishes, so there's no separate thread that
    // needs its own guard.
    struct ClearActiveOnDrop<'a> {
        builds: &'a Builds,
    }
    impl Drop for ClearActiveOnDrop<'_> {
        fn drop(&mut self) {
            *self.builds.active.lock().unwrap() = None;
        }
    }
    let _clear_guard = ClearActiveOnDrop { builds: &builds };

    // Throttled per the module-level doc comment: every non-"embedding"
    // phase (parsing/writing/done) always emits — each fires at most once
    // per source or once total, never per-chunk — and the LAST "embedding"
    // tick (done == total) always emits too, so the front end's progress bar
    // never gets stuck short of 100%.
    let last_embedding_emit = Mutex::new(Instant::now() - BUILD_PROGRESS_THROTTLE);
    let app_for_progress = app.clone();
    let progress = move |p: BuildProgress| {
        if p.phase == "embedding" && p.done < p.total {
            let mut last = last_embedding_emit.lock().unwrap();
            if last.elapsed() < BUILD_PROGRESS_THROTTLE {
                return;
            }
            *last = Instant::now();
        }
        let _ = app_for_progress.emit("build-progress", &BuildProgressEvent::from(p));
    };

    let cancel_for_build = cancel.clone();
    let app_for_thread = app.clone();
    let out_path_for_build = out_path.clone();
    let pack_id_for_build = pack_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let cache = app_for_thread.state::<EmbedderCache>();
        build_personal_pack_with_embedder(
            &file_paths,
            &pack_id_for_build,
            &out_path_for_build,
            &gguf_path,
            &pdfium_path,
            cache.inner(),
            &progress,
            &cancel_for_build,
        )
    })
    .await
    .map_err(|_| JOIN_ERROR_MESSAGE.to_string())?;

    result.map(|manifest| PackEntry {
        path: out_path.to_string_lossy().into_owned(),
        manifest,
    })
}

/// Flips the active build's cancel flag (spec §3a A1); no-op if no build is
/// running. Mirrors `cloud::download::cancel_download`.
#[tauri::command]
pub async fn cancel_build(builds: State<'_, Builds>) -> Result<(), String> {
    builds.request_cancel();
    Ok(())
}

/// A camelCase, front-end-facing view of `kpack_core::retrieve::Citation` —
/// the numbered source mapping a chat UI (§7, later) shows next to the
/// model's answer.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CitationInfo {
    pub n: usize,
    pub pack_id: String,
    pub chunk_id: i64,
    pub doc_title: String,
    pub section_path: String,
    pub locator: String,
}

impl From<Citation> for CitationInfo {
    fn from(c: Citation) -> Self {
        CitationInfo {
            n: c.n,
            pack_id: c.pack_id,
            chunk_id: c.chunk_id,
            doc_title: c.doc_title,
            section_path: c.section_path,
            locator: c.locator,
        }
    }
}

/// The `rag_query` command's camelCase result (spec §4: returning §4.1–4.2's
/// retrieval outcome across the Tauri IPC boundary; the chat send/generate
/// wiring that actually USES this is §3, a later milestone — this command
/// only runs retrieval and hands back what it found). `status` is
/// `"grounded"` or `"noEvidence"`. `prompt` is always populated but means a
/// different thing in each case: the full assembled grounded prompt (system
/// contract + numbered sources, ready to hand to the model) when `status ==
/// "grounded"`; the contract's fixed `no_evidence_marker()` text when
/// `status == "noEvidence"` — so the front end has ONE field to render
/// either way, and switches on `status` to decide whether `citations` is
/// meaningful. `citations` is always empty on `"noEvidence"`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RagQueryResult {
    pub status: String,
    pub prompt: Option<String>,
    pub citations: Vec<CitationInfo>,
}

/// `RetrievalResult` -> the IPC-facing `RagQueryResult`, factored out as a
/// pure function (no I/O) so it's directly unit-testable without a real
/// pack/embedder. See `RagQueryResult`'s doc comment for what each field
/// means in each branch.
fn map_retrieval_result(result: RetrievalResult) -> RagQueryResult {
    match result {
        RetrievalResult::Grounded { prompt, citations } => RagQueryResult {
            status: "grounded".to_string(),
            prompt: Some(prompt),
            citations: citations.into_iter().map(CitationInfo::from).collect(),
        },
        RetrievalResult::NoEvidence => RagQueryResult {
            status: "noEvidence".to_string(),
            prompt: Some(kpack_core::contract::no_evidence_marker().to_string()),
            citations: Vec::new(),
        },
    }
}

/// Pure inner logic behind the `rag_query` command: resolve every path in
/// `pack_paths` through `resolve_pack_in_dir(packs_dir, ..)` — spec §3a A6's
/// per-account read-path guard — before ever mounting it, so a crafted call
/// naming a path outside the current user's `packs_dir` (including another
/// account's pack) is a hard `Err`, not a silent skip; mounts each resolved
/// pack against this device's pinned embedder hash (the exact same
/// `LoadContext` gate `mount_pack_at` uses), resolves `gguf_path` through
/// `cache` (spec §3a A1's cached embedder), runs
/// `kpack_core::retrieve::retrieve`, and maps the result via
/// `map_retrieval_result`. Factored out (no `AppHandle`) so real-embedder
/// integration tests can exercise this exact path without constructing a
/// Tauri `AppHandle` — mirrors `mount_pack_at`/`build_personal_pack_with_embedder`'s
/// existing pattern in this file. (`crates/kpack-embed/tests/`'s own
/// integration tests call `kpack_core::retrieve::retrieve` directly rather
/// than this fn, since that crate can't depend on this one — the Tauri
/// binary — but the logic they exercise is identical: mount, embed,
/// retrieve.)
fn rag_query_inner(
    query: &str,
    pack_paths: &[String],
    packs_dir: &Path,
    gguf_path: &Path,
    tier: Tier,
    cache: &EmbedderCache,
) -> Result<RagQueryResult, String> {
    let available = vec![EMBEDDER_SHA256.to_string()];
    let ctx = LoadContext {
        available_embedder_sha256: &available,
        curator_key: kpack_core::sign::curator_verifying_key(),
    };

    let mut mounted: Vec<(Pack, Manifest)> = Vec::with_capacity(pack_paths.len());
    for path in pack_paths {
        let canonical = resolve_pack_in_dir(packs_dir, path)?;
        let (pack, manifest) = Pack::mount(&canonical, &ctx).map_err(|e| e.to_string())?;
        mounted.push((pack, manifest));
    }

    let embedder = cache.get_or_load(gguf_path)?;
    let result = retrieve(query, &mounted, embedder.as_ref(), tier).map_err(|e| e.to_string())?;

    Ok(map_retrieval_result(result))
}

#[tauri::command]
pub async fn rag_query(
    query: String,
    pack_paths: Vec<String>,
    app: AppHandle,
) -> Result<RagQueryResult, String> {
    let gguf_path = bundled_embedder_path(&app);
    // Resolved on the async side, before spawn_blocking — same "packs_dir()
    // is this command's to compute up front" idiom as build_personal_pack's
    // out_path. This is the current user's dir (spec §3a A6): every path in
    // pack_paths gets checked against it below via resolve_pack_in_dir, so a
    // pack belonging to a different account can never be mounted here even
    // if the front end (or a compromised renderer) hands us its path.
    let user_packs_dir = packs_dir(&app)?;
    // Tier (spec §4.3): reuse the existing hardware tier
    // (`hardware::detect`, spec §6's three-level "low"/"mid"/"high" device
    // classification) rather than inventing a second notion of device
    // capability. RAG's own tier concept is coarser — two levels, not three
    // — so "low" (the constrained, no-dGPU/<16GB-RAM tier §6 already treats
    // as the weakest) maps to `Tier::Small` (N=3, the 1-2 GB budget spec
    // §4.3 describes); "mid" and "high" both map to `Tier::Large` (N=5) —
    // there's no RAG-specific reason to further split the two stronger
    // hardware tiers. `hardware::detect()` always returns one of the three
    // strings (see its own tests), so the `_ => Tier::Large` arm is a
    // defensive default (documented per the task brief's "else default
    // Tier::Large"), not an expected path.
    let tier = match crate::hardware::detect().tier.as_str() {
        "low" => Tier::Small,
        _ => Tier::Large,
    };
    tauri::async_runtime::spawn_blocking(move || {
        let cache = app.state::<EmbedderCache>();
        rag_query_inner(&query, &pack_paths, &user_packs_dir, &gguf_path, tier, cache.inner())
    })
    .await
    .map_err(|_| JOIN_ERROR_MESSAGE.to_string())?
}

/// Pure directory scan behind the `list_packs` command (spec §3a A2): read
/// every `*.kpack` file directly in `dir` and try `mount_pack_at` on each —
/// on success it becomes a `PackEntry`; on failure (corrupt file, or a pack
/// built with a different embedder than this device's) it's silently
/// SKIPPED rather than failing the whole list, so one bad pack can't hide
/// every other one. Takes `dir` explicitly (not an `AppHandle`) so it's
/// unit-testable without constructing a Tauri app. Sorted by path for a
/// stable order across calls — directory read order is not guaranteed.
fn list_packs_in(dir: &Path) -> Vec<PackEntry> {
    let mut entries: Vec<PackEntry> = Vec::new();
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return entries;
    };
    for dir_entry in read_dir.flatten() {
        let path = dir_entry.path();
        let is_kpack = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase());
        if is_kpack.as_deref() != Some("kpack") {
            continue;
        }
        if let Ok(manifest) = mount_pack_at(&path) {
            entries.push(PackEntry {
                path: path.to_string_lossy().into_owned(),
                manifest,
            });
        }
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    entries
}

#[tauri::command]
pub async fn list_packs(app: AppHandle) -> Result<Vec<PackEntry>, String> {
    let dir = packs_dir(&app)?;
    tauri::async_runtime::spawn_blocking(move || list_packs_in(&dir))
        .await
        .map_err(|_| JOIN_ERROR_MESSAGE.to_string())
}

/// The path-safety boundary shared by `delete_pack_in`/`rag_query_inner`/
/// `mount_pack` (spec §3a A2 — SECURITY CRITICAL; spec §3a A6 — now also the
/// per-account boundary, see the module doc comment): canonicalizes both
/// `packs_dir` (it exists — `packs_dir()` already `create_dir_all`s it) and
/// `target`, then refuses unless the canonicalized target's PARENT is
/// exactly the canonicalized `packs_dir` — a file living DIRECTLY in the
/// managed dir, not a subdir, not reached via `..`, not a symlink escape —
/// AND its extension is `kpack` (ASCII-lowercased). Canonicalizing BOTH
/// sides resolves `..` components and symlinks on both, so the `parent ==`
/// comparison is sound on Windows (`\\?\`-prefixed both sides) and Unix
/// alike. Returns the canonical target on success.
///
/// Because every call site passes the CURRENT USER's `packs_dir` (never the
/// shared root), this single check does double duty as the cross-account
/// boundary: a path resolving into a different account's subdirectory has a
/// different canonical parent and is refused here exactly like an
/// outside-the-store path — the account that built a pack is the only
/// account that can delete, mount, or query it. This is what keeps
/// `delete_pack`/`rag_query`/`mount_pack` from ever letting a path from the
/// front end (or a compromised renderer) reach a traversal, an absolute
/// path, or another account's pack.
fn resolve_pack_in_dir(packs_dir: &Path, target: &str) -> Result<PathBuf, String> {
    let canonical_dir = packs_dir
        .canonicalize()
        .map_err(|e| format!("packs dir unavailable: {e}"))?;

    let target_path = Path::new(target);
    if !target_path.exists() {
        return Err("no such pack".to_string());
    }
    let canonical_target = target_path
        .canonicalize()
        .map_err(|e| format!("couldn't resolve {target}: {e}"))?;

    let is_kpack = canonical_target
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    let lives_directly_in_packs_dir = canonical_target.parent() == Some(canonical_dir.as_path());

    if is_kpack.as_deref() != Some("kpack") || !lives_directly_in_packs_dir {
        return Err("refusing to use a path outside the packs store".to_string());
    }

    Ok(canonical_target)
}

/// Pure delete logic behind the `delete_pack` command: resolve `target`
/// through `resolve_pack_in_dir` (see its doc comment for the full
/// path-safety/cross-account story), then remove the resolved path.
fn delete_pack_in(packs_dir: &Path, target: &str) -> Result<(), String> {
    let canonical_target = resolve_pack_in_dir(packs_dir, target)?;
    std::fs::remove_file(&canonical_target).map_err(|e| format!("couldn't delete pack: {e}"))
}

#[tauri::command]
pub async fn delete_pack(path: String, app: AppHandle) -> Result<(), String> {
    let dir = packs_dir(&app)?;
    tauri::async_runtime::spawn_blocking(move || delete_pack_in(&dir, &path))
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
        assert_eq!(source_type_for(Path::new("a.pdf")).unwrap(), "pdf");
        assert_eq!(source_type_for(Path::new("a.PDF")).unwrap(), "pdf");
        assert_eq!(source_type_for(Path::new("a.html")).unwrap(), "html");
        assert_eq!(source_type_for(Path::new("a.htm")).unwrap(), "html");
        assert_eq!(source_type_for(Path::new("a.HTML")).unwrap(), "html");
        assert!(source_type_for(Path::new("a.docx")).is_err());
        assert!(source_type_for(Path::new("no-extension")).is_err());
    }

    /// `pdf_has_no_text` (§3b B3, B1-review): pure, no pdfium involved —
    /// locks down the scanned/image-only detection rule independent of the
    /// real-library `#[ignore]`d integration test.
    #[test]
    fn pdf_has_no_text_flags_only_a_fully_textless_document() {
        // Every page empty or whitespace-only -> true.
        assert!(pdf_has_no_text(&[(1, "".to_string()), (2, "   \n\t  ".to_string())]));
        // No pages at all -> vacuously true (nothing to build from).
        assert!(pdf_has_no_text(&[]));
        // Any page with real text -> false, even if other pages are blank
        // (a partially-scanned PDF still builds from the pages that have
        // text).
        assert!(!pdf_has_no_text(&[
            (1, "   ".to_string()),
            (2, "Real extracted text.".to_string()),
        ]));
        assert!(!pdf_has_no_text(&[(1, "Real extracted text.".to_string())]));
    }

    #[test]
    fn citation_info_from_maps_every_field() {
        let c = Citation {
            n: 1,
            pack_id: "p".to_string(),
            chunk_id: 42,
            doc_title: "Doc".to_string(),
            section_path: "Sec".to_string(),
            locator: "p.1".to_string(),
        };
        let info = CitationInfo::from(c);
        assert_eq!(info.n, 1);
        assert_eq!(info.pack_id, "p");
        assert_eq!(info.chunk_id, 42);
        assert_eq!(info.doc_title, "Doc");
        assert_eq!(info.section_path, "Sec");
        assert_eq!(info.locator, "p.1");
    }

    #[test]
    fn map_retrieval_result_grounded_carries_prompt_and_citations() {
        let citation = Citation {
            n: 1,
            pack_id: "p".to_string(),
            chunk_id: 1,
            doc_title: "Doc".to_string(),
            section_path: "Sec".to_string(),
            locator: "p.1".to_string(),
        };
        let result = RetrievalResult::Grounded {
            prompt: "the prompt".to_string(),
            citations: vec![citation],
        };
        let mapped = map_retrieval_result(result);
        assert_eq!(mapped.status, "grounded");
        assert_eq!(mapped.prompt.as_deref(), Some("the prompt"));
        assert_eq!(mapped.citations.len(), 1);
        assert_eq!(mapped.citations[0].doc_title, "Doc");
    }

    #[test]
    fn map_retrieval_result_no_evidence_carries_the_contract_marker() {
        let mapped = map_retrieval_result(RetrievalResult::NoEvidence);
        assert_eq!(mapped.status, "noEvidence");
        assert_eq!(
            mapped.prompt.as_deref(),
            Some(kpack_core::contract::no_evidence_marker())
        );
        assert!(mapped.citations.is_empty());
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
        let pdfium_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources").join(PDFIUM_RELATIVE_PATH);

        let dir = unique_dir("roundtrip");
        let md_path = dir.join("source.md");
        std::fs::write(
            &md_path,
            "# Vitamin K\n\nVitamin K is a fat-soluble vitamin involved in blood clotting. \
             It also plays a role in bone metabolism.\n",
        )
        .unwrap();

        let out_path = dir.join("personal.kpack");
        let cache = EmbedderCache::default();
        let cancel = AtomicBool::new(false);
        let manifest = build_personal_pack_with_embedder(
            &[md_path.to_string_lossy().into_owned()],
            "test-pack",
            &out_path,
            &gguf_path,
            &pdfium_path,
            &cache,
            &|_| {},
            &cancel,
        )
        .expect("build_personal_pack_with_embedder should succeed against the real embedder");

        assert_eq!(manifest.pack_id, "test-pack");
        assert_eq!(manifest.pack_tier, "personal");
        assert_eq!(manifest.embedding_dims, 768);
        assert_eq!(manifest.embedder_sha256, EMBEDDER_SHA256);
        assert_eq!(manifest.embedder_name, EMBEDDER_NAME);
        assert!(!manifest.gate_calibrated);
        assert!(!manifest.prefixes_present);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Same real-embedder gate as above: an unsupported file extension is
    /// refused with a clear error before the pack builder ever runs —
    /// proven with the real `BgeEmbedder`/`EmbedderCache` in the loop (not
    /// just a pure-Rust unit check) so this exercises the exact refusal
    /// path `build_personal_pack` takes in production. Uses `.docx` (still
    /// unsupported post-§3b B3, since `.pdf` moved to the supported column —
    /// `source_type_for`'s own unit test below covers that shift directly).
    #[test]
    #[ignore]
    fn unsupported_file_type_is_refused_before_building() {
        let gguf_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources").join(EMBEDDER_RELATIVE_PATH);
        let pdfium_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources").join(PDFIUM_RELATIVE_PATH);
        let dir = unique_dir("unsupported");
        let bad_path = dir.join("source.docx");
        std::fs::write(&bad_path, b"not-really-a-docx").unwrap();

        let out_path = dir.join("personal.kpack");
        let cache = EmbedderCache::default();
        let cancel = AtomicBool::new(false);
        let err = build_personal_pack_with_embedder(
            &[bad_path.to_string_lossy().into_owned()],
            "test-pack",
            &out_path,
            &gguf_path,
            &pdfium_path,
            &cache,
            &|_| {},
            &cancel,
        )
        .unwrap_err();
        assert!(err.contains("unsupported file type"), "error was: {err}");
        assert!(!out_path.exists(), "no pack should be written on refusal");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Spec §3a A1's real-embedder proof: (1) `EmbedderCache::get_or_load`
    /// called twice against the same GGUF path returns the SAME `Arc` (not
    /// a second load — `Arc::ptr_eq`, the strongest form of "same
    /// instance," stronger than a load-count that could pass even if two
    /// different `BgeEmbedder`s happened to occupy the same heap slot
    /// across two frees); (2) a real build through that cache emits every
    /// `BuildProgress` phase (`"parsing"`/`"embedding"`/`"writing"`/`"done"`),
    /// proving `build_pack_with_progress` and the cache compose correctly
    /// end to end, not just against the mock embedder (`kpack-core`'s own
    /// `t5_build_pack_with_progress_...` test already covers the mock case).
    ///
    /// `#[ignore]`d for the same reason as the tests above: needs the
    /// bundled GGUF on disk and the `real`-feature native build. Run
    /// explicitly with `$env:LIBCLANG_PATH = "C:\Program Files\LLVM\bin";
    /// cargo test -p cleophis --lib -- --ignored
    /// kpack::tests::embedder_cache_loads_once_and_build_emits_progress_phases`.
    #[test]
    #[ignore]
    fn embedder_cache_loads_once_and_build_emits_progress_phases() {
        let gguf_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources").join(EMBEDDER_RELATIVE_PATH);
        assert!(
            gguf_path.exists(),
            "bundled embedder GGUF missing at {} — run tools/fetch-embedder.mjs first",
            gguf_path.display()
        );
        let pdfium_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources").join(PDFIUM_RELATIVE_PATH);

        let cache = EmbedderCache::default();
        let first = cache.get_or_load(&gguf_path).expect("first load should succeed");
        let second = cache.get_or_load(&gguf_path).expect("second call should reuse the cache");
        assert!(
            Arc::ptr_eq(&first, &second),
            "get_or_load should return the SAME Arc on a second call — the model must load once"
        );

        let dir = unique_dir("cache-progress");
        let md_path = dir.join("source.md");
        std::fs::write(
            &md_path,
            "# Vitamin K\n\nVitamin K is a fat-soluble vitamin involved in blood clotting.\n\n\
             ## More\n\nIt also plays a role in bone metabolism, and interacts with warfarin.\n",
        )
        .unwrap();
        let out_path = dir.join("personal.kpack");

        let events: Mutex<Vec<BuildProgress>> = Mutex::new(Vec::new());
        let progress = |p: BuildProgress| events.lock().unwrap().push(p);
        let cancel = AtomicBool::new(false);

        let manifest = build_personal_pack_with_embedder(
            &[md_path.to_string_lossy().into_owned()],
            "test-pack",
            &out_path,
            &gguf_path,
            &pdfium_path,
            &cache,
            &progress,
            &cancel,
        )
        .expect("build should succeed against the real embedder, reusing the cached Arc");
        assert_eq!(manifest.pack_tier, "personal");

        let events = events.into_inner().unwrap();
        let phases: Vec<&str> = events.iter().map(|p| p.phase.as_str()).collect();
        assert!(phases.contains(&"parsing"), "phases: {phases:?}");
        assert!(phases.contains(&"embedding"), "phases: {phases:?}");
        assert!(phases.contains(&"writing"), "phases: {phases:?}");
        assert!(phases.contains(&"done"), "phases: {phases:?}");

        // The cache still holds exactly the model loaded before the build —
        // the build's own internal `get_or_load` call did not reload it.
        let third = cache.get_or_load(&gguf_path).expect("post-build call should reuse the cache");
        assert!(Arc::ptr_eq(&first, &third));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sanitize_pack_name_maps_display_names_to_safe_ids() {
        assert_eq!(sanitize_pack_name("My Notes").as_deref(), Some("my-notes"));
        assert_eq!(sanitize_pack_name("a/b\\c").as_deref(), Some("abc"));
        assert_eq!(sanitize_pack_name("  padded  ").as_deref(), Some("padded"));
        assert_eq!(
            sanitize_pack_name("already-clean_123").as_deref(),
            Some("already-clean_123")
        );
        assert_eq!(sanitize_pack_name("   "), None);
        assert_eq!(sanitize_pack_name("!!!"), None);

        let long = "a".repeat(100);
        let sanitized = sanitize_pack_name(&long).expect("all-alnum input should survive");
        assert_eq!(sanitized.len(), 64, "over-long name should be capped at 64 chars");
    }

    /// Per-account isolation (spec §3a A6 — SECURITY): a Supabase UUID
    /// passes through `account_dir_segment` intact.
    #[test]
    fn account_dir_segment_keeps_a_uuid_intact() {
        let uuid = "3fa85f64-5717-4562-b3fc-2c963f66afa6";
        assert_eq!(account_dir_segment(uuid).as_deref(), Some(uuid));
    }

    /// Any non-clean id — separators, traversal dots, or anything else
    /// outside `[A-Za-z0-9_-]` — is REJECTED outright, not stripped down to
    /// a shortened remainder (spec §3a A6 review hardening: stripping is
    /// non-injective and could alias two distinct ids onto the same
    /// directory, reopening the leak).
    #[test]
    fn account_dir_segment_rejects_ids_containing_separators_or_traversal_dots() {
        assert_eq!(account_dir_segment("../evil"), None);
        assert_eq!(account_dir_segment("a/b"), None);
        assert_eq!(account_dir_segment("a\\b"), None);
        assert_eq!(account_dir_segment(".."), None);
    }

    /// Nothing survives (empty input, or input that's entirely punctuation)
    /// -> `None`, never an empty string that could collapse the path back to
    /// the shared `packs/` root.
    #[test]
    fn account_dir_segment_none_when_input_is_empty_or_all_punctuation() {
        assert_eq!(account_dir_segment(""), None);
        assert_eq!(account_dir_segment("!!!"), None);
    }

    /// `user_packs_dir`: two different user ids resolve to two different
    /// directories under the same app-data root (spec §3a A6).
    #[test]
    fn user_packs_dir_scopes_different_users_to_different_dirs() {
        let app_data = unique_dir("user-packs-dir-scope");
        let a = user_packs_dir(&app_data, "user-aaaa").unwrap();
        let b = user_packs_dir(&app_data, "user-bbbb").unwrap();
        assert_ne!(a, b);
        assert_eq!(a, app_data.join("packs").join("user-aaaa"));
        let _ = std::fs::remove_dir_all(&app_data);
    }

    /// A malformed/empty user id is a hard `Err` — `user_packs_dir` must
    /// NEVER return the bare `packs/` root, since that would re-open the
    /// device-global leak this fix closes.
    #[test]
    fn user_packs_dir_refuses_a_malformed_id_never_returns_the_shared_root() {
        let app_data = unique_dir("user-packs-dir-malformed");
        assert!(user_packs_dir(&app_data, "").is_err());
        assert!(user_packs_dir(&app_data, "!!!").is_err());
        let _ = std::fs::remove_dir_all(&app_data);
    }

    /// THE cross-account isolation assertion (spec §3a A6 — the security
    /// fix this task exists for): a pack built and written into account A's
    /// directory is resolvable by account A, but `resolve_pack_in_dir`
    /// against account B's directory refuses it — its canonical parent is
    /// A's dir, not B's. Since `delete_pack_in`, `rag_query_inner`, and the
    /// `mount_pack` command all route through this exact check with the
    /// caller's OWN `packs_dir`, this proves account B can neither delete
    /// nor read (query/mount) account A's pack. Deliberately un-skippable —
    /// no `#[ignore]`.
    #[test]
    fn resolve_pack_in_dir_refuses_a_pack_in_a_different_accounts_dir() {
        let root = unique_dir("cross-account-isolation");
        let account_a_dir = user_packs_dir(&root, "user-aaaa").unwrap();
        let account_b_dir = user_packs_dir(&root, "user-bbbb").unwrap();
        std::fs::create_dir_all(&account_a_dir).unwrap();
        std::fs::create_dir_all(&account_b_dir).unwrap();

        let account_a_pack = account_a_dir.join("secret.kpack");
        std::fs::write(&account_a_pack, b"").unwrap();

        // Account A can resolve its own pack.
        assert!(resolve_pack_in_dir(&account_a_dir, &account_a_pack.to_string_lossy()).is_ok());

        // Account B, handed the exact same path, cannot: its canonical
        // parent is account A's dir, not account B's.
        let err = resolve_pack_in_dir(&account_b_dir, &account_a_pack.to_string_lossy()).unwrap_err();
        assert!(!err.is_empty());
        assert!(
            account_a_pack.exists(),
            "a refused cross-account resolve must never touch the file"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Path-safety tests for `delete_pack_in` (spec §3a A2 — SECURITY
    /// CRITICAL). No real pack format needed: `delete_pack_in` only cares
    /// about paths + extension, so empty files stand in for real `.kpack`s.
    #[test]
    fn delete_pack_in_deletes_a_real_kpack_directly_in_the_packs_dir() {
        let dir = unique_dir("delete-ok");
        let target = dir.join("foo.kpack");
        std::fs::write(&target, b"").unwrap();

        delete_pack_in(&dir, &target.to_string_lossy()).expect("should delete");
        assert!(!target.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_pack_in_refuses_the_wrong_extension() {
        let dir = unique_dir("delete-wrong-ext");
        let target = dir.join("foo.txt");
        std::fs::write(&target, b"").unwrap();

        let err = delete_pack_in(&dir, &target.to_string_lossy()).unwrap_err();
        assert!(!err.is_empty());
        assert!(target.exists(), "wrong-extension file must be left untouched");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// THE security assertion: a path outside the packs dir — whether a
    /// plain sibling path or a `..`-traversal string that resolves right
    /// back into that same sibling — must be refused, and the file left
    /// untouched. This is what proves `delete_pack` can never become an
    /// arbitrary-file-delete primitive, un-skippable per the task brief.
    #[test]
    fn delete_pack_in_refuses_a_path_outside_the_packs_dir() {
        let root = unique_dir("delete-outside-root");
        let packs = root.join("packs");
        std::fs::create_dir_all(&packs).unwrap();
        let outside = root.join("escape.kpack");
        std::fs::write(&outside, b"").unwrap();

        // Directly outside, as a plain absolute path.
        let err = delete_pack_in(&packs, &outside.to_string_lossy()).unwrap_err();
        assert!(!err.is_empty());
        assert!(outside.exists(), "file outside the packs dir must be left untouched");

        // The same file, reached via a `..` traversal string rooted at packs/.
        let traversal = packs.join("..").join("escape.kpack");
        let err = delete_pack_in(&packs, &traversal.to_string_lossy()).unwrap_err();
        assert!(!err.is_empty());
        assert!(
            outside.exists(),
            "traversal path must also be refused, file left untouched"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn delete_pack_in_refuses_a_file_in_a_subdirectory_of_the_packs_dir() {
        let dir = unique_dir("delete-subdir");
        let sub = dir.join("nested");
        std::fs::create_dir_all(&sub).unwrap();
        let target = sub.join("foo.kpack");
        std::fs::write(&target, b"").unwrap();

        let err = delete_pack_in(&dir, &target.to_string_lossy()).unwrap_err();
        assert!(!err.is_empty());
        assert!(target.exists(), "file in a subdir must be left untouched");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_pack_in_reports_a_clean_error_for_a_nonexistent_path() {
        let dir = unique_dir("delete-missing");
        let missing = dir.join("nope.kpack");

        let err = delete_pack_in(&dir, &missing.to_string_lossy()).unwrap_err();
        assert_eq!(err, "no such pack");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// End-to-end proof that build → list → delete compose (spec §3a A2):
    /// build a personal pack with a caller-supplied `pack_id` into a temp
    /// "packs" dir, confirm `list_packs_in` sees exactly that one entry with
    /// the right id and path, then delete it through `delete_pack_in` and
    /// confirm the dir is empty again.
    ///
    /// `#[ignore]`d for the same reason as the round-trip test above: needs
    /// the bundled GGUF on disk and the `real`-feature native build. Run
    /// explicitly with `$env:LIBCLANG_PATH = "C:\Program Files\LLVM\bin";
    /// cargo test -p cleophis --lib -- --ignored kpack::tests::build_list_delete_compose_with_real_embedder`.
    #[test]
    #[ignore]
    fn build_list_delete_compose_with_real_embedder() {
        let gguf_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources").join(EMBEDDER_RELATIVE_PATH);
        assert!(
            gguf_path.exists(),
            "bundled embedder GGUF missing at {} — run tools/fetch-embedder.mjs first",
            gguf_path.display()
        );
        let pdfium_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources").join(PDFIUM_RELATIVE_PATH);

        let dir = unique_dir("build-list-delete");
        let packs_dir = dir.join("packs");
        std::fs::create_dir_all(&packs_dir).unwrap();

        let md_path = dir.join("source.md");
        std::fs::write(
            &md_path,
            "# Vitamin K\n\nVitamin K is a fat-soluble vitamin involved in blood clotting.\n",
        )
        .unwrap();

        let out_path = packs_dir.join(unique_pack_filename());
        let cache = EmbedderCache::default();
        let cancel = AtomicBool::new(false);
        build_personal_pack_with_embedder(
            &[md_path.to_string_lossy().into_owned()],
            "rt-test",
            &out_path,
            &gguf_path,
            &pdfium_path,
            &cache,
            &|_| {},
            &cancel,
        )
        .expect("build should succeed against the real embedder");

        let listed = list_packs_in(&packs_dir);
        assert_eq!(listed.len(), 1, "expected exactly one pack, got {listed:?}");
        assert_eq!(listed[0].manifest.pack_id, "rt-test");
        assert_eq!(listed[0].path, out_path.to_string_lossy());

        delete_pack_in(&packs_dir, &listed[0].path).expect("delete should succeed");
        assert!(
            list_packs_in(&packs_dir).is_empty(),
            "packs dir should be empty after delete"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// §3b B3 end-to-end proof: build a personal pack from a real PDF
    /// (reusing `kpack-pdf`'s own committed fixture,
    /// `crates/kpack-pdf/tests/fixtures/sample.pdf` — two pages, "...page
    /// one alpha" / "...page two bravo") through the exact app path
    /// (`build_personal_pack_with_embedder`'s `.pdf` branch:
    /// `kpack_pdf::extract_pages` -> `kpack_core::document_from_pages` ->
    /// `SourceContent::Prebuilt`), then queries it via `rag_query_inner` and
    /// asserts a returned citation carries a `p.N` page locator — proving
    /// PDF -> page-locator -> citation end to end through the app, not just
    /// through `kpack-pdf`/`kpack-core` in isolation.
    ///
    /// `#[ignore]`d for the same reasons as the tests above (bundled GGUF +
    /// `real`-feature native build), plus needs `pdfium.dll` at the
    /// resources path (`tools/fetch-pdfium.mjs`). Run explicitly with
    /// `$env:LIBCLANG_PATH = "C:\Program Files\LLVM\bin"; cargo test -p
    /// cleophis --lib -- --ignored kpack::tests::build_from_pdf_produces_a_page_locator_citation`.
    #[test]
    #[ignore]
    fn build_from_pdf_produces_a_page_locator_citation() {
        let gguf_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources").join(EMBEDDER_RELATIVE_PATH);
        assert!(
            gguf_path.exists(),
            "bundled embedder GGUF missing at {} — run tools/fetch-embedder.mjs first",
            gguf_path.display()
        );
        let pdfium_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources").join(PDFIUM_RELATIVE_PATH);
        assert!(
            pdfium_path.exists(),
            "pdfium.dll missing at {} — run tools/fetch-pdfium.mjs first",
            pdfium_path.display()
        );
        let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("crates")
            .join("kpack-pdf")
            .join("tests")
            .join("fixtures")
            .join("sample.pdf");
        assert!(
            fixture_path.exists(),
            "PDF fixture missing at {} — kpack-pdf's own fixture should already be committed",
            fixture_path.display()
        );

        let dir = unique_dir("pdf-build");
        let packs_dir = dir.join("packs");
        std::fs::create_dir_all(&packs_dir).unwrap();

        let out_path = packs_dir.join(unique_pack_filename());
        let cache = EmbedderCache::default();
        let cancel = AtomicBool::new(false);
        let manifest = build_personal_pack_with_embedder(
            &[fixture_path.to_string_lossy().into_owned()],
            "pdf-test",
            &out_path,
            &gguf_path,
            &pdfium_path,
            &cache,
            &|_| {},
            &cancel,
        )
        .expect("build from a real PDF should succeed");
        assert_eq!(manifest.pack_tier, "personal");

        let result = rag_query_inner(
            "kpack-pdf fixture page one alpha",
            &[out_path.to_string_lossy().into_owned()],
            &packs_dir,
            &gguf_path,
            Tier::Large,
            &cache,
        )
        .expect("rag_query_inner should succeed against the PDF-built pack");

        match result.status.as_str() {
            "grounded" => {
                assert!(!result.citations.is_empty(), "expected at least one citation");
                assert!(
                    result.citations.iter().any(|c| c.locator.starts_with("p.")),
                    "expected a p.N page locator among citations, got: {:?}",
                    result.citations
                );
            }
            other => panic!(
                "expected a grounded retrieval for an in-corpus PDF query, got {other}: {:?}",
                result.prompt
            ),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }
}
