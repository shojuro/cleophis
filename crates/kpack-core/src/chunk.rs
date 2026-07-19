//! Structure-aware sliding-window chunking (spec §1.3, steps 2–4), ported
//! from K.Y.T. Consumes the `tree` module's [`crate::tree::Document`] and
//! produces [`ChunkDraft`]s — everything a `format::Chunk` row needs except
//! the §2.3 contextual prefix (a build-time LLM step; see [`ChunkDraft::prefix`]).
//!
//! ## The three block kinds, three chunking strategies (spec §1.3 step 3)
//! - **Paragraph:** sentence-split, then slid through a token window
//!   (target 400 tokens, 18% overlap by default) that never splits a
//!   sentence. A single sentence longer than the target is its own chunk,
//!   flagged `oversize_sentence` — UNLESS it's also longer than the
//!   embedder's `max_input_tokens()` ceiling (a real embedder hard-errors
//!   or truncates past that), in which case it's split deterministically
//!   into pieces that each stay within the ceiling (see
//!   [`split_to_ceiling`]) — PDF-extracted text in particular can contain
//!   long runs with no sentence breaks at all.
//! - **Table:** serialized row-wise, one line per row, with the header
//!   repeated in every chunk; rows are grouped up to `target_tokens` and
//!   never split mid-row. Tables never share a chunk with paragraph text.
//!   A row group (header included) that alone exceeds the embedder's
//!   `max_input_tokens()` ceiling is split via [`split_to_ceiling`], same
//!   as the paragraph path.
//! - **Code:** kept atomic — one code block is always exactly one chunk,
//!   even past `target_tokens` — UNLESS it exceeds the embedder's
//!   `max_input_tokens()` ceiling, in which case it too is split via
//!   [`split_to_ceiling`] rather than handed to the embedder whole.
//!
//! Chunking never crosses a [`crate::tree::Block`] boundary (so tables
//! can't merge with paragraphs) or a [`crate::tree::Section`] boundary (so
//! chunks never straddle headings, spec §1.3 step 1) — each block is walked
//! and chunked independently, in document order.
//!
//! ## The ceiling is enforced pack-wide (task B-chunkcap)
//! No `ChunkDraft` this module emits — paragraph, table, or code — can ever
//! have `token_count > embedder.max_input_tokens()`. `BgeEmbedder`'s
//! `embed_raw` truncation (kpack-embed) is a pure backstop for anything
//! that somehow slips past this, not a path this chunker relies on to stay
//! within bounds.
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
                    // Atomic whenever it fits — split only past the
                    // embedder's ceiling (module doc: "the ceiling is
                    // enforced pack-wide").
                    let ceiling = embedder.max_input_tokens();
                    push_chunk_within_ceiling(&section_path, locator, text.clone(), embedder, ceiling, &mut out);
                }
            }
        }
    }
    out
}

/// Push `text` as a single [`ChunkDraft`] if it fits within `ceiling`;
/// otherwise split it via [`split_to_ceiling`] and push one `ChunkDraft`
/// per piece. Shared by the code-block path (above) and the table path
/// ([`push_table_chunk`]) — `chunk_paragraph`'s oversize-sentence path has
/// its own call site since it also needs to set `oversize_sentence: true`
/// and distinguish `effective_target` from `ceiling`. Every pushed draft
/// gets `oversize_sentence: false` (per [`ChunkDraft::oversize_sentence`]'s
/// doc comment, that flag is specifically about the paragraph
/// sentence-snap, not a generic "this chunk was split" marker).
fn push_chunk_within_ceiling(
    section_path: &str,
    locator: &str,
    text: String,
    embedder: &dyn Embedder,
    ceiling: usize,
    out: &mut Vec<ChunkDraft>,
) {
    let token_count = embedder.token_count(&text);
    if token_count <= ceiling {
        out.push(ChunkDraft {
            section_path: section_path.to_string(),
            locator: locator.to_string(),
            prefix: String::new(),
            text,
            token_count,
            oversize_sentence: false,
        });
        return;
    }

    for (piece_text, piece_tokens) in split_to_ceiling(&text, embedder, ceiling) {
        debug_assert!(
            piece_tokens <= ceiling,
            "split_to_ceiling produced a piece over the ceiling: {piece_tokens} > {ceiling}"
        );
        out.push(ChunkDraft {
            section_path: section_path.to_string(),
            locator: locator.to_string(),
            prefix: String::new(),
            text: piece_text,
            token_count: piece_tokens,
            oversize_sentence: false,
        });
    }
}

/// Sentence-split `text`, then slide a token window over the sentence
/// stream (spec §1.3 step 2), emitting one [`ChunkDraft`] per window and
/// carrying an `overlap_pct`-sized trailing-sentence overlap into the next
/// window. A sentence alone over `target_tokens` becomes its own chunk,
/// `oversize_sentence = true`, and consumes no overlap budget — UNLESS it's
/// also over `embedder.max_input_tokens()` (the embedder's hard ceiling),
/// in which case it's deterministically split into multiple
/// `oversize_sentence = true` pieces that each stay within the ceiling (see
/// [`split_to_ceiling`]), so no chunk this function emits can ever exceed
/// what the embedder can actually accept.
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

    let ceiling = embedder.max_input_tokens();
    // Belt-and-suspenders: today target_tokens (400) < ceiling (512), so
    // this is a no-op — but if target_tokens were ever configured above the
    // embedder's ceiling, clamp the window's effective growth cap so no
    // window this loop grows can exceed what the embedder accepts.
    let effective_target = cfg.target_tokens.min(ceiling);

    let mut i = 0usize;
    while i < sentences.len() {
        let start_tokens = embedder.token_count(&sentences[i]);

        // A single sentence longer than the target window is its own
        // chunk, flagged (spec §1.3 step 2) — split only if it also
        // breaches the embedder's hard ceiling; otherwise emitted whole, as
        // before.
        if start_tokens > effective_target {
            if start_tokens <= ceiling {
                out.push(ChunkDraft {
                    section_path: section_path.to_string(),
                    locator: locator.to_string(),
                    prefix: String::new(),
                    text: sentences[i].clone(),
                    token_count: start_tokens,
                    oversize_sentence: true,
                });
            } else {
                for (piece_text, piece_tokens) in split_to_ceiling(&sentences[i], embedder, ceiling) {
                    debug_assert!(
                        piece_tokens <= ceiling,
                        "split_to_ceiling produced a piece over the ceiling: {piece_tokens} > {ceiling}"
                    );
                    out.push(ChunkDraft {
                        section_path: section_path.to_string(),
                        locator: locator.to_string(),
                        prefix: String::new(),
                        text: piece_text,
                        token_count: piece_tokens,
                        oversize_sentence: true,
                    });
                }
            }
            i += 1;
            continue;
        }

        // Grow the window sentence-by-sentence while the whole window's
        // real token count (not a per-sentence sum — real tokenizers don't
        // sum linearly across a join) stays within effective_target.
        //
        // TODO(K8-perf): this rebuilds and re-tokenizes the whole growing
        // window text on every sentence (O(n²) over a section's sentence
        // count), and the overlap walk below now does the same thing over
        // its (smaller) trailing window. Correctness-first for K5; revisit
        // only if K8's real-corpus determinism run turns out slow.
        let mut window_end = i;
        let mut window_text = sentences[i].clone();
        let mut window_tokens = start_tokens;
        while window_end + 1 < sentences.len() {
            let candidate_text = format!("{window_text} {}", sentences[window_end + 1]);
            let candidate_tokens = embedder.token_count(&candidate_text);
            if candidate_tokens > effective_target {
                break;
            }
            window_text = candidate_text;
            window_tokens = candidate_tokens;
            window_end += 1;
        }

        debug_assert!(
            window_tokens <= ceiling,
            "chunk window exceeded the embedder's ceiling: {window_tokens} > {ceiling}"
        );
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

        // overlap_pct% of THIS WINDOW's own tokens (not the target) —
        // carry back trailing sentences from the window just emitted,
        // working backward from its last sentence, until including one
        // more would push the carried text's real (joined) token count
        // over that budget. Always includes at least one sentence (the
        // walk's first step is unconditional), so adjacent chunks always
        // share ≥1 sentence whenever the window held more than one to
        // begin with — the case this chunker can vouch for without
        // splitting a sentence to hit the overlap budget exactly.
        //
        // Bounding the budget to the window's OWN size (rather than a
        // fixed fraction of target_tokens) is what keeps overlap bounded
        // for small windows: a window well under target_tokens used to be
        // able to fit its ENTIRE content inside the fixed target-sized
        // budget, so the whole window got carried into the next one
        // (~85-90% overlap between successive chunks, bloating the pack).
        // Scaling the budget to the window itself means carrying the whole
        // window would require the budget to cover 100% of it, which
        // overlap_pct% of it (< 100%) never does once the window has more
        // than one sentence — see the forward-progress guard below.
        let overlap_budget = (window_tokens as u64 * cfg.overlap_pct as u64 / 100) as usize;
        let mut overlap_start = window_end;
        // Real joined-text token count (not a per-sentence sum), for
        // consistency with the window-growth loop above — real tokenizers
        // don't sum linearly across a join.
        let mut overlap_text = sentences[window_end].clone();
        let mut k = window_end;
        while k != i {
            let candidate_text = format!("{} {overlap_text}", sentences[k - 1]);
            if embedder.token_count(&candidate_text) > overlap_budget {
                break;
            }
            overlap_text = candidate_text;
            overlap_start = k - 1;
            k -= 1;
        }

        // Guarantee forward progress: if the overlap walk consumed the
        // entire just-emitted window (overlap_start == i), advance past it
        // rather than re-emitting the same window forever. With the
        // window-relative budget above, this is now expected only when the
        // window was a single sentence (window_end == i): for a window of
        // 2+ sentences, the unconditionally-included last sentence already
        // spends part of the budget, leaving less than overlap_pct% of the
        // window for the rest — never enough to also cover every sentence
        // back to i, since together they make up the window's *entire*
        // content (100% > overlap_pct% whenever overlap_pct < 100). Kept
        // as a defensive guard, not relied on elsewhere as an invariant.
        i = overlap_start.max(i + 1);
    }
}

/// Deterministically split `text` into consecutive pieces, each with
/// `embedder.token_count(piece) <= ceiling`, covering the whole text with
/// no words lost and no UTF-8 code point ever split. Used by
/// [`chunk_paragraph`] when a single sentence exceeds the embedder's hard
/// ceiling (common in PDF-extracted text: dense passages with no sentence
/// breaks at all).
///
/// Greedy left-to-right word accumulation: `text` is split on whitespace;
/// words are joined by a single space and accumulated into the current
/// piece while the growing piece's real token count stays within `ceiling`.
/// When the next word would push it over, the accumulated piece is emitted
/// and a new one starts with that word. A single word whose OWN token count
/// already exceeds `ceiling` (pathological — e.g. a long token-less blob
/// with no whitespace) is hard-split at char boundaries by
/// [`split_word_to_ceiling`] instead.
///
/// Pure and deterministic in `text` + `ceiling` alone (no randomness, no
/// `HashMap`/hashing-order dependence) — required for the pack's
/// byte-identical-rebuild guarantee (spec's K8 determinism invariant).
fn split_to_ceiling(text: &str, embedder: &dyn Embedder, ceiling: usize) -> Vec<(String, usize)> {
    let mut pieces = Vec::new();
    let mut current = String::new();

    for word in text.split_whitespace() {
        let candidate = if current.is_empty() {
            word.to_string()
        } else {
            format!("{current} {word}")
        };
        if embedder.token_count(&candidate) <= ceiling {
            current = candidate;
            continue;
        }

        // `word` doesn't fit onto the current piece — flush what's
        // accumulated so far (if anything).
        if !current.is_empty() {
            let tokens = embedder.token_count(&current);
            pieces.push((std::mem::take(&mut current), tokens));
        }

        if embedder.token_count(word) <= ceiling {
            current = word.to_string();
        } else {
            // Pathological: even this single word alone breaches the
            // ceiling. Hard-split it at char boundaries and leave `current`
            // empty for the next word to start fresh.
            pieces.extend(split_word_to_ceiling(word, embedder, ceiling));
        }
    }

    if !current.is_empty() {
        let tokens = embedder.token_count(&current);
        pieces.push((current, tokens));
    }

    pieces
}

/// Hard-split a single pathological "word" (no internal whitespace) whose
/// own `token_count` exceeds `ceiling` into consecutive char-boundary
/// pieces, each with `token_count <= ceiling`. Grows each piece one char at
/// a time from its own start (never rescanning from the beginning of
/// `word`), so total work across all pieces is linear in `word`'s length.
/// Every piece boundary lands on a `char_indices` boundary, so a UTF-8 code
/// point is never split. If even a single char's `token_count` exceeds
/// `ceiling`, that char is still emitted alone (the minimum splittable
/// unit) rather than looping forever — `ceiling` simply can't be honored
/// below one code point in that case.
fn split_word_to_ceiling(word: &str, embedder: &dyn Embedder, ceiling: usize) -> Vec<(String, usize)> {
    let mut boundaries: Vec<usize> = word.char_indices().map(|(i, _)| i).collect();
    boundaries.push(word.len());

    let mut pieces = Vec::new();
    let mut start_b = 0usize; // index into `boundaries` of the piece's start char

    while start_b + 1 < boundaries.len() {
        let mut end_b = start_b + 1; // always include at least one char
        loop {
            let next_b = end_b + 1;
            if next_b >= boundaries.len() {
                break;
            }
            if embedder.token_count(&word[boundaries[start_b]..boundaries[next_b]]) > ceiling {
                break;
            }
            end_b = next_b;
        }
        let piece = &word[boundaries[start_b]..boundaries[end_b]];
        pieces.push((piece.to_string(), embedder.token_count(piece)));
        start_b = end_b;
    }

    pieces
}

/// A minimal, dependency-free sentence splitter (spec §1.3: "sentence-
/// snapped"). Splits on `.`, `?`, `!` immediately followed by whitespace or
/// end-of-text.
///
/// The abbreviation guard is deliberately narrow and biased toward
/// splitting: it suppresses a boundary ONLY for (a) a curated,
/// case-insensitive list of common abbreviations ("Dr.", "etc.", …), or (b)
/// a multi-dot initialism chain like "U.S." or "e.g." (see
/// [`is_multi_dot_initialism`]). It does NOT treat every single
/// alphanumeric character before a period as an abbreviation — that
/// earlier rule silently merged real sentence boundaries, e.g. "...see
/// Appendix A. This concludes the chapter." or "The dose was reduced to 5.
/// Follow up in a week." both wrongly became one sentence.
///
/// Known limitation: a genuine name initial not in the curated list, like
/// "J. Smith", is NOT suppressed and over-splits into a small one-token
/// fragment ("J."). This is intentional and considered acceptable: the
/// sliding window in [`chunk_paragraph`] re-absorbs such fragments into a
/// neighboring window, so it's harmless, whereas under-splitting produces
/// a genuinely wrong sentence boundary (and can spuriously trip
/// `oversize_sentence`). Deterministic — no locale or regex dependence.
///
/// Not a full NLP tokenizer — good enough for the "never split
/// mid-sentence" guarantee without a dependency.
fn split_sentences(text: &str) -> Vec<String> {
    const ABBREVIATIONS: &[&str] = &[
        "mr", "mrs", "ms", "dr", "prof", "fig", "figs", "eq", "no", "vol", "ch", "sec", "pp", "al", "vs", "cf",
        "eg", "ie", "etc", "st", "jr", "sr",
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
                    && (ABBREVIATIONS.contains(&word_before.to_lowercase().as_str())
                        || is_multi_dot_initialism(&chars, i));
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

/// True when the `.` at `chars[end]` closes a multi-dot initialism chain
/// like `U.S.` or `e.g.` — two or more single alphanumeric characters, each
/// immediately preceded by a `.`, with the chain not itself preceded by
/// another alphanumeric character. That last condition is what keeps this
/// from matching a lone digit (`5.` after a space) or a bare single-letter
/// word (`A.` after a space): only an actual `X.Y.` chain qualifies. Used
/// alongside the curated word list in [`split_sentences`]'s abbreviation
/// guard.
fn is_multi_dot_initialism(chars: &[char], end: usize) -> bool {
    let mut pos = end;
    let mut segments = 0usize;
    loop {
        if pos == 0 || !chars[pos - 1].is_alphanumeric() {
            return segments >= 2;
        }
        // The character before this run must not itself be alphanumeric —
        // otherwise it's a multi-char word (e.g. the "5" in "25."), not a
        // lone initial.
        if pos >= 2 && chars[pos - 2].is_alphanumeric() {
            return segments >= 2;
        }
        segments += 1;
        let letter_pos = pos - 1;
        if letter_pos == 0 || chars[letter_pos - 1] != '.' {
            return segments >= 2;
        }
        pos = letter_pos - 1;
    }
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
/// still becomes its own chunk rather than being split by row-grouping —
/// but [`push_table_chunk`] still splits that chunk's TEXT via
/// [`split_to_ceiling`] if it's over the embedder's `max_input_tokens()`
/// ceiling, same as the paragraph path.
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
    let ceiling = embedder.max_input_tokens();
    let header_line = header.join(" | ");
    let mut current_rows: Vec<String> = Vec::new();

    for row in rows {
        let row_line = render_table_row(header, row);
        let mut candidate_rows = current_rows.clone();
        candidate_rows.push(row_line.clone());
        let candidate_text = format!("{header_line}\n{}", candidate_rows.join("\n"));
        let candidate_tokens = embedder.token_count(&candidate_text);

        if candidate_tokens > cfg.target_tokens && !current_rows.is_empty() {
            push_table_chunk(&header_line, &current_rows, locator, section_path, embedder, ceiling, out);
            current_rows = vec![row_line];
        } else {
            current_rows.push(row_line);
        }
    }
    if !current_rows.is_empty() {
        push_table_chunk(&header_line, &current_rows, locator, section_path, embedder, ceiling, out);
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
    ceiling: usize,
    out: &mut Vec<ChunkDraft>,
) {
    let text = format!("{header_line}\n{}", rows.join("\n"));
    // Row-grouping above only bounds against cfg.target_tokens; a single
    // very wide row (with header) can still land here over the embedder's
    // hard ceiling, so split it the same way the paragraph/code paths do.
    push_chunk_within_ceiling(section_path, locator, text, embedder, ceiling, out);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::{EmbedError, MockEmbedder};
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

    // 9-13. split_sentences abbreviation-guard regressions (review Fix 1):
    // the old "any single char before a period is an abbreviation" rule
    // wrongly merged real sentence boundaries. The fix removes that rule
    // and instead only suppresses a curated word list or an actual
    // multi-dot initialism chain.

    // 9. "...see Appendix A. This concludes the chapter." must split into
    // TWO sentences (previously merged into one via the single-char rule).
    #[test]
    fn t9_split_sentences_breaks_after_capital_letter_reference() {
        let sentences = split_sentences("...see Appendix A. This concludes the chapter.");
        assert_eq!(sentences, vec!["...see Appendix A.", "This concludes the chapter."]);
    }

    // 10. "The dose was reduced to 5. Follow up in a week." must split into
    // TWO sentences (previously merged via the single-digit rule).
    #[test]
    fn t10_split_sentences_breaks_after_lone_digit() {
        let sentences = split_sentences("The dose was reduced to 5. Follow up in a week.");
        assert_eq!(sentences, vec!["The dose was reduced to 5.", "Follow up in a week."]);
    }

    // 11. "Dr. Smith arrived." stays ONE sentence — curated abbreviation
    // list still suppresses the boundary.
    #[test]
    fn t11_split_sentences_keeps_curated_abbreviation_together() {
        let sentences = split_sentences("Dr. Smith arrived.");
        assert_eq!(sentences, vec!["Dr. Smith arrived."]);
    }

    // 12. "U.S. troops." stays ONE sentence — multi-dot initialism chain
    // detection suppresses the boundary even though neither "U" nor "S"
    // alone is in the curated word list.
    #[test]
    fn t12_split_sentences_keeps_multi_dot_initialism_together() {
        let sentences = split_sentences("U.S. troops.");
        assert_eq!(sentences, vec!["U.S. troops."]);
    }

    // 13. Documented limitation: a genuine name initial not in the curated
    // list ("J. Smith") is NOT suppressed and over-splits into a tiny
    // fragment. Acceptable per split_sentences's doc comment — harmless,
    // self-healing via the sliding window — unlike under-splitting, which
    // would be a wrong boundary.
    #[test]
    fn t13_split_sentences_documented_limitation_over_splits_true_initial() {
        let sentences = split_sentences("J. Smith wrote the report.");
        assert_eq!(sentences, vec!["J.", "Smith wrote the report."]);
    }

    // 14. Overlap-runaway regression (review Fix 2): a window made of many
    // small sentences (each well under the fixed target-based overlap
    // budget) followed by a large sentence used to have its ENTIRE small
    // window carried into the next chunk as "overlap" (~85-90% overlap,
    // not the configured 18%), because the old budget was a fixed fraction
    // of target_tokens rather than of the window's own size. Assert the
    // fix: overlap stays near overlap_pct% of the window's own tokens, and
    // the next chunk is not a near-duplicate of its predecessor.
    #[test]
    fn t14_overlap_bounded_to_window_fraction_not_runaway() {
        let embedder = MockEmbedder::new(8);
        let cfg = ChunkConfig {
            target_tokens: 100,
            overlap_pct: 18,
        };

        // 8 small sentences, 5 tokens each ("SentenceN w0 w1 w2 w3.") = 40
        // tokens total — well under target_tokens, so they all fit in one
        // window. A trailing large sentence (90 tokens) doesn't fit
        // alongside them (40 + 90 > 100), so it forces a second window.
        let small_sentences: Vec<String> = (0..8).map(|n| format!("Sentence{n} {}.", words(4, "w"))).collect();
        let large_sentence = format!("Big {}.", words(89, "L"));
        let mut all_sentences = small_sentences.clone();
        all_sentences.push(large_sentence.clone());
        let text = all_sentences.join(" ");

        let doc = doc_with_section(
            vec![],
            vec![Block::Paragraph {
                text,
                locator: "p1".to_string(),
            }],
        );

        let chunks = chunk_document(&doc, &embedder, &cfg);
        assert_eq!(chunks.len(), 2, "expected exactly two windows, got {}", chunks.len());
        let (c0, c1) = (&chunks[0], &chunks[1]);

        // Chunk 0 is exactly the 8 small sentences; chunk 1 is the last of
        // those 8 (the legitimate one-sentence overlap) plus the large one.
        assert_eq!(c0.text, small_sentences.join(" "));
        assert_eq!(c1.text, format!("{} {}", small_sentences[7], large_sentence));
        assert_eq!(c0.token_count, 40);

        // Overlap measured in tokens: only the last small sentence (5
        // tokens) is shared, ~12.5% of chunk 0 — close to the configured
        // 18%, nowhere near the ~85-90% the bug produced (which would have
        // been all 40 tokens, i.e. the entire chunk 0 repeated).
        let shared_tokens = embedder.token_count(&small_sentences[7]);
        let overlap_fraction = shared_tokens as f64 / c0.token_count as f64;
        assert!(
            overlap_fraction <= 0.20,
            "overlap fraction {overlap_fraction} exceeds the ~overlap_pct% budget"
        );
        assert!(
            overlap_fraction < 0.85,
            "overlap runaway reproduced: fraction {overlap_fraction} approaches ~85-90%"
        );

        // No near-duplication: none of the other 7 small sentences (i.e.
        // everything but the legitimate 1-sentence overlap) reappear in
        // chunk 1 — chunk 1 advances by at least (100 - overlap_pct)% of
        // chunk 0's content rather than re-emitting most of it.
        for s in &small_sentences[0..7] {
            assert!(!c1.text.contains(s.as_str()), "chunk 1 unexpectedly duplicates chunk 0 sentence: {s}");
        }
        assert!(c1.text.contains(small_sentences[7].as_str()));
    }

    // 15-19. Task B-chunkcap: chunks must never exceed the embedder's hard
    // token ceiling (`Embedder::max_input_tokens`). A user hit this
    // building a PDF pack — PDF-extracted text can contain long runs with
    // no sentence breaks at all, so the old "oversize sentence, emitted
    // unsplit, no upper bound" path could hand the embedder a chunk over
    // its context window. These tests cover the new `split_to_ceiling` /
    // `split_word_to_ceiling` helpers directly (pure, unit-testable) and
    // the integration through `chunk_paragraph`.

    /// A test-only `Embedder` whose `token_count` is a plain char count,
    /// not a whitespace word count like `MockEmbedder`. Needed only for
    /// t17: `MockEmbedder`'s whitespace-based counting can never make a
    /// no-whitespace "word" exceed 1 token on its own, so it can't exercise
    /// `split_word_to_ceiling`'s pathological single-word case.
    struct CharCountEmbedder;

    impl Embedder for CharCountEmbedder {
        fn dims(&self) -> usize {
            8
        }
        fn embed_passage(&self, _text: &str) -> Result<Vec<f32>, EmbedError> {
            unimplemented!("split_word_to_ceiling tests only need token_count")
        }
        fn embed_query(&self, _text: &str) -> Result<Vec<f32>, EmbedError> {
            unimplemented!("split_word_to_ceiling tests only need token_count")
        }
        fn token_count(&self, text: &str) -> usize {
            text.chars().count()
        }
        fn max_input_tokens(&self) -> usize {
            usize::MAX
        }
    }

    // 15. split_to_ceiling (pure): an oversize word-run (no sentence
    // punctuation, so it can't rely on sentence boundaries) whose
    // token_count > ceiling splits into multiple pieces, each
    // token_count <= ceiling, and the pieces' words concatenate back to the
    // original words in order — no word lost, none reordered.
    #[test]
    fn t15_split_to_ceiling_splits_oversize_run_no_loss_each_piece_within_ceiling() {
        let embedder = MockEmbedder::with_max_input_tokens(8, 5);
        let text = words(23, "word");
        assert!(embedder.token_count(&text) > 5);

        let pieces = split_to_ceiling(&text, &embedder, 5);
        assert!(pieces.len() > 1, "expected the run split into multiple pieces");
        for (piece_text, piece_tokens) in &pieces {
            assert_eq!(*piece_tokens, embedder.token_count(piece_text));
            assert!(*piece_tokens <= 5, "piece exceeded ceiling: {piece_tokens}");
        }

        let reconstructed: Vec<&str> = pieces.iter().flat_map(|(t, _)| t.split_whitespace()).collect();
        let original: Vec<&str> = text.split_whitespace().collect();
        assert_eq!(reconstructed, original, "words lost or reordered by the split");
    }

    // 16. split_to_ceiling determinism: called twice on the same input ->
    // byte-identical output — the pack-determinism invariant (K8b:
    // byte-identical rebuilds) depends on this.
    #[test]
    fn t16_split_to_ceiling_is_deterministic() {
        let embedder = MockEmbedder::with_max_input_tokens(8, 7);
        let text = words(41, "tok");
        let first = split_to_ceiling(&text, &embedder, 7);
        let second = split_to_ceiling(&text, &embedder, 7);
        assert_eq!(first, second);
        assert!(first.len() > 1);
    }

    // 17. split_word_to_ceiling (pure): a pathological single "word" (no
    // internal whitespace) whose own token_count exceeds ceiling
    // hard-splits into pieces each token_count <= ceiling, reconstructs
    // exactly, and never panics on a multibyte string — if a piece
    // boundary ever landed mid-code-point, the `&str` slice itself would
    // panic, so "no panic" here IS the UTF-8-safety proof.
    #[test]
    fn t17_split_word_to_ceiling_hard_splits_pathological_multibyte_word_no_panic() {
        let embedder = CharCountEmbedder;
        let ceiling = 5;
        // "café" repeated: "é" is a multibyte UTF-8 char (2 bytes, 1 char)
        // — a byte-offset split landing between its two bytes would panic.
        let word: String = "café".repeat(10);
        assert!(embedder.token_count(&word) > ceiling);

        let pieces = split_word_to_ceiling(&word, &embedder, ceiling);
        assert!(!pieces.is_empty());
        let mut reconstructed = String::new();
        for (piece_text, piece_tokens) in &pieces {
            assert_eq!(*piece_tokens, embedder.token_count(piece_text));
            assert!(*piece_tokens <= ceiling, "piece exceeded ceiling: {piece_tokens}");
            reconstructed.push_str(piece_text);
        }
        assert_eq!(reconstructed, word, "text lost or reordered by the hard split");
    }

    // 18. chunk_paragraph integration: a single sentence-less run whose
    // token_count exceeds the embedder's max_input_tokens() ceiling is
    // split into multiple oversize_sentence chunks, each within the
    // ceiling, with no word lost or reordered — the actual bug report
    // (dense PDF text, no sentence breaks, one chunk over the embedder's
    // context window).
    #[test]
    fn t18_chunk_paragraph_splits_run_exceeding_embedder_ceiling() {
        let embedder = MockEmbedder::with_max_input_tokens(8, 10);
        let cfg = ChunkConfig {
            target_tokens: 5,
            overlap_pct: 18,
        };
        let long_run = words(50, "w");
        let doc = doc_with_section(
            vec!["Ch 1"],
            vec![Block::Paragraph {
                text: long_run.clone(),
                locator: "p1".to_string(),
            }],
        );

        let chunks = chunk_document(&doc, &embedder, &cfg);
        assert!(chunks.len() > 1, "expected the oversize run split into multiple chunks");
        for c in &chunks {
            assert!(c.oversize_sentence);
            assert!(
                c.token_count <= embedder.max_input_tokens(),
                "chunk exceeded the embedder ceiling: {} > {}",
                c.token_count,
                embedder.max_input_tokens()
            );
            assert_eq!(c.token_count, embedder.token_count(&c.text));
        }

        let reconstructed: Vec<&str> = chunks.iter().flat_map(|c| c.text.split_whitespace()).collect();
        let original: Vec<&str> = long_run.split_whitespace().collect();
        assert_eq!(reconstructed, original, "words lost or reordered by the split");
    }

    // 19. Determinism through the full chunk_document path: chunking the
    // same oversize input twice -> identical drafts (K8b's
    // byte-identical-rebuild invariant), exercised through the split
    // branch specifically.
    #[test]
    fn t19_chunk_document_oversize_split_is_deterministic() {
        let embedder = MockEmbedder::with_max_input_tokens(8, 10);
        let cfg = ChunkConfig {
            target_tokens: 5,
            overlap_pct: 18,
        };
        let long_run = words(73, "tok");
        let doc = doc_with_section(
            vec![],
            vec![Block::Paragraph {
                text: long_run,
                locator: "p1".to_string(),
            }],
        );

        let first = chunk_document(&doc, &embedder, &cfg);
        let second = chunk_document(&doc, &embedder, &cfg);
        assert_eq!(first, second);
        assert!(first.len() > 1);
    }

    // 20-22. Follow-up to task B-chunkcap (opus review): the ceiling was
    // paragraph-only — an oversize code block or an oversize table row
    // still reached the embedder unsplit, where BgeEmbedder's truncation
    // safety net would silently drop the tail. Extends the same
    // split_to_ceiling path (via push_chunk_within_ceiling) to code and
    // table chunks so the ceiling is enforced pack-wide.

    // 20. Code block over the embedder's ceiling splits into multiple
    // chunks, each within the ceiling, oversize_sentence stays false (it's
    // a paragraph-only flag), no word lost or reordered. A code block AT
    // or under the ceiling stays atomic (t6, unchanged).
    #[test]
    fn t20_code_block_splits_when_exceeding_embedder_ceiling() {
        let embedder = MockEmbedder::with_max_input_tokens(8, 10);
        let cfg = ChunkConfig::default();
        let code = words(30, "tok");
        let doc = doc_with_section(
            vec!["Appendix"],
            vec![Block::Code {
                text: code.clone(),
                locator: "c1".to_string(),
            }],
        );

        let chunks = chunk_document(&doc, &embedder, &cfg);
        assert!(chunks.len() > 1, "expected the oversize code block split into multiple chunks");
        for c in &chunks {
            assert_eq!(c.locator, "c1");
            assert!(!c.oversize_sentence, "oversize_sentence is paragraph-only, never set for code chunks");
            assert!(
                c.token_count <= embedder.max_input_tokens(),
                "chunk exceeded the embedder ceiling: {} > {}",
                c.token_count,
                embedder.max_input_tokens()
            );
            assert_eq!(c.token_count, embedder.token_count(&c.text));
        }

        let reconstructed: Vec<&str> = chunks.iter().flat_map(|c| c.text.split_whitespace()).collect();
        let original: Vec<&str> = code.split_whitespace().collect();
        assert_eq!(reconstructed, original, "words lost or reordered by the split");
    }

    // 21. A single table row (with header) that alone exceeds the
    // embedder's ceiling splits into multiple chunks, each within the
    // ceiling, oversize_sentence stays false, no word lost or reordered.
    // Rows/groups already within cfg.target_tokens (t5) are unaffected.
    #[test]
    fn t21_table_row_splits_when_exceeding_embedder_ceiling() {
        let embedder = MockEmbedder::with_max_input_tokens(8, 10);
        let cfg = ChunkConfig::default();
        let header = vec!["Description".to_string()];
        let long_value = words(40, "w");
        let rows = vec![vec![long_value]];
        let doc = doc_with_section(
            vec!["Data"],
            vec![Block::Table {
                header: header.clone(),
                rows: rows.clone(),
                locator: "t1".to_string(),
            }],
        );

        let chunks = chunk_document(&doc, &embedder, &cfg);
        assert!(chunks.len() > 1, "expected the oversize table row split into multiple chunks");
        for c in &chunks {
            assert_eq!(c.locator, "t1");
            assert!(!c.oversize_sentence, "oversize_sentence is paragraph-only, never set for table chunks");
            assert!(
                c.token_count <= embedder.max_input_tokens(),
                "chunk exceeded the embedder ceiling: {} > {}",
                c.token_count,
                embedder.max_input_tokens()
            );
            assert_eq!(c.token_count, embedder.token_count(&c.text));
        }

        let reconstructed: Vec<&str> = chunks.iter().flat_map(|c| c.text.split_whitespace()).collect();
        let original_text = format!("{}\n{}", header.join(" | "), render_table_row(&header, &rows[0]));
        let original: Vec<&str> = original_text.split_whitespace().collect();
        assert_eq!(reconstructed, original, "words lost or reordered by the split");
    }

    // 22. Determinism through chunk_document for the new code/table split
    // paths, together in one document: chunking twice -> identical drafts
    // (K8b byte-identical-rebuild invariant) — split_to_ceiling is the
    // same pure/deterministic helper the paragraph path already locked.
    #[test]
    fn t22_code_and_table_oversize_split_is_deterministic() {
        let embedder = MockEmbedder::with_max_input_tokens(8, 10);
        let cfg = ChunkConfig::default();
        let code = words(25, "c");
        let header = vec!["Col".to_string()];
        let rows = vec![vec![words(30, "v")]];
        let doc = doc_with_section(
            vec!["Ch"],
            vec![
                Block::Code {
                    text: code,
                    locator: "c1".to_string(),
                },
                Block::Table {
                    header,
                    rows,
                    locator: "t1".to_string(),
                },
            ],
        );

        let first = chunk_document(&doc, &embedder, &cfg);
        let second = chunk_document(&doc, &embedder, &cfg);
        assert_eq!(first, second);
        assert!(first.len() > 2, "expected multiple split pieces from both the code and table blocks");
    }
}
