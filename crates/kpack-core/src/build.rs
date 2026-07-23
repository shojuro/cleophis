//! End-to-end knowledge-pack builder (K8) — the capstone that wires every
//! prior module into one real `.kpack`: `parse` (K6) → `chunk` (K5) →
//! `embed` (K4a) → `format` (K1) → `manifest` (K2), spec §1.3/§1.4/§2.1–2.2.
//!
//! [`build_pack`] is embedder-agnostic (`&dyn Embedder`); this module's own
//! tests run it against [`crate::embed::MockEmbedder`] only — never a build
//! anyone should ship (see that type's doc comment) — because K4b's real
//! `bge-base-en-v1.5` backend lives in a separate crate (`kpack-embed`) and
//! isn't a dependency of this one. A caller wires the real embedder in.
//!
//! ## Determinism: mock-plumbing vs. real cross-build (spec §5, plan risk note)
//! This module's own `t2_build_pack_is_deterministic_mock` test locks that
//! `build_pack`'s CODE is deterministic — same inputs, byte-identical
//! output — under the mock embedder. It does **not** exercise spec §5's
//! actual cross-build determinism claim ("server pipeline and device
//! builder given the same corpus + embedder must produce identical chunk
//! sets"), because chunk boundaries are derived from
//! `Embedder::token_count`, and [`crate::embed::MockEmbedder::token_count`]
//! is a crude word-count approximation, not a real tokenizer (see that
//! method's doc comment — this is the exact risk the plan flags). The real
//! cross-build test — `BgeEmbedder` vs. a reference build, real tokenizer —
//! is a K4b-onward follow-up once `--features real` builds (needs cmake);
//! it is deliberately NOT here. Do not read this module's determinism test
//! as satisfying §5.
//!
//! ## Build steps
//! 1. Build into `<out_path>.part`, verify-then-atomic-rename (mirrors
//!    `src-tauri/src/cloud/download.rs`'s finalize step): open/create the
//!    part file as a fresh `.kpack` sized for `embedder.dims()`.
//! 2. For each [`SourceInput`]: resolve its [`SourceContent`] to a
//!    [`crate::tree::Document`] (`Raw` is parsed via [`crate::parse::parse`];
//!    `Prebuilt` is used as-is — see `SourceContent`'s doc comment), hash a
//!    stable content representation ([`sha256_hex`]) for `docs.sha256`,
//!    score [`crate::parse::extraction_quality`], insert the `docs` row,
//!    then chunk the document (`chunk::chunk_document`).
//! 3. For each [`crate::chunk::ChunkDraft`]: insert the `chunks`/`fts5` row,
//!    embed [`passage_input`]'s output (the §2.3 `prefix + "\n" + text`
//!    formula — `text` alone when `prefix` is empty, which is every K8
//!    build: see `ChunkDraft::prefix`'s doc comment), L2-normalize,
//!    int8-quantize, and insert the `vec0` row.
//! 4. Write every §1.2 manifest key ([`Manifest::write`]). `gate_abs_floor`/
//!    `gate_rel_margin` use placeholder personal-quick-build defaults
//!    ([`PLACEHOLDER_GATE_ABS_FLOOR`]/[`PLACEHOLDER_GATE_REL_MARGIN`]) —
//!    the real refusal-biased §3.4 defaults are a later milestone, not K8's
//!    scope; `gate_calibrated = false` and `prefixes_present = false`
//!    always (calibration and build-time LLM prefixes are curated-pipeline-
//!    only, §2.3/§2.4, neither of which K8 runs).
//! 5. Close the connection (`Pack` drops at the end of the inner build
//!    function, below), then atomically rename the `.part` into place.
//!    `build_pack` unconditionally removes any pre-existing `<out_path>.part`
//!    before step 1 ever starts, so every build begins from a clean slate:
//!    a leftover COMPLETE `.part` from a prior crash (or a prior build's
//!    failed final rename) is discarded, never merged/appended-to. Any
//!    failure during the build itself, or of the final rename, likewise
//!    removes the `.part` and returns the error — neither a half-built nor
//!    a stale `.part` is ever left where a later `Pack::open`/`mount` could
//!    mistake it for a real pack.

use crate::chunk::{chunk_document, ChunkConfig};
use crate::embed::{l2_normalize, quantize_int8, EmbedError, Embedder};
use crate::format::{self, Chunk, Doc, Pack};
use crate::manifest::{Manifest, PackTier, VEC_FORMAT_VERSION};
use crate::parse;
use crate::tree::{Block, Document, Section};

use sha2::{Digest, Sha256};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// One source document to build into a pack: its content ([`SourceContent`]
/// — either raw text for [`crate::parse::parse`] to parse, or an
/// already-built [`crate::tree::Document`] a caller assembled itself, e.g.
/// via [`document_from_pages`] for PDF text extracted outside this crate), a
/// display title, and a type hint (`"md"`/`"txt"`) that selects
/// [`crate::parse::parse`]'s parser for `Raw` content (ignored for
/// `Prebuilt` content, which skips `parse` entirely).
pub struct SourceInput {
    pub content: SourceContent,
    pub title: String,
    pub source_type: String,
}

/// [`SourceInput::content`]'s two shapes — the §3b bytes/`Document` seam.
/// `Raw` is K8's original behavior: text `build_pack` parses itself via
/// [`crate::parse::parse`] (MD/TXT). `Prebuilt` skips that parse step:
/// a caller — PDF text extraction (native, e.g. `kpack-pdf`), entirely
/// outside this dependency-free crate — hands in an already-built
/// [`crate::tree::Document`] (see [`document_from_pages`]), and `build_pack`
/// chunks/embeds it exactly like any parsed `Document`, since a `Document`
/// is a `Document` regardless of origin.
#[derive(Debug, Clone)]
pub enum SourceContent {
    Raw(String),
    Prebuilt(Document),
}

/// Everything [`build_pack`] needs about the pack being built that isn't
/// derivable from `sources`/`embedder`/`cfg` themselves — the manifest
/// identity and provenance fields (spec §1.2).
pub struct BuildMeta {
    pub pack_id: String,
    pub pack_version: String,
    pub pack_tier: PackTier,
    /// The embedder's display name (`embedder_name`, e.g.
    /// `"bge-base-en-v1.5-q8_0.gguf"`), stored as-is — never derived from
    /// `embedder`.
    pub embedder_name: String,
    /// The embedder GGUF's own sha256 (NOT this pack's hash) — what
    /// `Pack::mount`'s load-time gate matches against a device's installed
    /// embedders (spec §1.2). Supplied by the caller, who owns the GGUF
    /// file; this module never hashes an embedder file itself.
    pub embedder_sha256: String,
    pub built_by: String,
}

/// Placeholder `gate_abs_floor` for a personal quick build (spec §1.2). The
/// real refusal-biased personal defaults (§3.4) and curated per-pack
/// calibration (§2.4) are later milestones — K8 only proves the build
/// pipeline wires a manifest value through at all, so this number is not
/// tuned against any real probe set and MUST NOT be read as a safety
/// claim.
pub const PLACEHOLDER_GATE_ABS_FLOOR: f64 = 0.5;
/// Placeholder `gate_rel_margin` for a personal quick build — see
/// [`PLACEHOLDER_GATE_ABS_FLOOR`]'s doc comment; same caveat applies.
pub const PLACEHOLDER_GATE_REL_MARGIN: f64 = 0.05;

/// One progress tick from [`build_pack_with_progress`] (spec §3a A1).
/// `phase` is one of `"parsing"` (per source, as each is parsed+chunked),
/// `"embedding"` (per chunk, once the total chunk count is known),
/// `"writing"` (the single tick just before the manifest write + atomic
/// rename), or `"done"` (the final tick, after the rename succeeds) —
/// though this is a shared type: callers may emit their own phases through it
/// OUTSIDE `build_pack_with_progress` (src-tauri's scanned-PDF OCR step emits
/// `"ocr"` per page and `"ocr-notice"` for the soft-cap warning) —
/// `done`/`total` are chunk counts for `"embedding"`, and mirror the total
/// chunk count for `"writing"`/`"done"` (both `0` if the build produced no
/// chunks at all). Deliberately plain Rust (`Debug`/`Clone`/`PartialEq`/`Eq`
/// only, no `serde`) — this crate is Tauri-free (module doc comment) and its
/// other IPC-facing types (e.g. `Manifest`) are likewise serde-free here;
/// `src-tauri` wraps this in its own camelCase, `Serialize`-deriving event
/// type before it crosses the IPC boundary, the same `From`-conversion
/// pattern `kpack::PackManifestInfo`/`CitationInfo` already use for
/// `Manifest`/`Citation`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildProgress {
    pub phase: String,
    pub done: usize,
    pub total: usize,
}

/// Errors from [`build_pack`]/[`build_pack_with_progress`]: a small enum
/// wrapping the three failure sources ([`format::Error`], [`EmbedError`],
/// and [`std::io::Error`] from the final rename) plus a fourth,
/// cancellation ([`Error::Cancelled`], spec §3a A1) — hand-rolled (no
/// `thiserror`, matching this crate's other error types).
#[derive(Debug)]
pub enum Error {
    Format(format::Error),
    Embed(EmbedError),
    Io(std::io::Error),
    /// The caller's `cancel` flag was observed set between chunks (spec §3a
    /// A1). The `.part` file has already been removed by the time this is
    /// returned — same clean-slate guarantee as any other build failure
    /// (see the module doc comment's step 5).
    Cancelled,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Format(e) => write!(f, "pack format error: {e}"),
            Error::Embed(e) => write!(f, "embedder error: {e}"),
            Error::Io(e) => write!(f, "I/O error: {e}"),
            Error::Cancelled => write!(f, "build cancelled"),
        }
    }
}

impl std::error::Error for Error {}

impl From<format::Error> for Error {
    fn from(e: format::Error) -> Self {
        Error::Format(e)
    }
}

impl From<EmbedError> for Error {
    fn from(e: EmbedError) -> Self {
        Error::Embed(e)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

/// The exact text handed to the embedder for a chunk (spec §2.3): `text`
/// verbatim when `prefix` is empty — the case for every K8 build, since
/// this module never runs the build-time LLM prefix pass (curated-pipeline
/// only; see [`crate::chunk::ChunkDraft::prefix`]'s doc comment) — else
/// `"{prefix}\n{text}"`. Exported (not just an inline `format!` in
/// [`build_pack`]) so a future curated-pipeline caller that DOES populate
/// `prefix` reuses this exact formula rather than a hand-rolled copy that
/// could drift from what build-time chunking indexed into `fts` versus what
/// gets embedded — the same drift-guard rationale as
/// [`crate::embed::query_input`] on the query side.
pub fn passage_input(prefix: &str, text: &str) -> String {
    if prefix.is_empty() {
        text.to_string()
    } else {
        format!("{prefix}\n{text}")
    }
}

/// SHA-256 of `bytes`, lowercase hex — the exact output convention
/// `src-tauri/src/inference.rs::model_sha256` uses (`{b:02x}` per byte),
/// via the same incremental `Digest::update`/`finalize` API that function's
/// streaming read loop drives a chunk at a time. `bytes` here is already a
/// fully-loaded in-memory string — a [`SourceContent::Raw`] source's text
/// verbatim, or a [`SourceContent::Prebuilt`] `Document`'s flattened text
/// (see [`document_content_text`]) — not a multi-GB file read off disk, so
/// there is no streaming loop to mirror — the incremental API usage is the
/// part that stays consistent with `model_sha256`'s style.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// A deterministic, order-preserving flattening of every block's text in
/// `doc` — paragraph and code text verbatim, a table as its header row then
/// each data row (cells `|`-joined), one line per block — used ONLY to give
/// a [`SourceContent::Prebuilt`] source a stable hash/`extraction_quality`
/// input in place of the raw source bytes a `Raw` source has (there is no
/// original byte stream for an already-built `Document`). Two `Document`s
/// built from the same content (e.g. via [`document_from_pages`] on the same
/// pages) always flatten to the same string, so `docs.sha256` is stable
/// across builds — but this is NOT comparable to a `Raw` source's hash of
/// its actual source bytes; the two schemes are deliberately different
/// (see `sha256_hex`'s call sites in `build_pack_into_with_progress`), and
/// this function makes no attempt at cross-format equivalence (spec §3b's
/// scope is a stable per-source content hash, not that).
fn document_content_text(doc: &Document) -> String {
    let mut out = String::new();
    for section in &doc.sections {
        for block in &section.blocks {
            match block {
                Block::Paragraph { text, .. } | Block::Code { text, .. } => {
                    out.push_str(text);
                    out.push('\n');
                }
                Block::Table { header, rows, .. } => {
                    out.push_str(&header.join("|"));
                    out.push('\n');
                    for row in rows {
                        out.push_str(&row.join("|"));
                        out.push('\n');
                    }
                }
            }
        }
    }
    out
}

/// Build a page-locatored [`Document`] from already-extracted `(page,
/// text)` pairs — the pure half of §3b's PDF seam: PDF text extraction
/// itself (native, `pdfium`) lives entirely outside this dependency-free
/// crate (`kpack-pdf`), which produces exactly this `&[(u32, String)]`
/// shape and hands it to [`SourceContent::Prebuilt`]. This is the ONLY
/// structural knowledge kpack-core needs about PDFs: "here are pages of
/// text with page numbers" — no pdfium, no bytes, no native dependency.
///
/// Mirrors [`crate::parse::parse_txt`]'s shape exactly: one root
/// [`Section`] (empty `path` — no heading tree; kpack-core does no PDF
/// outline/bookmark parsing in this slice), each page's text split into
/// [`Block::Paragraph`]s on blank-line boundaries (the same rule
/// `parse_txt` uses, deliberately re-implemented here rather than imported
/// from `parse.rs`, since `parse_txt`'s version is fused with its
/// whole-document line-number locator bookkeeping, which a per-page caller
/// doesn't want) — except every block's `locator` is `"p.{page}"` (the page
/// number) instead of `parse_txt`'s `"L12"`/`"L12-L15"` line ranges, per
/// [`Block`]'s own doc comment ("a line/paragraph/page reference"). Pages
/// are processed in the given order; a page whose text has no blank-line
/// break becomes a single paragraph block; empty or whitespace-only
/// paragraphs (and pages that are empty/whitespace-only end to end) are
/// dropped, same as `parse_txt`. A `pages` slice that yields no blocks at
/// all produces zero sections (matching `parse_txt("")`'s empty-input
/// result), not an empty root `Section`.
pub fn document_from_pages(title: &str, pages: &[(u32, String)]) -> Document {
    let mut blocks = Vec::new();
    for (page, text) in pages {
        for paragraph in split_into_paragraphs(text) {
            blocks.push(Block::Paragraph {
                text: paragraph,
                locator: format!("p.{page}"),
            });
        }
    }
    let sections = if blocks.is_empty() {
        Vec::new()
    } else {
        vec![Section { path: Vec::new(), blocks }]
    };
    Document {
        title: title.to_string(),
        sections,
    }
}

/// Split `text` into trimmed, non-empty paragraphs on blank-line boundaries
/// — mirrors [`crate::parse::parse_txt`]'s paragraph-splitting rule (a blank
/// line is a paragraph break; a paragraph's interior lines are joined with
/// `\n`; each paragraph is trimmed) so PDF-page-sourced chunks split
/// identically to plain-text ones. See [`document_from_pages`]'s doc
/// comment for why this is a deliberate re-implementation, not a shared
/// function imported from `parse.rs`.
fn split_into_paragraphs(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut para_start: Option<usize> = None;
    let mut para_end = 0usize;
    for (i, line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            if let Some(start) = para_start.take() {
                push_paragraph_text(&mut out, &lines, start, para_end);
            }
        } else {
            if para_start.is_none() {
                para_start = Some(i);
            }
            para_end = i;
        }
    }
    if let Some(start) = para_start {
        push_paragraph_text(&mut out, &lines, start, para_end);
    }
    out
}

/// Join `lines[start..=end]` with `\n`, trim, and push the result onto `out`
/// unless it's empty — the per-paragraph finishing step both blank-line-break
/// points in [`split_into_paragraphs`] share.
fn push_paragraph_text(out: &mut Vec<String>, lines: &[&str], start: usize, end: usize) {
    let text = lines[start..=end].join("\n");
    let text = text.trim();
    if !text.is_empty() {
        out.push(text.to_string());
    }
}

/// Build `sources` into a `.kpack` at `out_path` using `embedder` and
/// `cfg`. See the module doc comment for the full step list. Builds into
/// `<out_path>.part` first; any failure removes the `.part` and returns the
/// error, leaving `out_path` untouched (never a half-built file at the
/// final path). On success, the `.part` is atomically renamed to
/// `out_path`.
///
/// Back-compat shim (spec §3a A1): delegates to
/// [`build_pack_with_progress`] with a no-op progress callback and a
/// `cancel` flag that is constructed fresh here and never set — so this
/// function's behavior, signature, and callers are all unchanged.
pub fn build_pack(
    sources: &[SourceInput],
    embedder: &dyn Embedder,
    meta: &BuildMeta,
    out_path: &Path,
    cfg: &ChunkConfig,
) -> Result<(), Error> {
    let cancel = AtomicBool::new(false);
    build_pack_with_progress(sources, embedder, meta, out_path, cfg, &|_| {}, &cancel)
}

/// [`build_pack`], plus progress reporting and cooperative cancellation
/// (spec §3a A1 — the build-progress-bar + cancel button seam). `progress`
/// is called synchronously on this thread (no channel, no async — pure
/// callback, same "closure, no network" shape as `download.rs`'s `emit`)
/// for every phase transition; see [`BuildProgress`]'s doc comment for the
/// exact phase sequence and what `done`/`total` mean in each. `cancel` is
/// checked once per chunk, between chunks (never mid-embed-call) — if it's
/// set, the `.part` is removed (same clean-slate guarantee as any other
/// build failure) and this returns `Err(Error::Cancelled)`.
///
/// Builds into `<out_path>.part` first; any failure — including
/// cancellation — removes the `.part` and returns the error, leaving
/// `out_path` untouched. On success, the `.part` is atomically renamed to
/// `out_path`.
pub fn build_pack_with_progress(
    sources: &[SourceInput],
    embedder: &dyn Embedder,
    meta: &BuildMeta,
    out_path: &Path,
    cfg: &ChunkConfig,
    progress: &dyn Fn(BuildProgress),
    cancel: &AtomicBool,
) -> Result<(), Error> {
    let part_path = part_path_for(out_path);
    // Every build starts from a clean slate: unconditionally discard any
    // pre-existing `.part` before `build_pack_into_with_progress`/
    // `Pack::open_or_create` ever touch it. A leftover COMPLETE `.part`
    // (from a prior crash, or a prior build whose final rename below
    // failed) would otherwise be REUSED by `open_or_create`, which opens an
    // existing file as-is (ignoring `dims`) and APPENDS to its schema —
    // doubling `docs`/`chunks` rows while `Pack::mount` succeeds silently on
    // the result. Absence of a `.part` is not an error, hence `let _ =`.
    let _ = std::fs::remove_file(&part_path);
    let total_chunks = match build_pack_into_with_progress(
        &part_path, sources, embedder, meta, cfg, progress, cancel,
    ) {
        Ok(total_chunks) => total_chunks,
        Err(e) => {
            let _ = std::fs::remove_file(&part_path);
            return Err(e);
        }
    };
    if let Err(e) = std::fs::rename(&part_path, out_path) {
        // A failed rename must not leave a complete `.part` behind either —
        // otherwise it becomes exactly the stale-leftover landmine this
        // function guards against on its NEXT invocation.
        let _ = std::fs::remove_file(&part_path);
        return Err(e.into());
    }
    progress(BuildProgress {
        phase: "done".to_string(),
        done: total_chunks,
        total: total_chunks,
    });
    Ok(())
}

/// The actual build, writing into `part_path`. Split from
/// [`build_pack_with_progress`] so the `Pack` (and its `Connection`) is
/// guaranteed to drop — closing the file — at this function's return,
/// before the caller ever attempts the rename. Returns the total chunk
/// count on success, so the caller's final `"done"` progress tick (fired
/// only after the rename succeeds, so it's not this function's job) can
/// report accurate `done`/`total` values.
///
/// Two passes over `sources`, not one, so the total chunk count is known
/// before the first `"embedding"` progress tick fires (spec §3a A1): pass 1
/// parses + chunks every source, inserting each `docs`/`chunks` row as it
/// goes (chunk embeddings aren't known yet, so `vec0` rows aren't inserted
/// here); pass 2 walks every chunk inserted in pass 1, in the same order,
/// embedding and inserting its `vec0` row — this is also where `cancel` is
/// checked, once per chunk between chunks.
fn build_pack_into_with_progress(
    part_path: &Path,
    sources: &[SourceInput],
    embedder: &dyn Embedder,
    meta: &BuildMeta,
    cfg: &ChunkConfig,
    progress: &dyn Fn(BuildProgress),
    cancel: &AtomicBool,
) -> Result<usize, Error> {
    let pack = Pack::open_or_create(part_path, embedder.dims())?;

    /// A chunk row already inserted into `pack` in pass 1, carrying just
    /// what pass 2 needs to embed it: its rowid (`format::Pack::insert_chunk`'s
    /// return) and the exact text pass 2 hands `embedder` (`prefix`/`text`,
    /// already combined via `passage_input`, kept as the two source fields
    /// rather than the combined string alone — matching the `Chunk` row's
    /// own shape, for a debug print/future field addition, at zero extra
    /// cost here).
    struct PendingChunk {
        chunk_id: i64,
        prefix: String,
        text: String,
    }

    let n_sources = sources.len();
    let mut pending: Vec<PendingChunk> = Vec::new();

    // Pass 1: parse + chunk every source, inserting `docs`/`chunks` rows.
    // `SourceContent::Raw` parses via `parse::parse` and hashes/sizes the
    // raw source text, same as always; `SourceContent::Prebuilt` skips
    // `parse::parse` (the Document is already built) and instead
    // hashes/sizes/scores `document_content_text`'s flattening of it — see
    // that function's and `sha256_hex`'s doc comments for why the two
    // schemes deliberately differ.
    for (i, source) in sources.iter().enumerate() {
        let (document, sha256, extraction_quality, source_size) = match &source.content {
            SourceContent::Raw(text) => (
                parse::parse(text, &source.title, &source.source_type),
                sha256_hex(text.as_bytes()),
                parse::extraction_quality(text),
                text.len() as i64,
            ),
            SourceContent::Prebuilt(doc) => {
                let flattened = document_content_text(doc);
                (
                    doc.clone(),
                    sha256_hex(flattened.as_bytes()),
                    parse::extraction_quality(&flattened),
                    flattened.len() as i64,
                )
            }
        };

        let doc_row = Doc {
            id: 0, // ignored by insert_doc; SQLite assigns the rowid.
            title: source.title.clone(),
            source_type: Some(source.source_type.clone()),
            sha256,
            source_path: None,
            source_size: Some(source_size),
            source_mtime: None,
            extraction_quality: Some(extraction_quality),
            added_at: rfc3339_now(),
        };
        let doc_id = pack.insert_doc(&doc_row)?;

        for draft in chunk_document(&document, embedder, cfg) {
            let chunk_row = Chunk {
                id: 0, // ignored by insert_chunk; SQLite assigns the rowid.
                doc_id,
                section_path: draft.section_path,
                locator: draft.locator,
                prefix: draft.prefix,
                text: draft.text,
                token_count: draft.token_count as i64,
            };
            let prefix = chunk_row.prefix.clone();
            let text = chunk_row.text.clone();
            let chunk_id = pack.insert_chunk(&chunk_row)?;
            pending.push(PendingChunk { chunk_id, prefix, text });
        }

        progress(BuildProgress {
            phase: "parsing".to_string(),
            done: i + 1,
            total: n_sources,
        });
    }

    // Pass 2: embed every chunk pass 1 inserted, in order. `cancel` is
    // checked here, once per chunk before that chunk's (potentially slow,
    // real-model) embed call — never mid-call.
    let total_chunks = pending.len();
    for (i, p) in pending.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }

        let embedding_input = passage_input(&p.prefix, &p.text);
        let raw_vector = embedder.embed_passage(&embedding_input)?;
        let normalized = l2_normalize(&raw_vector);
        let quantized = quantize_int8(&normalized);
        pack.insert_embedding(p.chunk_id, &quantized)?;

        progress(BuildProgress {
            phase: "embedding".to_string(),
            done: i + 1,
            total: total_chunks,
        });
    }

    progress(BuildProgress {
        phase: "writing".to_string(),
        done: total_chunks,
        total: total_chunks,
    });

    let manifest = Manifest {
        pack_id: meta.pack_id.clone(),
        pack_version: meta.pack_version.clone(),
        pack_tier: meta.pack_tier,
        embedder_name: meta.embedder_name.clone(),
        embedder_sha256: meta.embedder_sha256.clone(),
        embedding_dims: embedder.dims() as u32,
        embedding_quant: "int8".to_string(),
        chunk_target_tokens: cfg.target_tokens as u32,
        chunk_overlap_pct: cfg.overlap_pct,
        gate_abs_floor: PLACEHOLDER_GATE_ABS_FLOOR,
        gate_rel_margin: PLACEHOLDER_GATE_REL_MARGIN,
        gate_calibrated: false,
        prefixes_present: false,
        built_by: meta.built_by.clone(),
        license_ref: None,
        schema_version: format::SCHEMA_VERSION,
        vec_format_version: VEC_FORMAT_VERSION.to_string(),
    };
    manifest.write(&pack)?;

    Ok(total_chunks)
}

/// `<path>.part` — same path, `.part` appended to the whole file name.
/// Mirrors `manifest.rs`'s `sig_path_for` / `src-tauri/src/cloud/
/// download.rs`'s `part_path_for` convention.
fn part_path_for(final_path: &Path) -> PathBuf {
    let mut os = final_path.as_os_str().to_os_string();
    os.push(".part");
    PathBuf::from(os)
}

/// A UTC timestamp for `now`, RFC 3339 with second precision
/// (`"2026-07-19T12:34:56Z"`) — `docs.added_at`'s format (spec §1.1). Hand-
/// rolled from `SystemTime` rather than adding a `chrono`/`time` dependency
/// this crate doesn't otherwise need (this module's "no other dep"
/// constraint — see the crate doc comment's network-free invariant and
/// this module's own header): [`civil_from_days`] is Howard Hinnant's
/// public-domain days-since-epoch → (year, month, day) algorithm
/// (<http://howardhinnant.github.io/date_algorithms.html>), pure integer
/// arithmetic, proleptic Gregorian, no leap-second table, no locale/
/// timezone dependence — exactly what a build-time provenance stamp needs
/// and nothing more.
fn rfc3339_now() -> String {
    rfc3339_from_system_time(std::time::SystemTime::now())
}

fn rfc3339_from_system_time(t: std::time::SystemTime) -> String {
    // A build machine's clock is never before the Unix epoch in practice;
    // `unwrap_or_default()` degrades a hypothetically pre-epoch clock to
    // the epoch itself (1970-01-01T00:00:00Z) rather than panicking a
    // whole build over a merely-wrong clock.
    let since_epoch = t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let total_secs = since_epoch.as_secs();
    let days = (total_secs / 86_400) as i64;
    let secs_of_day = total_secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;
    let second = secs_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Howard Hinnant's `civil_from_days`: days since 1970-01-01 (may be
/// negative, for pre-epoch dates) → proleptic-Gregorian `(year, month,
/// day)`. Pure integer math, deterministic, no table lookups; see
/// [`rfc3339_now`]'s doc comment for why this is hand-rolled here.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let year = if month <= 2 { y + 1 } else { y };
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{render_sources, RenderChunk};
    use crate::embed::MockEmbedder;
    use crate::manifest::LoadContext;
    use std::path::PathBuf;

    /// Mirrors `format.rs`'s/`manifest.rs`'s own test helper (this repo
    /// hand-rolls temp dirs instead of depending on `tempfile`); duplicated
    /// per-module since each module's `#[cfg(test)]` helper is private to
    /// it.
    fn unique_dir(name: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "cleophis-kpack-build-test-{}-{}-{}",
            std::process::id(),
            unique,
            name
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A small fixture corpus (spec §5's "golden-pack" shape): two MD
    /// documents with headings, a table (doc 1), and a fenced code block
    /// (doc 2) — enough structural variety to exercise all three of
    /// `chunk`'s block kinds through the real `parse` → `chunk` seam.
    fn fixture_sources() -> Vec<SourceInput> {
        vec![
            SourceInput {
                title: "Vitamin K Basics".to_string(),
                source_type: "md".to_string(),
                content: SourceContent::Raw(
                    "\
# Chapter 1

## Vitamin K

Vitamin K is a fat-soluble vitamin involved in blood clotting. It also plays a role in bone metabolism.

| Drug | Class |
|------|-------|
| Warfarin | VKA |
| Heparin | Anticoagulant |
"
                    .to_string(),
                ),
            },
            SourceInput {
                title: "Getting Started".to_string(),
                source_type: "md".to_string(),
                content: SourceContent::Raw(
                    "\
# Getting Started

## Installation

Run the install script to set up the build tool on this machine.

```
cargo install kpack-cli
```

Verify the installation by checking the reported version string.
"
                    .to_string(),
                ),
            },
        ]
    }

    fn test_meta() -> BuildMeta {
        BuildMeta {
            pack_id: "test-personal-pack".to_string(),
            pack_version: "2026.07.1".to_string(),
            pack_tier: PackTier::Personal,
            embedder_name: "mock-embedder-8d".to_string(),
            embedder_sha256: "mockhash0000000000000000000000000000000000000000000000000000"
                .to_string(),
            built_by: "device-builder-test".to_string(),
        }
    }

    // 1. Golden-pack: build the fixture corpus, mount it, and prove the
    // whole pipeline (parse -> chunk -> embed -> format -> manifest)
    // actually mounts and is queryable through both retrieval lanes.
    #[test]
    fn t1_golden_pack_builds_mounts_and_queries() {
        let dir = unique_dir("t1");
        let out_path = dir.join("golden.kpack");
        let embedder = MockEmbedder::new(8);
        let cfg = ChunkConfig::default();
        let meta = test_meta();

        build_pack(&fixture_sources(), &embedder, &meta, &out_path, &cfg).unwrap();
        assert!(out_path.exists(), "build_pack should leave a file at out_path");
        assert!(
            !part_path_for(&out_path).exists(),
            "the .part file should be gone after a successful build (renamed away)"
        );

        let available = vec![meta.embedder_sha256.clone()];
        let ctx = LoadContext {
            available_embedder_sha256: &available,
            curator_key: None,
        };
        let (pack, manifest) = Pack::mount(&out_path, &ctx).unwrap();

        // Manifest keys round-trip correctly.
        assert_eq!(manifest.embedding_dims, 8);
        assert_eq!(manifest.pack_tier, PackTier::Personal);
        assert_eq!(manifest.chunk_target_tokens, cfg.target_tokens as u32);
        assert_eq!(manifest.chunk_overlap_pct, cfg.overlap_pct);
        assert!(!manifest.gate_calibrated);
        assert!(!manifest.prefixes_present);
        assert_eq!(manifest.embedder_sha256, meta.embedder_sha256);
        assert_eq!(manifest.embedding_quant, "int8");
        assert_eq!(manifest.license_ref, None);

        // Doc rows carry a real sha256 (64 lowercase hex chars) and a
        // non-empty added_at timestamp.
        let doc1 = pack.get_doc(1).unwrap().expect("doc 1 should exist");
        assert_eq!(doc1.sha256.len(), 64);
        assert!(
            doc1.sha256.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "sha256 should be lowercase hex, got {}",
            doc1.sha256
        );
        assert!(doc1.added_at.contains('T') && doc1.added_at.ends_with('Z'));
        assert_eq!(doc1.title, "Vitamin K Basics");

        // Chunk count > 0.
        assert!(
            pack.get_chunk(1).unwrap().is_some(),
            "expected at least one chunk (id=1) after a non-empty build"
        );

        // fts_search finds a known word from the fixture.
        let fts_hits = pack.fts_search("clotting", 5).unwrap();
        assert!(!fts_hits.is_empty(), "fts_search should find a chunk containing \"clotting\"");

        // vec_search finds a stored vector: recompute the exact vector
        // build_pack stored for the (single, short) Vitamin K paragraph
        // chunk — the mock embedder is deterministic, so this is the same
        // bytes on disk, not merely a "close" query.
        let known_chunk_text = "Vitamin K is a fat-soluble vitamin involved in blood clotting. It also plays a role in bone metabolism.";
        let raw = embedder.embed_passage(known_chunk_text).unwrap();
        let quantized = quantize_int8(&l2_normalize(&raw));
        let vec_hits = pack.vec_search(&quantized, 1).unwrap();
        assert_eq!(vec_hits.len(), 1, "vec_search should find the stored embedding");
        assert_eq!(
            vec_hits[0].1, 0.0,
            "top hit's distance to the recomputed vector should be exactly 0 — proving the stored \
             bytes ARE embed(known_chunk_text) quantized, not just that some row came back"
        );
        let hit_chunk = pack
            .get_chunk(vec_hits[0].0)
            .unwrap()
            .expect("vec_search hit id should resolve to a real chunk row");
        assert_eq!(
            hit_chunk.text, known_chunk_text,
            "the exact-distance hit should be the Vitamin K chunk whose vector we recomputed, \
             not merely a nearest neighbor"
        );
    }

    // 2. Build determinism (mock): building the SAME corpus twice yields
    // two packs whose chunk (section_path, locator, text, token_count)
    // sets are identical, and whose stored int8 vectors are byte-identical.
    //
    // LOUD NOTE: this proves build_pack's CODE path is deterministic under
    // the mock embedder — no randomness, no insertion-order drift, no
    // hashing-order dependence. It does NOT prove spec §5's cross-build
    // determinism claim (server pipeline vs. device builder producing
    // identical chunk sets from the SAME embedder), because chunk
    // boundaries here come from MockEmbedder::token_count, a crude
    // word-count approximation — NOT a real tokenizer. The real
    // cross-build determinism test needs the REAL tokenizer and a real
    // embedder (K4b, `BgeEmbedder`, once `--features real` builds with
    // cmake) and is a deliberate K8-onward follow-up, not this test. Do
    // not cite this test as satisfying §5.
    #[test]
    fn t2_build_pack_is_deterministic_mock_plumbing_only_not_section_5() {
        let dir = unique_dir("t2");
        let path_a = dir.join("a.kpack");
        let path_b = dir.join("b.kpack");
        let embedder = MockEmbedder::new(8);
        let cfg = ChunkConfig::default();
        let meta = test_meta();
        let sources = fixture_sources();

        build_pack(&sources, &embedder, &meta, &path_a, &cfg).unwrap();
        build_pack(&sources, &embedder, &meta, &path_b, &cfg).unwrap();

        let available = vec![meta.embedder_sha256.clone()];
        let ctx = LoadContext {
            available_embedder_sha256: &available,
            curator_key: None,
        };
        let (pack_a, _) = Pack::mount(&path_a, &ctx).unwrap();
        let (pack_b, _) = Pack::mount(&path_b, &ctx).unwrap();

        // Enumerate every chunk by id (1, 2, 3, ... until None) — insertion
        // order is deterministic and rowids are sequential from 1, so this
        // walks both packs' full chunk sets without needing a "list all"
        // API this slice doesn't otherwise need.
        let chunks_a = all_chunks(&pack_a);
        let chunks_b = all_chunks(&pack_b);
        assert!(!chunks_a.is_empty(), "fixture corpus should produce at least one chunk");

        let key = |c: &Chunk| (c.section_path.clone(), c.locator.clone(), c.text.clone(), c.token_count);
        let keys_a: Vec<_> = chunks_a.iter().map(key).collect();
        let keys_b: Vec<_> = chunks_b.iter().map(key).collect();
        assert_eq!(
            keys_a, keys_b,
            "the two builds' (section_path, locator, text, token_count) chunk sets must match"
        );

        // Byte-identical stored vectors: for every chunk, independently
        // recompute the exact quantized vector build_pack should have
        // stored (same embedder, same passage_input formula) and confirm
        // BOTH packs' vec0 lane returns that chunk as its own nearest
        // neighbor at distance 0 — i.e. what's on disk in each pack is
        // bit-for-bit that recomputed vector, and therefore bit-for-bit
        // identical to each other (both equal the same third value).
        for chunk in &chunks_a {
            let input = passage_input(&chunk.prefix, &chunk.text);
            let expected = quantize_int8(&l2_normalize(&embedder.embed_passage(&input).unwrap()));

            let hit_a = pack_a.vec_search(&expected, 1).unwrap();
            let hit_b = pack_b.vec_search(&expected, 1).unwrap();
            assert_eq!(hit_a.len(), 1);
            assert_eq!(hit_b.len(), 1);
            assert_eq!(hit_a[0].1, 0.0, "pack A's stored vector should exactly match the recomputed one");
            assert_eq!(hit_b[0].1, 0.0, "pack B's stored vector should exactly match the recomputed one");
        }
    }

    /// Walk `chunks` ids `1, 2, 3, ...` via the public `get_chunk` API
    /// until the first `None`, collecting every row — see t2's doc comment
    /// for why this is a safe full-pack enumeration here (deterministic,
    /// sequential-from-1 insertion order) without adding a new "list all"
    /// method to `format.rs`.
    fn all_chunks(pack: &Pack) -> Vec<Chunk> {
        let mut out = Vec::new();
        let mut id = 1;
        while let Some(chunk) = pack.get_chunk(id).unwrap() {
            out.push(chunk);
            id += 1;
        }
        out
    }

    // 3. No-socket: the full build -> mount -> retrieve -> render cycle
    // completes, entirely offline, on a corpus, embedder, and citation
    // renderer that are all pure in-process Rust.
    //
    // The actual network-incapability GUARANTEE is a dependency-graph fact,
    // not something a unit test can observe at runtime: `kpack-core`'s
    // `Cargo.toml` (see its own header comment) links no HTTP client
    // (`ureq`/`reqwest`/`hyper`/...), no `tokio`, nothing that can open a
    // socket — confirmed via `cargo tree -p kpack-core` showing none of
    // those crates anywhere in the resolved dependency tree. That's
    // enforced structurally: adding a socket-capable dependency here would
    // show up in that `cargo tree` output (and in a future `cargo
    // metadata`-based CI lint), not in this test. What THIS test proves is
    // the complementary fact: the full pipeline needs nothing beyond that
    // dependency-free graph to do real work.
    #[test]
    fn t3_full_cycle_completes_with_no_network_dependency() {
        let dir = unique_dir("t3");
        let out_path = dir.join("no-socket.kpack");
        let embedder = MockEmbedder::new(8);
        let cfg = ChunkConfig::default();
        let meta = test_meta();

        build_pack(&fixture_sources(), &embedder, &meta, &out_path, &cfg).unwrap();

        let available = vec![meta.embedder_sha256.clone()];
        let ctx = LoadContext {
            available_embedder_sha256: &available,
            curator_key: None,
        };
        let (pack, _manifest) = Pack::mount(&out_path, &ctx).unwrap();

        let query_vec = quantize_int8(&l2_normalize(&embedder.embed_query("Vitamin K").unwrap()));
        let vec_hits = pack.vec_search(&query_vec, 3).unwrap();
        let fts_hits = pack.fts_search("install", 5).unwrap();
        assert!(!vec_hits.is_empty());
        assert!(!fts_hits.is_empty());

        // Merge and dedup the retrieved chunk ids, resolve each to its
        // owning doc, and render through the real citation renderer (K7).
        let mut ids: Vec<i64> = vec_hits.iter().map(|(id, _)| *id).chain(fts_hits).collect();
        ids.sort_unstable();
        ids.dedup();
        assert!(!ids.is_empty());

        let resolved: Vec<(Chunk, Doc)> = ids
            .iter()
            .map(|&id| {
                let chunk = pack.get_chunk(id).unwrap().expect("retrieved id should resolve");
                let doc = pack
                    .get_doc(chunk.doc_id)
                    .unwrap()
                    .expect("chunk's doc_id should resolve");
                (chunk, doc)
            })
            .collect();
        let render_chunks: Vec<RenderChunk> = resolved
            .iter()
            .map(|(chunk, doc)| RenderChunk {
                source_title: &doc.title,
                section_path: &chunk.section_path,
                locator: &chunk.locator,
                text: &chunk.text,
            })
            .collect();

        let rendered = render_sources(&render_chunks);
        assert!(!rendered.is_empty(), "render_sources should produce non-empty citation text");
        assert!(rendered.starts_with("[1]"), "first citation should be numbered [1]");
    }

    // 4. Stale `.part` regression: simulate a crash (or a prior build whose
    // final rename failed) that left a COMPLETE, valid `.part` sitting at
    // `<out>.part`, then build again to the same `out_path`. Before Fix 1,
    // `Pack::open_or_create` would REUSE that leftover file's existing
    // schema (ignoring `dims`) and APPEND the new build's rows on top of
    // the old ones — doubled `docs`/`chunks` counts, with `Pack::mount`
    // succeeding silently on the corrupt-but-valid-looking result.
    // `build_pack` must instead discard any pre-existing `.part`
    // unconditionally before it starts, so the second build's counts match
    // a fresh build exactly — not 2x.
    #[test]
    fn t4_stale_complete_part_is_discarded_not_appended_to() {
        let dir = unique_dir("t4");
        let out_path = dir.join("stale.kpack");
        let embedder = MockEmbedder::new(8);
        let cfg = ChunkConfig::default();
        let meta = test_meta();
        let sources = fixture_sources();

        let available = vec![meta.embedder_sha256.clone()];
        let ctx = LoadContext {
            available_embedder_sha256: &available,
            curator_key: None,
        };

        // Baseline: a normal, single build establishes the CORRECT
        // doc/chunk counts we expect a second, independent build to match.
        build_pack(&sources, &embedder, &meta, &out_path, &cfg).unwrap();
        let (baseline_pack, _) = Pack::mount(&out_path, &ctx).unwrap();
        let baseline_chunk_count = all_chunks(&baseline_pack).len();
        assert!(baseline_chunk_count > 0, "fixture corpus should produce at least one chunk");
        drop(baseline_pack);

        // Plant a leftover COMPLETE `.part`: rename the just-built pack to
        // `<out>.part`, simulating a prior build that finished writing but
        // crashed (or whose final rename failed) before landing at
        // out_path. out_path itself is now gone, same as after a crash.
        let part_path = part_path_for(&out_path);
        std::fs::rename(&out_path, &part_path).unwrap();
        assert!(part_path.exists(), "stale .part should be planted");
        assert!(!out_path.exists(), "out_path should not exist yet (simulating post-crash state)");

        // Build again to the SAME out_path. If the stale `.part` were
        // reused/appended-to instead of discarded, this pack's chunk count
        // would be 2x the baseline (and doc ids 3/4 would exist alongside
        // the original 1/2).
        build_pack(&sources, &embedder, &meta, &out_path, &cfg).unwrap();

        let (pack, _manifest) = Pack::mount(&out_path, &ctx).unwrap();
        let chunks = all_chunks(&pack);
        assert_eq!(
            chunks.len(),
            baseline_chunk_count,
            "a stale .part must be discarded, not appended to — chunk count should equal a fresh \
             build's, not be doubled"
        );
        assert!(pack.get_doc(1).unwrap().is_some(), "doc 1 should exist");
        assert!(pack.get_doc(2).unwrap().is_some(), "doc 2 should exist");
        assert!(
            pack.get_doc(3).unwrap().is_none(),
            "doc count should be exactly 2 (fresh build), not 4 (doubled from stale .part reuse)"
        );
    }

    // 5. Progress: build_pack_with_progress fires "parsing" (one per
    // source), then "embedding" (one per chunk, done reaching total), then
    // "writing", then "done" — in that order — and the final "done" tick's
    // done/total match the actual chunk count, not just some number.
    #[test]
    fn t5_build_pack_with_progress_phases_fire_in_order_and_embedding_reaches_total() {
        let dir = unique_dir("t5");
        let out_path = dir.join("progress.kpack");
        let embedder = MockEmbedder::new(8);
        let cfg = ChunkConfig::default();
        let meta = test_meta();
        let sources = fixture_sources();

        let events: std::sync::Mutex<Vec<BuildProgress>> = std::sync::Mutex::new(Vec::new());
        let progress = |p: BuildProgress| events.lock().unwrap().push(p);
        let cancel = AtomicBool::new(false);

        build_pack_with_progress(&sources, &embedder, &meta, &out_path, &cfg, &progress, &cancel)
            .unwrap();

        let events = events.into_inner().unwrap();
        assert!(!events.is_empty(), "expected at least one progress tick");
        let phases: Vec<&str> = events.iter().map(|p| p.phase.as_str()).collect();

        let first_parsing = phases.iter().position(|&p| p == "parsing");
        let first_embedding = phases.iter().position(|&p| p == "embedding");
        let writing_idx = phases.iter().position(|&p| p == "writing");
        let done_idx = phases.iter().position(|&p| p == "done");
        assert!(
            first_parsing.is_some()
                && first_embedding.is_some()
                && writing_idx.is_some()
                && done_idx.is_some(),
            "expected all four phases to fire at least once, got phases: {phases:?}"
        );
        assert!(first_parsing.unwrap() < first_embedding.unwrap(), "phases: {phases:?}");
        assert!(first_embedding.unwrap() < writing_idx.unwrap(), "phases: {phases:?}");
        assert!(writing_idx.unwrap() < done_idx.unwrap(), "phases: {phases:?}");

        // Two "parsing" ticks (one per fixture source), each with the
        // correct total.
        let parsing_ticks: Vec<&BuildProgress> =
            events.iter().filter(|p| p.phase == "parsing").collect();
        assert_eq!(parsing_ticks.len(), sources.len());
        assert!(parsing_ticks.iter().all(|p| p.total == sources.len()));
        assert_eq!(parsing_ticks.last().unwrap().done, sources.len());

        // The embedding count reaches total: the LAST "embedding" tick's
        // done == total, and that total matches the fixture's real chunk
        // count (>0, and the number of "embedding" ticks fired).
        let embedding_ticks: Vec<&BuildProgress> =
            events.iter().filter(|p| p.phase == "embedding").collect();
        let last_embedding = embedding_ticks.last().unwrap();
        assert!(last_embedding.total > 0);
        assert_eq!(last_embedding.done, last_embedding.total);
        assert_eq!(embedding_ticks.len(), last_embedding.total);

        // "writing" and "done" both carry done == total == the same chunk
        // count "embedding" finished at.
        let writing_tick = &events[writing_idx.unwrap()];
        assert_eq!(writing_tick.done, writing_tick.total);
        assert_eq!(writing_tick.total, last_embedding.total);
        let done_tick = &events[done_idx.unwrap()];
        assert_eq!(done_tick.done, done_tick.total);
        assert_eq!(done_tick.total, last_embedding.total);

        assert!(out_path.exists());
        assert!(!part_path_for(&out_path).exists());
    }

    // 6. Cancel: setting `cancel` from inside the progress callback right
    // after the FIRST chunk's "embedding" tick must stop the build before
    // any further chunk is embedded, return Error::Cancelled, and leave NO
    // output file — only the `.part`, itself removed (never left half-built
    // at either path). The fixture corpus produces more than one chunk (a
    // paragraph + a table in doc 1, a paragraph + a code block in doc 2), so
    // this genuinely proves an early stop, not an artifact of a one-chunk
    // corpus.
    #[test]
    fn t6_cancel_after_first_chunk_returns_cancelled_and_leaves_no_output() {
        let dir = unique_dir("t6");
        let out_path = dir.join("cancelled.kpack");
        let embedder = MockEmbedder::new(8);
        let cfg = ChunkConfig::default();
        let meta = test_meta();
        let sources = fixture_sources();

        let cancel = AtomicBool::new(false);
        let embedding_ticks_seen = std::sync::Mutex::new(0usize);
        let progress = |p: BuildProgress| {
            if p.phase == "embedding" {
                *embedding_ticks_seen.lock().unwrap() += 1;
                if p.done == 1 {
                    cancel.store(true, Ordering::Relaxed);
                }
            }
        };

        let result =
            build_pack_with_progress(&sources, &embedder, &meta, &out_path, &cfg, &progress, &cancel);

        assert!(
            matches!(result, Err(Error::Cancelled)),
            "expected Err(Error::Cancelled), got {result:?}"
        );
        assert_eq!(result.unwrap_err().to_string(), "build cancelled");
        assert!(!out_path.exists(), "no output file should exist after a cancelled build");
        assert!(
            !part_path_for(&out_path).exists(),
            "the .part file should be removed after a cancelled build"
        );
        // Only the first chunk's "embedding" tick fired — the cancel check
        // at the top of the NEXT chunk's iteration stopped the build before
        // a second chunk was ever embedded.
        assert_eq!(*embedding_ticks_seen.lock().unwrap(), 1);
    }

    // 7. HTML round-trip (formats slice): a SourceContent::Raw HTML source
    // with source_type "html" takes the SAME Raw path as md/txt (no
    // build-time branch — see source_type_for's doc comment in
    // src-tauri/src/kpack.rs) all the way to a mounted, queryable chunk
    // whose locator is HTML-derived (the heading's "#id" anchor, not a
    // markdown line ref) — proving parse::parse's "html" arm ->
    // html::html_to_document -> chunk -> format seam works end to end, the
    // same way t1 proves it for markdown.
    #[test]
    fn t7_html_source_round_trips_through_build_and_mount_with_html_locator() {
        let dir = unique_dir("t7");
        let out_path = dir.join("html.kpack");
        let embedder = MockEmbedder::new(8);
        let cfg = ChunkConfig::default();
        let meta = test_meta();

        let html = "\
<html><body>
<h1 id=\"intro\">Introduction</h1>
<p>HTML ingestion proves the parse to chunk to format seam works end to end.</p>
</body></html>";
        let sources = vec![SourceInput {
            title: "HTML Doc".to_string(),
            source_type: "html".to_string(),
            content: SourceContent::Raw(html.to_string()),
        }];

        build_pack(&sources, &embedder, &meta, &out_path, &cfg).unwrap();

        let available = vec![meta.embedder_sha256.clone()];
        let ctx = LoadContext {
            available_embedder_sha256: &available,
            curator_key: None,
        };
        let (pack, _manifest) = Pack::mount(&out_path, &ctx).unwrap();

        let chunk = pack
            .get_chunk(1)
            .unwrap()
            .expect("expected at least one chunk from the HTML source");
        assert_eq!(chunk.section_path, "Introduction");
        assert_eq!(
            chunk.locator, "#intro",
            "an HTML chunk's locator should be the enclosing heading's id anchor, \
             not a markdown-style line reference"
        );
        assert!(chunk.text.contains("HTML ingestion proves"));
    }

    // 8. DOCX round-trip (formats slice): a SourceContent::Prebuilt(
    // document_from_docx(fixture)) fed through build_pack (mock embedder)
    // mounts, and a retrieved chunk carries a "¶{n}" locator — proving the
    // DOCX locator survives parse-skip -> chunk -> citation, the same
    // Prebuilt-path proof `prebuilt_document_round_trips_through_build_pack_
    // with_page_locator` gives PDF's "p.{page}" locator.
    #[test]
    fn t8_docx_source_round_trips_through_build_and_mount_with_paragraph_locator() {
        let dir = unique_dir("t8-docx");
        let out_path = dir.join("docx.kpack");
        let embedder = MockEmbedder::new(8);
        let cfg = ChunkConfig::default();
        let meta = test_meta();

        let sample_docx: &[u8] = include_bytes!("../tests/fixtures/sample.docx");
        let document = crate::docx::document_from_docx(sample_docx, "Sample DOCX").unwrap();
        let sources = vec![SourceInput {
            title: "Sample DOCX".to_string(),
            source_type: "docx".to_string(),
            content: SourceContent::Prebuilt(document),
        }];

        build_pack(&sources, &embedder, &meta, &out_path, &cfg).unwrap();

        let available = vec![meta.embedder_sha256.clone()];
        let ctx = LoadContext {
            available_embedder_sha256: &available,
            curator_key: None,
        };
        let (pack, _manifest) = Pack::mount(&out_path, &ctx).unwrap();

        let fts_hits = pack.fts_search("chapter", 5).unwrap();
        assert!(!fts_hits.is_empty(), "fts_search should find a chunk from the DOCX fixture");
        let hit_chunk = pack
            .get_chunk(fts_hits[0])
            .unwrap()
            .expect("fts hit id should resolve to a real chunk row");
        assert!(
            hit_chunk.locator.starts_with('\u{00b6}'),
            "DOCX chunks should carry a \"\u{00b6}{{n}}\" locator, got {:?}",
            hit_chunk.locator
        );
    }

    // 9. EPUB round-trip (formats slice): the EPUB analog of t8 — a
    // SourceContent::Prebuilt(document_from_epub(fixture)) mounts, and a
    // retrieved chunk's locator carries the chapter href prefix
    // document_from_epub stamps on (see epub.rs's module doc comment).
    #[test]
    fn t9_epub_source_round_trips_through_build_and_mount_with_chapter_locator() {
        let dir = unique_dir("t9-epub");
        let out_path = dir.join("epub.kpack");
        let embedder = MockEmbedder::new(8);
        let cfg = ChunkConfig::default();
        let meta = test_meta();

        let sample_epub: &[u8] = include_bytes!("../tests/fixtures/sample.epub");
        let document = crate::epub::document_from_epub(sample_epub, "Sample EPUB").unwrap();
        let sources = vec![SourceInput {
            title: "Sample EPUB".to_string(),
            source_type: "epub".to_string(),
            content: SourceContent::Prebuilt(document),
        }];

        build_pack(&sources, &embedder, &meta, &out_path, &cfg).unwrap();

        let available = vec![meta.embedder_sha256.clone()];
        let ctx = LoadContext {
            available_embedder_sha256: &available,
            curator_key: None,
        };
        let (pack, _manifest) = Pack::mount(&out_path, &ctx).unwrap();

        let fts_hits = pack.fts_search("paragraph", 5).unwrap();
        assert!(!fts_hits.is_empty(), "fts_search should find a chunk from the EPUB fixture");
        let hit_chunk = pack
            .get_chunk(fts_hits[0])
            .unwrap()
            .expect("fts hit id should resolve to a real chunk row");
        assert!(
            hit_chunk.locator.contains(".xhtml#"),
            "EPUB chunks should carry a chapter-href-prefixed locator, got {:?}",
            hit_chunk.locator
        );
    }

    // civil_from_days / rfc3339_from_system_time: hand-verified against
    // independently computed reference dates (Python's datetime), not just
    // round-tripped against this module's own arithmetic.
    #[test]
    fn civil_from_days_matches_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(11_017), (2000, 3, 1));
        assert_eq!(civil_from_days(20_653), (2026, 7, 19));
        assert_eq!(civil_from_days(19_782), (2024, 2, 29)); // leap day
        assert_eq!(civil_from_days(-1), (1969, 12, 31)); // pre-epoch
    }

    #[test]
    fn rfc3339_from_system_time_formats_known_instant() {
        let secs = 20_653u64 * 86_400 + 12 * 3600 + 34 * 60 + 56;
        let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs);
        assert_eq!(rfc3339_from_system_time(t), "2026-07-19T12:34:56Z");
    }

    // passage_input: empty prefix -> text verbatim; non-empty prefix ->
    // "{prefix}\n{text}" (spec §2.3's formula).
    #[test]
    fn passage_input_formula() {
        assert_eq!(passage_input("", "body text"), "body text");
        assert_eq!(
            passage_input("Chapter 2 > Setup", "body text"),
            "Chapter 2 > Setup\nbody text"
        );
    }

    // sha256_hex: matches a known SHA-256 test vector, lowercase hex.
    #[test]
    fn sha256_hex_known_vector() {
        // NIST test vector for SHA-256("abc").
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    // document_from_pages: two pages, each multi-paragraph -> the right
    // block count, "p.1"/"p.2" locators on the right blocks, one root
    // section with an empty section_path (no heading tree in this slice).
    #[test]
    fn document_from_pages_splits_paragraphs_and_locates_by_page() {
        let pages = vec![
            (1u32, "First paragraph on page one.\n\nSecond paragraph on page one.".to_string()),
            (2u32, "Only paragraph on page two, no blank lines inside it.".to_string()),
        ];
        let doc = document_from_pages("Scanned Book", &pages);

        assert_eq!(doc.sections.len(), 1);
        assert_eq!(doc.sections[0].path, Vec::<String>::new());

        let blocks = &doc.sections[0].blocks;
        assert_eq!(blocks.len(), 3, "2 paragraphs on page 1 + 1 on page 2 = 3 blocks");

        let expect = |b: &Block, text: &str, locator: &str| match b {
            Block::Paragraph { text: t, locator: l } => {
                assert_eq!(t, text);
                assert_eq!(l, locator);
            }
            other => panic!("expected Block::Paragraph, got {other:?}"),
        };
        expect(&blocks[0], "First paragraph on page one.", "p.1");
        expect(&blocks[1], "Second paragraph on page one.", "p.1");
        expect(
            &blocks[2],
            "Only paragraph on page two, no blank lines inside it.",
            "p.2",
        );
    }

    // document_from_pages: empty/whitespace-only paragraphs (and a
    // whitespace-only page) are dropped entirely, same as parse_txt.
    #[test]
    fn document_from_pages_drops_empty_and_whitespace_only_paragraphs() {
        let pages = vec![
            (1u32, "Real content.\n\n   \n\n\nMore real content.\n\n".to_string()),
            (2u32, "   \n\n  \t  \n".to_string()), // whitespace-only page -> no blocks at all
        ];
        let doc = document_from_pages("Doc", &pages);

        assert_eq!(doc.sections.len(), 1);
        let blocks = &doc.sections[0].blocks;
        assert_eq!(blocks.len(), 2, "the whitespace-only paragraphs and page 2 should contribute nothing");
        for b in blocks {
            match b {
                Block::Paragraph { locator, .. } => assert_eq!(locator, "p.1"),
                other => panic!("expected Block::Paragraph, got {other:?}"),
            }
        }
    }

    // document_from_pages: every page empty/whitespace-only -> zero
    // sections (matches parse_txt("")'s empty-input result), not an empty
    // root Section.
    #[test]
    fn document_from_pages_all_empty_pages_yields_no_sections() {
        let pages = vec![(1u32, "".to_string()), (2u32, "   \n\n".to_string())];
        let doc = document_from_pages("Doc", &pages);
        assert!(doc.sections.is_empty());
    }

    // document_from_pages's paragraph splitting matches parse_txt's exactly
    // (same blank-line-boundary rule) on the same source text — the direct
    // proof that PDF-page-sourced chunks split identically to plain-text
    // ones, as the brief requires.
    #[test]
    fn document_from_pages_paragraph_split_matches_parse_txt_behavior() {
        let text = "First paragraph,\nstill first.\n\nSecond paragraph.\n\n\nThird paragraph.\n";

        let txt_doc = parse::parse_txt(text, "Notes");
        let page_doc = document_from_pages("Notes", &[(1, text.to_string())]);

        let paragraph_texts = |doc: &Document| -> Vec<String> {
            doc.sections[0]
                .blocks
                .iter()
                .map(|b| match b {
                    Block::Paragraph { text, .. } => text.clone(),
                    other => panic!("expected Block::Paragraph, got {other:?}"),
                })
                .collect()
        };

        assert_eq!(paragraph_texts(&txt_doc), paragraph_texts(&page_doc));
    }

    // Round-trip: a SourceContent::Prebuilt(document_from_pages(...)) fed
    // through build_pack (mock embedder) mounts, and a vec_search/fts_search
    // hit's chunk carries the "p.N" locator — proving the page locator
    // survives parse-skip -> chunk -> citation, not just that
    // document_from_pages builds a well-formed Document in isolation.
    #[test]
    fn prebuilt_document_round_trips_through_build_pack_with_page_locator() {
        let dir = unique_dir("prebuilt-round-trip");
        let out_path = dir.join("prebuilt.kpack");
        let embedder = MockEmbedder::new(8);
        let cfg = ChunkConfig::default();
        let meta = test_meta();

        let clotting_text = "Vitamin K is a fat-soluble vitamin involved in blood clotting.";
        let pages = vec![
            (1u32, clotting_text.to_string()),
            (2u32, "Warfarin is a vitamin K antagonist used as an anticoagulant.".to_string()),
        ];
        let document = document_from_pages("Extracted PDF", &pages);
        let sources = vec![SourceInput {
            title: "Extracted PDF".to_string(),
            source_type: "pdf".to_string(),
            content: SourceContent::Prebuilt(document),
        }];

        build_pack(&sources, &embedder, &meta, &out_path, &cfg).unwrap();

        let available = vec![meta.embedder_sha256.clone()];
        let ctx = LoadContext {
            available_embedder_sha256: &available,
            curator_key: None,
        };
        let (pack, _manifest) = Pack::mount(&out_path, &ctx).unwrap();

        // fts5 lane: the chunk containing "clotting" carries the page-1
        // locator its originating block was built with.
        let fts_hits = pack.fts_search("clotting", 5).unwrap();
        assert!(!fts_hits.is_empty(), "fts_search should find the prebuilt-document's chunk");
        let hit_chunk = pack
            .get_chunk(fts_hits[0])
            .unwrap()
            .expect("fts hit id should resolve to a real chunk row");
        assert_eq!(
            hit_chunk.locator, "p.1",
            "the page locator must survive parse-skip -> chunk -> citation"
        );

        // vec0 lane: a query vector recomputed for the exact page-1 text
        // should also find a stored embedding (pass 2 embedded the
        // Prebuilt document's chunks same as any parsed one).
        let raw = embedder.embed_passage(clotting_text).unwrap();
        let query_i8 = quantize_int8(&l2_normalize(&raw));
        let vec_hits = pack.vec_search(&query_i8, 1).unwrap();
        assert_eq!(vec_hits.len(), 1);
        assert_eq!(vec_hits[0].1, 0.0, "stored vector should exactly match the recomputed one");
    }

    // Prebuilt-hash determinism (B2-review follow-up): t2 above only proves
    // build_pack is deterministic for Raw sources; document_content_text
    // (docs.sha256's input for a Prebuilt source) and document_from_pages
    // are deterministic by inspection today, but weren't locked by a test.
    // Building the SAME SourceContent::Prebuilt(document_from_pages(...))
    // document twice must yield byte-identical docs.sha256 and
    // byte-identical stored int8 embeddings across both builds — mirroring
    // t2's proof, for the Prebuilt path this time.
    #[test]
    fn prebuilt_document_build_is_deterministic() {
        let dir = unique_dir("prebuilt-determinism");
        let path_a = dir.join("a.kpack");
        let path_b = dir.join("b.kpack");
        let embedder = MockEmbedder::new(8);
        let cfg = ChunkConfig::default();
        let meta = test_meta();

        let pages = vec![
            (
                1u32,
                "Vitamin K is a fat-soluble vitamin involved in blood clotting.\n\n\
                 It also plays a role in bone metabolism."
                    .to_string(),
            ),
            (2u32, "Warfarin is a vitamin K antagonist used as an anticoagulant.".to_string()),
        ];
        let document = document_from_pages("Extracted PDF", &pages);
        let sources = vec![SourceInput {
            title: "Extracted PDF".to_string(),
            source_type: "pdf".to_string(),
            content: SourceContent::Prebuilt(document),
        }];

        build_pack(&sources, &embedder, &meta, &path_a, &cfg).unwrap();
        build_pack(&sources, &embedder, &meta, &path_b, &cfg).unwrap();

        let available = vec![meta.embedder_sha256.clone()];
        let ctx = LoadContext {
            available_embedder_sha256: &available,
            curator_key: None,
        };
        let (pack_a, _) = Pack::mount(&path_a, &ctx).unwrap();
        let (pack_b, _) = Pack::mount(&path_b, &ctx).unwrap();

        // docs.sha256: document_content_text's flattening of the SAME
        // Prebuilt Document must hash the same both times.
        let doc_a = pack_a.get_doc(1).unwrap().expect("doc 1 should exist in pack A");
        let doc_b = pack_b.get_doc(1).unwrap().expect("doc 1 should exist in pack B");
        assert_eq!(doc_a.sha256.len(), 64);
        assert_eq!(
            doc_a.sha256, doc_b.sha256,
            "docs.sha256 must be byte-identical across two builds of the same Prebuilt document"
        );

        // Stored int8 embeddings: same (section_path, locator, text,
        // token_count) chunk keys, and byte-identical stored vectors —
        // t2's exact proof technique, applied to the Prebuilt path.
        let chunks_a = all_chunks(&pack_a);
        let chunks_b = all_chunks(&pack_b);
        assert!(!chunks_a.is_empty(), "fixture pages should produce at least one chunk");

        let key = |c: &Chunk| (c.section_path.clone(), c.locator.clone(), c.text.clone(), c.token_count);
        let keys_a: Vec<_> = chunks_a.iter().map(key).collect();
        let keys_b: Vec<_> = chunks_b.iter().map(key).collect();
        assert_eq!(keys_a, keys_b, "the two builds' chunk sets must match");

        for chunk in &chunks_a {
            let input = passage_input(&chunk.prefix, &chunk.text);
            let expected = quantize_int8(&l2_normalize(&embedder.embed_passage(&input).unwrap()));

            let hit_a = pack_a.vec_search(&expected, 1).unwrap();
            let hit_b = pack_b.vec_search(&expected, 1).unwrap();
            assert_eq!(hit_a.len(), 1);
            assert_eq!(hit_b.len(), 1);
            assert_eq!(hit_a[0].1, 0.0, "pack A's stored vector should exactly match the recomputed one");
            assert_eq!(hit_b[0].1, 0.0, "pack B's stored vector should exactly match the recomputed one");
        }
    }
}
