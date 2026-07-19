//! EPUB → [`crate::tree::Document`] (spec §3.2's parser list, "formats"
//! follow-on to the HTML slice). Like DOCX, EPUB is BINARY (a zip), so
//! callers reach [`document_from_epub`] bytes-in/`Document`-out, wrapped in
//! `SourceContent::Prebuilt` (see `src-tauri/src/kpack.rs`'s
//! `source_type_for` doc comment) — there is no build-time text path for it.
//!
//! ## "A zip of XHTML" — reuses `html_to_document`
//! An EPUB's spine is a sequence of XHTML chapter documents, so this module
//! does no HTML parsing of its own: [`document_from_epub`] walks the spine
//! via the [`epub`] crate (pure Rust — `zip` + `xml-rs`; `Cargo.toml`'s own
//! comment on this dependency has the `cargo tree` confirmation) and, for
//! EACH chapter's XHTML, calls [`crate::html::html_to_document`] directly —
//! the same DOM walk, block mapping, and boilerplate caveat that module's
//! own doc comment describes apply here per chapter, unchanged.
//!
//! ## Merging chapters into one `Document`
//! `html_to_document` returns one `Document` per chapter; this module
//! merges them into a single `Document` by prefixing every one of a
//! chapter's `Section::path`s with a leading path element identifying the
//! chapter (its spine id, e.g. `"ch1"`, falling back to its href if the
//! epub has no id for that entry) — so `["Chapter 1", "Section A"]`
//! becomes `["ch1", "Chapter 1", "Section A"]`. Chapters are walked in
//! spine order (the epub crate's own reading order), so the merged
//! `Document`'s sections come out in that same order, chapter by chapter.
//!
//! ## Locators
//! Every block's locator gets the chapter's href prefixed onto
//! `html_to_document`'s own locator (an anchor `"#id"` or a running
//! `"b{n}"`, per that module's doc comment), joined on `'#'`:
//! `"{href}#id"` or `"{href}#b{n}"` — e.g. `"OEBPS/ch1.xhtml#intro"`. This
//! makes every locator globally unique across the whole epub (two chapters
//! can otherwise both produce a bare `"b1"`) while staying anchored to a
//! real file a citation could point back into.
//!
//! ## Empty chapters
//! A spine entry whose content can't be read as a string (a non-text
//! resource, or a read failure) contributes zero sections rather than
//! failing the whole parse — the same "degrade per-file/per-chapter, never
//! the whole build" stance `parse::parse`'s dispatcher takes.

use crate::html::html_to_document;
use crate::tree::{Block, Document, Section};
use epub::doc::EpubDoc;
use std::fmt;
use std::io::Cursor;

/// [`document_from_epub`]'s failure mode: the bytes handed in aren't a
/// readable `.epub` (not a zip, missing `META-INF/container.xml`/OPF, or a
/// malformed spine) — hand-rolled, no `thiserror`, matching
/// [`crate::docx::Error`] and this crate's other error types. The caller
/// (`src-tauri/src/kpack.rs`'s build branch) wraps this in the same
/// plain-language, user-facing message the PDF/DOCX branches use.
#[derive(Debug)]
pub enum Error {
    Parse(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Parse(msg) => write!(f, "epub parse error: {msg}"),
        }
    }
}

impl std::error::Error for Error {}

/// Parse `bytes` (an `.epub` file's raw bytes) into a [`Document`] titled
/// `title`. See the module doc comment for the per-chapter
/// `html_to_document` reuse, the chapter-merge scheme, and the locator
/// prefix. Returns [`Error::Parse`] — never panics — on a corrupt or
/// unreadable `.epub` (not a zip, no valid OPF/spine, ...).
pub fn document_from_epub(bytes: &[u8], title: &str) -> Result<Document, Error> {
    let mut doc =
        EpubDoc::from_reader(Cursor::new(bytes)).map_err(|e| Error::Parse(e.to_string()))?;

    let mut sections: Vec<Section> = Vec::new();
    loop {
        let href = doc
            .get_current_path()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let chapter_id = doc.get_current_id().filter(|id| !id.is_empty()).unwrap_or_else(|| href.clone());

        if let Some((content, _mime)) = doc.get_current_str() {
            let chapter_doc = html_to_document(&content, &chapter_id);
            for section in chapter_doc.sections {
                let mut path = Vec::with_capacity(section.path.len() + 1);
                path.push(chapter_id.clone());
                path.extend(section.path);
                let blocks = section
                    .blocks
                    .into_iter()
                    .map(|block| prefix_locator(block, &href))
                    .collect();
                sections.push(Section { path, blocks });
            }
        }

        if !doc.go_next() {
            break;
        }
    }

    Ok(Document {
        title: title.to_string(),
        sections,
    })
}

/// Prefix `block`'s locator with `"{href}#"` (see the module doc comment's
/// locator scheme). `html_to_document`'s own locators never carry a
/// leading `'#'` themselves except the anchor form (`"#id"`), so a leading
/// `'#'` is stripped first to avoid a doubled `"##"`.
fn prefix_locator(block: Block, href: &str) -> Block {
    let with_prefix = |locator: String| format!("{href}#{}", locator.trim_start_matches('#'));
    match block {
        Block::Paragraph { text, locator } => Block::Paragraph {
            text,
            locator: with_prefix(locator),
        },
        Block::Table { header, rows, locator } => Block::Table {
            header,
            rows,
            locator: with_prefix(locator),
        },
        Block::Code { text, locator } => Block::Code {
            text,
            locator: with_prefix(locator),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny, real 2-chapter `.epub` — "Chapter One"/"Chapter Two", one
    /// `<h1>` + one `<p>` each — hand-built (no pure-Rust `.epub` writer
    /// exists) and committed at
    /// `crates/kpack-core/tests/fixtures/sample.epub`. `include_bytes!` so
    /// the test doesn't depend on the process's working directory.
    const SAMPLE_EPUB: &[u8] = include_bytes!("../tests/fixtures/sample.epub");

    // 1. Real fixture: 2 chapters, in spine order, each producing a
    // section whose path is prefixed with the chapter's spine id; text
    // from both chapters is present; locators carry the chapter's href.
    #[test]
    fn t1_real_epub_gives_two_chapter_sections_in_spine_order() {
        let doc = document_from_epub(SAMPLE_EPUB, "Sample").expect("fixture should parse");
        assert_eq!(doc.sections.len(), 2, "one section per chapter (each chapter has one heading)");

        assert_eq!(doc.sections[0].path, vec!["ch1".to_string(), "Chapter One".to_string()]);
        assert_eq!(doc.sections[1].path, vec!["ch2".to_string(), "Chapter Two".to_string()]);

        let text = |section: usize| match &doc.sections[section].blocks[0] {
            Block::Paragraph { text, .. } => text.as_str(),
            other => panic!("expected Block::Paragraph, got {other:?}"),
        };
        assert_eq!(text(0), "This is the first paragraph of chapter one.");
        assert_eq!(text(1), "This is the first paragraph of chapter two.");

        let locator = |section: usize| match &doc.sections[section].blocks[0] {
            Block::Paragraph { locator, .. } => locator.as_str(),
            other => panic!("expected Block::Paragraph, got {other:?}"),
        };
        // "b1": html_to_document's own running-block-index fallback, since
        // neither fixture <h1> carries an id attribute — prefixed with the
        // chapter's href (see the module doc comment's locator scheme).
        assert_eq!(locator(0), "OEBPS/ch1.xhtml#b1");
        assert_eq!(locator(1), "OEBPS/ch2.xhtml#b1");
    }

    // 2. Determinism: parsing the same bytes twice yields identical
    // Documents.
    #[test]
    fn t2_document_from_epub_is_deterministic() {
        let doc1 = document_from_epub(SAMPLE_EPUB, "Sample").unwrap();
        let doc2 = document_from_epub(SAMPLE_EPUB, "Sample").unwrap();
        assert_eq!(doc1, doc2);
    }

    // 3. Corrupt/unreadable input -> a clean Err, never a panic.
    #[test]
    fn t3_corrupt_bytes_yield_clean_err_not_panic() {
        let result = document_from_epub(b"this is not an epub file at all", "Bad");
        assert!(matches!(result, Err(Error::Parse(_))), "expected Err(Parse), got {result:?}");
    }
}
