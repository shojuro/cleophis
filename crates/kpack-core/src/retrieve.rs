//! Per-pack retrieval + reciprocal-rank fusion (RRF) + the per-pack gate —
//! spec §4.1's first two slices. Composes `format::Pack`'s two independent
//! lanes (`vec_search` dense, `fts_search` lexical), fuses their rankings
//! with RRF, annotates every fused candidate with a dense cosine
//! similarity, and decides pack-level pass/fail against that pack's own
//! calibrated thresholds. Pure, network-free, and deliberately does NOT
//! fuse across packs or decide `NO_EVIDENCE` (R3) — this module produces
//! the per-pack fused, gated candidate list R3 builds on.
//!
//! ## Per-pack gate (Δ1, §4.1)
//! [`pack_passes_gate`] is the Δ1 safety mechanism: each mounted pack is
//! judged on ITS OWN calibrated thresholds (`gate_abs_floor`,
//! `gate_rel_margin`), evaluated on `dense_cosine` (not `fused_score`),
//! strictly BEFORE any cross-pack fusion. This ordering is safety-critical
//! — see the function's own doc comment for the full rationale on why
//! gating must happen per-pack, pre-fusion, rather than post-fusion on
//! "the winning pack's thresholds." [`pack_passes_gate_manifest`] is the
//! same decision read off a pack's [`crate::manifest::Manifest`] directly.
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
use crate::manifest::Manifest;
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
/// to the query (independent of fusion — [`pack_passes_gate`] scores on
/// this, not `fused_score`, per the plan's decision to gate on dense
/// cosine).
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
/// Does not itself apply [`pack_passes_gate`] or fuse across packs (R3) —
/// callers run the gate over this function's output.
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

/// Spec §4.1's Δ1 safety mechanism — the per-pack calibrated gate,
/// evaluated on **dense cosine** (`dense_cosine`, an absolute cosine
/// similarity), never on `fused_score` (an RRF sum with no fixed scale, so
/// there's no meaningful "floor" to compare it against).
///
/// This is deliberately the *only* place a pack's candidates get judged
/// against ITS OWN thresholds, and it runs strictly BEFORE any cross-pack
/// fusion (R3). That ordering is the whole point (§4.1's rationale,
/// restated): gating after cross-pack fusion on "the winning pack's
/// thresholds" would let a weak chunk from a loosely-gated pack (e.g. an
/// uncalibrated personal pack, §3.4) win the fused ranking and be judged by
/// the loose gate — bypassing a strictly-calibrated pack's floor exactly on
/// the queries where that matters. Gating each pack independently, before
/// fusion, guarantees the loosest-calibrated mounted pack can never lower
/// the safety floor for a stricter pack's domain. A pack that fails its own
/// gate contributes NOTHING to the fused result — not even at a
/// diminished weight.
///
/// Does not fuse across packs or decide `NO_EVIDENCE` (R3's job) — this is
/// only the single-pack pass/fail decision.
///
/// ## Rule
/// 1. Empty `candidates` → `false` (nothing to pass a gate with).
/// 2. `top1` = the maximum `dense_cosine` among `candidates` (ranking is
///    done internally on `dense_cosine`; caller order is irrelevant).
/// 3. **Absolute floor:** `top1 >= abs_floor`.
/// 4. **Relative margin:** let `cos_at_rank_min10` be the `dense_cosine` at
///    rank `min(10, n)` (1-based, descending — the 10th-highest score, or
///    the lowest-ranked candidate's score if `n < 10`); require `top1 -
///    cos_at_rank_min10 >= rel_margin`.
/// 5. **Special case, `n == 1`:** the single candidate passes on the
///    absolute floor alone — there is no rank-10 (or any lower rank) to
///    compare against, so the margin check is skipped entirely rather than
///    trivially comparing `top1` against itself (which would always be a
///    margin of `0.0` and could never pass a positive `rel_margin`).
/// 6. For `1 < n < 10`, "rank `min(10, n)`" clamps to `n` — the LOWEST
///    cosine among the candidates — so the margin is `top1` minus the
///    worst-ranked candidate's cosine.
///
/// Both checks are inclusive (`>=`): a pack sitting exactly on either
/// threshold passes.
pub fn pack_passes_gate(candidates: &[Candidate], abs_floor: f64, rel_margin: f64) -> bool {
    if candidates.is_empty() {
        return false;
    }

    // Rank descending by dense_cosine -- independent of input order, per
    // the gate's contract (it ranks internally, never trusts caller order).
    let mut cosines: Vec<f64> = candidates.iter().map(|c| c.dense_cosine as f64).collect();
    cosines.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));

    let top1 = cosines[0];
    if top1 < abs_floor {
        return false;
    }

    // n == 1: absolute floor alone, no margin check (there is no rank-10 /
    // lower rank to compare top1 against).
    if cosines.len() == 1 {
        return true;
    }

    // rank min(10, n), 1-based -> 0-based index min(10, n) - 1. For 1 < n <
    // 10 this clamps to n - 1, the last (lowest) cosine.
    let rank_idx = cosines.len().min(10) - 1;
    let cos_at_rank_min10 = cosines[rank_idx];

    (top1 - cos_at_rank_min10) >= rel_margin
}

/// Convenience wrapper over [`pack_passes_gate`] that pulls `abs_floor` /
/// `rel_margin` from a mounted pack's own [`Manifest`] (`gate_abs_floor`,
/// `gate_rel_margin`, §1.2) so callers don't have to re-plumb the two
/// thresholds separately from the manifest they already have in hand. The
/// raw-threshold form above stays the unit-testable core; this is a thin
/// pass-through.
pub fn pack_passes_gate_manifest(candidates: &[Candidate], manifest: &Manifest) -> bool {
    pack_passes_gate(candidates, manifest.gate_abs_floor, manifest.gate_rel_margin)
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

    /// Builds a `Candidate` for gate tests where only `dense_cosine`
    /// matters -- `chunk_id` and `fused_score` are placeholders the gate
    /// never reads.
    fn candidate(chunk_id: i64, dense_cosine: f32) -> Candidate {
        Candidate {
            chunk_id,
            fused_score: 0.0,
            dense_cosine,
        }
    }

    /// A minimal but fully valid `Manifest` for `pack_passes_gate_manifest`
    /// tests -- every field besides the two gate thresholds is an arbitrary
    /// valid placeholder.
    fn test_manifest(abs_floor: f64, rel_margin: f64) -> Manifest {
        Manifest {
            pack_id: "gate-test-pack".to_string(),
            pack_version: "2026.07.1".to_string(),
            pack_tier: PackTier::Personal,
            embedder_name: "mock-embedder-gate".to_string(),
            embedder_sha256: "deadbeef".repeat(8),
            embedding_dims: TEST_DIMS as u32,
            embedding_quant: "int8".to_string(),
            chunk_target_tokens: 400,
            chunk_overlap_pct: 18,
            gate_abs_floor: abs_floor,
            gate_rel_margin: rel_margin,
            gate_calibrated: false,
            prefixes_present: false,
            built_by: "device-builder-test".to_string(),
            license_ref: None,
            schema_version: crate::format::SCHEMA_VERSION,
            vec_format_version: crate::manifest::VEC_FORMAT_VERSION.to_string(),
        }
    }

    // 4. Gate: all strong (top1 well above floor, wide margin over
    // rank-10) -> passes. 12 candidates, evenly spread, floor and margin
    // both comfortably cleared.
    #[test]
    fn t4_gate_all_strong_passes() {
        let cosines: [f32; 12] = [
            0.95, 0.93, 0.91, 0.89, 0.87, 0.85, 0.83, 0.81, 0.79, 0.77, 0.75, 0.73,
        ];
        let candidates: Vec<Candidate> = cosines
            .iter()
            .enumerate()
            .map(|(i, &c)| candidate(i as i64, c))
            .collect();
        // top1 = 0.95, rank-10 (10th highest, index 9) = 0.77, margin = 0.18.
        assert!(pack_passes_gate(&candidates, 0.5, 0.05));
    }

    // 5. Gate: top1 below abs_floor -> fails, even with a big margin.
    #[test]
    fn t5_gate_top1_below_floor_fails_despite_big_margin() {
        let candidates = vec![candidate(1, 0.3), candidate(2, 0.05)];
        // margin = 0.3 - 0.05 = 0.25, well over rel_margin -- but top1 =
        // 0.3 < abs_floor = 0.5, so the pack must still fail.
        assert!(!pack_passes_gate(&candidates, 0.5, 0.1));
    }

    // 6. Gate: top1 above floor but (top1 - cos@rank10) < rel_margin (flat
    // distribution) -> fails.
    #[test]
    fn t6_gate_flat_distribution_fails_margin() {
        let cosines: [f32; 10] = [0.82, 0.815, 0.81, 0.805, 0.80, 0.80, 0.80, 0.80, 0.80, 0.80];
        let candidates: Vec<Candidate> = cosines
            .iter()
            .enumerate()
            .map(|(i, &c)| candidate(i as i64, c))
            .collect();
        // top1 = 0.82 >= floor 0.5 (passes the floor), rank-10 (index 9) =
        // 0.80, margin = 0.02 < rel_margin 0.05 -- fails on margin alone.
        assert!(!pack_passes_gate(&candidates, 0.5, 0.05));
    }

    // 7. Boundary: top1 exactly == abs_floor and margin exactly ==
    // rel_margin -> passes (>= is inclusive on both checks). Also asserts
    // the boundary direction explicitly: nudging either value the wrong
    // side of its threshold by an epsilon flips the verdict to fail.
    #[test]
    fn t7_gate_boundary_inclusive_both_directions() {
        // Dyadic (exact-binary-fraction) thresholds and endpoints, so the
        // f32 dense_cosine -> f64 comparison boundary is bit-exact rather
        // than landing a hair off from float rounding (0.1 and friends
        // aren't exactly representable in binary and would make an
        // "exactly on the boundary" test flaky by construction).
        let abs_floor = 0.5; // 2^-1
        let rel_margin = 0.125; // 2^-3
        // 10 candidates, descending from top1 = 0.5 to rank-10 = 0.375:
        // margin = 0.5 - 0.375 = 0.125, exactly rel_margin. The 8 interior
        // values only need to sort between the two endpoints -- they're
        // never compared to a threshold directly.
        let cosines: [f32; 10] = [0.5, 0.49, 0.48, 0.47, 0.46, 0.45, 0.44, 0.43, 0.42, 0.375];
        let candidates: Vec<Candidate> = cosines
            .iter()
            .enumerate()
            .map(|(i, &c)| candidate(i as i64, c))
            .collect();
        assert!(
            pack_passes_gate(&candidates, abs_floor, rel_margin),
            "exact boundary on both checks must pass (>= is inclusive)"
        );

        // Nudge top1 just under abs_floor -> must now fail.
        let mut below_floor = candidates.clone();
        below_floor[0].dense_cosine = 0.499;
        assert!(
            !pack_passes_gate(&below_floor, abs_floor, rel_margin),
            "top1 just below abs_floor must fail"
        );

        // Nudge rank-10 up so the margin is just under rel_margin -> must
        // now fail (top1 unchanged at exactly the floor).
        let mut below_margin = candidates.clone();
        below_margin[9].dense_cosine = 0.376;
        assert!(
            !pack_passes_gate(&below_margin, abs_floor, rel_margin),
            "margin just below rel_margin must fail"
        );
    }

    // 8. Single candidate above floor passes (no margin check); single
    // candidate below floor fails.
    #[test]
    fn t8_gate_single_candidate_floor_only() {
        assert!(
            pack_passes_gate(&[candidate(1, 0.9)], 0.5, 0.9),
            "single candidate above floor passes on the floor alone, \
             regardless of how strict rel_margin is -- there's no rank-10 \
             to compare against"
        );
        assert!(
            !pack_passes_gate(&[candidate(1, 0.4)], 0.5, 0.0),
            "single candidate below floor fails even with rel_margin = 0.0"
        );
    }

    // 9. n between 2 and 9: margin compares top1 to the LOWEST cosine
    // (rank clamps to last, n-1 in 0-based terms). One case that passes,
    // one that fails on exactly that comparison.
    #[test]
    fn t9_gate_small_n_margin_clamps_to_lowest() {
        let passing = vec![
            candidate(1, 0.9),
            candidate(2, 0.85),
            candidate(3, 0.8),
            candidate(4, 0.75),
            candidate(5, 0.6),
        ];
        // n = 5 < 10, so rank_min(10,5) = rank 5 = the lowest (0.6).
        // margin = 0.9 - 0.6 = 0.3 >= rel_margin 0.2 -> passes.
        assert!(pack_passes_gate(&passing, 0.5, 0.2));

        let failing = vec![
            candidate(1, 0.9),
            candidate(2, 0.85),
            candidate(3, 0.8),
            candidate(4, 0.75),
            candidate(5, 0.75),
        ];
        // Same shape, but the lowest cosine is raised to 0.75: margin =
        // 0.9 - 0.75 = 0.15 < rel_margin 0.2 -> fails, even though top1
        // clears the floor easily.
        assert!(!pack_passes_gate(&failing, 0.5, 0.2));
    }

    // 10. pack_passes_gate_manifest reads the manifest's gate_abs_floor /
    // gate_rel_margin and agrees with the raw-threshold form, both for a
    // passing and a failing candidate set.
    #[test]
    fn t10_gate_manifest_form_agrees_with_raw_form() {
        let manifest = test_manifest(0.5, 0.1);

        let strong = vec![
            candidate(1, 0.9),
            candidate(2, 0.6),
            candidate(3, 0.2),
        ];
        assert_eq!(
            pack_passes_gate_manifest(&strong, &manifest),
            pack_passes_gate(&strong, manifest.gate_abs_floor, manifest.gate_rel_margin)
        );
        assert!(pack_passes_gate_manifest(&strong, &manifest));

        let weak = vec![candidate(1, 0.3)];
        assert_eq!(
            pack_passes_gate_manifest(&weak, &manifest),
            pack_passes_gate(&weak, manifest.gate_abs_floor, manifest.gate_rel_margin)
        );
        assert!(!pack_passes_gate_manifest(&weak, &manifest));
    }

    // 11. Ordering independence: the same candidate set in a different
    // input order gives the same verdict -- the gate ranks internally by
    // dense_cosine and never trusts input order.
    #[test]
    fn t11_gate_ordering_independence() {
        let in_order = vec![
            candidate(1, 0.9),
            candidate(2, 0.85),
            candidate(3, 0.8),
            candidate(4, 0.75),
            candidate(5, 0.6),
        ];
        let mut shuffled = in_order.clone();
        shuffled.reverse();
        // Also scramble beyond a plain reversal.
        shuffled.swap(0, 2);
        shuffled.swap(1, 4);

        let abs_floor = 0.5;
        let rel_margin = 0.2;
        assert_eq!(
            pack_passes_gate(&in_order, abs_floor, rel_margin),
            pack_passes_gate(&shuffled, abs_floor, rel_margin),
        );
        assert!(pack_passes_gate(&in_order, abs_floor, rel_margin));
    }
}
