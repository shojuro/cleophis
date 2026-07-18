//! Per-pack retrieval + reciprocal-rank fusion (RRF) — spec §4.1's first
//! slice. Composes `format::Pack`'s two independent lanes (`vec_search`
//! dense, `fts_search` lexical), fuses their rankings with RRF, and
//! annotates every fused candidate with a dense cosine similarity for R2's
//! gate to consume. Pure, network-free, and deliberately does NOT gate
//! (R2) or fuse across packs (R3) — this module produces the per-pack
//! fused, cosine-annotated candidate list those later slices build on.
//!
//! ## RRF (reciprocal rank fusion)
//! [`rrf`] implements the standard formula: a chunk's fused score is the
//! sum, over every lane it appears in, of `1.0 / (k_rrf + rank)`, where
//! `rank` is 1-based position in that lane's ranked list. A chunk present
//! in multiple lanes accumulates a contribution from each — so a mediocre
//! rank in two lanes can (and by design, should) outscore a great rank in
//! only one. [`DEFAULT_K_RRF`] (60.0) is spec §4.1's default; it damps the
//! influence of rank 1 vs. rank 2 (a smaller `k_rrf` makes top ranks
//! dominate more sharply).
//!
//! ## fts5 query safety
//! Raw user query text can contain fts5 MATCH operators/syntax (`OR`,
//! `AND`, `NOT`, `(`, `)`, `-`, `*`, `:`, unbalanced `"`, …) that would
//! otherwise either change the query's meaning or fail with an fts5 syntax
//! error. [`safe_fts5_query`] neutralizes all of that: split the raw text
//! on whitespace, wrap each term in double quotes (doubling any embedded
//! `"` per fts5's own quoted-string escape), and join with spaces. Every
//! term becomes a literal single-token phrase match; space-separated
//! quoted phrases are fts5's implicit AND, so the result still means
//! "every one of these words must appear" — just with zero operator
//! parsing surface left for user-supplied text to exploit.
use crate::embed::dot_int8;
use crate::format::{self, Pack};
use std::fmt;

/// Reciprocal-rank fusion's default `k_rrf` (spec §4.1).
pub const DEFAULT_K_RRF: f64 = 60.0;

/// The int8 quantization scale (127², spec §1.4: L2-normalize then ×127,
/// clamped to `[-127, 127]`) that turns a raw `dot_int8` accumulation back
/// into an approximate cosine similarity: `dot_int8(a, b) / 16129 ≈
/// cos(a, b)` when both `a` and `b` are that pipeline's output.
const INT8_DOT_SCALE: f32 = 16129.0;

/// Errors from this module: either lane's SQLite query
/// ([`format::Error`]) or the dense-cosine dot product
/// ([`crate::embed::EmbedError`], e.g. if `query_i8`'s length doesn't
/// match a stored embedding's — a caller bug, since both should come from
/// the same pack's fixed `dims`). Hand-rolled, matching this crate's other
/// error types (no `thiserror`).
#[derive(Debug)]
pub enum Error {
    Format(format::Error),
    Embed(crate::embed::EmbedError),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Format(e) => write!(f, "pack format error: {e}"),
            Error::Embed(e) => write!(f, "embedder error: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<format::Error> for Error {
    fn from(e: format::Error) -> Self {
        Error::Format(e)
    }
}

impl From<crate::embed::EmbedError> for Error {
    fn from(e: crate::embed::EmbedError) -> Self {
        Error::Embed(e)
    }
}

type Result<T> = std::result::Result<T, Error>;

/// One per-pack fused retrieval candidate: a `chunk_id`, its RRF-fused
/// score across the dense + lexical lanes, and its dense cosine similarity
/// to the query (independent of fusion — R2's gate scores on this, not
/// `fused_score`, per the plan's decision to gate on dense cosine).
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub chunk_id: i64,
    pub fused_score: f64,
    pub dense_cosine: f32,
}

/// Reciprocal-rank fusion over an arbitrary number of ranked "lanes" (each
/// a slice of `chunk_id`s in rank order, 1-based — index 0 is rank 1). A
/// chunk's fused score is the sum, over every lane it appears in, of `1.0
/// / (k_rrf + rank)`. Returns `(chunk_id, fused_score)` pairs sorted by
/// score descending; ties break by `chunk_id` ascending so the output is
/// deterministic regardless of input/iteration order. Pure — no I/O.
pub fn rrf(lanes: &[&[i64]], k_rrf: f64) -> Vec<(i64, f64)> {
    let mut scores: std::collections::HashMap<i64, f64> = std::collections::HashMap::new();
    for lane in lanes {
        for (idx, &chunk_id) in lane.iter().enumerate() {
            let rank = (idx + 1) as f64; // 1-based
            *scores.entry(chunk_id).or_insert(0.0) += 1.0 / (k_rrf + rank);
        }
    }
    let mut fused: Vec<(i64, f64)> = scores.into_iter().collect();
    fused.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    fused
}

/// Turn arbitrary user query text into a query string that's syntactically
/// safe to pass to `fts_search`'s `MATCH` — see the module doc comment for
/// the approach (whitespace-split terms, each wrapped as a quoted literal
/// phrase, joined by spaces = implicit AND). Every fts5 metacharacter
/// (operator keywords, parens, unbalanced quotes, `-`/`*`/`:` prefixes) is
/// absorbed into a quoted literal and never reaches fts5's operator
/// parser. An all-whitespace/empty `text` yields an empty string (fts5
/// treats an empty `MATCH` argument as "match nothing" territory — callers
/// with a possibly-empty query should short-circuit before calling
/// `fts_search`, same as any other empty-query UX decision; that's outside
/// this pure string helper's job).
pub fn safe_fts5_query(text: &str) -> String {
    text.split_whitespace()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Per-pack retrieve + RRF-fuse: spec §4.1's dense + lexical dual-lane
/// search over one pack. `query_i8` is the caller's already-embedded,
/// L2-normalized, int8-quantized query vector (R4 wires the real
/// `Embedder::embed_query` call — this function takes the vector, not raw
/// text, to stay embedder-agnostic); `query_text` is that same query's raw
/// text, used only for the lexical lane.
///
/// 1. `dense = pack.vec_search(query_i8, k)` → ranked `(chunk_id,
///    distance)`.
/// 2. `lexical = pack.fts_search(safe_fts5_query(query_text), k)` → ranked
///    `chunk_id`s.
/// 3. `fused = rrf(&[dense_ids, lexical], DEFAULT_K_RRF)`.
/// 4. Each fused chunk's `dense_cosine` is recomputed from its OWN stored
///    embedding (via [`crate::format::Pack::get_embedding`]) against
///    `query_i8` — not derived from `vec_search`'s `distance` column, so
///    it's well-defined even for a chunk that only matched the lexical
///    lane. A chunk with no stored embedding (a build-pipeline invariant
///    violation, since every chunk should be embedded post-build) gets
///    `dense_cosine = 0.0` rather than erroring the whole retrieval.
/// 5. Returns candidates in fused order (highest `fused_score` first).
///
/// Does NOT gate (R2) or fuse across packs (R3).
pub fn retrieve_pack(
    pack: &Pack,
    query_text: &str,
    query_i8: &[i8],
    k: usize,
) -> Result<Vec<Candidate>> {
    let dense = pack.vec_search(query_i8, k)?;
    let dense_ids: Vec<i64> = dense.iter().map(|(chunk_id, _)| *chunk_id).collect();

    let lexical_query = safe_fts5_query(query_text);
    let lexical = pack.fts_search(&lexical_query, k)?;

    let fused = rrf(&[&dense_ids, &lexical], DEFAULT_K_RRF);

    fused
        .into_iter()
        .map(|(chunk_id, fused_score)| {
            let dense_cosine = match pack.get_embedding(chunk_id)? {
                Some(stored) => {
                    let raw = dot_int8(query_i8, &stored)?;
                    (raw as f32 / INT8_DOT_SCALE).clamp(-1.0, 1.0)
                }
                None => 0.0,
            };
            Ok(Candidate {
                chunk_id,
                fused_score,
                dense_cosine,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::{build_pack, BuildMeta, SourceInput};
    use crate::chunk::ChunkConfig;
    use crate::embed::{l2_normalize, quantize_int8, Embedder, MockEmbedder};
    use crate::format::{Chunk, Doc};
    use crate::manifest::PackTier;
    use std::path::PathBuf;

    /// Mirrors `format.rs`'s/`build.rs`'s own test helper (this repo
    /// hand-rolls temp dirs instead of depending on `tempfile`); duplicated
    /// per-module since each module's `#[cfg(test)]` helper is private.
    fn unique_dir(name: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "cleophis-kpack-retrieve-test-{}-{}-{}",
            std::process::id(),
            unique,
            name
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const TEST_DIMS: usize = 8;

    fn sample_doc() -> Doc {
        Doc {
            id: 0,
            title: "Test Doc".to_string(),
            source_type: Some("md".to_string()),
            sha256: "deadbeef".repeat(8),
            source_path: None,
            source_size: Some(1234),
            source_mtime: None,
            extraction_quality: None,
            added_at: "2026-07-19T00:00:00Z".to_string(),
        }
    }

    // 1. rrf: two lanes -> correct fused order and scores; a chunk present
    // in BOTH lanes outranks one present in only a single lane, even at a
    // decent rank there.
    #[test]
    fn t1_rrf_fuses_two_lanes_and_multi_lane_membership_wins() {
        // dense (rank order): 10, 20, 30
        // lexical (rank order): 20, 40, 10
        let dense: [i64; 3] = [10, 20, 30];
        let lexical: [i64; 3] = [20, 40, 10];
        let k_rrf = 60.0;

        let fused = rrf(&[&dense, &lexical], k_rrf);

        let s10 = 1.0 / 61.0 + 1.0 / 63.0; // dense rank1 + lexical rank3
        let s20 = 1.0 / 62.0 + 1.0 / 61.0; // dense rank2 + lexical rank1
        let s30 = 1.0 / 63.0; // dense rank3 only
        let s40 = 1.0 / 62.0; // lexical rank2 only

        assert_eq!(fused.len(), 4);
        let expected = vec![(20, s20), (10, s10), (40, s40), (30, s30)];
        for (i, (id, score)) in fused.iter().enumerate() {
            assert_eq!(*id, expected[i].0, "position {i} chunk_id mismatch");
            assert!(
                (score - expected[i].1).abs() < 1e-12,
                "position {i} score mismatch: got {score}, expected {}",
                expected[i].1
            );
        }

        // Chunk 20 (both lanes) outranks chunk 30 (single lane, worse rank)
        // -- the multi-lane-membership property this test exists to lock.
        assert!(fused[0].1 > fused.iter().find(|(id, _)| *id == 30).unwrap().1);
    }

    // 1b. Deterministic tie-break: two chunks with an EXACTLY equal fused
    // score (each in exactly one single-item lane, both at rank 1) sort by
    // chunk_id ascending -- not by HashMap iteration order, which would
    // otherwise make this test flaky across runs.
    #[test]
    fn t1b_rrf_tie_break_is_chunk_id_ascending() {
        let lane_a: [i64; 1] = [100];
        let lane_b: [i64; 1] = [5];
        let fused = rrf(&[&lane_a, &lane_b], 60.0);
        assert_eq!(fused, vec![(5, 1.0 / 61.0), (100, 1.0 / 61.0)]);
    }

    // 2. fts5-safety: safe_fts5_query neutralizes fts5 metacharacters
    // (unbalanced quotes, OR, parens) so a query built from them does not
    // error `fts_search`'s MATCH, AND still finds a real match on the
    // literal words shared with a fixture chunk's text.
    #[test]
    fn t2_safe_fts5_query_handles_metacharacters_without_erroring() {
        let dir = unique_dir("t2-fts-safety");
        let path = dir.join("t2.kpack");
        let pack = Pack::open_or_create(&path, TEST_DIMS).unwrap();
        let doc_id = pack.insert_doc(&sample_doc()).unwrap();
        let chunk = Chunk {
            id: 0,
            doc_id,
            section_path: String::new(),
            locator: "test.md#L1".to_string(),
            prefix: String::new(),
            text: "he said this depends on x or y".to_string(),
            token_count: 7,
        };
        let chunk_id = pack.insert_chunk(&chunk).unwrap();

        // Raw query mixes fts5 operator keywords, parens, and an unbalanced
        // quote -- any of which would be a syntax error (or a meaning
        // change) if passed to fts5 MATCH unescaped.
        let raw_query = "he said \"x\" OR (y)";
        let safe = safe_fts5_query(raw_query);

        let results = pack.fts_search(&safe, 5);
        assert!(
            results.is_ok(),
            "safe fts5 query must not error: {:?}",
            results.err()
        );
        assert_eq!(
            results.unwrap(),
            vec![chunk_id],
            "the safe query's literal terms should still match the fixture chunk"
        );
    }

    fn test_meta() -> BuildMeta {
        BuildMeta {
            pack_id: "test-retrieve-pack".to_string(),
            pack_version: "2026.07.1".to_string(),
            pack_tier: PackTier::Personal,
            embedder_name: "mock-embedder-retrieve".to_string(),
            embedder_sha256: "mockhash0000000000000000000000000000000000000000000000000000"
                .to_string(),
            built_by: "device-builder-test".to_string(),
        }
    }

    // 3. retrieve_pack end-to-end with MockEmbedder: build a small two-doc
    // pack, retrieve against a query whose embedding is the Vitamin K
    // chunk's OWN stored passage vector (the mock is deterministic and has
    // no semantic properties, so pinning the dense query vector to a
    // specific chunk's own vector -- rather than trusting the mock's hash
    // to "understand" relatedness -- is the only way to deterministically
    // pin the dense lane; see embed.rs's MockEmbedder doc comment) and
    // whose text shares words with that same chunk (pins the lexical
    // lane). Both lanes then agree: candidates come back fused, every
    // dense_cosine lands in [-1, 1], the Vitamin K chunk ranks first (rank
    // 1 in both lanes is the maximum possible fused score, so this is
    // exact, not probabilistic), and its dense_cosine is the highest in
    // the set (self dot product is the max any vector can score against
    // it, by Cauchy-Schwarz).
    #[test]
    fn t3_retrieve_pack_end_to_end_with_mock_embedder() {
        let dir = unique_dir("t3-retrieve-e2e");
        let out_path = dir.join("e2e.kpack");
        let embedder = MockEmbedder::new(TEST_DIMS);
        let cfg = ChunkConfig::default();
        let meta = test_meta();

        let vitamin_k_text =
            "Vitamin K is a fat-soluble vitamin involved in blood clotting and bone metabolism.";
        let sources = vec![
            SourceInput {
                title: "Vitamin K".to_string(),
                source_type: "md".to_string(),
                content: format!("# Vitamin K\n\n{vitamin_k_text}\n"),
            },
            SourceInput {
                title: "Getting Started".to_string(),
                source_type: "md".to_string(),
                content: "# Getting Started\n\nRun the install script to set up the build tool on this machine.\n"
                    .to_string(),
            },
        ];

        build_pack(&sources, &embedder, &meta, &out_path, &cfg).unwrap();
        let pack = Pack::open(&out_path).unwrap();

        // Pin the dense lane: the exact quantized vector build_pack stored
        // for the Vitamin K chunk (same formula build.rs's own golden-pack
        // test uses to recompute a known stored vector).
        let raw = embedder.embed_passage(vitamin_k_text).unwrap();
        let query_i8 = quantize_int8(&l2_normalize(&raw));

        // Pin the lexical lane: shares real words with the Vitamin K
        // chunk, none with the Getting Started chunk.
        let query_text = "Vitamin K blood clotting";

        let candidates = retrieve_pack(&pack, query_text, &query_i8, 20).unwrap();
        assert!(!candidates.is_empty(), "expected at least one fused candidate");

        for c in &candidates {
            assert!(
                (-1.0..=1.0).contains(&c.dense_cosine),
                "dense_cosine out of range: {}",
                c.dense_cosine
            );
        }

        let vk_hits = pack.fts_search("clotting", 5).unwrap();
        assert_eq!(vk_hits.len(), 1, "fixture should have exactly one clotting-matching chunk");
        let vitamin_k_chunk_id = vk_hits[0];

        let top = &candidates[0];
        assert_eq!(
            top.chunk_id, vitamin_k_chunk_id,
            "the chunk ranking first in both lanes (dense: exact query-vector match; lexical: \
             shared words) must be the fused top candidate"
        );

        let max_cosine = candidates
            .iter()
            .map(|c| c.dense_cosine)
            .fold(f32::MIN, f32::max);
        assert_eq!(
            top.dense_cosine, max_cosine,
            "the exact-query-vector-match chunk's self dot product should be the highest cosine \
             in the candidate set"
        );
    }
}
