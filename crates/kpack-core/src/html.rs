//! HTML → [`crate::tree::Document`] (spec §3.2's parser list, "formats"
//! follow-on to K6). The first of the pure-Rust document formats — DOCX and
//! EPUB land next, and EPUB's per-chapter XHTML REUSES [`html_to_document`]
//! directly, which is why it's `pub` and free of any file-path/build-branch
//! assumptions (it takes an in-memory `&str`, nothing else).
//!
//! HTML is text-native (unlike PDF), so callers reach this the same way
//! they reach [`crate::parse::parse_markdown`]: via `parse::parse`'s
//! `"html"` arm, fed from `SourceContent::Raw` — no dedicated build branch.
//!
//! ## DOM walk
//! [`html_to_document`] parses via `scraper::Html::parse_document` (pure
//! Rust, html5ever-based — no native dependency, no network) and walks the
//! `<body>` in document order, recursing into container elements (`div`,
//! `section`, `ul`, `nav`, …) to find the block elements that actually
//! carry content: `h1`..`h6` open/close [`crate::tree::Section`]s exactly
//! like [`crate::parse::parse_markdown`]'s heading stack (a level-N heading
//! pops the stack to depth N-1 and pushes its own text); `p`/`li`/
//! `blockquote`/`pre` become [`crate::tree::Block::Paragraph`]s under the
//! current section. `pre` gets no special code-block treatment in this v1 —
//! it's flattened to a Paragraph like everything else, the same "flatten
//! reasonably" stance [`crate::parse::parse_markdown`] takes on lists.
//! `table` becomes a structured [`crate::tree::Block::Table`] (header row
//! detected from a `<thead>` row, or an all-`<th>` row if there's no
//! `<thead>`, else no header at all — the same header-vs-data split
//! [`crate::parse::parse_markdown`] does for GFM tables), so the chunker's
//! row-wise/header-repeat handling applies to HTML tables too.
//! `script`/`style`/`head` subtrees are skipped entirely (not walked at
//! all), so their text never reaches a `Block`.
//!
//! ## No content is silently dropped
//! A container that is NOT one of the tags above (`div`, `span`, `a`,
//! `section`, `body`, …) is not content itself, but real-world HTML
//! routinely puts bare text directly inside one anyway — a CMS/static-site
//! export wrapping a paragraph in a `div` instead of a `p`, a raw
//! `<body>text</body>` with no wrapper at all, and so on. The walk
//! captures a container's DIRECT text-node children (non-whitespace) as
//! their own `Block::Paragraph`, in document order relative to its element
//! children, in ADDITION to recursing into those element children — so
//! `<div>Loose text<p>Wrapped</p></div>` yields BOTH "Loose text" and
//! "Wrapped", in that order: never zero, never a duplicate. A recognized
//! leaf's (`p`/`li`/heading/etc.) own text is always captured by ITS OWN
//! visitor via `.text()`, which never recurses back through this loose-text
//! path, so nothing is ever double-counted. (Review Critical, formats
//! slice: this — plus whole `<table>`s vanishing — used to drop real
//! content with no error and no signal; see `html.rs`'s tests 5-8.)
//!
//! ## Honest caveat: no readability pass
//! There is no pure-Rust main-content/readability extractor in this v1, so
//! page boilerplate — nav links, footers, ad copy — that lives in ordinary
//! `<p>`/heading tags is walked and kept exactly like real content; only
//! `script`/`style`/`head` are special-cased. This mirrors the markdown
//! parser's list handling (a readable, deterministic stand-in, not a
//! structural understanding of the page) rather than hiding the gap: a
//! `<nav>` or `<footer>` full of boilerplate `<p>`s WILL leak into the
//! `Document`, and a real readability pass (main-content detection) is a
//! later enhancement, not attempted here.
//!
//! ## Locators
//! HTML has no line numbers to cite the way source text does, so the
//! locator scheme is anchor-based instead: while a heading with an `id`
//! attribute is the current section's heading, every block under it gets
//! that heading's `"#id"` as its locator (a real, clickable in-page
//! anchor); once headings without an `id` (or content before any heading)
//! are current, blocks fall back to a running 1-based `"b{n}"` block index
//! over the whole document — deterministic and unique, if not clickable.
//! Two headings sharing the same `id` (invalid HTML, but not uncommon in
//! hand-rolled real-world pages) produce colliding `#id` locators — this
//! scheme trusts the source's `id` uniqueness, it does not enforce it.

use crate::tree::{Block, Document, Section};
use scraper::{ElementRef, Html, Selector};

/// Parse `html` into a [`Document`] titled `title`. See the module doc
/// comment for the DOM walk, block mapping, locator scheme, and the
/// boilerplate caveat. `pub` so the EPUB parser (a later slice) can call
/// this per-chapter on each chapter's XHTML body.
pub fn html_to_document(html: &str, title: &str) -> Document {
    let document = Html::parse_document(html);
    let root = body_element(&document).unwrap_or_else(|| document.root_element());

    let mut walker = Walker::default();
    walker.walk_children(root);
    walker.finish(title)
}

/// The document's `<body>`, when `scraper`'s permissive parse produced one
/// (it always does for real HTML — html5ever fabricates a `<body>` even for
/// a bare fragment). Falls back to the document root so a headless fragment
/// (no `<html>`/`<body>` at all) still gets walked rather than producing an
/// empty `Document`.
fn body_element(document: &Html) -> Option<ElementRef<'_>> {
    let selector = Selector::parse("body").ok()?;
    document.select(&selector).next()
}

/// Running state for the document-order DOM walk, mirroring
/// [`crate::parse::parse_markdown`]'s heading-stack fields one for one.
#[derive(Default)]
struct Walker {
    sections: Vec<Section>,
    heading_stack: Vec<String>,
    current_path: Vec<String>,
    current_blocks: Vec<Block>,
    /// The current section heading's `id` attribute, when it has one — see
    /// the module doc comment's locator scheme.
    current_heading_id: Option<String>,
    /// Running 1-based index over every block emitted so far (not reset per
    /// section), used for the `"b{n}"` locator fallback.
    block_index: usize,
}

impl Walker {
    /// Visit every child of `element` in document order: ELEMENT children
    /// are dispatched to [`Walker::visit`] (recognized leaves become their
    /// own Block, containers recurse back through here); TEXT-node
    /// children — content that lives directly in `element` with no
    /// wrapping tag at all — become their own `Block::Paragraph` via
    /// [`Walker::visit_loose_text`], so nothing typed straight into a
    /// container is silently lost (see the module doc comment's "No
    /// content is silently dropped"). This never double-counts a
    /// recognized leaf's own text: `visit_text_block`/`visit_heading` read
    /// their text via `.text()` and never call back into `walk_children`,
    /// so a `<p>`'s inner text nodes are never ALSO visited here.
    fn walk_children(&mut self, element: ElementRef) {
        for child in element.children() {
            if let Some(child_el) = ElementRef::wrap(child) {
                self.visit(child_el);
            } else if let Some(text) = child.value().as_text() {
                self.visit_loose_text(text);
            }
        }
    }

    fn visit(&mut self, element: ElementRef) {
        match element.value().name() {
            // Never walked at all — their text (code, CSS, and anything the
            // <head> carries, e.g. <title>) must never reach a Block.
            "script" | "style" | "head" => {}
            "h1" => self.visit_heading(element, 1),
            "h2" => self.visit_heading(element, 2),
            "h3" => self.visit_heading(element, 3),
            "h4" => self.visit_heading(element, 4),
            "h5" => self.visit_heading(element, 5),
            "h6" => self.visit_heading(element, 6),
            // A table's whole subtree (thead/tbody/tr/td/th) is handled by
            // visit_table itself (via a scoped `<tr>` selection), NOT by
            // recursing through walk_children — so its cells are captured
            // exactly once, as structured Table cells, not as loose text.
            "table" => self.visit_table(element),
            // Text-block leaves: their own full descendant text (inline
            // markup like <strong>/<em>/<a> included) becomes one Paragraph;
            // deliberately NOT recursed into further, so a <blockquote>
            // wrapping a <p> yields one block, not two.
            "p" | "li" | "blockquote" | "pre" => self.visit_text_block(element),
            // Everything else (div/section/article/ul/ol/nav/footer/...) is
            // not content itself — recurse to find target elements nested
            // inside (walk_children also captures this element's own loose
            // text-node children, see its doc comment). nav/footer are
            // intentionally NOT skipped here (see the module doc comment's
            // boilerplate caveat).
            _ => self.walk_children(element),
        }
    }

    fn visit_heading(&mut self, element: ElementRef, level: usize) {
        self.close_section();
        self.heading_stack.truncate(level.saturating_sub(1));
        let text = collapse_whitespace(&element.text().collect::<String>());
        self.heading_stack.push(text);
        self.current_path = self.heading_stack.clone();
        self.current_heading_id = element
            .value()
            .attr("id")
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string);
    }

    fn visit_text_block(&mut self, element: ElementRef) {
        let text = collapse_whitespace(&element.text().collect::<String>());
        self.emit_paragraph(text);
    }

    /// A text node that is a DIRECT child of a container being recursed
    /// through — i.e. not wrapped in any recognized tag at all — becomes
    /// its own Paragraph. Same whitespace handling and emptiness check as
    /// every other text block; see [`Walker::walk_children`] for why this
    /// never overlaps with a recognized leaf's own text.
    fn visit_loose_text(&mut self, text: &str) {
        let text = collapse_whitespace(text);
        self.emit_paragraph(text);
    }

    /// Build a structured `Block::Table` from `element` (a `<table>`):
    /// every `<tr>` anywhere in its subtree becomes one row, in document
    /// order; the FIRST row that looks like a header (inside a `<thead>`,
    /// or — absent a `<thead>` — a row whose cells are all `<th>`) becomes
    /// `header`, every other row becomes a `rows` entry. A table with
    /// neither gets an empty `header` and every row as data — still a real
    /// `Block::Table`, not a dropped one. Rows with zero `<td>`/`<th>`
    /// cells (e.g. a stray whitespace-only `<tr>`) are skipped; a present-
    /// but-empty cell still counts (preserves column alignment, matching
    /// `parse_markdown`'s GFM table handling).
    fn visit_table(&mut self, element: ElementRef) {
        let Some(row_selector) = Selector::parse("tr").ok() else {
            // Unreachable in practice — "tr" is always a valid selector —
            // but fails soft rather than panicking on the (impossible)
            // error path.
            return;
        };

        let mut header: Vec<String> = Vec::new();
        let mut rows: Vec<Vec<String>> = Vec::new();
        let mut header_taken = false;

        for row in element.select(&row_selector) {
            let cells = table_row_cells(row);
            if cells.is_empty() {
                continue;
            }
            if !header_taken && is_header_row(row) {
                header = cells;
                header_taken = true;
            } else {
                rows.push(cells);
            }
        }

        if header.is_empty() && rows.is_empty() {
            return;
        }

        let locator = self.next_locator();
        self.current_blocks.push(Block::Table { header, rows, locator });
    }

    /// The locator for the next block: the current section heading's
    /// `"#id"` anchor when it has one, else the running `"b{n}"` block
    /// index (see the module doc comment's locator scheme). Always
    /// advances `block_index`, even when the `#id` branch is taken, so the
    /// running count stays a true count of every block emitted so far.
    fn next_locator(&mut self) -> String {
        self.block_index += 1;
        match &self.current_heading_id {
            Some(id) => format!("#{id}"),
            None => format!("b{}", self.block_index),
        }
    }

    fn emit_paragraph(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        let locator = self.next_locator();
        self.current_blocks.push(Block::Paragraph { text, locator });
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

/// `row`'s cell text, in document order, from its DIRECT `<td>`/`<th>`
/// children only (a nested table inside a cell has its own `<tr>`s, picked
/// up separately by `element.select("tr")` in [`Walker::visit_table`], not
/// folded into this row). Every cell is included even if its text is
/// empty, to preserve column alignment — matches `parse_markdown`'s GFM
/// table-cell handling, which never skips an empty cell either.
fn table_row_cells(row: ElementRef) -> Vec<String> {
    row.children()
        .filter_map(ElementRef::wrap)
        .filter(|cell| matches!(cell.value().name(), "td" | "th"))
        .map(|cell| collapse_whitespace(&cell.text().collect::<String>()))
        .collect()
}
/// Whether `row` is this table's header row: either it lives inside a
/// `<thead>` anywhere up its ancestor chain, or — for a table with no
/// `<thead>` at all — every one of its cells is a `<th>` (and it has at
/// least one cell). A row with a mix of `<th>`/`<td>`, or all `<td>`, is
/// never treated as a header.
fn is_header_row(row: ElementRef) -> bool {
    let in_thead = row
        .ancestors()
        .any(|a| a.value().as_element().map(|e| e.name()) == Some("thead"));
    if in_thead {
        return true;
    }
    let mut saw_cell = false;
    let mut all_th = true;
    for child in row.children() {
        if let Some(cell) = ElementRef::wrap(child) {
            match cell.value().name() {
                "th" => saw_cell = true,
                "td" => {
                    saw_cell = true;
                    all_th = false;
                }
                _ => {}
            }
        }
    }
    saw_cell && all_th
}

/// Collapse runs of whitespace (including the newlines/indentation of
/// pretty-printed HTML source) to single spaces and trim the ends — mirrors
/// the text-cleanup [`crate::parse::parse_markdown`]/[`crate::parse::parse_txt`]
/// apply via `.trim()` on their own accumulated buffers.
fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    // 1. Headings -> correct Section.path ancestry; paragraph text present;
    // boilerplate <nav>/<footer> <p> text IS present (the documented
    // caveat — a later readability pass would remove it, not this one);
    // script/style content is NOT present anywhere in the Document.
    #[test]
    fn t1_section_structure_and_documented_boilerplate_leak() {
        let html = "\
<html>
<head><title>Ignored Title</title><style>.x { color: red; }</style></head>
<body>
<nav><p>Home | About | Contact</p></nav>
<h1 id=\"top\">Chapter 1</h1>
<p>First paragraph of chapter 1.</p>
<h2>Section A</h2>
<p>Paragraph under Section A.</p>
<script>alert('should not appear');</script>
<footer><p>Copyright 2026 Boilerplate Inc.</p></footer>
</body>
</html>";
        let doc = html_to_document(html, "Doc");

        let paths: Vec<Vec<String>> = doc.sections.iter().map(|s| s.path.clone()).collect();
        assert_eq!(
            paths,
            vec![
                Vec::<String>::new(),
                vec!["Chapter 1".to_string()],
                vec!["Chapter 1".to_string(), "Section A".to_string()],
            ]
        );

        let all_text: Vec<&str> = doc
            .sections
            .iter()
            .flat_map(|s| s.blocks.iter())
            .map(|b| match b {
                Block::Paragraph { text, .. } => text.as_str(),
                other => panic!("expected Block::Paragraph, got {other:?}"),
            })
            .collect();

        // Root section (before the first heading) holds the <nav> boilerplate.
        assert!(all_text.iter().any(|t| t.contains("Home | About | Contact")));
        assert!(all_text.iter().any(|t| t.contains("First paragraph of chapter 1")));
        assert!(all_text.iter().any(|t| t.contains("Paragraph under Section A")));
        // Honest caveat: the <footer> boilerplate leaks in too, undocumented
        // by omission — asserted present, not hidden.
        assert!(all_text.iter().any(|t| t.contains("Copyright 2026 Boilerplate Inc")));

        // script/style text must never appear anywhere.
        let joined = all_text.join(" ");
        assert!(!joined.contains("should not appear"));
        assert!(!joined.contains("color: red"));
        assert!(!joined.contains("Ignored Title"));
    }

    // 2. Locators: a heading with an id -> every block under it gets "#id";
    // content with no id in scope (root, or a heading without one) falls
    // back to a running "b{n}" block index.
    #[test]
    fn t2_locators_are_heading_id_or_running_block_index() {
        let html = "\
<body>
<p>Root paragraph, no heading yet.</p>
<h1 id=\"intro\">Intro</h1>
<p>Under Intro, first.</p>
<p>Under Intro, second.</p>
<h2>No Id Here</h2>
<p>Under the id-less heading.</p>
</body>";
        let doc = html_to_document(html, "Doc");
        assert_eq!(doc.sections.len(), 3);

        let locator = |block: &Block| match block {
            Block::Paragraph { locator, .. } => locator.clone(),
            other => panic!("expected Block::Paragraph, got {other:?}"),
        };

        assert_eq!(locator(&doc.sections[0].blocks[0]), "b1");
        assert_eq!(locator(&doc.sections[1].blocks[0]), "#intro");
        assert_eq!(locator(&doc.sections[1].blocks[1]), "#intro");
        // Falls back to the running index (not reset per section) once
        // back under a heading with no id.
        assert_eq!(locator(&doc.sections[2].blocks[0]), "b4");
    }

    // 3. Empty/whitespace-only blocks are dropped rather than emitted as
    // empty Paragraphs.
    #[test]
    fn t3_empty_and_whitespace_only_blocks_are_dropped() {
        let html = "\
<body>
<h1>Ch 1</h1>
<p>   </p>
<p></p>
<p>Real content.</p>
</body>";
        let doc = html_to_document(html, "Doc");
        assert_eq!(doc.sections.len(), 1);
        assert_eq!(doc.sections[0].blocks.len(), 1);
        match &doc.sections[0].blocks[0] {
            Block::Paragraph { text, .. } => assert_eq!(text, "Real content."),
            other => panic!("expected Block::Paragraph, got {other:?}"),
        }
    }

    // 4. Determinism: same source parsed twice yields identical Documents.
    #[test]
    fn t4_html_to_document_is_deterministic() {
        let html = "<body><h1>A</h1><p>Text.</p><ul><li>One</li><li>Two</li></ul></body>";
        let doc1 = html_to_document(html, "Doc");
        let doc2 = html_to_document(html, "Doc");
        assert_eq!(doc1, doc2);
    }

    // 5-8: review Critical — content that isn't wrapped in a recognized
    // leaf tag (a bare <div>/<span>/<body> text node) or lives inside a
    // <table> used to vanish with zero blocks and no error. These lock
    // down the fix: nothing is silently dropped anymore.

    // 5. Loose text — content typed directly into a container with no
    // wrapping <p>/<li>/etc — is captured, not silently dropped.
    #[test]
    fn t5_loose_text_in_bare_containers_is_captured_not_dropped() {
        let div_doc = html_to_document("<body><div>bare text</div></body>", "Doc");
        assert_eq!(div_doc.sections.len(), 1);
        assert_eq!(div_doc.sections[0].blocks.len(), 1);
        match &div_doc.sections[0].blocks[0] {
            Block::Paragraph { text, .. } => assert_eq!(text, "bare text"),
            other => panic!("expected Block::Paragraph, got {other:?}"),
        }

        let body_doc = html_to_document("<body>raw text, no wrapper at all.</body>", "Doc");
        assert_eq!(body_doc.sections.len(), 1);
        assert_eq!(body_doc.sections[0].blocks.len(), 1);
        match &body_doc.sections[0].blocks[0] {
            Block::Paragraph { text, .. } => assert_eq!(text, "raw text, no wrapper at all."),
            other => panic!("expected Block::Paragraph, got {other:?}"),
        }

        let span_doc = html_to_document("<body><span>span text</span></body>", "Doc");
        assert_eq!(span_doc.sections[0].blocks.len(), 1);
        match &span_doc.sections[0].blocks[0] {
            Block::Paragraph { text, .. } => assert_eq!(text, "span text"),
            other => panic!("expected Block::Paragraph, got {other:?}"),
        }
    }

    // 6. Mixed loose text + a wrapped child in the same container: BOTH
    // survive, in document order, with no duplication and no drop — the
    // loose text is captured once by the container's own walk, the <p>'s
    // text is captured once by its own leaf visitor, never both/neither.
    #[test]
    fn t6_loose_text_and_wrapped_child_both_survive_in_order() {
        let doc = html_to_document("<body><div>Loose text<p>Wrapped</p></div></body>", "Doc");
        assert_eq!(doc.sections.len(), 1);
        let texts: Vec<&str> = doc.sections[0]
            .blocks
            .iter()
            .map(|b| match b {
                Block::Paragraph { text, .. } => text.as_str(),
                other => panic!("expected Block::Paragraph, got {other:?}"),
            })
            .collect();
        assert_eq!(texts, vec!["Loose text", "Wrapped"]);
    }

    // 7. A <table> with a <thead> header row and two <tbody> data rows ->
    // a structured Block::Table (mirrors parse_markdown's GFM table
    // handling), not zero blocks.
    #[test]
    fn t7_table_with_header_and_rows_becomes_structured_table_block() {
        let html = "\
<body>
<table>
<thead><tr><th>Name</th><th>Age</th></tr></thead>
<tbody>
<tr><td>Alice</td><td>30</td></tr>
<tr><td>Bob</td><td>40</td></tr>
</tbody>
</table>
</body>";
        let doc = html_to_document(html, "Doc");
        assert_eq!(doc.sections.len(), 1);
        assert_eq!(doc.sections[0].blocks.len(), 1);
        match &doc.sections[0].blocks[0] {
            Block::Table { header, rows, .. } => {
                assert_eq!(header, &vec!["Name".to_string(), "Age".to_string()]);
                assert_eq!(
                    rows,
                    &vec![
                        vec!["Alice".to_string(), "30".to_string()],
                        vec!["Bob".to_string(), "40".to_string()],
                    ]
                );
            }
            other => panic!("expected Block::Table, got {other:?}"),
        }
    }

    // 8. A <table> with no <thead>/<th> at all (just a bare <tr><td>) — the
    // exact review-reported case that used to vanish entirely — still
    // produces a Block::Table, with an empty header and the cell text
    // present rather than lost.
    #[test]
    fn t8_table_with_no_header_still_captures_cell_text() {
        let doc = html_to_document(
            "<body><table><tr><td>Alice</td><td>30</td></tr></table></body>",
            "Doc",
        );
        assert_eq!(doc.sections.len(), 1);
        assert_eq!(doc.sections[0].blocks.len(), 1);
        match &doc.sections[0].blocks[0] {
            Block::Table { header, rows, .. } => {
                assert!(header.is_empty());
                assert_eq!(rows, &vec![vec!["Alice".to_string(), "30".to_string()]]);
            }
            other => panic!("expected Block::Table, got {other:?}"),
        }
    }
}
