//! Structure-aware sliding-window chunking (spec §1.3, steps 2–4), ported
//! from K.Y.T. Consumes the `tree` module's [`crate::tree::Document`] and
//! produces [`ChunkDraft`]s — everything a `format::Chunk` row needs except
//! the §2.3 contextual prefix (a build-time LLM step; see [`ChunkDraft::prefix`]).
//!
//! ## The three block kinds, three chunking strategies (spec §1.3 step 3)
//! - **Paragraph:** sentence-split, then slid through a token window
//!   (target 400 tokens, 18% overlap by default) that never splits a
//!   sentence. A single sentence longer than the target is its own chunk,
//!   flagged `oversize_sentence`.
//! - **Table:** serialized row-wise, one line per row, with the header
//!   repeated in every chunk; rows are grouped up to `target_tokens` and
//!   never split mid-row. Tables never share a chunk with paragraph text.
//! - **Code:** kept atomic — one code block is always exactly one chunk,
//!   even past `target_tokens`.
//!
//! Chunking never crosses a [`crate::tree::Block`] boundary (so tables
//! can't merge with paragraphs) or a [`crate::tree::Section`] boundary (so
//! chunks never straddle headings, spec §1.3 step 1) — each block is walked
//! and chunked independently, in document order.
//!
//! ## Determinism (K8's cross-build lock)
//! `chunk_document` has no randomness, no hashing-order dependence, and no
//! interior mutability — same `(doc, embedder, cfg)` in, byte-identical
//! `Vec<ChunkDraft>` out, every call. This is the property K8's cross-build
//! determinism test locks, run there with the REAL tokenizer (K4b), never
//! [`crate::embed::MockEmbedder`] — see that type's doc comment.

use crate::embed::Embedder;
use crate::tree::{Block, Document};

/// Sliding-window parameters (spec §1.3 step 2). `Default` is the spec's
/// own numbers: 400 target tokens, 18% overlap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkConfig {
    pub target_tokens: usize,
    pub overlap_pct: u32,
}

impl Default for ChunkConfig {
    fn default() -> Self {
        ChunkConfig {
            target_tokens: 400,
            overlap_pct: 18,
        }
    }
}

/// A draft chunk, produced by [`chunk_document`] and destined (after §2.3's
/// optional prefix pass) to become a `format::Chunk` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkDraft {
    /// The owning section's heading breadcrumb (`Section::section_path_string`),
    /// `""` for the root/no-heading case (D6) — never absent.
    pub section_path: String,
    /// The source position (line/paragraph/page ref) this chunk was drawn
    /// from, copied from the originating `Block`'s `locator`.
    pub locator: String,
    /// ALWAYS `""` here. The §2.3 contextual prefix is a build-time LLM
    /// step that runs later (curated: always; personal: only on an
    /// "enhanced" build) and is absent on quick builds — this chunker never
    /// has an LLM available, so it never populates this field. Do not
    /// confuse this with `section_path`: the heading breadcrumb belongs
    /// there, not here.
    pub prefix: String,
    /// The chunk body text (row-wise-serialized for tables, verbatim for
    /// code, sentence-windowed for paragraphs).
    pub text: String,
    /// `text`'s token count under the embedder's own tokenizer.
    pub token_count: usize,
    /// `true` only for a single paragraph sentence that alone exceeds
    /// `target_tokens` (spec §1.3 step 2: "a sentence longer than the
    /// window is its own chunk, flagged"). Always `false` for table and
    /// code chunks — this flag is specifically about the sentence-snapped
    /// paragraph window, not a generic "this chunk is oversize" marker.
    pub oversize_sentence: bool,
}

/// Chunk every section of `doc` independently (never across sections, spec
/// §1.3 step 1) using `embedder`'s tokenizer for token counts and `cfg`'s
/// window parameters. Within a section, each [`Block`] is walked in
/// document order and chunked by its own kind-specific rule (see the module
/// doc comment); blocks never merge into the same chunk.
pub fn chunk_document(doc: &Document, embedder: &dyn Embedder, cfg: &ChunkConfig) -> Vec<ChunkDraft> {
    let mut out = Vec::new();
    for section in &doc.sections {
        let section_path = section.section_path_string();
        for block in &section.blocks {
            match block {
                Block::Paragraph { text, locator } => {
                    chunk_paragraph(text, locator, &section_path, embedder, cfg, &mut out);
                }
                Block::Table { header, rows, locator } => {
                    chunk_table(header, rows, locator, &section_path, embedder, cfg, &mut out);
                }
                Block::Code { text, locator } => {
                    out.push(ChunkDraft {
                        section_path: section_path.clone(),
                        locator: locator.clone(),
                        prefix: String::new(),
                        text: text.clone(),
                        token_count: embedder.token_count(text),
                        oversize_sentence: false,
                    });
                }
            }
        }
    }
    out
}

/// Sentence-split `text`, then slide a token window over the sentence
/// stream (spec §1.3 step 2), emitting one [`ChunkDraft`] per window and
/// carrying an `overlap_pct`-sized trailing-sentence overlap into the next
/// window. A sentence alone over `target_tokens` becomes its own chunk,
/// `oversize_sentence = true`, and consumes no overlap budget.
fn chunk_paragraph(
    text: &str,
    locator: &str,
    section_path: &str,
    embedder: &dyn Embedder,
    cfg: &ChunkConfig,
    out: &mut Vec<ChunkDraft>,
) {
    let sentences = split_sentences(text);
    if sentences.is_empty() {
        return;
    }

    let mut i = 0usize;
    while i < sentences.len() {
        let start_tokens = embedder.token_count(&sentences[i]);

        // A single sentence longer than the target window is its own
        // chunk, flagged, and never split (spec §1.3 step 2).
        if start_tokens > cfg.target_tokens {
            out.push(ChunkDraft {
                section_path: section_path.to_string(),
                locator: locator.to_string(),
                prefix: String::new(),
                text: sentences[i].clone(),
                token_count: start_tokens,
                oversize_sentence: true,
            });
            i += 1;
            continue;
        }

        // Grow the window sentence-by-sentence while the whole window's
        // real token count (not a per-sentence sum — real tokenizers don't
        // sum linearly across a join) stays within target_tokens.
        let mut window_end = i;
        let mut window_text = sentences[i].clone();
        let mut window_tokens = start_tokens;
        while window_end + 1 < sentences.len() {
            let candidate_text = format!("{window_text} {}", sentences[window_end + 1]);
            let candidate_tokens = embedder.token_count(&candidate_text);
            if candidate_tokens > cfg.target_tokens {
                break;
            }
            window_text = candidate_text;
            window_tokens = candidate_tokens;
            window_end += 1;
        }

        out.push(ChunkDraft {
            section_path: section_path.to_string(),
            locator: locator.to_string(),
            prefix: String::new(),
            text: window_text,
            token_count: window_tokens,
            oversize_sentence: false,
        });

        if window_end + 1 >= sentences.len() {
            break;
        }

        // 18%-of-target overlap: carry back trailing sentences from the
        // window just emitted, working backward from its last sentence,
        // until their accumulated token count would exceed the overlap
        // budget. Always includes at least one sentence (the loop's first
        // iteration is unconditional), so adjacent chunks always share
        // ≥1 sentence whenever the window held more than one to begin
        // with — the case this chunker can vouch for without splitting a
        // sentence to hit the overlap budget exactly.
        let overlap_budget = (cfg.target_tokens as u64 * cfg.overlap_pct as u64 / 100) as usize;
        let mut overlap_start = window_end;
        let mut overlap_tokens = 0usize;
        let mut k = window_end;
        loop {
            let sentence_tokens = embedder.token_count(&sentences[k]);
            if overlap_tokens > 0 && overlap_tokens + sentence_tokens > overlap_budget {
                break;
            }
            overlap_tokens += sentence_tokens;
            overlap_start = k;
            if k == i {
                break;
            }
            k -= 1;
        }

        // Guarantee forward progress: if the overlap walk consumed the
        // entire just-emitted window (overlap_start == i, only possible
        // when that window was a single sentence, since a multi-sentence
        // window's loop always stops at k == window_end on its first
        // iteration when window_end > i), advance past it rather than
        // re-emitting the same window forever.
        i = overlap_start.max(i + 1);
    }
}

/// A minimal, dependency-free sentence splitter (spec §1.3: "sentence-
/// snapped"). Splits on `.`, `?`, `!` immediately followed by whitespace or
/// end-of-text. A small abbreviation guard suppresses splits after a
/// single-letter token (initials: "J. Smith") or a short list of common
/// abbreviations ("Dr.", "etc.", …) so those don't create spurious sentence
/// boundaries. Not a full NLP tokenizer — good enough for the "never split
/// mid-sentence" guarantee without a dependency.
fn split_sentences(text: &str) -> Vec<String> {
    const ABBREVIATIONS: &[&str] = &[
        "mr", "mrs", "ms", "dr", "prof", "sr", "jr", "st", "vs", "etc", "eg", "ie", "fig", "no", "vol", "approx",
    ];

    let chars: Vec<char> = text.chars().collect();
    let mut sentences = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;

    while i < chars.len() {
        let c = chars[i];
        let is_terminator = c == '.' || c == '?' || c == '!';
        if is_terminator {
            let next_is_boundary = i + 1 >= chars.len() || chars[i + 1].is_whitespace();
            if next_is_boundary {
                let word_before = word_ending_at(&chars, i);
                let is_abbreviation = c == '.'
                    && (word_before.chars().count() == 1
                        || ABBREVIATIONS.contains(&word_before.to_lowercase().as_str()));
                if !is_abbreviation {
                    let sentence: String = chars[start..=i].iter().collect();
                    push_trimmed(&mut sentences, &sentence);
                    let mut j = i + 1;
                    while j < chars.len() && chars[j].is_whitespace() {
                        j += 1;
                    }
                    start = j;
                    i = j;
                    continue;
                }
            }
        }
        i += 1;
    }

    if start < chars.len() {
        let remainder: String = chars[start..].iter().collect();
        push_trimmed(&mut sentences, &remainder);
    }

    sentences
}

/// Push `s` trimmed of surrounding whitespace, skipping it if that leaves
/// nothing (consecutive terminators, trailing whitespace runs).
fn push_trimmed(sentences: &mut Vec<String>, s: &str) {
    let trimmed = s.trim();
    if !trimmed.is_empty() {
        sentences.push(trimmed.to_string());
    }
}

/// The run of alphanumeric characters immediately preceding index `end`
/// (the terminator's own position) in `chars` — i.e. the word the
/// terminator is attached to, used by [`split_sentences`]'s abbreviation
/// guard.
fn word_ending_at(chars: &[char], end: usize) -> String {
    let mut j = end;
    let mut word: Vec<char> = Vec::new();
    while j > 0 && chars[j - 1].is_alphanumeric() {
        word.push(chars[j - 1]);
        j -= 1;
    }
    word.reverse();
    word.into_iter().collect()
}

/// Serialize a table row-wise (spec §1.3 step 3): a `header_line` (the
/// column names joined by `" | "`) plus one line per row, each row
/// rendered as `header1: v1 | header2: v2 …`. Rows are grouped into chunks
/// up to `target_tokens`, with `header_line` repeated at the top of every
/// chunk; a single row (with header) that alone exceeds `target_tokens`
/// still becomes its own chunk rather than being split.
fn chunk_table(
    header: &[String],
    rows: &[Vec<String>],
    locator: &str,
    section_path: &str,
    embedder: &dyn Embedder,
    cfg: &ChunkConfig,
    out: &mut Vec<ChunkDraft>,
) {
    if rows.is_empty() {
        return;
    }
    let header_line = header.join(" | ");
    let mut current_rows: Vec<String> = Vec::new();

    for row in rows {
        let row_line = render_table_row(header, row);
        let mut candidate_rows = current_rows.clone();
        candidate_rows.push(row_line.clone());
        let candidate_text = format!("{header_line}\n{}", candidate_rows.join("\n"));
        let candidate_tokens = embedder.token_count(&candidate_text);

        if candidate_tokens > cfg.target_tokens && !current_rows.is_empty() {
            push_table_chunk(&header_line, &current_rows, locator, section_path, embedder, out);
            current_rows = vec![row_line];
        } else {
            current_rows.push(row_line);
        }
    }
    if !current_rows.is_empty() {
        push_table_chunk(&header_line, &current_rows, locator, section_path, embedder, out);
    }
}

/// Render one table row as `header1: v1 | header2: v2 …` (spec §1.3 step
/// 3). Zips `header` with `row`; a row shorter than `header` (a ragged
/// table) simply renders fewer pairs rather than panicking.
fn render_table_row(header: &[String], row: &[String]) -> String {
    header
        .iter()
        .zip(row.iter())
        .map(|(h, v)| format!("{h}: {v}"))
        .collect::<Vec<_>>()
        .join(" | ")
}

fn push_table_chunk(
    header_line: &str,
    rows: &[String],
    locator: &str,
    section_path: &str,
    embedder: &dyn Embedder,
    out: &mut Vec<ChunkDraft>,
) {
    let text = format!("{header_line}\n{}", rows.join("\n"));
    let token_count = embedder.token_count(&text);
    out.push(ChunkDraft {
        section_path: section_path.to_string(),
        locator: locator.to_string(),
        prefix: String::new(),
        text,
        token_count,
        oversize_sentence: false,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::MockEmbedder;
    use crate::tree::{Document, Section};

    fn doc_with_section(path: Vec<&str>, blocks: Vec<Block>) -> Document {
        Document {
            title: "Test Doc".to_string(),
            sections: vec![Section {
                path: path.into_iter().map(str::to_string).collect(),
                blocks,
            }],
        }
    }

    fn words(n: usize, prefix: &str) -> String {
        (0..n).map(|i| format!("{prefix}{i}")).collect::<Vec<_>>().join(" ")
    }

    // 1. A single-paragraph section under target_tokens → exactly one
    // chunk, correct section_path/locator, prefix=="", oversize_sentence
    // ==false.
    #[test]
    fn t1_single_short_paragraph_is_one_chunk() {
        let embedder = MockEmbedder::new(8);
        let cfg = ChunkConfig::default();
        let doc = doc_with_section(
            vec!["Ch 1", "Intro"],
            vec![Block::Paragraph {
                text: "The quick brown fox jumps over the lazy dog.".to_string(),
                locator: "p1".to_string(),
            }],
        );

        let chunks = chunk_document(&doc, &embedder, &cfg);
        assert_eq!(chunks.len(), 1);
        let c = &chunks[0];
        assert_eq!(c.section_path, "Ch 1 > Intro");
        assert_eq!(c.locator, "p1");
        assert_eq!(c.prefix, "");
        assert!(!c.oversize_sentence);
        assert_eq!(c.text, "The quick brown fox jumps over the lazy dog.");
        assert_eq!(c.token_count, embedder.token_count(&c.text));
    }

    // 2. A long paragraph spanning multiple windows → multiple chunks,
    // each <= target (allowing sentence-snap slack), adjacent chunks
    // overlap (share >=1 sentence), no sentence split mid-way.
    #[test]
    fn t2_long_paragraph_multi_window_with_overlap_no_mid_sentence_split() {
        let embedder = MockEmbedder::new(8);
        // Each sentence is ~6 tokens ("SentenceN word0 word1 word2 word3 word4.");
        // target 20 tokens => ~3 sentences/window, comfortably multi-sentence.
        let sentence_count = 12;
        let sentences: Vec<String> = (0..sentence_count)
            .map(|n| format!("Sentence{n} {}.", words(4, "w")))
            .collect();
        let text = sentences.join(" ");
        let cfg = ChunkConfig {
            target_tokens: 20,
            overlap_pct: 18,
        };
        let doc = doc_with_section(
            vec![],
            vec![Block::Paragraph {
                text: text.clone(),
                locator: "p1".to_string(),
            }],
        );

        let chunks = chunk_document(&doc, &embedder, &cfg);
        assert!(chunks.len() > 1, "expected multiple windows, got {}", chunks.len());

        for c in &chunks {
            assert!(
                c.token_count <= cfg.target_tokens,
                "chunk exceeded target_tokens: {} > {}",
                c.token_count,
                cfg.target_tokens
            );
            assert!(!c.oversize_sentence);
        }

        // Adjacent chunks share at least one sentence (the raw "SentenceN ..."
        // sentence tokens re-appear verbatim, since we never split mid-sentence).
        for pair in chunks.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            let a_sentences: Vec<&str> = a.text.split(". ").collect();
            let overlap = a_sentences
                .iter()
                .any(|s| !s.is_empty() && b.text.contains(s.trim_end_matches('.')));
            assert!(overlap, "expected adjacent chunks to share a sentence:\nA: {}\nB: {}", a.text, b.text);
        }

        // No sentence text was ever split mid-way: every original sentence
        // ("SentenceN ... .") appears whole (as a substring) in at least one
        // chunk.
        for s in &sentences {
            let whole = chunks.iter().any(|c| c.text.contains(s.as_str()));
            assert!(whole, "sentence was split mid-way, not found whole in any chunk: {s}");
        }
    }

    // 3. A single sentence longer than target_tokens -> its own chunk with
    // oversize_sentence==true.
    #[test]
    fn t3_oversize_single_sentence_flagged() {
        let embedder = MockEmbedder::new(8);
        let cfg = ChunkConfig {
            target_tokens: 5,
            overlap_pct: 18,
        };
        let long_sentence = format!("{}.", words(20, "w"));
        let doc = doc_with_section(
            vec!["Ch 1"],
            vec![Block::Paragraph {
                text: long_sentence.clone(),
                locator: "p1".to_string(),
            }],
        );

        let chunks = chunk_document(&doc, &embedder, &cfg);
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].oversize_sentence);
        assert_eq!(chunks[0].text, long_sentence);
        assert!(chunks[0].token_count > cfg.target_tokens);
    }

    // 4. Two sections -> chunks never straddle sections (each chunk's
    // section_path is one section's).
    #[test]
    fn t4_chunks_never_straddle_sections() {
        let embedder = MockEmbedder::new(8);
        let cfg = ChunkConfig::default();
        let doc = Document {
            title: "Test Doc".to_string(),
            sections: vec![
                Section {
                    path: vec!["Ch 1".to_string()],
                    blocks: vec![Block::Paragraph {
                        text: "First section text here.".to_string(),
                        locator: "p1".to_string(),
                    }],
                },
                Section {
                    path: vec!["Ch 2".to_string()],
                    blocks: vec![Block::Paragraph {
                        text: "Second section text here.".to_string(),
                        locator: "p2".to_string(),
                    }],
                },
            ],
        };

        let chunks = chunk_document(&doc, &embedder, &cfg);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].section_path, "Ch 1");
        assert_eq!(chunks[1].section_path, "Ch 2");
        assert!(chunks[0].text.contains("First"));
        assert!(chunks[1].text.contains("Second"));
    }

    // 5. Table block -> row-wise chunks with the header repeated in each;
    // not merged with adjacent paragraph text.
    #[test]
    fn t5_table_row_wise_with_repeated_header_not_merged_with_paragraph() {
        let embedder = MockEmbedder::new(8);
        let cfg = ChunkConfig {
            target_tokens: 12,
            overlap_pct: 18,
        };
        let header = vec!["Name".to_string(), "Age".to_string()];
        let rows = vec![
            vec!["Alice".to_string(), "30".to_string()],
            vec!["Bob".to_string(), "40".to_string()],
            vec!["Carol".to_string(), "50".to_string()],
        ];
        let doc = doc_with_section(
            vec!["Data"],
            vec![
                Block::Paragraph {
                    text: "Some preceding prose.".to_string(),
                    locator: "p1".to_string(),
                },
                Block::Table {
                    header: header.clone(),
                    rows: rows.clone(),
                    locator: "t1".to_string(),
                },
            ],
        );

        let chunks = chunk_document(&doc, &embedder, &cfg);
        // First chunk is the paragraph; the rest are table chunks — never
        // mixed together.
        assert_eq!(chunks[0].text, "Some preceding prose.");
        let table_chunks = &chunks[1..];
        assert!(!table_chunks.is_empty());
        for c in table_chunks {
            assert!(c.text.starts_with("Name | Age"), "header not repeated: {}", c.text);
            assert!(!c.text.contains("preceding prose"));
            assert_eq!(c.locator, "t1");
        }
        // Every row appears, rendered as "header: value" pairs, across the
        // table chunks.
        let joined: String = table_chunks.iter().map(|c| c.text.as_str()).collect::<Vec<_>>().join("\n");
        assert!(joined.contains("Name: Alice | Age: 30"));
        assert!(joined.contains("Name: Bob | Age: 40"));
        assert!(joined.contains("Name: Carol | Age: 50"));
    }

    // 6. Code block -> atomic (one chunk, not split), even if large.
    #[test]
    fn t6_code_block_is_atomic_even_if_large() {
        let embedder = MockEmbedder::new(8);
        let cfg = ChunkConfig {
            target_tokens: 5,
            overlap_pct: 18,
        };
        let code = "fn main() {\n    println!(\"hello world, this is a fairly long code block\");\n}".to_string();
        let doc = doc_with_section(
            vec!["Appendix"],
            vec![Block::Code {
                text: code.clone(),
                locator: "c1".to_string(),
            }],
        );

        let chunks = chunk_document(&doc, &embedder, &cfg);
        assert_eq!(chunks.len(), 1, "code block must never be split");
        assert_eq!(chunks[0].text, code);
        assert!(chunks[0].token_count > cfg.target_tokens);
    }

    // 7. Root/no-heading section -> section_path == "" (D6), never a
    // panic.
    #[test]
    fn t7_root_section_empty_section_path_no_panic() {
        let embedder = MockEmbedder::new(8);
        let cfg = ChunkConfig::default();
        let doc = doc_with_section(
            vec![],
            vec![Block::Paragraph {
                text: "Root level text with no heading ancestry at all.".to_string(),
                locator: "p1".to_string(),
            }],
        );

        let chunks = chunk_document(&doc, &embedder, &cfg);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].section_path, "");
    }

    // 8. Determinism: chunk_document called twice on the same inputs ->
    // identical Vec<ChunkDraft>.
    #[test]
    fn t8_chunk_document_is_deterministic() {
        let embedder = MockEmbedder::new(8);
        let cfg = ChunkConfig::default();
        let sentences: Vec<String> = (0..10).map(|n| format!("Sentence number {n} has some words in it.")).collect();
        let doc = Document {
            title: "Determinism Doc".to_string(),
            sections: vec![
                Section {
                    path: vec!["Ch 1".to_string()],
                    blocks: vec![
                        Block::Paragraph {
                            text: sentences.join(" "),
                            locator: "p1".to_string(),
                        },
                        Block::Table {
                            header: vec!["A".to_string(), "B".to_string()],
                            rows: vec![vec!["1".to_string(), "2".to_string()], vec!["3".to_string(), "4".to_string()]],
                            locator: "t1".to_string(),
                        },
                        Block::Code {
                            text: "let x = 1;".to_string(),
                            locator: "c1".to_string(),
                        },
                    ],
                },
                Section {
                    path: vec![],
                    blocks: vec![Block::Paragraph {
                        text: "A root-level closing paragraph.".to_string(),
                        locator: "p2".to_string(),
                    }],
                },
            ],
        };

        let first = chunk_document(&doc, &embedder, &cfg);
        let second = chunk_document(&doc, &embedder, &cfg);
        assert_eq!(first, second);
        assert!(!first.is_empty());
    }
}
