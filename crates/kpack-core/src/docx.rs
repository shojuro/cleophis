//! DOCX → [`crate::tree::Document`] (spec §3.2's parser list, "formats"
//! follow-on to the HTML slice). DOCX is BINARY (a zip of XML parts), so
//! callers reach [`document_from_docx`] the way `kpack-pdf`'s page
//! extraction is reached: bytes in, [`Document`] out, wrapped in
//! `SourceContent::Prebuilt` by the caller — there is no build-time text
//! path for it (see `src-tauri/src/kpack.rs`'s `source_type_for` doc
//! comment).
//!
//! ## Crate choice
//! [`docx_rust`] (crates.io name `docx-rust`, repo `cstkingkey` — a
//! read-focused fork of bokuweb's write-first `docx-rs`) was smoke-tested
//! against a real python-docx-generated `.docx` fixture (this module's own
//! tests, below) before being committed to: it cleanly opens the zip,
//! parses `word/document.xml`, and exposes each paragraph's `w:pStyle` and
//! text. No `zip`+`quick-xml` fallback was needed. Pure Rust throughout
//! (`zip` + `hard-xml`), no native/network dependency (`Cargo.toml`'s own
//! comment on this dependency has the `cargo tree` confirmation).
//!
//! ## Paragraph walk
//! [`document_from_docx`] iterates `Docx::document.body.content` in
//! document order — mirroring [`crate::html::html_to_document`]'s DOM walk
//! and [`crate::parse::parse_markdown`]'s heading stack: a paragraph whose
//! style id is `"Heading1"`..`"Heading9"` ([`heading_level`]) pops the
//! heading stack to that level minus one and pushes its own text, opening a
//! new [`crate::tree::Section`]; every other non-empty paragraph becomes a
//! [`crate::tree::Block::Paragraph`] under the current section. A heading
//! paragraph gets no `Block` of its own, same as HTML/Markdown.
//!
//! ## Tables (v1: flattened, not structured)
//! A `w:tbl` is walked row by row; each row's cells are joined with tabs
//! into a single [`crate::tree::Block::Paragraph`] — the same "flatten
//! reasonably" stance [`crate::parse::parse_markdown`] takes on lists,
//! rather than [`crate::tree::Block::Table`]'s structured `header`/`rows`
//! shape (which needs a header/body distinction DOCX tables don't mark
//! explicitly the way GFM tables do). Structured DOCX tables are a later
//! enhancement, not attempted here.
//!
//! ## Locators
//! DOCX has no page or line concept, so every emitted block (paragraph or
//! flattened table row) gets a running 1-based index over the whole
//! document: `"¶{n}"`. The counter only advances for blocks actually
//! emitted — heading paragraphs and skipped empty paragraphs don't consume
//! a number — mirroring [`crate::html::html_to_document`]'s `"b{n}"`
//! fallback locator scheme.
//!
//! Empty (or whitespace-only) paragraphs and table rows are dropped
//! entirely, same as every other parser in this crate.

use crate::tree::{Block, Document, Section};
use docx_rust::document::{BodyContent, Paragraph, Table, TableRowContent};
use docx_rust::DocxFile;
use std::fmt;
use std::io::Cursor;

/// [`document_from_docx`]'s failure mode: the bytes handed in aren't a
/// readable `.docx` (not a zip at all, a zip missing `word/document.xml`,
/// or XML `hard-xml` can't parse) — hand-rolled, no `thiserror`, matching
/// this crate's other error types (e.g. [`crate::build::Error`]). The
/// caller (`src-tauri/src/kpack.rs`'s build branch) wraps this in the same
/// plain-language, user-facing message the PDF branch uses; this variant's
/// own `Display` is the lower-level "why" that message's `({e})` suffix
/// shows.
#[derive(Debug)]
pub enum Error {
    Parse(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Parse(msg) => write!(f, "docx parse error: {msg}"),
        }
    }
}

impl std::error::Error for Error {}

/// Parse `bytes` (a `.docx` file's raw bytes) into a [`Document`] titled
/// `title`. See the module doc comment for the paragraph walk, table
/// flattening, and locator scheme. Returns [`Error::Parse`] — never
/// panics — on a corrupt or unreadable `.docx` (not a zip, missing/invalid
/// `word/document.xml`, ...).
pub fn document_from_docx(bytes: &[u8], title: &str) -> Result<Document, Error> {
    let docx_file =
        DocxFile::from_reader(Cursor::new(bytes)).map_err(|e| Error::Parse(e.to_string()))?;
    let docx = docx_file.parse().map_err(|e| Error::Parse(e.to_string()))?;

    let mut walker = Walker::default();
    for content in &docx.document.body.content {
        match content {
            BodyContent::Paragraph(p) => walker.visit_paragraph(p),
            BodyContent::Table(t) => walker.visit_table(t),
            // Sdt/SectionProperty/TableCell/Run at the body's top level
            // carry no content this v1 walk knows how to place into a
            // Section — skipped, same as HTML's script/style/head.
            _ => {}
        }
    }
    Ok(walker.finish(title))
}

/// Running state for the document-order body walk, mirroring
/// [`crate::html::html_to_document`]'s `Walker` field for field.
#[derive(Default)]
struct Walker {
    sections: Vec<Section>,
    heading_stack: Vec<String>,
    current_path: Vec<String>,
    current_blocks: Vec<Block>,
    /// Running 1-based index over every block emitted so far (not reset per
    /// section or per heading) — the `"¶{n}"` locator (see the module doc
    /// comment).
    paragraph_index: usize,
}

impl Walker {
    fn visit_paragraph(&mut self, p: &Paragraph) {
        let style_id = p
            .property
            .as_ref()
            .and_then(|prop| prop.style_id.as_ref())
            .map(|s| s.value.as_ref());
        if let Some(level) = style_id.and_then(heading_level) {
            self.close_section();
            self.heading_stack.truncate(level.saturating_sub(1));
            self.heading_stack.push(collapse_whitespace(&p.text()));
            self.current_path = self.heading_stack.clone();
            return;
        }
        self.push_block(collapse_whitespace(&p.text()));
    }

    fn visit_table(&mut self, table: &Table) {
        for row in &table.rows {
            let cells: Vec<String> = row
                .cells
                .iter()
                .filter_map(|content| match content {
                    TableRowContent::TableCell(cell) => {
                        Some(collapse_whitespace(&cell_text(cell)))
                    }
                    // SDT cells (structured document tags) are rare and
                    // carry no plain text this v1 walk extracts — skipped,
                    // not an error.
                    TableRowContent::SDT(_) => None,
                })
                .collect();
            self.push_block(cells.join("\t"));
        }
    }

    fn push_block(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        self.paragraph_index += 1;
        self.current_blocks.push(Block::Paragraph {
            text,
            locator: format!("¶{}", self.paragraph_index),
        });
    }

    fn close_section(&mut self) {
        if !self.current_blocks.is_empty() {
            self.sections.push(Section {
                path: self.current_path.clone(),
                blocks: std::mem::take(&mut self.current_blocks),
            });
        }
    }

    fn finish(mut self, title: &str) -> Document {
        self.close_section();
        Document {
            title: title.to_string(),
            sections: self.sections,
        }
    }
}

/// A table cell's text: every run's text concatenated in order (no
/// separator — matches `Paragraph::text()`'s own "gather every run's text"
/// behavior, so a word split across two runs isn't spuriously space-split).
fn cell_text(cell: &docx_rust::document::TableCell) -> String {
    cell.iter_text().fold(String::new(), |mut acc, s| {
        acc.push_str(s.as_ref());
        acc
    })
}

/// `"Heading1"`..`"Heading9"` → `Some(1)`..`Some(9)`; anything else (a body
/// style, `None`, or an out-of-range/non-numeric suffix) → `None`. Mirrors
/// [`crate::parse::parse_markdown`]'s `h1`..`h6` handling, extended to
/// DOCX's own `"Heading1".."Heading9"` style-id convention (spec'd in the
/// FMT-DOCX-EPUB brief).
fn heading_level(style_id: &str) -> Option<usize> {
    let level: usize = style_id.strip_prefix("Heading")?.parse().ok()?;
    (1..=9).contains(&level).then_some(level)
}

/// Collapse runs of whitespace to single spaces and trim the ends — the
/// same text-cleanup [`crate::html::html_to_document`] applies, since a
/// DOCX run's text can carry line breaks/tabs that aren't meaningful
/// paragraph structure here.
fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use docx_rust::document::{TableCell, TableRow};

    /// A tiny, real `.docx` — Heading1 "Chapter 1" > paragraph, Heading2
    /// "Section A" (nested under Chapter 1) > paragraph — generated via
    /// python-docx (no pure-Rust `.docx` writer exists) and committed at
    /// `crates/kpack-core/tests/fixtures/sample.docx`. `include_bytes!` so
    /// the test doesn't depend on the process's working directory.
    const SAMPLE_DOCX: &[u8] = include_bytes!("../tests/fixtures/sample.docx");

    // 1. Real fixture: Heading1/Heading2 style ids produce correctly
    // nested Section paths, paragraph text is present, and locators are
    // the running "¶{n}" scheme (not reset per section, headings consume
    // no number).
    #[test]
    fn t1_real_docx_gives_nested_sections_and_paragraph_locators() {
        let doc = document_from_docx(SAMPLE_DOCX, "Sample").expect("fixture should parse");

        let paths: Vec<Vec<String>> = doc.sections.iter().map(|s| s.path.clone()).collect();
        assert_eq!(
            paths,
            vec![
                vec!["Chapter 1".to_string()],
                vec!["Chapter 1".to_string(), "Section A".to_string()],
            ]
        );

        let block_text = |section: usize, block: usize| match &doc.sections[section].blocks[block] {
            Block::Paragraph { text, .. } => text.as_str(),
            other => panic!("expected Block::Paragraph, got {other:?}"),
        };
        assert_eq!(block_text(0, 0), "First paragraph text under chapter 1.");
        assert_eq!(block_text(1, 0), "Second paragraph text under section A.");

        let locator = |section: usize, block: usize| match &doc.sections[section].blocks[block] {
            Block::Paragraph { locator, .. } => locator.as_str(),
            other => panic!("expected Block::Paragraph, got {other:?}"),
        };
        assert_eq!(locator(0, 0), "¶1");
        assert_eq!(locator(1, 0), "¶2");
    }

    // 2. Determinism: parsing the same bytes twice yields identical
    // Documents (no HashMap-ordering or similar nondeterminism leaking
    // through docx-rust).
    #[test]
    fn t2_document_from_docx_is_deterministic() {
        let doc1 = document_from_docx(SAMPLE_DOCX, "Sample").unwrap();
        let doc2 = document_from_docx(SAMPLE_DOCX, "Sample").unwrap();
        assert_eq!(doc1, doc2);
    }

    // 3. Corrupt/unreadable input -> a clean Err, never a panic.
    #[test]
    fn t3_corrupt_bytes_yield_clean_err_not_panic() {
        let result = document_from_docx(b"this is not a docx file at all", "Bad");
        assert!(matches!(result, Err(Error::Parse(_))), "expected Err(Parse), got {result:?}");
    }

    // 4. heading_level: the documented "Heading1".."Heading9" range, and
    // the boundaries/non-matches around it.
    #[test]
    fn t4_heading_level_recognizes_heading1_through_9_only() {
        assert_eq!(heading_level("Heading1"), Some(1));
        assert_eq!(heading_level("Heading9"), Some(9));
        assert_eq!(heading_level("Heading5"), Some(5));
        assert_eq!(heading_level("Heading10"), None, "out of the 1..=9 range");
        assert_eq!(heading_level("Heading0"), None, "out of the 1..=9 range");
        assert_eq!(heading_level("Normal"), None, "not a heading style at all");
        assert_eq!(heading_level("HeadingA"), None, "non-numeric suffix");
    }

    // 5. Table flattening (review gap — sample.docx has no table): a
    // 2-row (header + data), 2-cell-per-row `Table` built in memory via
    // docx-rust's own builder API, run through `Walker::visit_table`
    // directly — each row becomes one Block::Paragraph, cells tab-joined
    // in order, no cell dropped, and rows consume the running "¶{n}"
    // locator the same as ordinary paragraphs (module doc comment's
    // "Tables" section).
    #[test]
    fn t5_visit_table_flattens_rows_to_tab_joined_paragraphs_no_cell_dropped() {
        let table = Table::default()
            .push_row(
                TableRow::default()
                    .push_cell(TableCell::paragraph(Paragraph::default().push_text("Name")))
                    .push_cell(TableCell::paragraph(Paragraph::default().push_text("Age"))),
            )
            .push_row(
                TableRow::default()
                    .push_cell(TableCell::paragraph(Paragraph::default().push_text("Alice")))
                    .push_cell(TableCell::paragraph(Paragraph::default().push_text("30"))),
            );

        let mut walker = Walker::default();
        walker.visit_table(&table);
        let doc = walker.finish("Table Test");

        assert_eq!(doc.sections.len(), 1, "no heading was ever opened -> one root section");
        assert_eq!(doc.sections[0].path, Vec::<String>::new());

        let blocks = &doc.sections[0].blocks;
        assert_eq!(blocks.len(), 2, "one Block::Paragraph per table row, no cell dropped");
        assert_eq!(
            blocks[0],
            Block::Paragraph { text: "Name\tAge".to_string(), locator: "\u{b6}1".to_string() }
        );
        assert_eq!(
            blocks[1],
            Block::Paragraph { text: "Alice\t30".to_string(), locator: "\u{b6}2".to_string() }
        );
    }
}
