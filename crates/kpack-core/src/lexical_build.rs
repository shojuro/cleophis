//! The lexical-only pack builder (Phase 1h M4b) — the bundled NHS reference
//! pack's writer. Same chunker, same parser and same `docs`/`chunks`/
//! `titles`/`fts` rows as [`crate::build::build_pack`], but:
//!
//! - **Merged (curated).** With `LexicalBuildMeta::merge`, small sibling
//!   chunks of a section are joined (`chunk::chunk_document_merged`); the
//!   sizes go to the manifest (`merge_target_tokens`, `merge_max_tokens`).
//! - **No embedder.** Chunks are measured by a [`TokenCounter`] (the BGE
//!   WordPiece port, [`crate::wordpiece`]) through
//!   [`chunk_document_with`], so the pack chunks exactly as the desktop
//!   does and nothing is embedded. The `vec` table exists (the schema and
//!   the load gate require it) but is empty, one dimension wide.
//! - **Refuses dense use.** The manifest's embedder hash is
//!   [`LEXICAL_EMBEDDER_SHA256`], which matches no embedder file, so
//!   `Pack::mount` (dense) refuses the pack and only `Pack::mount_lexical`
//!   opens it; the dense gate floor is [`LEXICAL_GATE_ABS_FLOOR`] (no
//!   cosine can clear it) and `gate_calibrated` is false: nothing was
//!   calibrated, because the lexical gate is the title match itself
//!   (`crate::lookup`, design ruling I4) and reads no manifest field.
//! - **Reproducible.** Every timestamp derives from `source_date_epoch`
//!   (`docs.added_at`); rows go in the caller's order; everything lands in
//!   one transaction and the file is `VACUUM`ed, so the same inputs give a
//!   byte-identical `.kpack` (the signature is over those bytes).
//! - **Content-addressed.** Each chunk has a [`chunk_content_sha256`]; the
//!   pack's [`pack_content_sha256`] over them, in pack order, is written to
//!   the manifest as `content_sha256`.
//! - **Cited.** Each doc keeps its URL in `docs.source_path` and its
//!   retrieval date in `docs.source_mtime` (for `nhs-web`, see
//!   `retrieve::CitationSource`).

use crate::build::{part_path_for, rfc3339_from_system_time, sha256_hex, title_entry_for, Error};
use crate::chunk::{chunk_document_merged, chunk_document_with, ChunkConfig, MergeConfig, TokenCounter};
use crate::format::{self, Chunk, Doc, Pack};
use crate::manifest::{Manifest, PackTier, VEC_FORMAT_VERSION};
use crate::parse;
use std::path::Path;

/// `embedder_name` of a lexical-only pack.
pub const LEXICAL_EMBEDDER_NAME: &str = "none (lexical-only pack)";
/// `embedder_sha256` of a lexical-only pack: not a hex digest, so it can
/// never equal an installed embedder's hash — the dense mount refuses it.
pub const LEXICAL_EMBEDDER_SHA256: &str = "none-lexical-only";
/// The (empty) `vec` table's width: the smallest the schema allows.
pub const LEXICAL_EMBEDDING_DIMS: usize = 1;
/// The dense gate floor of a lexical-only pack: no cosine exceeds 1, so a
/// dense lane over this pack could never pass (fail closed).
pub const LEXICAL_GATE_ABS_FLOOR: f64 = 1.0;
/// Manifest key recording [`pack_content_sha256`].
pub const MANIFEST_CONTENT_SHA256: &str = "content_sha256";
/// Manifest key recording `source_date_epoch`.
pub const MANIFEST_SOURCE_DATE_EPOCH: &str = "source_date_epoch";
/// Manifest key recording the retrieval mode (`"lexical"`).
pub const MANIFEST_RETRIEVAL_MODE: &str = "retrieval_mode";
/// Manifest keys recording the curated merge post-pass's sizes.
pub const MANIFEST_MERGE_TARGET_TOKENS: &str = "merge_target_tokens";
pub const MANIFEST_MERGE_MAX_TOKENS: &str = "merge_max_tokens";

/// One page of a lexical pack, as the caller read it.
#[derive(Debug, Clone)]
pub struct LexicalSource {
    /// Display title (`docs.title`, `titles.title`).
    pub title: String,
    /// The page's slug (slugified into `titles.slug`).
    pub slug: String,
    /// Aliases, brands and other names the title index should match.
    pub variants: Vec<String>,
    /// The Markdown body, front matter already removed.
    pub body: String,
    /// `docs.source_type`, e.g. [`format::SOURCE_TYPE_NHS_WEB`].
    pub source_type: String,
    /// `docs.source_path` — the page URL for `nhs-web`.
    pub source_path: Option<String>,
    /// `docs.source_mtime` — the retrieval date for `nhs-web`.
    pub source_mtime: Option<String>,
    /// sha256 (hex) and byte length of the raw source file.
    pub sha256: String,
    pub source_size: i64,
}

/// Identity and provenance of a lexical pack.
#[derive(Debug, Clone)]
pub struct LexicalBuildMeta {
    pub pack_id: String,
    pub pack_version: String,
    pub pack_tier: PackTier,
    pub built_by: String,
    pub license_ref: Option<String>,
    /// Seconds since the Unix epoch; every timestamp in the pack.
    pub source_date_epoch: u64,
    /// Further manifest keys, written verbatim after the standard ones.
    pub extra_manifest: Vec<(String, String)>,
    /// The curated merge post-pass (`chunk::chunk_document_merged`), or
    /// `None` for the plain chunker. Curated packs only: `Some` with
    /// `PackTier::Personal` is refused.
    pub merge: Option<MergeConfig>,
}

/// What [`build_lexical_pack`] wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexicalBuildReport {
    pub docs: usize,
    pub chunks: usize,
    pub content_sha256: String,
}

/// sha256 (hex) of one chunk's content: the compact JSON array
/// `[text, section_path, locator, title]` — exactly Python's
/// `json.dumps([text, section_path, locator, title], ensure_ascii=False,
/// separators=(",", ":"))` (and serde_json's compact output), UTF-8.
pub fn chunk_content_sha256(text: &str, section_path: &str, locator: &str, title: &str) -> String {
    let mut s = String::from("[");
    for (i, field) in [text, section_path, locator, title].iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        push_json_string(&mut s, field);
    }
    s.push(']');
    sha256_hex(s.as_bytes())
}

/// sha256 (hex) of a pack's content: over each chunk's content sha, in pack
/// order, each as 64 lowercase hex characters followed by `\n`.
pub fn pack_content_sha256<'a>(chunk_shas: impl IntoIterator<Item = &'a str>) -> String {
    let mut s = String::new();
    for sha in chunk_shas {
        s.push_str(sha);
        s.push('\n');
    }
    sha256_hex(s.as_bytes())
}

/// A JSON string literal with Python's / serde_json's escapes: `"` `\`
/// `\b` `\f` `\n` `\r` `\t`, other controls below U+0020 as `\u00xx`
/// (lowercase hex); everything else verbatim.
fn push_json_string(out: &mut String, v: &str) {
    out.push('"');
    for c in v.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Build `sources`, in the given order, into a lexical-only `.kpack` at
/// `out_path` (via `<out_path>.part` and an atomic rename, like
/// `build_pack`). Refuses (before writing anything) a source with an empty
/// title, a slug that slugifies to nothing or repeats another's, and (while
/// building) a source that yields no chunk — a page with no body could
/// never be found. See the module doc for everything else.
pub fn build_lexical_pack(
    sources: &[LexicalSource],
    tokens: &dyn TokenCounter,
    cfg: &ChunkConfig,
    meta: &LexicalBuildMeta,
    out_path: &Path,
) -> Result<LexicalBuildReport, Error> {
    if meta.merge.is_some() && meta.pack_tier != PackTier::Curated {
        return Err(schema("the chunk merge post-pass is for curated packs only".to_string()));
    }
    let mut slugs: Vec<String> = Vec::with_capacity(sources.len());
    for s in sources {
        let slug = crate::lookup::slugify(&s.slug);
        if s.title.trim().is_empty() || slug.is_empty() {
            return Err(schema(format!("source {:?} has an empty title or slug", s.slug)));
        }
        if slugs.contains(&slug) {
            return Err(schema(format!("duplicate slug {slug:?}")));
        }
        slugs.push(slug);
    }

    let part = part_path_for(out_path);
    let _ = std::fs::remove_file(&part);
    let report = match build_into(&part, sources, tokens, cfg, meta) {
        Ok(r) => r,
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            return Err(e);
        }
    };
    if let Err(e) = std::fs::rename(&part, out_path) {
        let _ = std::fs::remove_file(&part);
        return Err(e.into());
    }
    Ok(report)
}

fn schema(msg: String) -> Error {
    Error::Format(format::Error::Schema(msg))
}

fn build_into(
    part: &Path,
    sources: &[LexicalSource],
    tokens: &dyn TokenCounter,
    cfg: &ChunkConfig,
    meta: &LexicalBuildMeta,
) -> Result<LexicalBuildReport, Error> {
    let added_at = rfc3339_from_system_time(
        std::time::UNIX_EPOCH + std::time::Duration::from_secs(meta.source_date_epoch),
    );
    let pack = Pack::open_or_create(part, LEXICAL_EMBEDDING_DIMS)?;
    pack.begin_bulk()?;
    let mut chunk_shas: Vec<String> = Vec::new();
    for source in sources {
        let document = parse::parse(&source.body, &source.title, "md");
        let doc_id = pack.insert_doc(&Doc {
            id: 0,
            title: source.title.clone(),
            source_type: Some(source.source_type.clone()),
            sha256: source.sha256.clone(),
            source_path: source.source_path.clone(),
            source_size: Some(source.source_size),
            source_mtime: source.source_mtime.clone(),
            extraction_quality: Some(parse::extraction_quality(&source.body)),
            added_at: added_at.clone(),
        })?;
        let variants: Vec<&str> = source.variants.iter().map(String::as_str).collect();
        pack.insert_title(&title_entry_for(doc_id, &source.title, Some(&source.slug), &variants))?;
        let drafts = match &meta.merge {
            Some(m) => chunk_document_merged(&document, tokens, cfg, m),
            None => chunk_document_with(&document, tokens, cfg),
        };
        if drafts.is_empty() {
            return Err(schema(format!("source {:?} yields no chunk", source.slug)));
        }
        for draft in drafts {
            chunk_shas.push(chunk_content_sha256(&draft.text, &draft.section_path, &draft.locator, &source.title));
            pack.insert_chunk(&Chunk {
                id: 0,
                doc_id,
                section_path: draft.section_path,
                locator: draft.locator,
                prefix: draft.prefix,
                text: draft.text,
                token_count: draft.token_count as i64,
            })?;
        }
    }
    let content_sha256 = pack_content_sha256(chunk_shas.iter().map(String::as_str));
    Manifest {
        pack_id: meta.pack_id.clone(),
        pack_version: meta.pack_version.clone(),
        pack_tier: meta.pack_tier,
        embedder_name: LEXICAL_EMBEDDER_NAME.to_string(),
        embedder_sha256: LEXICAL_EMBEDDER_SHA256.to_string(),
        embedding_dims: LEXICAL_EMBEDDING_DIMS as u32,
        embedding_quant: "int8".to_string(),
        chunk_target_tokens: cfg.target_tokens as u32,
        chunk_overlap_pct: cfg.overlap_pct,
        gate_abs_floor: LEXICAL_GATE_ABS_FLOOR,
        gate_rel_margin: 0.0,
        gate_calibrated: false,
        prefixes_present: false,
        built_by: meta.built_by.clone(),
        license_ref: meta.license_ref.clone(),
        schema_version: format::SCHEMA_VERSION,
        vec_format_version: VEC_FORMAT_VERSION.to_string(),
    }
    .write(&pack)?;
    pack.manifest_set(MANIFEST_RETRIEVAL_MODE, "lexical")?;
    pack.manifest_set(MANIFEST_CONTENT_SHA256, &content_sha256)?;
    pack.manifest_set(MANIFEST_SOURCE_DATE_EPOCH, &meta.source_date_epoch.to_string())?;
    if let Some(m) = &meta.merge {
        pack.manifest_set(MANIFEST_MERGE_TARGET_TOKENS, &m.target_tokens.to_string())?;
        pack.manifest_set(MANIFEST_MERGE_MAX_TOKENS, &m.max_tokens.to_string())?;
    }
    for (k, v) in &meta.extra_manifest {
        pack.manifest_set(k, v)?;
    }
    pack.commit_bulk()?;
    pack.vacuum()?;
    Ok(LexicalBuildReport {
        docs: sources.len(),
        chunks: chunk_shas.len(),
        content_sha256,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lookup::{retrieve_lexical, LexicalOutcome};
    use crate::manifest::LoadContext;
    use crate::retrieve::CitationSource;
    use std::path::PathBuf;

    fn unique_dir(name: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "cleophis-kpack-lexical-test-{}-{}-{}",
            std::process::id(),
            unique,
            name
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    struct Words;
    impl TokenCounter for Words {
        fn token_count(&self, text: &str) -> usize {
            text.split_whitespace().count()
        }
        fn max_input_tokens(&self) -> usize {
            512
        }
    }

    fn page(title: &str, slug: &str, variants: &[&str], body: &str) -> LexicalSource {
        LexicalSource {
            title: title.to_string(),
            slug: slug.to_string(),
            variants: variants.iter().map(|v| v.to_string()).collect(),
            body: body.to_string(),
            source_type: format::SOURCE_TYPE_NHS_WEB.to_string(),
            source_path: Some(format!("https://www.nhs.uk/conditions/{slug}/")),
            source_mtime: Some("2026-09-26".to_string()),
            sha256: sha256_hex(body.as_bytes()),
            source_size: body.len() as i64,
        }
    }

    fn sources() -> Vec<LexicalSource> {
        vec![
            page("Asthma", "asthma", &[], "# Asthma\n\nA lung condition.\n\n## Symptoms\n\nWheezing.\n"),
            page("Gout", "gout", &["gouty arthritis"], "# Gout\n\nJoint pain.\n\n## Treatment\n\nRest.\n"),
            page(
                "Chronic obstructive pulmonary disease (COPD)",
                "chronic-obstructive-pulmonary-disease-copd",
                &["COPD", "COPD"],
                "# COPD\n\nA lung disease.\n",
            ),
        ]
    }

    fn meta(epoch: u64) -> LexicalBuildMeta {
        LexicalBuildMeta {
            pack_id: "reference-test".to_string(),
            pack_version: "2026.09.1".to_string(),
            pack_tier: PackTier::Personal,
            built_by: "lexical-test".to_string(),
            license_ref: Some("OGL v3".to_string()),
            source_date_epoch: epoch,
            extra_manifest: vec![("tokenizer".to_string(), "words".to_string())],
            merge: None,
        }
    }

    fn build(name: &str, epoch: u64) -> (PathBuf, LexicalBuildReport) {
        let path = unique_dir(name).join("ref.kpack");
        let r = build_lexical_pack(&sources(), &Words, &ChunkConfig::default(), &meta(epoch), &path).unwrap();
        (path, r)
    }

    fn ctx() -> LoadContext<'static> {
        LoadContext {
            available_embedder_sha256: &[],
            curator_key: None,
        }
    }

    #[test]
    fn same_inputs_same_bytes_and_timestamps_follow_the_epoch() {
        let (a, ra) = build("det-a", 1_790_380_800);
        let (b, rb) = build("det-b", 1_790_380_800);
        assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
        assert_eq!(ra, rb);
        assert!(!part_path_for(&a).exists());
        let (c, rc) = build("det-c", 1_790_380_801);
        assert_ne!(std::fs::read(&a).unwrap(), std::fs::read(&c).unwrap());
        assert_eq!(ra.content_sha256, rc.content_sha256, "content does not depend on the epoch");
        let pack = Pack::open(&a).unwrap();
        for id in 1..=3 {
            assert_eq!(pack.get_doc(id).unwrap().unwrap().added_at, "2026-09-26T00:00:00Z");
        }
        assert_eq!(pack.manifest_get(MANIFEST_SOURCE_DATE_EPOCH).unwrap().as_deref(), Some("1790380800"));
    }

    #[test]
    fn mounts_lexically_answers_lookups_with_sources_and_refuses_dense_mount() {
        let (path, report) = build("mount", 1_790_380_800);
        assert_eq!(report.docs, 3);
        let (pack, manifest) = Pack::mount_lexical(&path, &ctx()).unwrap();
        assert_eq!(manifest.embedder_sha256, LEXICAL_EMBEDDER_SHA256);
        assert_eq!(manifest.gate_abs_floor, LEXICAL_GATE_ABS_FLOOR);
        assert!(!manifest.gate_calibrated);
        assert_eq!(pack.manifest_get(MANIFEST_RETRIEVAL_MODE).unwrap().as_deref(), Some("lexical"));
        assert_eq!(pack.manifest_get("tokenizer").unwrap().as_deref(), Some("words"));
        assert_eq!(
            pack.manifest_get(MANIFEST_CONTENT_SHA256).unwrap().as_deref(),
            Some(report.content_sha256.as_str())
        );
        let LexicalOutcome::Found { title, source, .. } = retrieve_lexical(&pack, "copd", 3).unwrap() else {
            panic!("variant should resolve");
        };
        assert_eq!(title, "Chronic obstructive pulmonary disease (COPD)");
        assert_eq!(
            source,
            CitationSource {
                url: Some("https://www.nhs.uk/conditions/chronic-obstructive-pulmonary-disease-copd/".to_string()),
                retrieved_at: Some("2026-09-26".to_string()),
            }
        );
        assert!(matches!(retrieve_lexical(&pack, "gouty arthritis", 3).unwrap(), LexicalOutcome::Found { .. }));
        assert!(matches!(retrieve_lexical(&pack, "astma", 3).unwrap(), LexicalOutcome::DidYouMean { .. }));
        assert_eq!(retrieve_lexical(&pack, "zeltrofen", 3).unwrap(), LexicalOutcome::NotFound);
        // Duplicate variants collapse; the vec table is empty.
        let titles = pack.titles().unwrap();
        assert_eq!(titles[2].variants, vec!["COPD".to_string()]);
        assert_eq!(pack.vec_dims().unwrap(), 1);
        drop(pack);
        let dense = Pack::mount(
            &path,
            &LoadContext {
                available_embedder_sha256: &["ad1afe72cd6654a558667a3db10878b049a75bfd72912e1dabb91310d671173c".to_string()],
                curator_key: None,
            },
        );
        assert!(dense.is_err(), "a lexical-only pack must never mount for dense retrieval");
    }

    #[test]
    fn content_sha_is_over_the_ordered_chunk_shas() {
        let (path, report) = build("content", 1_790_380_800);
        let pack = Pack::open(&path).unwrap();
        let mut shas = Vec::new();
        for doc_id in 1..=3 {
            let title = pack.get_doc(doc_id).unwrap().unwrap().title;
            for c in pack.chunks_for_doc(doc_id).unwrap() {
                shas.push(chunk_content_sha256(&c.text, &c.section_path, &c.locator, &title));
            }
        }
        assert_eq!(shas.len(), report.chunks);
        assert_eq!(pack_content_sha256(shas.iter().map(String::as_str)), report.content_sha256);
    }

    #[test]
    fn chunk_content_sha_matches_the_documented_json_encoding() {
        // sha256 of `["a\"b\\\n\t\b\f\u0001é","S > T","L1-L2","T"]` — computed
        // independently with Python's json.dumps(..., ensure_ascii=False,
        // separators=(",", ":")).
        let got = chunk_content_sha256("a\"b\\\n\t\u{8}\u{c}\u{1}é", "S > T", "L1-L2", "T");
        let want = sha256_hex("[\"a\\\"b\\\\\\n\\t\\b\\f\\u0001é\",\"S > T\",\"L1-L2\",\"T\"]".as_bytes());
        assert_eq!(got, want);
    }

    #[test]
    fn refuses_duplicate_slugs_and_bodyless_pages() {
        let dir = unique_dir("refuse");
        let mut dup = sources();
        dup.push(page("Gout again", "gout", &[], "# G\n\nx.\n"));
        let e = build_lexical_pack(&dup, &Words, &ChunkConfig::default(), &meta(0), &dir.join("a.kpack"));
        assert!(e.unwrap_err().to_string().contains("duplicate slug"));
        let mut empty = sources();
        empty.push(page("Empty", "empty", &[], ""));
        let out = dir.join("b.kpack");
        let e = build_lexical_pack(&empty, &Words, &ChunkConfig::default(), &meta(0), &out);
        assert!(e.unwrap_err().to_string().contains("yields no chunk"));
        assert!(!out.exists() && !part_path_for(&out).exists());
    }

    #[test]
    fn the_merge_pass_is_curated_only_recorded_and_shrinks_the_chunk_count() {
        let dir = unique_dir("merge");
        let m = MergeConfig { target_tokens: 150, max_tokens: 256 };
        let mut personal = meta(0);
        personal.merge = Some(m);
        let e = build_lexical_pack(&sources(), &Words, &ChunkConfig::default(), &personal, &dir.join("p.kpack"));
        assert!(e.unwrap_err().to_string().contains("curated packs only"));
        let mut curated = personal.clone();
        curated.pack_tier = PackTier::Curated;
        let mut srcs = sources();
        srcs.push(page("Flu", "flu", &[], "# Flu\n\nFlu is common.\n\nIt spreads easily.\n\nMost people recover.\n"));
        let merged = build_lexical_pack(&srcs, &Words, &ChunkConfig::default(), &curated, &dir.join("c.kpack")).unwrap();
        let mut plain_meta = curated.clone();
        plain_meta.merge = None;
        let plain = build_lexical_pack(&srcs, &Words, &ChunkConfig::default(), &plain_meta, &dir.join("n.kpack")).unwrap();
        assert_eq!((merged.chunks, plain.chunks), (6, 8));
        assert_ne!(merged.content_sha256, plain.content_sha256);
        let pack = Pack::open(dir.join("c.kpack")).unwrap();
        assert_eq!(pack.manifest_get(MANIFEST_MERGE_TARGET_TOKENS).unwrap().as_deref(), Some("150"));
        assert_eq!(pack.manifest_get(MANIFEST_MERGE_MAX_TOKENS).unwrap().as_deref(), Some("256"));
        // Never across a section: Asthma keeps its lede and its Symptoms apart.
        let asthma = pack.chunks_for_doc(1).unwrap();
        assert_eq!(asthma.len(), 2);
        assert_ne!(asthma[0].section_path, asthma[1].section_path);
        let flu = pack.chunks_for_doc(4).unwrap();
        assert_eq!(flu.len(), 1);
        assert_eq!(flu[0].text, "Flu is common.\n\nIt spreads easily.\n\nMost people recover.");
    }
}
