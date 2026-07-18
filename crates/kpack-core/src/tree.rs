//! The parsed-document intermediate form (spec §1.3 step 1: "Parse to a
//! heading tree"). K6's parsers (PDF/EPUB/DOCX/MD/HTML) produce this; the
//! `chunk` module (K5) consumes it. Pure data — no parsing logic lives
//! here, only the shape both sides agree on.
//!
//! ## Leaf sections (spec §1.3)
//! "Every chunk belongs to exactly one leaf section; chunks never straddle
//! heading boundaries." A [`Section`] is therefore always a LEAF of the
//! heading tree: its `path` is the full ancestry from the document root
//! down to (and including) this section's own heading, and its `blocks`
//! are the content that lives directly under that heading, before the next
//! heading of equal-or-shallower depth. A document with no headings at all
//! is still one root `Section`, with an empty `path` (D6 — see
//! [`Section::section_path_string`]).

/// A parsed source document: a title plus its heading-tree sections, each
/// already flattened to a leaf list in document order (spec §1.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    pub title: String,
    pub sections: Vec<Section>,
}

/// One leaf section of the heading tree. `path` is the heading ancestry
/// (e.g. `["Ch 4", "Hemostasis", "Vitamin K"]`); `blocks` is that section's
/// content in document order. Chunking (`chunk::chunk_document`) treats
/// each `Section` as an independent unit — chunks are never assembled
/// across two sections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub path: Vec<String>,
    pub blocks: Vec<Block>,
}

impl Section {
    /// The citation-display form of `path` (spec §1.1's `section_path`
    /// example: `"Ch 4 > Hemostasis > Vitamin K"`) — joins `path` with
    /// `" > "`. An empty `path` (D6's root/no-heading case) yields `""`,
    /// never a panic and never a stray leading/trailing separator.
    pub fn section_path_string(&self) -> String {
        self.path.join(" > ")
    }
}

/// One content unit within a [`Section`], in document order. Every variant
/// carries a `locator` — the citation source position (a line/paragraph/page
/// reference); K6's parsers fill this from the real source, tests supply it
/// directly. `locator` is a caller-defined opaque string, not parsed or
/// validated here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// Free-text prose. The `chunk` module sentence-splits and
    /// sliding-windows this.
    Paragraph { text: String, locator: String },
    /// A table, kept as structured cells (not pre-flattened text) so the
    /// chunker can serialize it row-wise with the header repeated per chunk
    /// (spec §1.3).
    Table {
        header: Vec<String>,
        rows: Vec<Vec<String>>,
        locator: String,
    },
    /// A code block, chunked atomically (never split, spec §1.3).
    Code { text: String, locator: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    // 1. Multi-element path joins with " > ", matching spec §1.1's
    // section_path example format exactly.
    #[test]
    fn t1_section_path_string_joins_with_arrow() {
        let section = Section {
            path: vec!["Ch 4".to_string(), "Hemostasis".to_string(), "Vitamin K".to_string()],
            blocks: vec![],
        };
        assert_eq!(section.section_path_string(), "Ch 4 > Hemostasis > Vitamin K");
    }

    // 2. Empty path (D6's root/no-heading case) → "", not a panic, not a
    // stray separator.
    #[test]
    fn t2_section_path_string_empty_path_is_empty_string() {
        let section = Section {
            path: vec![],
            blocks: vec![],
        };
        assert_eq!(section.section_path_string(), "");
    }

    // 3. Single-element path → just that element, no separator at all.
    #[test]
    fn t3_section_path_string_single_element_no_separator() {
        let section = Section {
            path: vec!["Introduction".to_string()],
            blocks: vec![],
        };
        assert_eq!(section.section_path_string(), "Introduction");
    }
}
