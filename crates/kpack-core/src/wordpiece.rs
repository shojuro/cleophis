//! The BGE WordPiece token counter, tokenizer-only (Phase 1h M4b).
//!
//! The bundled reference pack is LEXICAL-ONLY (no embedder on Android), but
//! it must chunk exactly as the desktop does, and the desktop chunker
//! ([`crate::chunk`]) measures chunks in `bge-base-en-v1.5` tokens counted by
//! llama.cpp (`kpack-embed`'s `BgeEmbedder::token_count`: `str_to_token`
//! with `AddBos::Always` and special-token parsing on). This module is that
//! count without llama.cpp: a port of llama.cpp's WPM tokenizer
//! (`llm_tokenizer_wpm_session` in the vendored `llama-vocab.cpp`), over
//! llama.cpp's own Unicode tables ([`crate::wordpiece_tables`], generated
//! from the vendored `unicode-data.cpp`) and the vocabulary stored in the
//! bundled GGUF ([`BGE_VOCAB`], extracted from `tokenizer.ggml.tokens`).
//! It implements [`TokenCounter`], so `chunk::chunk_document_with` chunks a
//! pack with it and nothing is ever embedded.
//!
//! ## The algorithm (mirrors llama.cpp, step for step)
//! 1. Special-token partition: the five BERT control tokens (`[PAD]`,
//!    `[UNK]`, `[CLS]`, `[SEP]`, `[MASK]`) written literally in the text
//!    are one token each (`parse_special = true` in `str_to_token`).
//! 2. Per raw fragment, `preprocess`: codepoints → llama.cpp's one-to-one
//!    "NFD" map (which strips accents: `é` → `e`) → whitespace ends a word;
//!    NUL, U+FFFD and control characters are dropped; punctuation, ASCII
//!    symbols and CJK ideographs are single-character words; everything
//!    else is lowercased and appended to the current word.
//! 3. Per word, greedy longest match over bytes of `"▁" + word` against
//!    the vocabulary (whose word-initial pieces carry the `▁` phantom
//!    space and continuation pieces none — llama.cpp's form of `##`). A
//!    word that cannot be covered is one `[UNK]`.
//! 4. `[CLS]` before and `[SEP]` after: the count includes both.
//!
//! Parity with llama.cpp is checked over the whole reference corpus by the
//! `kpack-embed` test `wordpiece_parity` (needs the real feature and the
//! GGUF); this module's own tests pin golden counts.

use crate::chunk::TokenCounter;
use crate::wordpiece_tables::{FLAGS, LOWERCASE, NFD, WHITESPACE};
use std::collections::HashSet;

/// The `bge-base-en-v1.5` vocabulary exactly as the bundled GGUF stores it
/// (`tokenizer.ggml.tokens`, one token per line, in id order — llama.cpp's
/// phantom-space form of the `bert-base-uncased` WordPiece vocab).
pub const BGE_VOCAB: &str = include_str!("../assets/bge-base-en-v1.5.wpm-vocab.txt");

/// sha256 of the vendored llama.cpp `unicode-data.cpp` the tables were
/// generated from — provenance a pack manifest can record.
pub const UNICODE_TABLES_SHA256: &str = crate::wordpiece_tables::SOURCE_SHA256;

/// sha256 of [`BGE_VOCAB`] (the GGUF's token list), pinned by a test.
pub const BGE_VOCAB_SHA256: &str = "30e577ebcdd5bfd324eaa21b13fffa867ff56f1bcb390a1048184129d377f81a";

/// `bge-base-en-v1.5`'s context window — `kpack-embed`'s `BGE_N_CTX`, the
/// ceiling the desktop chunker splits at.
pub const BGE_MAX_INPUT_TOKENS: usize = 512;

/// The BERT control tokens llama.cpp parses out of raw text.
const SPECIAL_TOKENS: [&str; 5] = ["[PAD]", "[UNK]", "[CLS]", "[SEP]", "[MASK]"];

const FLAG_PUNCTUATION: u16 = 0x0020;
const FLAG_SYMBOL: u16 = 0x0040;
const FLAG_CONTROL: u16 = 0x0080;
const FLAG_UNDEFINED: u16 = 0x0001;

/// A WordPiece token counter over one vocabulary. See the module doc.
pub struct WordPieceTokenizer {
    vocab: HashSet<Vec<u8>>,
    max_token_len: usize,
    max_input_tokens: usize,
}

impl WordPieceTokenizer {
    /// Build from a vocabulary in llama.cpp's form (one token per line).
    /// Errors if the text is empty or lacks one of the BERT control tokens.
    pub fn from_vocab(vocab_text: &str, max_input_tokens: usize) -> Result<Self, String> {
        let mut vocab = HashSet::new();
        let mut max_token_len = 0usize;
        for token in vocab_text.split('\n').filter(|t| !t.is_empty()) {
            max_token_len = max_token_len.max(token.len());
            vocab.insert(token.as_bytes().to_vec());
        }
        if vocab.is_empty() {
            return Err("empty vocabulary".to_string());
        }
        for special in SPECIAL_TOKENS {
            if !vocab.contains(special.as_bytes()) {
                return Err(format!("vocabulary lacks the control token {special}"));
            }
        }
        Ok(WordPieceTokenizer {
            vocab,
            max_token_len,
            max_input_tokens,
        })
    }

    /// The desktop's tokenizer: [`BGE_VOCAB`] with a 512-token ceiling.
    pub fn bge_base_en_v1_5() -> Self {
        Self::from_vocab(BGE_VOCAB, BGE_MAX_INPUT_TOKENS)
            .expect("the bundled BGE vocabulary is well-formed")
    }

    /// Token count of `text` including `[CLS]` and `[SEP]` — what
    /// `BgeEmbedder::token_count` returns for the same text.
    pub fn count(&self, text: &str) -> usize {
        let mut n = 2; // [CLS] ... [SEP]
        let mut rest = text;
        while !rest.is_empty() {
            match earliest_special(rest) {
                Some((at, len)) => {
                    n += self.count_raw(&rest[..at]) + 1;
                    rest = &rest[at + len..];
                }
                None => {
                    n += self.count_raw(rest);
                    break;
                }
            }
        }
        n
    }

    /// Tokens for one raw (special-token-free) fragment.
    fn count_raw(&self, text: &str) -> usize {
        preprocess(text).iter().map(|w| self.count_word(w)).sum()
    }

    /// Greedy longest match over `"▁" + word`'s bytes; an uncoverable word
    /// is one `[UNK]`. Mirrors `llm_tokenizer_wpm_session::tokenize`.
    fn count_word(&self, word: &str) -> usize {
        if word.is_empty() {
            return 0;
        }
        let mut word1 = "\u{2581}".as_bytes().to_vec();
        word1.extend_from_slice(word.as_bytes());
        let n = word1.len();
        let mut tokens = 0usize;
        let mut i = 0usize;
        while i < n {
            let mut matched = false;
            let mut j = n.min(i + self.max_token_len + 1);
            while j > i {
                if self.vocab.contains(&word1[i..j]) {
                    tokens += 1;
                    matched = true;
                    i = j;
                    break;
                }
                j -= 1;
            }
            if !matched {
                return 1; // discard all: the whole word is [UNK]
            }
        }
        tokens.max(1)
    }
}

impl TokenCounter for WordPieceTokenizer {
    fn token_count(&self, text: &str) -> usize {
        self.count(text)
    }
    fn max_input_tokens(&self) -> usize {
        self.max_input_tokens
    }
}

/// The earliest literal control token in `text`: (byte offset, byte length).
fn earliest_special(text: &str) -> Option<(usize, usize)> {
    SPECIAL_TOKENS
        .iter()
        .filter_map(|s| text.find(s).map(|at| (at, s.len())))
        .min()
}

/// llama.cpp's `llm_tokenizer_wpm_session::preprocess` (lowercase on).
fn preprocess(text: &str) -> Vec<String> {
    let mut words: Vec<String> = vec![String::new()];
    for c in text.chars() {
        let cpt = nfd(c as u32);
        let flags = flags(cpt);
        if is_whitespace(cpt) {
            if !words.last().map_or(true, |w| w.is_empty()) {
                words.push(String::new());
            }
            continue;
        }
        if cpt == 0 || cpt == 0xFFFD || flags & FLAG_CONTROL != 0 {
            continue;
        }
        let lower = char::from_u32(to_lower(cpt)).unwrap_or('\u{FFFD}');
        if flags & FLAG_PUNCTUATION != 0 || (cpt < 0x7F && flags & FLAG_SYMBOL != 0) || is_chinese_char(cpt) {
            if !words.last().map_or(true, |w| w.is_empty()) {
                words.push(String::new());
            }
            let last = words.last_mut().expect("never empty");
            last.clear();
            last.push(lower);
            words.push(String::new());
        } else {
            words.last_mut().expect("never empty").push(lower);
        }
    }
    if words.last().map_or(false, |w| w.is_empty()) {
        words.pop();
    }
    words
}

/// `unicode_cpts_normalize_nfd`: a one-to-one map over ranges.
fn nfd(cpt: u32) -> u32 {
    let idx = NFD.partition_point(|&(first, _, _)| first <= cpt);
    if idx == 0 {
        return cpt;
    }
    let (first, last, to) = NFD[idx - 1];
    if first <= cpt && cpt <= last {
        to
    } else {
        cpt
    }
}

/// `unicode_cpt_flags_from_cpt`'s category bits (the range table).
fn flags(cpt: u32) -> u16 {
    if cpt >= 0x110000 {
        return FLAG_UNDEFINED;
    }
    let idx = FLAGS.partition_point(|&(start, _)| start <= cpt);
    FLAGS[idx - 1].1
}

/// `unicode_cpt_flags::is_whitespace` (set from `unicode_set_whitespace`).
fn is_whitespace(cpt: u32) -> bool {
    WHITESPACE.binary_search(&cpt).is_ok()
}

/// `unicode_tolower`.
fn to_lower(cpt: u32) -> u32 {
    match LOWERCASE.binary_search_by_key(&cpt, |&(from, _)| from) {
        Ok(i) => LOWERCASE[i].1,
        Err(_) => cpt,
    }
}

/// `llm_tokenizer_wpm_session::is_chinese_char`, ranges verbatim.
fn is_chinese_char(cpt: u32) -> bool {
    (0x04E00..=0x09FFF).contains(&cpt)
        || (0x03400..=0x04DBF).contains(&cpt)
        || (0x20000..=0x2A6DF).contains(&cpt)
        || (0x2A700..=0x2B73F).contains(&cpt)
        || (0x2B740..=0x2B81F).contains(&cpt)
        || (0x2B920..=0x2CEAF).contains(&cpt)
        || (0x0F900..=0x0FAFF).contains(&cpt)
        || (0x2F800..=0x2FA1F).contains(&cpt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn tok() -> WordPieceTokenizer {
        WordPieceTokenizer::bge_base_en_v1_5()
    }

    // Golden counts (with [CLS]/[SEP]), from the HF `bert-base-uncased`
    // tokenizer over the same vocabulary; these inputs exercise nothing
    // where HF and llama.cpp differ. The full-corpus llama.cpp parity check
    // lives in kpack-embed (`wordpiece_parity`).
    #[test]
    fn golden_counts() {
        let cases: &[(&str, usize)] = &[
            ("", 2),
            ("Gout", 4),
            ("Paracetamol is a common painkiller.", 13),
            ("Take 1 or 2 tablets every 4 to 6 hours \u{2013} no more than 8 in 24 hours.", 21),
            ("Crohn\u{2019}s disease", 8),
            ("M\u{e9}ni\u{e8}re\u{2019}s disease and Guillain-Barr\u{e9} syndrome", 14),
            ("co-codamol 30/500mg", 11),
            ("HbA1c   \t\n  levels", 7),
            ("zeltrofenazine", 8),
            ("\u{a3}5 at 37\u{b0}C", 7),
            ("\u{201c}quoted\u{201d} text \u{2022} bullet", 8),
            ("Kidney stones.\n\n## Complications of gout", 11),
            ("ARDS (acute respiratory distress syndrome)", 10),
        ];
        let t = tok();
        for (text, want) in cases {
            assert_eq!(t.count(text), *want, "{text:?}");
        }
    }

    #[test]
    fn literal_control_tokens_are_one_token_each() {
        let t = tok();
        // [CLS] + "a" + [MASK] + "b" + [SEP]
        assert_eq!(t.count("a[MASK]b"), 5);
        // lowercase is not a control token: "[", "mask", "]" = 3
        assert_eq!(t.count("[mask]"), 5);
    }

    #[test]
    fn an_uncoverable_word_is_one_unk_and_controls_are_dropped() {
        let t = tok();
        // U+0627 ARABIC ALEF: a vocab entry, one token (a word of its own).
        assert_eq!(t.count("\u{627}"), 3);
        // A soft hyphen (Cf, control) is dropped, not a token.
        assert_eq!(t.count("gout\u{ad}"), t.count("gout"));
        // A private-use codepoint is category C in llama.cpp's table: dropped.
        assert_eq!(t.count("ab\u{e000}cd"), t.count("abcd"));
        // An Ethiopic letter (in the corpus, not in the vocab) cannot be
        // covered, so its whole word is one [UNK].
        assert_eq!(t.count("ab\u{1235}cd"), 3);
    }

    #[test]
    fn nfd_and_lowercase_follow_llama_cpp_tables() {
        assert_eq!(nfd(0xE9), 0x65); // é -> e
        assert_eq!(nfd(0x41), 0x41);
        assert_eq!(to_lower(0x41), 0x61);
        assert_eq!(to_lower(0x61), 0x61);
        assert!(is_whitespace(0x202F)); // narrow no-break space
        assert!(flags(0x2013) & FLAG_PUNCTUATION != 0); // en dash
        assert!(flags(0x00AD) & FLAG_CONTROL != 0); // soft hyphen
    }

    #[test]
    fn vocab_asset_is_the_pinned_gguf_token_list() {
        let sha: String = Sha256::digest(BGE_VOCAB.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(sha, BGE_VOCAB_SHA256);
        assert_eq!(BGE_VOCAB.lines().count(), 30522);
        assert_eq!(UNICODE_TABLES_SHA256.len(), 64);
    }

    #[test]
    fn ceiling_is_bge_context() {
        assert_eq!(tok().max_input_tokens(), 512);
        assert!(WordPieceTokenizer::from_vocab("a\nb\n", 512).is_err());
    }
}
