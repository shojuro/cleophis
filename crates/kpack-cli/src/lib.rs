//! kpack-cli (Phase 1h Task M4b): builds the bundled, lexical-only NHS
//! reference pack from the committed corpus, and the two contract files the
//! triage repo consumes.
//!
//! ```text
//! SOURCE_DATE_EPOCH=<secs> kpack-cli build-reference --corpus DIR --clusters FILE --out DIR
//! kpack-cli verify --corpus DIR --clusters FILE --dir DIR
//! ```
//!
//! `build-reference` writes `<out>/<pack-id>.kpack` (default
//! `reference-uk-v1.kpack`), `<out>/chunks.jsonl` and `<out>/titles.json`.
//! `verify` rebuilds from scratch into a temporary directory, with the
//! `source_date_epoch` recorded in `<dir>/titles.json`, and compares all
//! three files byte for byte (the pack only when present).
//!
//! ## Inputs, checked before anything is written
//! - `corpus/index.json` and every `corpus/<section>/<slug>.md` it lists:
//!   the file's front matter (one `key: <JSON>` per line, as
//!   `tools/reference/fetch_nhs.py` writes it) must agree with the index
//!   (title, URL, slug, section) and the body's sha256 with the index's.
//! - the clusters file (`reference-clusters.json`): every corpus entry is in
//!   exactly one cluster, and each page's names — its title, the title's
//!   parentheticals, aliases, brands and slug words, the same list
//!   `tools/reference/clusters.py::entry_names` makes — must all be in its
//!   cluster's `names`, and every cluster name must belong to one of its
//!   pages. Those names (less the title) are the page's title-index
//!   variants.
//!
//! ## Outputs
//! - The pack: one doc per page, in (slug, section) order; `docs.source_path`
//!   = the page URL, `docs.source_mtime` = its retrieval date; chunked by
//!   the BGE WordPiece count ([`kpack_core::WordPieceTokenizer`]) with the
//!   desktop's chunk settings; `PackTier::Curated` (the founder signs it).
//! - `chunks.jsonl`: every chunk read back from the built pack, in pack
//!   order, one JSON object per line: `chunk_id, doc_id, slug, title,
//!   section_path, locator, url, retrieved_at, text, token_count,
//!   content_sha`.
//! - `titles.json`: a header (pack identity, `content_sha256`, counts, the
//!   input pins and the hashing rules) and one title-index entry per page.
//!
//! No network, no embedder, no LLM (A28/A33).

use kpack_core::lexical_build::{MANIFEST_CONTENT_SHA256, MANIFEST_SOURCE_DATE_EPOCH};
use kpack_core::{
    build_lexical_pack, chunk_content_sha256, pack_content_sha256, ChunkConfig, LexicalBuildMeta, LexicalSource,
    Pack, PackTier, WordPieceTokenizer, SOURCE_TYPE_NHS_WEB,
};
use serde::Serialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

/// The bundled pack's id: `src-tauri/build.rs` embeds
/// `resources/packs/reference-uk-v1.kpack` (+ `.sig`) and `rag_lookup`
/// requires the manifest `pack_id` to equal the file stem.
pub const DEFAULT_PACK_ID: &str = "reference-uk-v1";
/// The pack version (manifest `pack_version`, catalog `referencePack.version`).
pub const DEFAULT_PACK_VERSION: &str = "2026.09.1";
pub const CHUNKS_FILE: &str = "chunks.jsonl";
pub const TITLES_FILE: &str = "titles.json";
pub const TITLES_SCHEMA: &str = "cleophis/reference-titles/v1";
/// How `content_sha` is computed (recorded in the titles header).
pub const CHUNK_CONTENT_SHA_RULE: &str = "sha256 of the UTF-8 compact JSON array [text, section_path, locator, title] \
     (Python: json.dumps([...], ensure_ascii=False, separators=(\",\", \":\")))";
/// How the pack's `content_sha256` is computed (recorded in the titles header).
pub const PACK_CONTENT_SHA_RULE: &str =
    "sha256 over every chunk's content_sha in pack order, each as 64 lowercase hex characters followed by \\n";

/// A plain-language failure. Every error the CLI prints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliError(pub String);

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CliError {}

fn err<T>(msg: impl Into<String>) -> Result<T, CliError> {
    Err(CliError(msg.into()))
}

/// What to build.
#[derive(Debug, Clone)]
pub struct Options {
    pub pack_id: String,
    pub pack_version: String,
    /// Seconds since the Unix epoch — every timestamp in the pack.
    pub source_date_epoch: u64,
}

/// What `build_reference` wrote.
#[derive(Debug, Clone)]
pub struct BuildSummary {
    pub pack_path: PathBuf,
    pub pack_sha256: String,
    pub content_sha256: String,
    pub docs: usize,
    pub chunks: usize,
}

/// One corpus page, as read and checked.
#[derive(Debug, Clone)]
struct CorpusPage {
    key: String,
    section: String,
    slug: String,
    title: String,
    url: String,
    retrieved: String,
    licence: String,
    attribution: String,
    aliases: Vec<String>,
    brands: Vec<String>,
    body: String,
    raw_sha256: String,
    raw_len: usize,
}

/// One line of `chunks.jsonl` (field order is the contract).
#[derive(Debug, Serialize)]
struct ChunkRow<'a> {
    chunk_id: i64,
    doc_id: i64,
    slug: &'a str,
    title: &'a str,
    section_path: &'a str,
    locator: &'a str,
    url: &'a str,
    retrieved_at: &'a str,
    text: &'a str,
    token_count: i64,
    content_sha: String,
}

#[derive(Debug, Serialize)]
struct TitlesFile {
    header: TitlesHeader,
    titles: Vec<TitleRow>,
}

#[derive(Debug, Serialize)]
struct TitlesHeader {
    schema: String,
    pack_id: String,
    pack_version: String,
    pack_file: String,
    content_sha256: String,
    chunk_content_sha: String,
    pack_content_sha: String,
    chunks: usize,
    titles: usize,
    source_date_epoch: u64,
    corpus_index_sha256: String,
    clusters_sha256: String,
    tokenizer: String,
    tokenizer_vocab_sha256: String,
    chunk_target_tokens: usize,
    chunk_overlap_pct: u32,
    normalisation: String,
    licence: String,
    attribution: String,
}

#[derive(Debug, Serialize)]
struct TitleRow {
    doc_id: i64,
    slug: String,
    section: String,
    title: String,
    normalised_title: String,
    variants: Vec<String>,
    url: String,
    retrieved_at: String,
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

fn read(path: &Path) -> Result<Vec<u8>, CliError> {
    std::fs::read(path).map_err(|e| CliError(format!("cannot read {}: {e}", path.display())))
}

fn read_json(path: &Path) -> Result<Value, CliError> {
    serde_json::from_slice(&read(path)?).map_err(|e| CliError(format!("{} is not valid JSON: {e}", path.display())))
}

/// Mirrors `fetch_nhs.py::split_front_matter`: `---\n`, then one
/// `key: <JSON>` per line, up to the first `\n---\n`; the body follows.
fn split_front_matter(text: &str) -> Result<(Map<String, Value>, &str), String> {
    if !text.starts_with("---\n") {
        return Err("no front matter".to_string());
    }
    let end = text[4..].find("\n---\n").map(|i| i + 4).ok_or("unterminated front matter")?;
    let mut fm = Map::new();
    for line in text[4..end].split('\n') {
        let (k, v) = line.split_once(": ").ok_or_else(|| format!("front matter line {line:?} is not `key: value`"))?;
        let v: Value = serde_json::from_str(v).map_err(|e| format!("front matter {k}: {e}"))?;
        fm.insert(k.to_string(), v);
    }
    Ok((fm, &text[end + 5..]))
}

fn fm_str(fm: &Map<String, Value>, key: &str) -> Result<String, String> {
    fm.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("front matter lacks string {key:?}"))
}

fn fm_list(fm: &Map<String, Value>, key: &str) -> Result<Vec<String>, String> {
    match fm.get(key) {
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| v.as_str().map(str::to_string).ok_or_else(|| format!("{key:?} holds a non-string")))
            .collect(),
        _ => Err(format!("front matter lacks list {key:?}")),
    }
}

/// Read and check every page `corpus/index.json` lists.
fn read_corpus(dir: &Path) -> Result<(Vec<CorpusPage>, String), CliError> {
    let index_path = dir.join("index.json");
    let index_sha = sha256_hex(&read(&index_path)?);
    let index = read_json(&index_path)?;
    let entries = index
        .get("entries")
        .and_then(Value::as_object)
        .ok_or_else(|| CliError(format!("{} has no \"entries\" object", index_path.display())))?;
    let mut pages = Vec::with_capacity(entries.len());
    for (key, rec) in entries {
        let field = |k: &str| {
            rec.get(k)
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| CliError(format!("index entry {key} lacks {k:?}")))
        };
        let (section, slug) = (field("section")?, field("slug")?);
        if key != &format!("{section}/{slug}") || slug.contains(['/', '\\']) || section.contains(['/', '\\']) {
            return err(format!("index entry {key} does not match its section/slug"));
        }
        let path = dir.join(&section).join(format!("{slug}.md"));
        let raw = read(&path)?;
        let text = std::str::from_utf8(&raw).map_err(|_| CliError(format!("{} is not UTF-8", path.display())))?;
        let bad = |why: String| CliError(format!("{}: {why}", path.display()));
        let (fm, body) = split_front_matter(text).map_err(bad)?;
        if sha256_hex(body.as_bytes()) != field("sha256")? {
            return Err(bad("body sha256 differs from corpus/index.json".to_string()));
        }
        if Some(raw.len() as u64) != rec.get("bytes").and_then(Value::as_u64) {
            return Err(bad("byte length differs from corpus/index.json".to_string()));
        }
        let page = CorpusPage {
            key: key.clone(),
            title: fm_str(&fm, "title").map_err(bad)?,
            url: fm_str(&fm, "url").map_err(bad)?,
            retrieved: fm_str(&fm, "retrieved").map_err(bad)?,
            licence: fm_str(&fm, "licence").map_err(bad)?,
            attribution: fm_str(&fm, "attribution").map_err(bad)?,
            aliases: fm_list(&fm, "aliases").map_err(bad)?,
            brands: fm_list(&fm, "brands").map_err(bad)?,
            body: body.to_string(),
            raw_sha256: sha256_hex(&raw),
            raw_len: raw.len(),
            section,
            slug,
        };
        for (k, want) in [("title", &page.title), ("url", &page.url)] {
            if rec.get(k).and_then(Value::as_str) != Some(want.as_str()) {
                return Err(bad(format!("front matter {k} differs from corpus/index.json")));
            }
        }
        for (k, want) in [("slug", &page.slug), ("section", &page.section)] {
            if fm.get(k).and_then(Value::as_str) != Some(want.as_str()) {
                return Err(bad(format!("front matter {k} differs from its path")));
            }
        }
        if !is_iso_date(&page.retrieved) {
            return Err(bad(format!("retrieved {:?} is not YYYY-MM-DD", page.retrieved)));
        }
        if !page.url.starts_with("https://www.nhs.uk/") {
            return Err(bad(format!("url {:?} is not an https://www.nhs.uk/ page", page.url)));
        }
        pages.push(page);
    }
    // No page on disk that the index does not list.
    let mut on_disk = 0usize;
    for section in pages.iter().map(|p| p.section.clone()).collect::<BTreeSet<_>>() {
        let rd = std::fs::read_dir(dir.join(&section)).map_err(|e| CliError(format!("{section}: {e}")))?;
        on_disk += rd
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().map_or(false, |x| x == "md"))
            .count();
    }
    if on_disk != pages.len() {
        return err(format!("corpus has {on_disk} pages on disk but index.json lists {}", pages.len()));
    }
    Ok((pages, index_sha))
}

fn is_iso_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter().enumerate().all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

/// `clusters.py::_PAREN.findall(title)`: the text inside each `( ... )`.
fn parentheticals(title: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = title;
    while let Some(open) = rest.find('(') {
        let after = &rest[open + 1..];
        match after.find(')') {
            Some(close) => {
                out.push(after[..close].to_string());
                rest = &after[close + 1..];
            }
            None => break,
        }
    }
    out
}

/// `clusters.py::entry_names`: title, its parentheticals, aliases, brands,
/// slug words — exact strings, first occurrence kept, empties dropped.
fn entry_names(p: &CorpusPage) -> Vec<String> {
    let mut names = vec![p.title.clone()];
    names.extend(parentheticals(&p.title).into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()));
    names.extend(p.aliases.iter().cloned());
    names.extend(p.brands.iter().cloned());
    names.push(p.slug.replace("--", " ").replace('-', " "));
    let mut out: Vec<String> = Vec::new();
    for n in names {
        if !n.is_empty() && !out.contains(&n) {
            out.push(n);
        }
    }
    out
}

struct Clusters {
    sha256: String,
    /// page key -> its title-index variants (entry names less the title).
    variants: BTreeMap<String, Vec<String>>,
}

/// Read the clusters file and check it against the corpus (module doc).
fn read_clusters(path: &Path, pages: &[CorpusPage]) -> Result<Clusters, CliError> {
    let sha256 = sha256_hex(&read(path)?);
    let v = read_json(path)?;
    let bad = |why: String| CliError(format!("{}: {why}", path.display()));
    let entry_to_cluster = v
        .get("entry_to_cluster")
        .and_then(Value::as_object)
        .ok_or_else(|| bad("no \"entry_to_cluster\" object".to_string()))?;
    let mut clusters: BTreeMap<String, (Vec<String>, Vec<String>)> = BTreeMap::new();
    for c in v.get("clusters").and_then(Value::as_array).ok_or_else(|| bad("no \"clusters\" list".to_string()))? {
        let strs = |k: &str| -> Result<Vec<String>, CliError> {
            c.get(k)
                .and_then(Value::as_array)
                .ok_or_else(|| bad(format!("a cluster lacks {k:?}")))?
                .iter()
                .map(|x| x.as_str().map(str::to_string).ok_or_else(|| bad(format!("{k:?} holds a non-string"))))
                .collect()
        };
        let id = c.get("id").and_then(Value::as_str).ok_or_else(|| bad("a cluster lacks \"id\"".to_string()))?;
        if clusters.insert(id.to_string(), (strs("entries")?, strs("names")?)).is_some() {
            return Err(bad(format!("duplicate cluster id {id}")));
        }
    }
    let by_key: BTreeMap<&str, &CorpusPage> = pages.iter().map(|p| (p.key.as_str(), p)).collect();
    let mut mapped: Vec<&str> = entry_to_cluster.keys().map(String::as_str).collect();
    mapped.sort_unstable();
    let mut keys: Vec<&str> = by_key.keys().copied().collect();
    keys.sort_unstable();
    if mapped != keys {
        return Err(bad("entry_to_cluster does not list exactly the corpus entries".to_string()));
    }
    let mut variants = BTreeMap::new();
    for (id, (entries, names)) in &clusters {
        let mut member_names: BTreeSet<String> = BTreeSet::new();
        for key in entries {
            let page = by_key.get(key.as_str()).ok_or_else(|| bad(format!("cluster {id} lists unknown entry {key}")))?;
            if entry_to_cluster.get(key).and_then(Value::as_str) != Some(id.as_str()) {
                return Err(bad(format!("entry {key} is in cluster {id} but entry_to_cluster disagrees")));
            }
            let own = entry_names(page);
            if let Some(missing) = own.iter().find(|n| !names.contains(n)) {
                return Err(bad(format!("cluster {id} lacks {missing:?}, a name of {key}")));
            }
            member_names.extend(own.iter().cloned());
            variants.insert(key.clone(), own[1..].to_vec());
        }
        if let Some(stray) = names.iter().find(|n| !member_names.contains(*n)) {
            return Err(bad(format!("cluster {id} name {stray:?} belongs to none of its pages")));
        }
    }
    if variants.len() != pages.len() {
        return Err(bad("some corpus entry is in no cluster's \"entries\"".to_string()));
    }
    Ok(Clusters { sha256, variants })
}

/// The fabrication bank's fake names as the clusters file records them
/// (`fabrication.fakes_checked`) — for the absence checks.
pub fn fake_names(clusters_path: &Path) -> Result<Vec<String>, CliError> {
    let v = read_json(clusters_path)?;
    Ok(v.pointer("/fabrication/fakes_checked")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default())
}

fn tokenizer_label() -> String {
    format!(
        "bge-base-en-v1.5 WordPiece (kpack_core::wordpiece, a port of llama.cpp's WPM tokenizer; \
         unicode tables sha256 {}), counts include [CLS] and [SEP]",
        kpack_core::wordpiece::UNICODE_TABLES_SHA256
    )
}

/// Write `bytes` to `path` via `<path>.part` and a rename.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), CliError> {
    let mut part = path.as_os_str().to_os_string();
    part.push(".part");
    let part = PathBuf::from(part);
    std::fs::write(&part, bytes).map_err(|e| CliError(format!("cannot write {}: {e}", part.display())))?;
    std::fs::rename(&part, path).map_err(|e| {
        let _ = std::fs::remove_file(&part);
        CliError(format!("cannot write {}: {e}", path.display()))
    })
}

/// Build the pack and write `chunks.jsonl` / `titles.json` into `out_dir`.
pub fn build_reference(
    corpus: &Path,
    clusters_path: &Path,
    out_dir: &Path,
    opts: &Options,
) -> Result<BuildSummary, CliError> {
    if !kpack_core::lookup::slugify(&opts.pack_id).eq(&opts.pack_id) || opts.pack_id.is_empty() {
        return err(format!("pack id {:?} must be lower-case letters, digits and hyphens", opts.pack_id));
    }
    let (mut pages, index_sha) = read_corpus(corpus)?;
    let clusters = read_clusters(clusters_path, &pages)?;
    pages.sort_by(|a, b| (a.slug.as_str(), a.section.as_str()).cmp(&(b.slug.as_str(), b.section.as_str())));
    let licences: BTreeSet<(&str, &str)> = pages.iter().map(|p| (p.licence.as_str(), p.attribution.as_str())).collect();
    if licences.len() != 1 {
        return err(format!("the corpus carries {} different licence/attribution pairs; expected one", licences.len()));
    }
    let (licence, attribution) = *licences.iter().next().expect("one pair");
    for p in &pages {
        if kpack_core::lookup::slugify(&p.slug) != p.slug {
            return err(format!("slug {:?} ({}) is not in slug form", p.slug, p.key));
        }
    }

    let sources: Vec<LexicalSource> = pages
        .iter()
        .map(|p| LexicalSource {
            title: p.title.clone(),
            slug: p.slug.clone(),
            variants: clusters.variants[&p.key].clone(),
            body: p.body.clone(),
            source_type: SOURCE_TYPE_NHS_WEB.to_string(),
            source_path: Some(p.url.clone()),
            source_mtime: Some(p.retrieved.clone()),
            sha256: p.raw_sha256.clone(),
            source_size: p.raw_len as i64,
        })
        .collect();
    let pack_file = format!("{}.kpack", opts.pack_id);
    let meta = LexicalBuildMeta {
        pack_id: opts.pack_id.clone(),
        pack_version: opts.pack_version.clone(),
        pack_tier: PackTier::Curated,
        built_by: format!("kpack-cli build-reference {}", env!("CARGO_PKG_VERSION")),
        license_ref: Some(format!("{licence}: {attribution}")),
        source_date_epoch: opts.source_date_epoch,
        extra_manifest: vec![
            ("pack_file".to_string(), pack_file.clone()),
            ("tokenizer".to_string(), tokenizer_label()),
            ("tokenizer_vocab_sha256".to_string(), kpack_core::wordpiece::BGE_VOCAB_SHA256.to_string()),
            ("corpus_index_sha256".to_string(), index_sha.clone()),
            ("clusters_sha256".to_string(), clusters.sha256.clone()),
        ],
        merge: None,
    };
    let cfg = ChunkConfig::default();
    std::fs::create_dir_all(out_dir).map_err(|e| CliError(format!("cannot create {}: {e}", out_dir.display())))?;
    let pack_path = out_dir.join(&pack_file);
    let report = build_lexical_pack(&sources, &WordPieceTokenizer::bge_base_en_v1_5(), &cfg, &meta, &pack_path)
        .map_err(|e| CliError(format!("pack build failed: {e}")))?;

    // Everything below is read back from the pack just written, so the
    // contract files describe the pack itself, not the build's intentions.
    let pack = Pack::open(&pack_path).map_err(|e| CliError(format!("cannot reopen the pack: {e}")))?;
    let manifest_sha = pack
        .manifest_get(MANIFEST_CONTENT_SHA256)
        .map_err(|e| CliError(e.to_string()))?
        .unwrap_or_default();
    let titles = pack.titles().map_err(|e| CliError(e.to_string()))?;
    if titles.len() != pages.len() {
        return err("the pack's title index does not have one row per page");
    }
    let mut chunks_jsonl = String::new();
    let mut chunk_shas: Vec<String> = Vec::new();
    let mut title_rows = Vec::with_capacity(pages.len());
    for (i, (page, entry)) in pages.iter().zip(&titles).enumerate() {
        let doc_id = (i + 1) as i64;
        let doc = pack
            .get_doc(doc_id)
            .map_err(|e| CliError(e.to_string()))?
            .ok_or_else(|| CliError(format!("doc {doc_id} missing from the pack")))?;
        if entry.doc_id != doc_id || doc.title != page.title || entry.slug != page.slug {
            return err(format!("pack row {doc_id} is not page {}", page.key));
        }
        let url = doc.source_path.clone().unwrap_or_default();
        let retrieved_at = doc.source_mtime.clone().unwrap_or_default();
        for c in pack.chunks_for_doc(doc_id).map_err(|e| CliError(e.to_string()))? {
            let content_sha = chunk_content_sha256(&c.text, &c.section_path, &c.locator, &doc.title);
            chunk_shas.push(content_sha.clone());
            let row = ChunkRow {
                chunk_id: c.id,
                doc_id,
                slug: &page.slug,
                title: &doc.title,
                section_path: &c.section_path,
                locator: &c.locator,
                url: &url,
                retrieved_at: &retrieved_at,
                text: &c.text,
                token_count: c.token_count,
                content_sha,
            };
            chunks_jsonl.push_str(&serde_json::to_string(&row).map_err(|e| CliError(e.to_string()))?);
            chunks_jsonl.push('\n');
        }
        title_rows.push(TitleRow {
            doc_id,
            slug: entry.slug.clone(),
            section: page.section.clone(),
            title: entry.title.clone(),
            normalised_title: entry.normalised_title.clone(),
            variants: entry.variants.clone(),
            url,
            retrieved_at,
        });
    }
    let content_sha256 = pack_content_sha256(chunk_shas.iter().map(String::as_str));
    if content_sha256 != manifest_sha || content_sha256 != report.content_sha256 || chunk_shas.len() != report.chunks {
        return err("the chunks read back from the pack do not match its manifest content_sha256");
    }
    let titles_file = TitlesFile {
        header: TitlesHeader {
            schema: TITLES_SCHEMA.to_string(),
            pack_id: opts.pack_id.clone(),
            pack_version: opts.pack_version.clone(),
            pack_file,
            content_sha256: content_sha256.clone(),
            chunk_content_sha: CHUNK_CONTENT_SHA_RULE.to_string(),
            pack_content_sha: PACK_CONTENT_SHA_RULE.to_string(),
            chunks: chunk_shas.len(),
            titles: title_rows.len(),
            source_date_epoch: opts.source_date_epoch,
            corpus_index_sha256: index_sha,
            clusters_sha256: clusters.sha256.clone(),
            tokenizer: tokenizer_label(),
            tokenizer_vocab_sha256: kpack_core::wordpiece::BGE_VOCAB_SHA256.to_string(),
            chunk_target_tokens: cfg.target_tokens,
            chunk_overlap_pct: cfg.overlap_pct,
            normalisation: "kpack_core::lookup::normalise_title (Phase 1h M5)".to_string(),
            licence: licence.to_string(),
            attribution: attribution.to_string(),
        },
        titles: title_rows,
    };
    let mut titles_json = serde_json::to_string_pretty(&titles_file).map_err(|e| CliError(e.to_string()))?;
    titles_json.push('\n');
    drop(pack);
    write_atomic(&out_dir.join(CHUNKS_FILE), chunks_jsonl.as_bytes())?;
    write_atomic(&out_dir.join(TITLES_FILE), titles_json.as_bytes())?;
    Ok(BuildSummary {
        pack_sha256: sha256_hex(&read(&pack_path)?),
        pack_path,
        content_sha256,
        docs: pages.len(),
        chunks: chunk_shas.len(),
    })
}

/// The header fields `verify` needs from a committed `titles.json`.
fn committed_header(dir: &Path) -> Result<(Options, String), CliError> {
    let path = dir.join(TITLES_FILE);
    let v = read_json(&path)?;
    let h = v.get("header").ok_or_else(|| CliError(format!("{} has no header", path.display())))?;
    let s = |k: &str| {
        h.get(k)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| CliError(format!("{} header lacks {k:?}", path.display())))
    };
    let epoch = h
        .get("source_date_epoch")
        .and_then(Value::as_u64)
        .ok_or_else(|| CliError(format!("{} header lacks source_date_epoch", path.display())))?;
    Ok((
        Options {
            pack_id: s("pack_id")?,
            pack_version: s("pack_version")?,
            source_date_epoch: epoch,
        },
        s("content_sha256")?,
    ))
}

/// What `verify` compared.
#[derive(Debug, Clone)]
pub struct VerifyReport {
    pub content_sha256: String,
    pub pack_sha256: String,
    pub pack_compared: bool,
    pub mismatches: Vec<String>,
}

/// Rebuild from scratch (into a temporary directory, with the committed
/// header's pack id, version and `source_date_epoch`) and compare
/// `chunks.jsonl`, `titles.json` and — when `<dir>/<pack-id>.kpack` exists —
/// the pack, byte for byte, plus the pack's manifest `content_sha256`.
pub fn verify(corpus: &Path, clusters_path: &Path, dir: &Path) -> Result<VerifyReport, CliError> {
    let (opts, committed_sha) = committed_header(dir)?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let tmp = std::env::temp_dir().join(format!("kpack-cli-verify-{}-{nanos}", std::process::id()));
    let result = (|| {
        let fresh = build_reference(corpus, clusters_path, &tmp, &opts)?;
        let mut mismatches = Vec::new();
        if fresh.content_sha256 != committed_sha {
            mismatches.push(format!(
                "content_sha256: rebuilt {} but {} records {committed_sha}",
                fresh.content_sha256, TITLES_FILE
            ));
        }
        for name in [CHUNKS_FILE, TITLES_FILE] {
            if read(&tmp.join(name))? != read(&dir.join(name))? {
                mismatches.push(format!("{name} differs from a fresh build"));
            }
        }
        let committed_pack = dir.join(format!("{}.kpack", opts.pack_id));
        let pack_compared = committed_pack.exists();
        if pack_compared {
            if sha256_hex(&read(&committed_pack)?) != fresh.pack_sha256 {
                mismatches.push(format!("{} differs from a fresh build", committed_pack.display()));
            }
            let pack = Pack::open(&committed_pack).map_err(|e| CliError(e.to_string()))?;
            let recorded = pack.manifest_get(MANIFEST_CONTENT_SHA256).map_err(|e| CliError(e.to_string()))?;
            if recorded.as_deref() != Some(fresh.content_sha256.as_str()) {
                mismatches.push("the pack's manifest content_sha256 differs from a fresh build".to_string());
            }
            let epoch = pack.manifest_get(MANIFEST_SOURCE_DATE_EPOCH).map_err(|e| CliError(e.to_string()))?;
            if epoch.as_deref() != Some(opts.source_date_epoch.to_string().as_str()) {
                mismatches.push("the pack's source_date_epoch differs from titles.json".to_string());
            }
        }
        Ok(VerifyReport {
            content_sha256: fresh.content_sha256,
            pack_sha256: fresh.pack_sha256,
            pack_compared,
            mismatches,
        })
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parentheticals_follow_the_python_regex() {
        assert_eq!(parentheticals("Chronic obstructive pulmonary disease (COPD)"), vec!["COPD"]);
        assert_eq!(parentheticals("A (b) c (d e)"), vec!["b", "d e"]);
        assert_eq!(parentheticals("A (b (c) d"), vec!["b (c"]);
        assert_eq!(parentheticals("no parens ("), Vec::<String>::new());
    }

    #[test]
    fn front_matter_split_mirrors_fetch_nhs() {
        let (fm, body) = split_front_matter("---\ntitle: \"Gout\"\naliases: []\n---\n# Gout\n").unwrap();
        assert_eq!(fm["title"], "Gout");
        assert_eq!(body, "# Gout\n");
        assert!(split_front_matter("# no front matter").is_err());
        assert!(split_front_matter("---\ntitle Gout\n---\n").is_err());
    }

    #[test]
    fn iso_dates() {
        assert!(is_iso_date("2026-09-26"));
        assert!(!is_iso_date("26-09-2026"));
        assert!(!is_iso_date("2026-9-26"));
    }
}
