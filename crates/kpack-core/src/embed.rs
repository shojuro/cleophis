//! Embedding: the `Embedder` trait, the int8 quantization math the pack
//! format actually stores and scores against (spec §1.4), and a
//! deterministic mock for unit tests elsewhere in this crate (K5's chunker,
//! K8's build pipeline). This module is pure Rust — no network dep, no
//! native/ML dep. K4b (`kpack-embed`, a separate crate) links `llama.cpp`
//! and provides the real `bge-base-en-v1.5` implementation of `Embedder`;
//! this module only defines the trait boundary and the math both that real
//! impl and `format::Pack`'s scoring share.
//!
//! ## Query prefix convention (drift guard)
//! BGE v1.5 (and similar instruction-tuned embedders) score higher when a
//! *query* is prefixed with an instruction sentence at query time only —
//! passages are embedded raw, with no prefix. If build-time chunking and
//! query-time search each hand-rolled that prefix separately, they could
//! drift (a typo, a trailing-space difference) and silently degrade
//! retrieval. Spec §1.4: "baked into the core library so build and query
//! can't drift." [`BGE_QUERY_INSTRUCTION`] and [`query_input`] are the ONE
//! place that convention lives — every `Embedder::embed_query` impl (this
//! module's mock, and K4b's real backend) MUST call `query_input` rather
//! than reimplementing the prefix.
//!
//! ## Quantization (the shipped scoring reality)
//! Spec §1.4: vectors are L2-normalized, then int8 scalar-quantized for
//! storage; `format::Pack`'s `vec` table stores the int8 form, and
//! `vec_search`/calibration (§2.4) score against *that*, never the fp32
//! original. [`l2_normalize`], [`quantize_int8`], and [`dot_int8`] are that
//! exact pipeline as three pure, independently testable functions.
//!
//! ## `MockEmbedder` — plumbing tests only, never a determinism test
//! `MockEmbedder` (available under `#[cfg(test)]`, or via the opt-in
//! `test-util` Cargo feature for use from elsewhere in this crate) is a
//! deterministic stand-in with NO real semantic properties — same text
//! always hashes to the same vector, so tests can assert retrieval
//! plumbing without a real ~60MB GGUF model. Its `token_count` is a crude
//! word-count approximation, not a real tokenizer, and must never be
//! trusted for anything beyond its own unit tests here — in particular it
//! must NEVER back the K8 cross-build determinism test, whose chunk
//! boundaries depend on the REAL tokenizer K4b provides. See
//! `MockEmbedder::token_count`'s doc comment.

use std::fmt;

/// The BGE v1.5 query-side instruction prefix (spec §1.4: "BGE's 'Represent
/// this sentence…' query instruction"), documented by BAAI for the
/// `bge-*-en-v1.5` model family. Applies to queries only — passages are
/// embedded raw, with no prefix.
pub const BGE_QUERY_INSTRUCTION: &str =
    "Represent this sentence for searching relevant passages: ";

/// Prepend [`BGE_QUERY_INSTRUCTION`] to `text`. The ONE place the query
/// prefix convention lives (spec §1.4) — every `Embedder::embed_query` impl
/// must route through this rather than reimplementing the prefix, so
/// build-time and query-time embedding can't drift apart.
pub fn query_input(text: &str) -> String {
    format!("{BGE_QUERY_INSTRUCTION}{text}")
}

/// Errors from an `Embedder` implementation, or from this module's own pure
/// math functions. Hand-rolled (no `thiserror`, matching `format::Error`'s
/// and `sign`'s style in this crate) — small on purpose.
#[derive(Debug)]
pub enum EmbedError {
    /// An impl-specific backend failure (e.g. K4b's `llama.cpp` call
    /// erroring). The message is backend-supplied and opaque to this crate.
    Backend(String),
    /// A vector-length mismatch: `expected` vs. `got` dims. Used both by
    /// `Embedder` impls validating their own output and by [`dot_int8`]
    /// when its two operands differ in length.
    Dims { expected: usize, got: usize },
}

impl fmt::Display for EmbedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EmbedError::Backend(msg) => write!(f, "embedder backend error: {msg}"),
            EmbedError::Dims { expected, got } => {
                write!(f, "embedding dims mismatch: expected {expected}, got {got}")
            }
        }
    }
}

impl std::error::Error for EmbedError {}

/// An embedding backend: turns text into a dense fp32 vector. Implemented
/// by K4b's real `llama.cpp`-backed embedder and, for tests, by
/// [`MockEmbedder`] in this module.
pub trait Embedder {
    /// The dimensionality of vectors this embedder produces (e.g. 768 for
    /// `bge-base-en-v1.5`).
    fn dims(&self) -> usize;

    /// Embed a passage/chunk for storage. NO query instruction is applied —
    /// passages are embedded as-is (spec §1.4's asymmetric BGE convention:
    /// only queries get the instruction prefix). Returns raw fp32; callers
    /// run [`l2_normalize`]/[`quantize_int8`] before persisting.
    fn embed_passage(&self, text: &str) -> Result<Vec<f32>, EmbedError>;

    /// Embed a search query. Implementations MUST prepend the BGE query
    /// instruction via [`query_input`] (not a hand-rolled copy of the
    /// prefix) so build-time passage embedding and query-time search can
    /// never drift apart (spec §1.4).
    fn embed_query(&self, text: &str) -> Result<Vec<f32>, EmbedError>;

    /// Token count under this embedder's own tokenizer — the unit K5's
    /// chunker targets (400 tokens/chunk, spec §1.3). A real impl's count
    /// must match its actual tokenizer exactly, since chunk boundaries are
    /// derived from it. [`MockEmbedder::token_count`] is APPROXIMATE and
    /// must never be used for anything beyond this module's own unit tests
    /// — see that method's doc comment.
    fn token_count(&self, text: &str) -> usize;

    /// The largest `token_count` value `embed_passage`/`embed_query` can
    /// accept without error — the embedder's hard context-window ceiling
    /// (`token_count` and the embedder's own internal tokenization must
    /// agree exactly, so a chunk with `token_count(text) <= max_input_tokens()`
    /// is guaranteed to embed successfully). K5's chunker (`chunk.rs`) reads
    /// this to split any oversize unit before it ever reaches the embedder,
    /// rather than relying on the embedder to reject or silently mishandle
    /// over-length input.
    fn max_input_tokens(&self) -> usize;
}

/// L2-normalize `v` (divide every component by the vector's Euclidean
/// norm). A zero vector (norm == 0) returns an all-zero vector of the same
/// length rather than dividing by zero, which would otherwise produce
/// `NaN` in every component.
pub fn l2_normalize(v: &[f32]) -> Vec<f32> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm == 0.0 {
        return vec![0.0; v.len()];
    }
    v.iter().map(|x| x / norm).collect()
}

/// Scalar-quantize an L2-normalized vector (components in roughly `[-1,
/// 1]`) to int8: each component is scaled by 127, rounded to the nearest
/// integer, and clamped to `[-127, 127]` — never `-128`. `i8::MIN` is
/// `-128`, which has no positive counterpart at this width; excluding it
/// keeps the quantized range symmetric around zero, matching the
/// L2-normalized input's symmetry. This mirrors exactly what
/// `format::Pack::insert_embedding` stores in the `vec0` `int8[dims]`
/// column (spec §1.4).
pub fn quantize_int8(normalized: &[f32]) -> Vec<i8> {
    normalized
        .iter()
        .map(|&x| (x * 127.0).round().clamp(-127.0, 127.0) as i8)
        .collect()
}

/// int8 dot product: `sum(a[i] as i32 * b[i] as i32)`, accumulated in
/// `i32` (not `i8`/`i16`) to avoid overflow — even at hundreds of
/// dimensions, each term is at most `127 * 127`, comfortably inside `i32`.
/// Because both inputs were L2-normalized before quantization, this is an
/// integer approximation of cosine similarity (spec §1.4) — the exact
/// scoring `format::Pack::vec_search`'s `vec0` `MATCH` performs internally.
///
/// Returns `Err(EmbedError::Dims { expected, got })` on a length mismatch
/// (`expected` = `a.len()`, `got` = `b.len()`) rather than panicking or
/// silently truncating to the shorter length — callers should never
/// mismatch (both vectors come from the same pack's fixed `dims`), so a
/// mismatch signals a bug worth surfacing explicitly, not a case to paper
/// over.
pub fn dot_int8(a: &[i8], b: &[i8]) -> Result<i32, EmbedError> {
    if a.len() != b.len() {
        return Err(EmbedError::Dims {
            expected: a.len(),
            got: b.len(),
        });
    }
    Ok(a.iter().zip(b.iter()).map(|(&x, &y)| x as i32 * y as i32).sum())
}

/// A deterministic, semantics-free stand-in for a real `Embedder`, gated to
/// test builds (`#[cfg(test)]`) or the opt-in `test-util` Cargo feature (so
/// K5's chunker and K8's build-pipeline tests elsewhere in this crate can
/// construct one too). NEVER compiled into a normal (non-test,
/// non-`test-util`) build — there is no real embedding model here, so
/// shipping it would silently produce garbage vectors.
///
/// **Risk note (plan):** this mock must NEVER back a determinism test that
/// spans a real build. Its vectors carry no semantic meaning (they're a
/// hash of the input text, not a learned embedding), and its `token_count`
/// is a crude approximation, not the real tokenizer. It exists purely to
/// test plumbing — does `embed_query` differ from `embed_passage`? is the
/// same text stable across calls? — never to stand in for actual retrieval
/// quality or exact chunk-boundary token counts.
#[cfg(any(test, feature = "test-util"))]
pub struct MockEmbedder {
    dims: usize,
    max_input_tokens: usize,
}

/// [`MockEmbedder::new`]'s default `max_input_tokens` — mirrors K4b's real
/// `bge-base-en-v1.5` ceiling (512) so plumbing tests that don't care about
/// the ceiling behave as if it weren't there (every `target_tokens` value
/// used elsewhere in this crate's tests is comfortably under it). Tests
/// that specifically exercise oversize-unit splitting construct a mock
/// with a SMALL ceiling via [`MockEmbedder::with_max_input_tokens`] instead.
#[cfg(any(test, feature = "test-util"))]
const MOCK_DEFAULT_MAX_INPUT_TOKENS: usize = 512;

#[cfg(any(test, feature = "test-util"))]
impl MockEmbedder {
    pub fn new(dims: usize) -> Self {
        MockEmbedder {
            dims,
            max_input_tokens: MOCK_DEFAULT_MAX_INPUT_TOKENS,
        }
    }

    /// Construct with an explicit `max_input_tokens` ceiling instead of the
    /// default — lets `chunk.rs`'s oversize-split tests exercise the
    /// splitting path with a SMALL ceiling, without a real ~60MB GGUF
    /// model.
    pub fn with_max_input_tokens(dims: usize, max_input_tokens: usize) -> Self {
        MockEmbedder { dims, max_input_tokens }
    }
}

#[cfg(any(test, feature = "test-util"))]
impl Embedder for MockEmbedder {
    fn dims(&self) -> usize {
        self.dims
    }

    fn embed_passage(&self, text: &str) -> Result<Vec<f32>, EmbedError> {
        Ok(l2_normalize(&pseudo_random_vector(text, self.dims)))
    }

    fn embed_query(&self, text: &str) -> Result<Vec<f32>, EmbedError> {
        // Route through `query_input` (not a hand-rolled prefix) so this
        // mock exercises the exact same drift guard a real impl must.
        let prefixed = query_input(text);
        Ok(l2_normalize(&pseudo_random_vector(&prefixed, self.dims)))
    }

    /// APPROXIMATE (unicode whitespace word count) — a stand-in usable only
    /// for this module's own unit tests. MUST NOT be used for the K8
    /// cross-build determinism test: chunk boundaries there need the REAL
    /// tokenizer from K4b's `llama.cpp` embedder, or chunk token counts —
    /// and therefore chunk boundaries — will silently diverge from what a
    /// real build produces.
    fn token_count(&self, text: &str) -> usize {
        text.split_whitespace().count()
    }

    fn max_input_tokens(&self) -> usize {
        self.max_input_tokens
    }
}

/// FNV-1a over `bytes` — a small, dependency-free, deterministic hash used
/// only to seed [`pseudo_random_vector`]. Not cryptographic; picked for
/// simplicity and zero dependencies, not security.
#[cfg(any(test, feature = "test-util"))]
fn fnv1a(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

/// A deterministic pseudo-random f32 vector of length `dims`, seeded from
/// `text`'s FNV-1a hash and expanded with a xorshift64 stream: same text →
/// same seed → same vector; different text → (almost certainly) a
/// different vector. Components land in roughly `[-1, 1)`, pre-L2-norm.
/// NOT a real embedding — [`MockEmbedder`] only.
#[cfg(any(test, feature = "test-util"))]
fn pseudo_random_vector(text: &str, dims: usize) -> Vec<f32> {
    // xorshift64 is only well-behaved from a non-zero seed. FNV-1a's offset
    // basis is non-zero, and the hash loop only XORs/multiplies it, so
    // `state` is non-zero for every possible `text`, including "".
    let mut state = fnv1a(text.as_bytes());
    (0..dims)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            // Top 24 bits mix best under xorshift; map them to [0, 1) then
            // rescale to [-1, 1).
            let unit = (state >> 40) as f32 / (1u64 << 24) as f32;
            unit * 2.0 - 1.0
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // 1. l2_normalize produces unit norm (±epsilon); zero-vector → zeros,
    // no NaN.
    #[test]
    fn t1_l2_normalize_unit_norm_and_zero_safe() {
        let v = vec![3.0, 4.0]; // 3-4-5 triangle
        let normed = l2_normalize(&v);
        let norm: f32 = normed.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-6, "expected unit norm, got {norm}");

        let zero = vec![0.0, 0.0, 0.0];
        let normed_zero = l2_normalize(&zero);
        assert_eq!(normed_zero, vec![0.0, 0.0, 0.0]);
        assert!(normed_zero.iter().all(|x| !x.is_nan()));
    }

    // 2. quantize_int8 round-trip: a known normalized vector quantizes into
    // the expected int8 range; no value is -128; dot_int8 of a vector with
    // itself is positive and larger than with an orthogonal one (ranking
    // sanity — the ≈cosine property).
    #[test]
    fn t2_quantize_and_dot_int8_cosine_sanity() {
        let normed = l2_normalize(&[3.0, 4.0]);
        let q = quantize_int8(&normed);
        assert_eq!(q.len(), 2);
        for &x in &q {
            assert!((-127..=127).contains(&x));
            assert_ne!(x, i8::MIN, "quantize_int8 must never emit -128");
        }

        let self_dot = dot_int8(&q, &q).unwrap();
        assert!(self_dot > 0, "dot of a vector with itself should be positive");

        // [4.0, -3.0] is perpendicular to [3.0, 4.0] (orthogonal).
        let orthogonal = quantize_int8(&l2_normalize(&[4.0, -3.0]));
        let cross_dot = dot_int8(&q, &orthogonal).unwrap();
        assert!(
            self_dot > cross_dot,
            "self-similarity ({self_dot}) should exceed orthogonal similarity ({cross_dot})"
        );
    }

    // 3. dot_int8 length-mismatch → Err, no panic.
    #[test]
    fn t3_dot_int8_length_mismatch_errs_not_panics() {
        let a: [i8; 3] = [1, 2, 3];
        let b: [i8; 2] = [1, 2];
        let result = dot_int8(&a, &b);
        assert!(matches!(
            result,
            Err(EmbedError::Dims {
                expected: 3,
                got: 2
            })
        ));
    }

    // 4. query_input prepends the exact BGE instruction; embed_query and
    // embed_passage on the same text give DIFFERENT vectors (prefix
    // applied).
    #[test]
    fn t4_query_input_prepends_instruction_and_differs_from_passage() {
        let text = "how does vitamin K interact with warfarin";
        let prefixed = query_input(text);
        assert_eq!(prefixed, format!("{BGE_QUERY_INSTRUCTION}{text}"));
        assert!(prefixed.starts_with(BGE_QUERY_INSTRUCTION));

        let embedder = MockEmbedder::new(16);
        let passage_vec = embedder.embed_passage(text).unwrap();
        let query_vec = embedder.embed_query(text).unwrap();
        assert_ne!(
            passage_vec, query_vec,
            "embed_query must differ from embed_passage on the same text (prefix applied)"
        );
    }

    // 5. Mock determinism: same text → identical vector across two calls;
    // different text → different vector; output length == dims and is
    // L2-normalized.
    #[test]
    fn t5_mock_embedder_is_deterministic() {
        let embedder = MockEmbedder::new(32);

        let a1 = embedder.embed_passage("the quick brown fox").unwrap();
        let a2 = embedder.embed_passage("the quick brown fox").unwrap();
        assert_eq!(a1, a2, "same text should produce an identical vector");

        let b = embedder
            .embed_passage("a completely different sentence")
            .unwrap();
        assert_ne!(a1, b, "different text should produce a different vector");

        assert_eq!(a1.len(), 32);
        let norm: f32 = a1.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-5,
            "mock output should be L2-normalized, got norm {norm}"
        );
    }
}
