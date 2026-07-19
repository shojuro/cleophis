//! Markdown + plain-text parsers → [`crate::tree::Document`] (spec §1.3
//! step 1 "Parse to a heading tree"; §3.2's parser list: "pulldown-cmark for
//! MD"). This module produces what [`crate::chunk`] (K5) consumes; the two
//! sides agree on the shape defined in [`crate::tree`], not on anything
//! here. PDF/EPUB/DOCX parsers are out of scope for K6 and land later
//! (§3.2's remaining formats); HTML lands in the formats slice as
//! [`crate::html::html_to_document`], dispatched from [`parse`] below —
//! it's a separate module (not here) because the follow-on EPUB parser
//! reuses it directly, per-chapter.
//!
//! ## Heading tree walk (markdown)
//! [`parse_markdown`] walks `pulldown-cmark`'s event stream with a running
//! heading stack: a level-N heading pops the stack to depth N-1 and pushes
//! its own text, so every subsequent content block is attached to a
//! [`crate::tree::Section`] whose `path` is that heading's full ancestry —
//! matching [`crate::tree::Section`]'s own doc comment ("its own heading"
//! included). Content before the first heading goes to a root section
//! (`path == []`, D6 — see [`crate::tree::Section::section_path_string`]).
//! Consecutive blocks under one heading are grouped into a single `Section`
//! (never one `Section` per block) — a new `Section` is only cut when the
//! heading path changes or the document ends.
//!
//! ## Block mapping
//! - Paragraphs → [`crate::tree::Block::Paragraph`].
//! - Fenced/indented code → [`crate::tree::Block::Code`] (atomic — the
//!   chunker never splits it, spec §1.3 step 3).
//! - GFM tables (`Options::ENABLE_TABLES`) → [`crate::tree::Block::Table`]
//!   with structured `header`/`rows` (not pre-flattened text), so the
//!   chunker can serialize them row-wise itself (spec §1.3 step 3,
//!   [`crate::chunk`]'s module doc comment).
//! - Lists and blockquotes have no dedicated `Block` variant (v1, per the
//!   K6 brief: "treat as Paragraph text (flatten reasonably)"). A
//!   blockquote's inner paragraph already becomes an ordinary
//!   `Block::Paragraph` with no extra handling needed here — pulldown-cmark
//!   emits it as a normal nested `Paragraph` tag. A list, tight or loose,
//!   nested or not, is flattened into ONE `Block::Paragraph` whose text is
//!   `"- "`-prefixed lines (see `Target::ListItem` below); this is a
//!   readable, deterministic v1 stand-in, not a structural representation.
//!
//! ## Locators
//! Every block's `locator` is a human-readable source position for
//! citations, derived from `pulldown-cmark`'s `OffsetIter` byte ranges
//! (`Parser::into_offset_iter`) mapped to 1-based line numbers: `"L12"` for
//! a single-line block, `"L12-L15"` for a multi-line one. The byte→line
//! mapping ([`line_number`]) is a deterministic binary search over
//! precomputed line-start offsets — no locale or iteration-order
//! dependence, so the same source always yields the same locators.

use crate::html::html_to_document;
use crate::tree::{Block, Document, Section};
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

/// Parse `source` as CommonMark + GFM tables into a [`Document`] titled
/// `title` (spec §1.3 step 1, §3.2). See the module doc comment for the
/// heading-tree walk, block mapping, and locator scheme.
pub fn parse_markdown(source: &str, title: &str) -> Document {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);

    let line_starts = compute_line_starts(source);

    let mut sections: Vec<Section> = Vec::new();
    let mut heading_stack: Vec<String> = Vec::new();
    let mut current_path: Vec<String> = Vec::new();
    let mut current_blocks: Vec<Block> = Vec::new();

    let mut target = Target::None;
    let mut buffers = Buffers::default();

    // Byte offsets of the currently-open paragraph/code/list block, for
    // computing that block's locator once it closes.
    let mut paragraph_start = 0usize;
    let mut code_start = 0usize;
    let mut list_start = 0usize;
    let mut list_depth = 0usize;

    // Table scratch state — a table's own structure (header vs. body rows)
    // can't be tracked with a single text buffer like the other block
    // kinds, so it gets dedicated fields.
    let mut table_header: Vec<String> = Vec::new();
    let mut table_rows: Vec<Vec<String>> = Vec::new();
    let mut current_row: Vec<String> = Vec::new();
    let mut in_table_head = false;
    let mut table_start = 0usize;

    for (event, range) in Parser::new_ext(source, options).into_offset_iter() {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                if !current_blocks.is_empty() {
                    sections.push(Section {
                        path: current_path.clone(),
                        blocks: std::mem::take(&mut current_blocks),
                    });
                }
                heading_stack.truncate(heading_level_to_usize(level).saturating_sub(1));
                buffers.heading.clear();
                target = Target::Heading;
            }
            Event::End(TagEnd::Heading(_)) => {
                heading_stack.push(buffers.heading.trim().to_string());
                current_path = heading_stack.clone();
                target = Target::None;
                // Headings intentionally get no own locator/Block.
            }
            Event::Start(Tag::Paragraph) => {
                if list_depth == 0 {
                    buffers.paragraph.clear();
                    paragraph_start = range.start;
                    target = Target::Paragraph;
                } else if !buffers.list.is_empty() && !buffers.list.ends_with(char::is_whitespace) {
                    // Loose-list continuation paragraph: pulldown-cmark
                    // wraps every block child of a loose-list item in its
                    // own Paragraph tag, so without a separator this
                    // paragraph's text would glue directly onto the
                    // previous one's. Mirrors the separator logic in the
                    // `Tag::Item` handler below.
                    buffers.list.push('\n');
                }
            }
            Event::End(TagEnd::Paragraph) => {
                if list_depth == 0 {
                    let text = buffers.paragraph.trim();
                    if !text.is_empty() {
                        current_blocks.push(Block::Paragraph {
                            text: text.to_string(),
                            locator: line_locator(&line_starts, paragraph_start, range.end),
                        });
                    }
                    target = Target::None;
                }
            }
            Event::Start(Tag::CodeBlock(_)) => {
                buffers.code.clear();
                code_start = range.start;
                target = Target::Code;
            }
            Event::End(TagEnd::CodeBlock) => {
                let text = buffers.code.trim_end_matches('\n').to_string();
                current_blocks.push(Block::Code {
                    text,
                    locator: line_locator(&line_starts, code_start, range.end),
                });
                target = Target::None;
            }
            Event::Start(Tag::List(_)) => {
                list_depth += 1;
                if list_depth == 1 {
                    buffers.list.clear();
                    list_start = range.start;
                    target = Target::ListItem;
                }
            }
            Event::End(TagEnd::List(_)) => {
                list_depth -= 1;
                if list_depth == 0 {
                    let text = buffers.list.trim();
                    if !text.is_empty() {
                        current_blocks.push(Block::Paragraph {
                            text: text.to_string(),
                            locator: line_locator(&line_starts, list_start, range.end),
                        });
                    }
                    target = Target::None;
                }
            }
            Event::Start(Tag::Item) => {
                if target == Target::ListItem {
                    if !buffers.list.is_empty() {
                        buffers.list.push('\n');
                    }
                    buffers.list.push_str("- ");
                }
            }
            Event::Start(Tag::Table(_)) => {
                table_header.clear();
                table_rows.clear();
                current_row.clear();
                table_start = range.start;
                in_table_head = false;
            }
            Event::Start(Tag::TableHead) => {
                in_table_head = true;
            }
            Event::End(TagEnd::TableHead) => {
                in_table_head = false;
            }
            Event::Start(Tag::TableRow) => {
                current_row.clear();
            }
            Event::End(TagEnd::TableRow) => {
                table_rows.push(std::mem::take(&mut current_row));
            }
            Event::Start(Tag::TableCell) => {
                buffers.cell.clear();
                target = Target::TableCell;
            }
            Event::End(TagEnd::TableCell) => {
                let value = buffers.cell.trim().to_string();
                if in_table_head {
                    table_header.push(value);
                } else {
                    current_row.push(value);
                }
                target = Target::None;
            }
            Event::End(TagEnd::Table) => {
                current_blocks.push(Block::Table {
                    header: std::mem::take(&mut table_header),
                    rows: std::mem::take(&mut table_rows),
                    locator: line_locator(&line_starts, table_start, range.end),
                });
            }
            Event::Text(text) | Event::Code(text) => buffers.push(target, &text),
            Event::SoftBreak => buffers.push(target, " "),
            Event::HardBreak => buffers.push(target, "\n"),
            // Everything else (emphasis/links/images/HTML/rules/footnotes/
            // task-list markers/...) either has no text payload or is
            // inline formatting we deliberately flatten away — the active
            // `target`'s buffer already accumulates the surrounding text.
            _ => {}
        }
    }

    if !current_blocks.is_empty() {
        sections.push(Section {
            path: current_path,
            blocks: current_blocks,
        });
    }

    Document {
        title: title.to_string(),
        sections,
    }
}

/// Parse `source` as plain text into a [`Document`] titled `title`: one
/// root [`Section`] (`path == []`, no headings at all), its content split
/// on blank lines into [`Block::Paragraph`]s, each with a line-range
/// locator (spec §3.2).
pub fn parse_txt(source: &str, title: &str) -> Document {
    let lines: Vec<&str> = source.lines().collect();
    let mut blocks = Vec::new();

    // 0-based index of the current paragraph's first and most recent
    // non-blank line; `None` when not currently inside a paragraph (i.e.
    // the previous line, if any, was blank).
    let mut para_start: Option<usize> = None;
    let mut para_end = 0usize;

    for (i, line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            if let Some(start) = para_start.take() {
                push_txt_paragraph(&mut blocks, &lines, start, para_end);
            }
        } else {
            if para_start.is_none() {
                para_start = Some(i);
            }
            para_end = i;
        }
    }
    if let Some(start) = para_start {
        push_txt_paragraph(&mut blocks, &lines, start, para_end);
    }

    let sections = if blocks.is_empty() {
        // Match parse_markdown("")'s zero-sections result: a content-less
        // input (empty, or whitespace/blank-only) has nothing to chunk, so
        // it shouldn't manufacture an empty root Section either.
        Vec::new()
    } else {
        vec![Section { path: Vec::new(), blocks }]
    };

    Document {
        title: title.to_string(),
        sections,
    }
}

fn push_txt_paragraph(blocks: &mut Vec<Block>, lines: &[&str], start: usize, end: usize) {
    let text = lines[start..=end].join("\n");
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    let locator = if start == end {
        format!("L{}", start + 1)
    } else {
        format!("L{}-L{}", start + 1, end + 1)
    };
    blocks.push(Block::Paragraph {
        text: text.to_string(),
        locator,
    });
}

/// Dispatch on `source_type`: `"md"`/`"markdown"` → [`parse_markdown`],
/// `"html"` → [`html_to_document`] (the formats slice's first pure-Rust
/// format — HTML is text-native, so it reaches this dispatcher the same
/// way MD/TXT do, via `SourceContent::Raw`), anything else (including
/// `"txt"`) → [`parse_txt`] (spec §3.2's parser selection; unrecognized
/// extensions degrade to plain text rather than failing the whole file,
/// consistent with "parser failures degrade per-file, never per-build").
pub fn parse(source: &str, title: &str, source_type: &str) -> Document {
    match source_type {
        "md" | "markdown" => parse_markdown(source, title),
        "html" => html_to_document(source, title),
        _ => parse_txt(source, title),
    }
}

/// Minimal `extraction_quality` (spec §3.2 F2): the fraction of `text`'s
/// characters that are printable-or-whitespace, as opposed to control
/// characters or the U+FFFD replacement character (the tell-tale of a
/// mis-decoded byte stream). This is a deliberate stand-in, adequate ONLY
/// because MD/TXT are text-native and essentially never mojibake — the
/// real dictionary-word-ratio + ligature-artifact scoring §3.2 describes
/// (needed for PDF's lossy text-layer extraction) lands with the §3
/// builder milestone, not here. An empty string has no garbled characters
/// to find, so it scores a clean 1.0 rather than the 0/0 that would
/// otherwise divide-by-zero.
pub fn extraction_quality(text: &str) -> f64 {
    if text.is_empty() {
        return 1.0;
    }
    let total = text.chars().count();
    let good = text
        .chars()
        .filter(|&c| c != '\u{FFFD}' && (c.is_whitespace() || !c.is_control()))
        .count();
    good as f64 / total as f64
}

/// Which in-progress block a `Text`/`Code`/`SoftBreak`/`HardBreak` event's
/// content belongs to, tracked as a single current value (not a stack)
/// because none of these block kinds nest inside one another in the
/// subset of markdown this parser handles — a table cell can't contain a
/// paragraph, a code block can't contain a heading, etc. Lists are the one
/// case that spans nested `Start`/`End` events of their own kind (nested
/// sub-lists, loose-list item paragraphs); `ListItem` is deliberately left
/// set for the whole outer list's duration rather than toggled per nested
/// tag, which is what makes the "flatten reasonably" list handling work
/// without a stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    None,
    Heading,
    Paragraph,
    Code,
    ListItem,
    TableCell,
}

/// The text accumulators for each [`Target`], reset (`.clear()`) when its
/// block starts and read when it ends.
#[derive(Debug, Default)]
struct Buffers {
    heading: String,
    paragraph: String,
    code: String,
    list: String,
    cell: String,
}

impl Buffers {
    fn push(&mut self, target: Target, s: &str) {
        match target {
            Target::None => {}
            Target::Heading => self.heading.push_str(s),
            Target::Paragraph => self.paragraph.push_str(s),
            Target::Code => self.code.push_str(s),
            Target::ListItem => self.list.push_str(s),
            Target::TableCell => self.cell.push_str(s),
        }
    }
}

fn heading_level_to_usize(level: HeadingLevel) -> usize {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// Byte offset of the start of every line in `source` (index `i` → the
/// 0-based byte offset of line `i+1`), used by [`line_number`] to map a
/// `pulldown-cmark` event's byte range to a 1-based line number. `source`
/// is scanned once per parse; `Document`s aren't large enough for this to
/// matter, and it keeps the mapping a deterministic function of `source`
/// alone (no dependence on event order).
fn compute_line_starts(source: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (i, b) in source.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    starts
}

/// The 1-based line number containing byte offset `pos`, via binary search
/// over `line_starts` ([`compute_line_starts`]). Deterministic: the same
/// `(line_starts, pos)` always yields the same line number, no locale or
/// hashing dependence.
fn line_number(line_starts: &[usize], pos: usize) -> usize {
    match line_starts.binary_search(&pos) {
        // `pos` is exactly a line's first byte -> that line, 1-based.
        Ok(idx) => idx + 1,
        // `pos` falls strictly between line_starts[idx-1] and
        // line_starts[idx] -> it's on line idx (1-based); `idx` is never 0
        // here since line_starts[0] == 0 and pos >= 0 always match Ok, not
        // Err, at that boundary.
        Err(idx) => idx,
    }
}

/// A human-readable line-range locator for the byte range `[start, end)`
/// (spec §3.2's citation-source-position requirement): `"L12"` when the
/// range stays on one line, `"L12-L15"` when it spans several. `end` is
/// pulldown-cmark's exclusive byte offset, so it's stepped back by one
/// before mapping to a line — otherwise a block whose range ends exactly at
/// the newline starting the next line would over-report by one line.
fn line_locator(line_starts: &[usize], start: usize, end: usize) -> String {
    let start_line = line_number(line_starts, start);
    let end_pos = end.saturating_sub(1).max(start);
    let end_line = line_number(line_starts, end_pos);
    if start_line == end_line {
        format!("L{start_line}")
    } else {
        format!("L{start_line}-L{end_line}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{chunk_document, ChunkConfig};
    use crate::embed::MockEmbedder;

    // 1. Nested headings -> correct Section.path ancestry for content under
    // each heading; content before the first heading -> root section
    // (path == []).
    #[test]
    fn t1_nested_headings_give_correct_section_ancestry() {
        let source = "\
Intro text before any heading.

# Ch 1

Ch 1 intro paragraph.

## Ch 1 Sub A

Sub A paragraph.

# Ch 2

Ch 2 paragraph.
";
        let doc = parse_markdown(source, "Doc");
        let paths: Vec<Vec<String>> = doc.sections.iter().map(|s| s.path.clone()).collect();
        assert_eq!(
            paths,
            vec![
                Vec::<String>::new(),
                vec!["Ch 1".to_string()],
                vec!["Ch 1".to_string(), "Ch 1 Sub A".to_string()],
                vec!["Ch 2".to_string()],
            ]
        );
        assert_eq!(doc.sections[0].blocks.len(), 1);
        match &doc.sections[0].blocks[0] {
            Block::Paragraph { text, .. } => assert_eq!(text, "Intro text before any heading."),
            other => panic!("expected root paragraph, got {other:?}"),
        }
        match &doc.sections[2].blocks[0] {
            Block::Paragraph { text, .. } => assert_eq!(text, "Sub A paragraph."),
            other => panic!("expected Sub A paragraph, got {other:?}"),
        }
    }

    // 2. A fenced code block -> Block::Code (atomic, not split into
    // paragraphs).
    #[test]
    fn t2_fenced_code_block_is_atomic_code_block() {
        let source = "\
# Ch 1

```rust
fn main() {
    println!(\"hi\");
}
```
";
        let doc = parse_markdown(source, "Doc");
        assert_eq!(doc.sections.len(), 1);
        assert_eq!(doc.sections[0].blocks.len(), 1);
        match &doc.sections[0].blocks[0] {
            Block::Code { text, .. } => {
                assert!(text.contains("fn main()"));
                assert!(text.contains("println!(\"hi\");"));
            }
            other => panic!("expected Block::Code, got {other:?}"),
        }
    }

    // 3. A GFM table -> Block::Table with the right header + rows.
    #[test]
    fn t3_gfm_table_has_correct_header_and_rows() {
        let source = "\
# Data

| Name  | Age |
|-------|-----|
| Alice | 30  |
| Bob   | 40  |
";
        let doc = parse_markdown(source, "Doc");
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

    // 4. Paragraph locators are plausible line refs; deterministic across
    // two parses.
    #[test]
    fn t4_paragraph_locators_are_plausible_and_deterministic() {
        let source = "\
# Ch 1

First paragraph on line 3.

Second paragraph on line 5.
";
        let doc1 = parse_markdown(source, "Doc");
        let doc2 = parse_markdown(source, "Doc");
        assert_eq!(doc1, doc2, "parse_markdown must be deterministic across calls");

        let locators: Vec<&str> = doc1.sections[0]
            .blocks
            .iter()
            .map(|b| match b {
                Block::Paragraph { locator, .. } => locator.as_str(),
                other => panic!("expected paragraphs, got {other:?}"),
            })
            .collect();
        assert_eq!(locators, vec!["L3", "L5"]);
    }

    // 5. parse_txt -> one root section, paragraphs split on blank lines.
    #[test]
    fn t5_parse_txt_one_root_section_split_on_blank_lines() {
        let source = "First paragraph,\nstill first.\n\nSecond paragraph.\n\n\nThird paragraph.\n";
        let doc = parse_txt(source, "Notes");
        assert_eq!(doc.sections.len(), 1);
        assert_eq!(doc.sections[0].path, Vec::<String>::new());
        let texts: Vec<&str> = doc.sections[0]
            .blocks
            .iter()
            .map(|b| match b {
                Block::Paragraph { text, .. } => text.as_str(),
                other => panic!("expected paragraphs, got {other:?}"),
            })
            .collect();
        assert_eq!(texts, vec!["First paragraph,\nstill first.", "Second paragraph.", "Third paragraph."]);
    }

    // 6. extraction_quality ~1.0 for clean text; lower for a string stuffed
    // with replacement chars (U+FFFD).
    #[test]
    fn t6_extraction_quality_clean_vs_replacement_stuffed() {
        let clean = "This is perfectly ordinary, printable English prose.";
        let clean_score = extraction_quality(clean);
        assert!(clean_score > 0.99, "clean text scored {clean_score}, expected ~1.0");

        let garbled: String = "Some text \u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD} here".to_string();
        let garbled_score = extraction_quality(&garbled);
        assert!(
            garbled_score < clean_score,
            "garbled score {garbled_score} should be lower than clean score {clean_score}"
        );
        assert!(garbled_score < 0.8, "expected a visibly depressed score, got {garbled_score}");
    }

    // 7. Integration: parse_markdown(fixture) -> chunk::chunk_document(&doc,
    // &MockEmbedder, &Default) yields chunks with the expected
    // section_paths (proves the parse -> chunk seam works end to end).
    #[test]
    fn t7_integration_parse_markdown_then_chunk_document() {
        let source = "\
Root note before any heading.

# Chapter 1

## Vitamin K

Vitamin K is a fat-soluble vitamin involved in blood clotting.

```rust
let dose_mg = 5;
```

| Drug    | Class      |
|---------|------------|
| Warfarin | VKA       |
";
        let doc = parse_markdown(source, "Anticoagulation Notes");
        let embedder = MockEmbedder::new(8);
        let chunks = chunk_document(&doc, &embedder, &ChunkConfig::default());

        let section_paths: Vec<&str> = chunks.iter().map(|c| c.section_path.as_str()).collect();
        assert_eq!(
            section_paths,
            vec!["", "Chapter 1 > Vitamin K", "Chapter 1 > Vitamin K", "Chapter 1 > Vitamin K"]
        );

        assert!(chunks[0].text.contains("Root note"));
        assert!(chunks[1].text.contains("fat-soluble vitamin"));
        assert_eq!(chunks[2].text, "let dose_mg = 5;");
        assert!(chunks[3].text.starts_with("Drug | Class"));
        assert!(chunks[3].text.contains("Drug: Warfarin | Class: VKA"));
    }

    // 8. Regression: a loose list's continuation paragraph (blank line
    // inside an item) must be separated from the preceding text, not glued
    // word-to-word — and item boundaries stay separated too.
    #[test]
    fn t8_loose_list_continuation_paragraph_is_not_glued() {
        let source = "\
- item one

  continuation paragraph inside item one

- item two
";
        let doc = parse_markdown(source, "Doc");
        assert_eq!(doc.sections.len(), 1);
        assert_eq!(doc.sections[0].blocks.len(), 1);
        let text = match &doc.sections[0].blocks[0] {
            Block::Paragraph { text, .. } => text.as_str(),
            other => panic!("expected Block::Paragraph, got {other:?}"),
        };
        assert!(!text.contains("onecontinuation"), "words glued together: {text:?}");
        assert!(text.contains("one"), "missing distinct word \"one\": {text:?}");
        assert!(text.contains("continuation"), "missing distinct word \"continuation\": {text:?}");
        assert!(text.contains("- item one"));
        assert!(text.contains("- item two"));
        // Item boundary separation: "item one\n- item two", not glued.
        assert!(!text.contains("oneitem two"), "item boundary glued: {text:?}");
    }

    // 9. Regression: empty-input parity between parse_markdown and
    // parse_txt — a content-less input yields zero sections either way, but
    // a txt with real content still yields one root section.
    #[test]
    fn t9_empty_input_parity_between_md_and_txt() {
        assert!(parse_markdown("", "Doc").sections.is_empty());
        assert!(parse_txt("   \n\n", "Doc").sections.is_empty());

        let doc = parse_txt("Some real content.", "Doc");
        assert_eq!(doc.sections.len(), 1);
        assert_eq!(doc.sections[0].path, Vec::<String>::new());
    }
}
