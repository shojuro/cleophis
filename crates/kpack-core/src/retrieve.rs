//! Per-pack retrieval + reciprocal-rank fusion (RRF) + the per-pack gate +
//! cross-pack fusion + `NO_EVIDENCE` + tier select/dedupe + prompt assembly —
//! spec §4.1–4.2. Composes `format::Pack`'s two independent lanes
//! (`vec_search` dense, `fts_search` lexical), fuses their rankings with
//! RRF, annotates every fused candidate with a dense cosine similarity,
//! decides pack-level pass/fail against that pack's own calibrated
//! thresholds, cross-pack-fuses only the survivors, and renders the K7
//! prompt contract's numbered-source block into the final grounded prompt.
//! Pure, network-free. [`retrieve`] is spec §4's top-level entry point:
//! embed the query, call [`retrieve_pack`] per already-mounted pack to
//! build the [`PackHit`]s [`assemble`] consumes, then hand off to
//! [`assemble`]. Mounting the packs themselves (opening `.kpack` files,
//! running the load-time integrity gate) stays outside this pure,
//! network-free module — that's the Tauri command layer's job
//! (`src-tauri/src/kpack.rs`'s `rag_query` command).
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
//! [`assemble`] is where this ordering is enforced end to end: it gates
//! every [`PackHit`] independently (step 1) and only THEN cross-pack-fuses
//! the survivors (step 3) — see [`assemble`]'s own doc comment.
//!
//! ## RRF (reciprocal rank fusion)
//! [`rrf`] implements the standard formula: a chunk's fused score is the
//! sum, over every lane it appears in, of `1.0 / (k_rrf + rank)`, where
//! `rank` is 1-based position in that lane's ranked list. A chunk present
//! in multiple lanes accumulates a contribution from each — so a mediocre
//! rank in two lanes can (and by design, should) outscore a great rank in
//! only one. [`DEFAULT_K_RRF`] (60.0) is spec §4.1's default; it damps the
//! influence of rank 1 vs. rank 2 (a smaller `k_rrf` makes top ranks
//! dominate more sharply). [`assemble`]'s cross-pack fusion step reuses this
//! same formula (via a private tuple-keyed analog, since a bare `chunk_id`
//! is only unique WITHIN a pack) over each surviving pack's already-fused
//! candidate list as one lane per pack.
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
use crate::contract::{self, RenderChunk};
use crate::embed::{dot_int8, l2_normalize, quantize_int8, Embedder};
use crate::format::{self, Pack};
use crate::manifest::{Manifest, PackTier};
use std::fmt;

/// Reciprocal-rank fusion's default `k_rrf` (spec §4.1).
pub const DEFAULT_K_RRF: f64 = 60.0;

/// RAG-quality quick win: a LENIENT interim runtime gate floor for
/// PERSONAL-tier packs only, overriding whatever `gate_abs_floor` was baked
/// into that pack's manifest at build time. The baked personal-pack default
/// (`build::PLACEHOLDER_GATE_ABS_FLOOR`, 0.5) turned out to over-reject
/// reasonable queries over a user's own PDF pack (e.g. "what is the title?"
/// → `NoEvidence`) — the user wants the model to at least TRY on personal
/// content. This is an override applied at query time in [`assemble`], not a
/// change to the build-time constant, so EXISTING already-built personal
/// packs benefit immediately without a rebuild. Curated packs are
/// unaffected — they keep using their own manifest's calibrated thresholds
/// (§2.4/§3.4).
///
/// Still heuristic, NOT calibrated: 0.30 was chosen to still reject the
/// §4-measured out-of-scope case (the Mongolia-vs-hemostasis query, cosine
/// 0.2746), while admitting the in-scope queries the 0.5 placeholder was
/// wrongly rejecting. The real fixes are §2.4/§3.4's calibration work and
/// the contract-trained adapter track (parked, not this task) — this is
/// only a quick retrieval-side win in the meantime.
pub const PERSONAL_RUNTIME_GATE_ABS_FLOOR: f64 = 0.30;
/// RAG-quality quick win: the paired relative-margin override for
/// PERSONAL-tier packs — see [`PERSONAL_RUNTIME_GATE_ABS_FLOOR`]'s doc
/// comment for the full rationale. `0.0` makes personal packs effectively
/// abs-floor-only: the relative-margin check (`top1` vs. the rank-`min(10,
/// n)` cosine) is a calibration-dependent heuristic that adds false
/// refusals for personal use, where a small, uncalibrated pack's score
/// distribution has no reason to resemble a curated pack's. Still heuristic,
/// NOT calibrated — same caveat as the floor above.
pub const PERSONAL_RUNTIME_GATE_REL_MARGIN: f64 = 0.0;

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

    // An empty/whitespace-only query_text yields "" from safe_fts5_query (no
    // terms to quote); fts5's MATCH treats "" as a syntax error, not "match
    // nothing". Skip the lexical lane entirely in that case rather than
    // calling fts_search with an empty pattern -- the dense lane still runs
    // and RRF over a single lane is well-defined.
    let safe_q = safe_fts5_query(query_text);
    let lexical: Vec<i64> = if safe_q.is_empty() {
        Vec::new()
    } else {
        pack.fts_search(&safe_q, k)?
    };

    let fused = rrf(&[&dense_ids, &lexical], DEFAULT_K_RRF);

    fused
        .into_iter()
        .map(|(chunk_id, fused_score)| {
            // TODO(§5-perf): batch cosine lookups -- this issues one
            // get_embedding SELECT per fused candidate (N+1); fine for
            // correctness, a latency item for §5's floor-machine benchmark.
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
/// 1b. Any non-finite (`NaN`) `dense_cosine` among `candidates`, at any
///     rank → `false`. A `NaN` is a corrupt/degenerate input (unreachable
///     via `retrieve_pack` today — cosines are always finite via the
///     `/INT8_DOT_SCALE` clamp — but the gate refuses on it regardless of
///     provenance); this is checked BEFORE the `n == 1` special case below,
///     so a single `NaN` candidate cannot fail-open.
/// 2. `top1` = the maximum `dense_cosine` among `candidates` (ranking is
///    done internally on `dense_cosine`; caller order is irrelevant).
/// 3. **Absolute floor:** `top1 >= abs_floor` (a `NaN`/negative-infinity
///    `abs_floor` is never satisfiable, so it also fails closed).
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

    // A non-finite cosine (NaN) ANYWHERE in the set -- not just at top1 --
    // is a corrupt/degenerate input. partial_cmp has no total order for
    // NaN, so sort_by's `unwrap_or(Equal)` fallback below does not
    // guarantee a NaN sorts to position 0; checking only `cosines[0]` after
    // sorting could let a buried NaN dodge the floor check entirely. Bail
    // explicitly, before sorting, so this holds for every n.
    if cosines.iter().any(|c| !c.is_finite()) {
        return false;
    }

    cosines.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));

    let top1 = cosines[0];
    // `!(top1 >= abs_floor)` rather than `top1 < abs_floor`: the two are
    // equivalent for finite top1 (guaranteed here, having passed the
    // is_finite check above), but written this way so a NaN abs_floor
    // itself also fails closed: `top1 >= NaN` is always false, so `!(...)`
    // is true and this refuses, whereas `top1 < NaN` is ALSO always false
    // and would wrongly fall through to the n==1 early return below.
    if !(top1 >= abs_floor) {
        return false;
    }

    // n == 1: absolute floor alone, no margin check (there is no rank-10 /
    // lower rank to compare top1 against). Reached only once top1 is known
    // finite and >= abs_floor, so this can no longer fail-open on NaN.
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

/// Retrieval "budget" tiers (spec §4.3) driving how many chunks
/// [`assemble`] selects into the final grounded prompt: [`Tier::Small`] is
/// the constrained 1–2 GB on-device tier, [`Tier::Large`] the roomier tier
/// on more capable hardware.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Small,
    Large,
}

impl Tier {
    /// The maximum number of chunks [`assemble`] selects for this tier
    /// (spec §4.3): 3 for [`Tier::Small`], 5 for [`Tier::Large`].
    pub fn n(&self) -> usize {
        match self {
            Tier::Small => 3,
            Tier::Large => 5,
        }
    }
}

/// One mounted pack's contribution to a single query: the pack itself, its
/// manifest (for the gate thresholds, §4.1, and `pack_id`), and R1's
/// [`retrieve_pack`] output for this query — `candidates` already in that
/// pack's own per-pack fused rank order. Building these (mounting packs,
/// embedding the query, calling `retrieve_pack` per pack) is R4's job;
/// [`assemble`] only consumes them.
pub struct PackHit<'a> {
    pub pack: &'a Pack,
    pub manifest: &'a Manifest,
    pub candidates: Vec<Candidate>,
}

/// One displayed citation in a [`RetrievalResult::Grounded`] prompt: the
/// numbered-source mapping a chat UI (§7) shows next to the model's answer.
/// `n` is the citation's 1-based number, matching its position in the
/// rendered `[n] (...)` line of the sources block.
#[derive(Debug, Clone, PartialEq)]
pub struct Citation {
    pub n: usize,
    pub pack_id: String,
    pub chunk_id: i64,
    pub doc_title: String,
    pub section_path: String,
    pub locator: String,
}

/// The outcome of a full retrieval + assembly pass (spec §4.1–4.2):
/// either a grounded prompt with its citations, or
/// [`RetrievalResult::NoEvidence`] when no mounted pack passed its own gate
/// — the anti-hallucination contract this whole pipeline exists for: the
/// runtime never fabricates a citation for a query nothing supports.
#[derive(Debug, Clone, PartialEq)]
pub enum RetrievalResult {
    Grounded { prompt: String, citations: Vec<Citation> },
    /// This variant is intentionally marker-free: the CALLER (R4/§3) is
    /// responsible for emitting the contract's [`contract::no_evidence_marker`]
    /// text for this case; the marker itself lives in the contract, not here.
    NoEvidence,
}

/// A [`rrf`] analog keyed on the cross-pack identity `(pack_index,
/// chunk_id)` rather than a bare `chunk_id` — a `chunk_id` is only unique
/// WITHIN one pack's `.kpack` file, so [`assemble`]'s cross-pack fusion
/// (step 3) needs the pack index folded into the key to keep two different
/// packs' chunk 1 from colliding. Identical formula and tie-break to
/// [`rrf`] (deterministic: ties break ascending on the key tuple); kept as
/// a small private duplicate rather than generalizing the public `rrf` over
/// a type parameter, so R1's public signature and tests stay untouched.
fn cross_pack_rrf(lanes: &[&[(usize, i64)]], k_rrf: f64) -> Vec<((usize, i64), f64)> {
    let mut scores: std::collections::HashMap<(usize, i64), f64> = std::collections::HashMap::new();
    for lane in lanes {
        for (idx, &key) in lane.iter().enumerate() {
            let rank = (idx + 1) as f64; // 1-based
            *scores.entry(key).or_insert(0.0) += 1.0 / (k_rrf + rank);
        }
    }
    let mut fused: Vec<((usize, i64), f64)> = scores.into_iter().collect();
    fused.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    fused
}

/// Spec §4.1–4.2's cross-pack pipeline, from the per-pack gate through the
/// final grounded prompt.
///
/// 1. **Per-pack gate (Δ1 — BEFORE any cross-pack mixing):** keep only
///    `hits` where [`pack_passes_gate`] is true for that pack's OWN
///    `candidates`, evaluated against that pack's EFFECTIVE thresholds:
///    [`PERSONAL_RUNTIME_GATE_ABS_FLOOR`]/[`PERSONAL_RUNTIME_GATE_REL_MARGIN`]
///    for a [`crate::manifest::PackTier::Personal`] pack (a runtime override
///    of its baked manifest values — see those constants' doc comments),
///    otherwise the pack's own `manifest.gate_abs_floor`/`gate_rel_margin`
///    unchanged. A failing pack contributes NOTHING — not even at a
///    diminished weight — to anything that follows. This is the Δ1 ordering
///    [`pack_passes_gate`]'s doc comment explains in full: gating each pack
///    independently, before fusion, guarantees the loosest-gated mounted
///    pack can never lower the safety floor for a stricter pack's domain.
/// 2. **`NO_EVIDENCE`:** if no pack passes its gate, return
///    [`RetrievalResult::NoEvidence`] — no prompt, no citations.
/// 3. **Cross-pack RRF over gate-passers only:** each surviving pack
///    contributes one lane — its `candidates`' `chunk_id`s, in that pack's
///    own already-fused rank order, paired with `hits`' index for that pack
///    so `(pack_index, chunk_id)` is a stable global identity. Fused via
///    [`cross_pack_rrf`] (same formula as [`rrf`], k = [`DEFAULT_K_RRF`]).
///    This fused ranking is **NOT re-gated** on any pack's thresholds —
///    gating already happened, per pack, in step 1; a pack that never
///    passed its own gate was never a candidate for this step regardless of
///    what its (hypothetical) fused score would have been.
/// 4. **Tier select + dedupe:** walk the unified ranking top-down,
///    selecting up to `tier.n()` chunks. Dedupe key: `(pack_index, doc_id,
///    section_path)` — a v1 approximation of "same doc, overlapping
///    window" (see the `TODO(§4-refine)` below); the higher-ranked chunk of
///    an overlapping pair wins since the walk is top-down and a later
///    duplicate key is simply skipped. `doc_id` comes from
///    `pack.get_chunk(chunk_id)`.
/// 5. **Assemble:** for each selected chunk (in rank order), fetch its doc
///    title (`get_doc(chunk.doc_id)`), build a `contract::RenderChunk`, and
///    render the numbered sources block via `contract::render_sources`. Also
///    collect the DISTINCT, non-empty titles of those same resolved docs
///    (first-seen order) into a short `doc_context` line ("These sources are
///    excerpts from: …") — a RUNTIME addition (not part of the versioned
///    contract) that gives the model the document's identity as legitimate
///    context, so "what is the title / what is this about" is answerable
///    even though a title never appears in a chunk's searchable body text.
///    The final `prompt` is `contract::system_contract()` (trailing
///    whitespace trimmed, so the layout doesn't depend on the contract
///    file's exact trailing newline) + a blank line + `doc_context` (when
///    non-empty) + a blank line + the rendered sources block; when
///    `doc_context` is empty (no resolved doc had a non-empty title), the
///    layout falls back to the original two-part form — no dangling blank
///    section. `citations` mirrors the rendered sources 1:1, `n` matching
///    each line's `[n]`.
pub fn assemble(hits: &[PackHit<'_>], tier: Tier) -> Result<RetrievalResult> {
    // 1. Per-pack gate -- Δ1: every mounted pack judged on its OWN
    // thresholds, strictly before any cross-pack mixing. Index is `hits`'
    // own index, not a re-numbering of the survivors -- kept stable so step
    // 3's (pack_index, chunk_id) identity can always be traced back to
    // `hits[pack_index]` in steps 4-5.
    //
    // RAG-quality quick win: PERSONAL-tier packs use the lenient RUNTIME
    // override thresholds ([`PERSONAL_RUNTIME_GATE_ABS_FLOOR`]/
    // [`PERSONAL_RUNTIME_GATE_REL_MARGIN`]) instead of whatever was baked
    // into their manifest at build time -- see those constants' doc
    // comments. CURATED packs are unaffected: they still read their own
    // manifest's calibrated `gate_abs_floor`/`gate_rel_margin`. Only the
    // threshold VALUES differ by tier; the gate-before-fusion ORDERING
    // (Δ1) and the NaN-fail-closed rule inside `pack_passes_gate` are
    // untouched.
    let passing: Vec<(usize, &PackHit<'_>)> = hits
        .iter()
        .enumerate()
        .filter(|(_, hit)| {
            let (abs_floor, rel_margin) = if hit.manifest.pack_tier == PackTier::Personal {
                (PERSONAL_RUNTIME_GATE_ABS_FLOOR, PERSONAL_RUNTIME_GATE_REL_MARGIN)
            } else {
                (hit.manifest.gate_abs_floor, hit.manifest.gate_rel_margin)
            };
            pack_passes_gate(&hit.candidates, abs_floor, rel_margin)
        })
        .collect();

    // 2. NO_EVIDENCE: no mounted pack passed its own gate.
    if passing.is_empty() {
        return Ok(RetrievalResult::NoEvidence);
    }

    // 3. Cross-pack RRF over gate-passers ONLY. One lane per surviving
    // pack; a pack that failed its own gate in step 1 contributes no lane
    // at all, so its candidates cannot be rescued by outscoring a
    // gate-passing pack's chunk here -- the fusion happens strictly after,
    // and only among, packs that already cleared their OWN floor.
    let lanes: Vec<Vec<(usize, i64)>> = passing
        .iter()
        .map(|(pack_index, hit)| {
            hit.candidates
                .iter()
                .map(|c| (*pack_index, c.chunk_id))
                .collect()
        })
        .collect();
    let lane_refs: Vec<&[(usize, i64)]> = lanes.iter().map(Vec::as_slice).collect();
    let unified = cross_pack_rrf(&lane_refs, DEFAULT_K_RRF);

    // 4. Tier select + dedupe, walking the unified ranking top-down.
    //
    // TODO(§4-refine): true window-overlap dedup. This keys on exact
    // `(pack_index, doc_id, section_path)` equality, not actual
    // locator/window overlap -- two DIFFERENT, non-overlapping chunks that
    // happen to share a `section_path` would be wrongly deduped, and two
    // truly-overlapping chunks with different `section_path` labels would
    // wrongly both survive. Good enough for v1 pending real overlap-range
    // tracking in the chunker; documented here rather than silently
    // approximated.
    let mut selected: Vec<(usize, format::Chunk)> = Vec::with_capacity(tier.n());
    let mut seen: std::collections::HashSet<(usize, i64, String)> = std::collections::HashSet::new();

    for ((pack_index, chunk_id), _score) in unified {
        if selected.len() >= tier.n() {
            break;
        }
        let hit = &hits[pack_index];
        // A missing chunk (get_chunk returning None) would be a
        // build-pipeline invariant violation -- this chunk_id came straight
        // out of THIS SAME pack's own candidates -- so it's skipped
        // defensively rather than erroring the whole assemble call (same
        // "shouldn't happen, degrade gracefully" posture as
        // retrieve_pack's missing-embedding case).
        let Some(chunk) = hit.pack.get_chunk(chunk_id)? else {
            continue;
        };
        let key = (pack_index, chunk.doc_id, chunk.section_path.clone());
        if !seen.insert(key) {
            continue; // a higher-ranked chunk already claimed this (pack, doc, section)
        }
        selected.push((pack_index, chunk));
    }

    // 5. Assemble: resolve each selected chunk's doc (for its title),
    // keeping chunk+doc+pack_index aligned so the RenderChunk/Citation
    // built below stay in the same rank order.
    let mut resolved: Vec<(usize, format::Chunk, format::Doc)> = Vec::with_capacity(selected.len());
    for (pack_index, chunk) in selected {
        let hit = &hits[pack_index];
        // Same defensive posture as the chunk fetch above -- the schema's
        // `doc_id INTEGER NOT NULL REFERENCES docs(id)` means a real
        // chunk's doc should always resolve; skip rather than error if it
        // somehow doesn't.
        if let Some(doc) = hit.pack.get_doc(chunk.doc_id)? {
            resolved.push((pack_index, chunk, doc));
        }
    }

    // Fix 1: if every selected chunk failed to resolve (its doc/chunk row
    // is missing -- e.g. a build-pipeline invariant violation catching up
    // with us), `resolved` is empty and there is nothing to cite. Falling
    // through would render a Grounded prompt whose sources block is empty
    // while still instructing the model to cite `[n]` -- a citation-less
    // "grounded" prompt undermines the exact anti-hallucination invariant
    // this module exists to guarantee. NO_EVIDENCE instead.
    if resolved.is_empty() {
        return Ok(RetrievalResult::NoEvidence);
    }

    let render_chunks: Vec<RenderChunk<'_>> = resolved
        .iter()
        .map(|(_, chunk, doc)| RenderChunk {
            source_title: &doc.title,
            section_path: &chunk.section_path,
            locator: &chunk.locator,
            text: &chunk.text,
        })
        .collect();
    let sources_block = contract::render_sources(&render_chunks);

    // RAG-quality quick win: the book/document's TITLE lives in doc
    // metadata (`doc.title`, already shown in each citation), not the
    // searchable body text -- so retrieval alone can never surface it as a
    // "match", and a query like "what is the title?" / "what is this
    // about?" would otherwise have nothing to ground on. Collect the
    // DISTINCT titles of the docs actually cited (the same docs already
    // resolved for `render_chunks`/citations above), preserving first-seen
    // order, and state them as a short context line the model can answer
    // identity questions from -- without touching the numbered-sources
    // semantics `render_sources` owns.
    let mut titles: Vec<&str> = Vec::new();
    for (_, _, doc) in &resolved {
        let title = doc.title.trim();
        if !title.is_empty() && !titles.contains(&title) {
            titles.push(title);
        }
    }
    let doc_context = if titles.is_empty() {
        String::new()
    } else {
        format!("These sources are excerpts from: {}.", titles.join("; "))
    };

    // TODO(adapter): `doc_context` is a runtime-only addition, not part of
    // the versioned prompt contract (`contracts/prompt-contract.v1.toml` /
    // `contract::system_contract()`) -- when the contract-trained adapter
    // track starts, fold this doc-identity line into the versioned contract
    // so the adapter is trained on the same format the runtime actually
    // sends, rather than this drifting silently ahead of it.
    let prompt = if doc_context.is_empty() {
        format!("{}\n\n{}", contract::system_contract().trim_end(), sources_block)
    } else {
        format!(
            "{}\n\n{}\n\n{}",
            contract::system_contract().trim_end(),
            doc_context,
            sources_block
        )
    };

    let citations: Vec<Citation> = resolved
        .iter()
        .enumerate()
        .map(|(i, (pack_index, chunk, doc))| Citation {
            n: i + 1,
            pack_id: hits[*pack_index].manifest.pack_id.clone(),
            chunk_id: chunk.id,
            doc_title: doc.title.clone(),
            section_path: chunk.section_path.clone(),
            locator: chunk.locator.clone(),
        })
        .collect();

    Ok(RetrievalResult::Grounded { prompt, citations })
}

/// Spec §4's top-level retrieval entry point — the one function a caller
/// (the Tauri `rag_query` command, `src-tauri/src/kpack.rs`) needs to go
/// from a raw query string and a list of already-mounted packs to a
/// [`RetrievalResult`]. Composes the rest of this module: embeds the query
/// once for the DENSE lane, then runs [`retrieve_pack`] per pack and hands
/// the results to [`assemble`].
///
/// 1. **Dense-lane query embedding:** `embedder.embed_query(query)` — the
///    `Embedder` trait's own contract is that implementations apply the BGE
///    instruction prefix internally (via `embed::query_input`), so this
///    function never re-applies it — then [`l2_normalize`] →
///    [`quantize_int8`] → `query_i8`. This mirrors, step for step, the
///    build-time pipeline a stored passage vector went through (spec §1.4),
///    so `query_i8` is directly comparable to what
///    `format::Pack::vec_search`/`get_embedding` score against.
/// 2. **Lexical-lane query text:** the RAW `query` string, unmodified — NOT
///    the BGE-prefixed instruction form `embed_query` embeds. `fts_search`'s
///    bm25 match is against the user's own words; prefixing it with
///    "Represent this sentence for searching relevant passages: " would
///    only inject noise tokens into a lexical match that has no notion of
///    "instruction" framing (see [`retrieve_pack`]'s own doc comment for how
///    that raw text is made fts5-safe).
/// 3. **Per-pack retrieval:** for each `(pack, manifest)` in `packs`, calls
///    [`retrieve_pack`] with `k=20` (spec §4.1's fixed dense/lexical fan-out
///    width — hardcoded here, the one production call site, rather than
///    threaded through as a parameter, so it can't drift between callers)
///    and wraps the result in a [`PackHit`].
/// 4. **Assemble:** [`assemble`]`(&hits, tier)` — the per-pack gate,
///    cross-pack fusion, `NO_EVIDENCE`, tier-select/dedupe, and
///    prompt-rendering pipeline this module's top doc comment describes.
///
/// Errors: an `Embedder` failure on `query` (a backend/dims problem)
/// surfaces as [`Error::Embed`]; any pack's `vec_search`/`fts_search`/
/// `get_embedding` I/O failure surfaces as [`Error::Format`] — both via the
/// same `?`-propagation [`retrieve_pack`] already uses.
pub fn retrieve(
    query: &str,
    packs: &[(Pack, Manifest)],
    embedder: &dyn Embedder,
    tier: Tier,
) -> Result<RetrievalResult> {
    let raw_query_vec = embedder.embed_query(query)?;
    let query_i8 = quantize_int8(&l2_normalize(&raw_query_vec));

    let mut hits: Vec<PackHit<'_>> = Vec::with_capacity(packs.len());
    for (pack, manifest) in packs {
        let candidates = retrieve_pack(pack, query, &query_i8, 20)?;
        hits.push(PackHit {
            pack,
            manifest,
            candidates,
        });
    }

    assemble(&hits, tier)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::{build_pack, BuildMeta, SourceContent, SourceInput};
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
                content: SourceContent::Raw(format!("# Vitamin K\n\n{vitamin_k_text}\n")),
            },
            SourceInput {
                title: "Getting Started".to_string(),
                source_type: "md".to_string(),
                content: SourceContent::Raw(
                    "# Getting Started\n\nRun the install script to set up the build tool on this machine.\n"
                        .to_string(),
                ),
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

    // 3b. Fix 1 regression: an empty (or all-whitespace) query_text must NOT
    // hard-error retrieve_pack. safe_fts5_query("") / safe_fts5_query("   ")
    // both yield "" (no terms to quote), which fts5's MATCH treats as a
    // syntax error rather than "match nothing" -- retrieve_pack must skip
    // the lexical lane in that case, not call fts_search("") and propagate
    // the raw fts5 error. The dense lane still runs and every returned
    // candidate has a valid dense_cosine.
    #[test]
    fn t3b_retrieve_pack_empty_or_whitespace_query_skips_lexical_lane_not_error() {
        let dir = unique_dir("t3b-empty-query");
        let out_path = dir.join("empty-query.kpack");
        let embedder = MockEmbedder::new(TEST_DIMS);
        let cfg = ChunkConfig::default();
        let meta = test_meta();

        let text = "Vitamin K is a fat-soluble vitamin involved in blood clotting.";
        let sources = vec![SourceInput {
            title: "Vitamin K".to_string(),
            source_type: "md".to_string(),
            content: SourceContent::Raw(format!("# Vitamin K\n\n{text}\n")),
        }];
        build_pack(&sources, &embedder, &meta, &out_path, &cfg).unwrap();
        let pack = Pack::open(&out_path).unwrap();

        let raw = embedder.embed_passage(text).unwrap();
        let query_i8 = quantize_int8(&l2_normalize(&raw));

        for query_text in ["", "   \t "] {
            let result = retrieve_pack(&pack, query_text, &query_i8, 20);
            assert!(
                result.is_ok(),
                "empty/whitespace query_text {query_text:?} must not error retrieve_pack: {:?}",
                result.err()
            );
            let candidates = result.unwrap();
            assert!(
                !candidates.is_empty(),
                "the dense lane alone should still surface candidates for {query_text:?}"
            );
            for c in &candidates {
                assert!(
                    (-1.0..=1.0).contains(&c.dense_cosine),
                    "dense_cosine out of range for {query_text:?}: {}",
                    c.dense_cosine
                );
            }
        }
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

    // 12. Fix 4(a): empty candidates always refuses, regardless of how
    // permissive the thresholds are (already covered structurally by the
    // early `is_empty` return -- this pins it as an explicit contract test).
    #[test]
    fn t12_gate_empty_candidates_always_fails() {
        assert!(!pack_passes_gate(&[], 0.0, 0.0));
        assert!(!pack_passes_gate(&[], -1.0, -1.0));
    }

    // 13. Fix 4(b) / Fix 3 regression lock: a NaN dense_cosine must resolve
    // to fail-closed (refuse) -- both the n == 1 case (the exact fail-OPEN
    // Fix 3 closes: floor guard used to let a NaN top1 fall through to the
    // n==1 early return) and a multi-candidate set where the NaN is buried
    // among otherwise-passing candidates (proving the check isn't just
    // "top1 happens to still be NaN after sorting").
    #[test]
    fn t13_gate_nan_cosine_fails_closed() {
        assert!(
            !pack_passes_gate(&[candidate(1, f32::NAN)], 0.5, 0.0),
            "a lone NaN candidate must refuse, not fail-open on the n==1 special case"
        );

        // Otherwise-passing multi-candidate set (top1 well above floor,
        // wide margin) with one NaN mixed in -- must still refuse.
        let with_nan = vec![
            candidate(1, 0.95),
            candidate(2, f32::NAN),
            candidate(3, 0.90),
            candidate(4, 0.88),
            candidate(5, 0.85),
        ];
        assert!(
            !pack_passes_gate(&with_nan, 0.5, 0.05),
            "a NaN anywhere in a multi-candidate set must refuse, even though every \
             finite candidate here would otherwise clear both the floor and the margin"
        );

        // NaN as the reported top1 itself (largest finite value pushed to a
        // non-top rank isn't possible to force via sort, so this covers the
        // direct case Fix 3's commit message describes).
        let nan_top = vec![candidate(1, f32::NAN), candidate(2, 0.6), candidate(3, 0.55)];
        assert!(!pack_passes_gate(&nan_top, 0.5, 0.01));
    }

    // 14. Fix 4(c): pin the `min(10, n)` clamp for n > 10. Both cases are
    // constructed so ranks 11+ are far below rank 10 -- if the margin check
    // mistakenly used the LAST rank (n or n-1) instead of rank-10, the much
    // bigger (top1 - last_rank) gap would flip a failing verdict to passing.
    #[test]
    fn t14_gate_n_over_10_margin_uses_rank10_not_last_rank() {
        // n = 11: rank-10 (index 9) = 0.72, rank-11 (index 10, last) = 0.05.
        // Correct: margin = top1(0.9) - rank10(0.72) = 0.18 < rel_margin
        // 0.20 -> fails. Buggy (last-rank): margin = 0.9 - 0.05 = 0.85 ->
        // would wrongly pass.
        let n11: [f32; 11] = [
            0.90, 0.88, 0.86, 0.84, 0.82, 0.80, 0.78, 0.76, 0.74, 0.72, 0.05,
        ];
        let candidates_11: Vec<Candidate> = n11
            .iter()
            .enumerate()
            .map(|(i, &c)| candidate(i as i64, c))
            .collect();
        assert!(
            !pack_passes_gate(&candidates_11, 0.5, 0.20),
            "n=11 must use rank-10's cosine (0.72) for the margin, not rank-11's (0.05) -- \
             using the last rank would wrongly pass"
        );

        // n = 12: rank-10 (index 9) = 0.70, ranks 11-12 (indices 10, 11,
        // last) = 0.10, 0.05. Correct: margin = 0.9 - 0.70 = 0.20 <
        // rel_margin 0.30 -> fails. Buggy (last-rank): margin = 0.9 - 0.05 =
        // 0.85 -> would wrongly pass.
        let n12: [f32; 12] = [
            0.90, 0.88, 0.86, 0.84, 0.82, 0.80, 0.78, 0.76, 0.72, 0.70, 0.10, 0.05,
        ];
        let candidates_12: Vec<Candidate> = n12
            .iter()
            .enumerate()
            .map(|(i, &c)| candidate(i as i64, c))
            .collect();
        assert!(
            !pack_passes_gate(&candidates_12, 0.5, 0.30),
            "n=12 must use rank-10's cosine (0.70) for the margin, not rank-12's (0.05) -- \
             using the last rank would wrongly pass"
        );
    }

    /// Builds a fresh empty `.kpack` at a unique temp path -- shared setup
    /// for the `assemble` tests below, which insert their own docs/chunks
    /// directly (no embedder needed; `assemble` never calls `vec_search` /
    /// `fts_search` / `get_embedding` -- it only reads back `Chunk`/`Doc`
    /// rows for chunk_ids the caller's synthetic `Candidate`s already name).
    fn empty_pack(name: &str) -> Pack {
        let dir = unique_dir(name);
        let path = dir.join("pack.kpack");
        Pack::open_or_create(&path, TEST_DIMS).unwrap()
    }

    /// Inserts one doc + one chunk into `pack` and returns the chunk's id.
    /// `section_path`/`locator` are exposed so dedupe tests can construct
    /// two chunks that share a `section_path` (the v1 dedupe key).
    fn insert_test_chunk(pack: &Pack, section_path: &str, locator: &str, text: &str) -> i64 {
        let doc_id = pack.insert_doc(&sample_doc()).unwrap();
        let chunk = Chunk {
            id: 0,
            doc_id,
            section_path: section_path.to_string(),
            locator: locator.to_string(),
            prefix: String::new(),
            text: text.to_string(),
            token_count: text.split_whitespace().count() as i64,
        };
        pack.insert_chunk(&chunk).unwrap()
    }

    // 15. Single pack passes its gate -> Grounded: the prompt contains the
    // system contract text AND a rendered "[1] (...)" source line;
    // citations map 1:1 to the rendered sources (one candidate in, one
    // citation out, numbered from 1); citation count <= tier.n().
    #[test]
    fn t15_assemble_single_pack_passes_gate_yields_grounded() {
        let pack = empty_pack("t15-single-pack-grounded");
        let chunk_id = insert_test_chunk(&pack, "Intro", "p.1", "hello world");

        let manifest = test_manifest(0.5, 0.05);
        let candidates = vec![candidate(chunk_id, 0.9)]; // n==1: floor alone, well clear of 0.5

        let hit = PackHit {
            pack: &pack,
            manifest: &manifest,
            candidates,
        };
        let result = assemble(&[hit], Tier::Small).unwrap();

        match result {
            RetrievalResult::Grounded { prompt, citations } => {
                assert!(
                    prompt.contains(contract::system_contract().trim_end()),
                    "prompt must contain the system contract text"
                );
                assert!(
                    prompt.contains("[1] ("),
                    "prompt must contain the rendered numbered source line: {prompt}"
                );
                assert_eq!(citations.len(), 1);
                assert!(citations.len() <= Tier::Small.n());
                assert_eq!(citations[0].n, 1);
                assert_eq!(citations[0].chunk_id, chunk_id);
                assert_eq!(citations[0].pack_id, manifest.pack_id);
                assert_eq!(citations[0].section_path, "Intro");
                assert_eq!(citations[0].locator, "p.1");
            }
            RetrievalResult::NoEvidence => panic!("expected Grounded, got NoEvidence"),
        }
    }

    // 16. All packs fail their gate -> NoEvidence. Two packs, both with a
    // single candidate whose dense_cosine sits below that pack's own
    // gate_abs_floor. CURATED tier (not test_manifest's default Personal),
    // so this pins the raw manifest floor (0.9) rather than the
    // PERSONAL_RUNTIME_GATE_ABS_FLOOR (0.30) override -- both candidates
    // (0.3, 0.4) would otherwise clear the lenient personal floor and flip
    // this to Grounded; t25-27 cover the personal-tier override itself.
    #[test]
    fn t16_assemble_all_packs_fail_gate_yields_no_evidence() {
        let pack_a = empty_pack("t16-fail-pack-a");
        let pack_b = empty_pack("t16-fail-pack-b");
        let chunk_a = insert_test_chunk(&pack_a, "A", "p.1", "pack a chunk");
        let chunk_b = insert_test_chunk(&pack_b, "B", "p.1", "pack b chunk");

        let mut manifest_a = test_manifest(0.9, 0.05);
        manifest_a.pack_tier = PackTier::Curated;
        let mut manifest_b = test_manifest(0.9, 0.05);
        manifest_b.pack_tier = PackTier::Curated;

        let hit_a = PackHit {
            pack: &pack_a,
            manifest: &manifest_a,
            candidates: vec![candidate(chunk_a, 0.3)], // well below floor 0.9
        };
        let hit_b = PackHit {
            pack: &pack_b,
            manifest: &manifest_b,
            candidates: vec![candidate(chunk_b, 0.4)], // also below floor 0.9
        };

        let result = assemble(&[hit_a, hit_b], Tier::Large).unwrap();
        assert_eq!(result, RetrievalResult::NoEvidence);
    }

    // 17. Δ1 regression (the crux): a STRICT pack (high gate_abs_floor)
    // whose single candidate sits just below ITS floor (fails its own
    // gate), and a LOOSE pack (low gate_abs_floor) whose single candidate
    // clears ITS floor (passes). The strict pack's candidate is given a
    // much higher fused_score than the loose pack's, but that magnitude is
    // a red herring for what this test actually locks: `cross_pack_rrf`
    // fuses on RANK/position within each pack's already-fused lane (same as
    // R1's `rrf`), never on `Candidate.fused_score`, so the fused_score gap
    // has no bearing on the cross-pack ranking either way. What actually
    // locks this regression is the step-1 gate filter -- the strict pack is
    // excluded from cross-pack fusion entirely because it failed its OWN
    // gate -- plus the deterministic `(pack_index, chunk_id)` tie-break that
    // keeps the outcome reproducible. Because gating happens per-pack,
    // strictly BEFORE cross-pack fusion (Δ1), the strict pack contributes
    // NOTHING: the result must contain only the loose pack's citation, and
    // the strict pack's chunk_id must never appear.
    //
    // CURATED tier (not test_manifest's default Personal): this test is
    // specifically about the raw manifest-threshold-driven Δ1 ordering, so
    // both packs are pinned to Curated to keep their own gate_abs_floor
    // (0.9 / 0.2) authoritative -- under Personal tier both candidates
    // (0.85, 0.25) would instead be judged against the shared
    // PERSONAL_RUNTIME_GATE_ABS_FLOOR (0.30), which would invert this
    // test's strict-vs-loose setup entirely. The personal-tier override
    // itself is covered separately by t25-27.
    #[test]
    fn t17_assemble_delta1_gate_before_fusion_regression() {
        let strict_pack = empty_pack("t17-strict-pack");
        let strict_chunk_id = insert_test_chunk(&strict_pack, "S", "p.1", "strict pack chunk");
        let mut strict_manifest = test_manifest(0.9, 0.05);
        strict_manifest.pack_id = "strict-pack".to_string();
        strict_manifest.pack_tier = PackTier::Curated;

        let loose_pack = empty_pack("t17-loose-pack");
        let loose_chunk_id = insert_test_chunk(&loose_pack, "L", "p.1", "loose pack chunk");
        let mut loose_manifest = test_manifest(0.2, 0.0);
        loose_manifest.pack_id = "loose-pack".to_string();
        loose_manifest.pack_tier = PackTier::Curated;

        let strict_hit = PackHit {
            pack: &strict_pack,
            manifest: &strict_manifest,
            // dense_cosine 0.85 < abs_floor 0.9 -> fails its OWN gate (n==1,
            // floor-only rule). fused_score 100.0 is deliberately far above
            // the loose pack's -- but `cross_pack_rrf` never reads
            // fused_score, only rank/position within each pack's own
            // already-fused lane, so this magnitude has zero effect on the
            // cross-pack outcome either way; it's set high purely to make
            // clear the strict chunk is excluded by the step-1 gate filter,
            // not by losing some fusion contest it was never entered into.
            candidates: vec![Candidate {
                chunk_id: strict_chunk_id,
                fused_score: 100.0,
                dense_cosine: 0.85,
            }],
        };
        let loose_hit = PackHit {
            pack: &loose_pack,
            manifest: &loose_manifest,
            // dense_cosine 0.25 >= abs_floor 0.2 -> passes (n==1, floor
            // alone). fused_score 1.0 is likewise irrelevant to the
            // cross-pack outcome -- see the strict candidate's comment above.
            candidates: vec![Candidate {
                chunk_id: loose_chunk_id,
                fused_score: 1.0,
                dense_cosine: 0.25,
            }],
        };

        let result = assemble(&[strict_hit, loose_hit], Tier::Large).unwrap();
        match result {
            RetrievalResult::Grounded { citations, .. } => {
                assert_eq!(
                    citations.len(),
                    1,
                    "only the loose pack's chunk should survive to the fused result"
                );
                assert_eq!(citations[0].chunk_id, loose_chunk_id);
                assert_eq!(citations[0].pack_id, "loose-pack");
                // Cross-pack identity must be checked on `pack_id`, NOT bare
                // `chunk_id` -- a chunk_id is only a SQLite rowid, unique
                // WITHIN one pack's file, so `strict_chunk_id` and
                // `loose_chunk_id` can (and here do, both being the first
                // chunk inserted into a fresh pack) collide numerically
                // across packs. `pack_id` is the only field that actually
                // distinguishes which pack a citation came from.
                assert!(
                    citations.iter().all(|c| c.pack_id != "strict-pack"),
                    "the strict pack must NEVER contribute a citation -- it failed its own \
                     gate before cross-pack fusion ever ran, regardless of its fused_score"
                );
            }
            RetrievalResult::NoEvidence => panic!("expected Grounded from the loose pack alone"),
        }
    }

    // 18. Tier select: more gate-passing candidates than tier.n() -> exactly
    // tier.n() selected. Six distinct docs/chunks (no dedupe collisions),
    // candidates given in descending dense_cosine/fused_score order so the
    // (single-lane) fused rank order is exactly the input order. Small=3,
    // Large=5.
    #[test]
    fn t18_assemble_tier_select_caps_at_tier_n() {
        let pack = empty_pack("t18-tier-select");
        let cosines = [0.90, 0.85, 0.80, 0.75, 0.70, 0.65];
        let mut candidates = Vec::with_capacity(cosines.len());
        for (i, &cos) in cosines.iter().enumerate() {
            let chunk_id = insert_test_chunk(
                &pack,
                &format!("Section {i}"),
                &format!("p.{i}"),
                &format!("chunk body {i}"),
            );
            candidates.push(Candidate {
                chunk_id,
                fused_score: 10.0 - i as f64,
                dense_cosine: cos,
            });
        }
        // top1 = 0.90, rank-6 (last, n=6<10) = 0.65, margin = 0.25 -- clears
        // abs_floor 0.5 / rel_margin 0.05 comfortably.
        let manifest = test_manifest(0.5, 0.05);

        let hit_small = PackHit {
            pack: &pack,
            manifest: &manifest,
            candidates: candidates.clone(),
        };
        let result_small = assemble(&[hit_small], Tier::Small).unwrap();
        let RetrievalResult::Grounded {
            citations: citations_small,
            ..
        } = result_small
        else {
            panic!("expected Grounded");
        };
        assert_eq!(citations_small.len(), Tier::Small.n());
        assert_eq!(citations_small.len(), 3);

        let hit_large = PackHit {
            pack: &pack,
            manifest: &manifest,
            candidates,
        };
        let result_large = assemble(&[hit_large], Tier::Large).unwrap();
        let RetrievalResult::Grounded {
            citations: citations_large,
            ..
        } = result_large
        else {
            panic!("expected Grounded");
        };
        assert_eq!(citations_large.len(), Tier::Large.n());
        assert_eq!(citations_large.len(), 5);
    }

    // 19. Dedupe: two chunks from the SAME doc that share a section_path
    // (the v1 "overlapping window" approximation) -> only the higher-ranked
    // of the pair appears in the final citations.
    #[test]
    fn t19_assemble_dedupes_overlapping_same_doc_chunks() {
        let pack = empty_pack("t19-dedupe");
        let doc_id = pack.insert_doc(&sample_doc()).unwrap();
        let chunk_a = Chunk {
            id: 0,
            doc_id,
            section_path: "Same Section".to_string(),
            locator: "p.1-2".to_string(),
            prefix: String::new(),
            text: "chunk a text".to_string(),
            token_count: 3,
        };
        let chunk_a_id = pack.insert_chunk(&chunk_a).unwrap();
        let chunk_b = Chunk {
            id: 0,
            doc_id,
            section_path: "Same Section".to_string(),
            locator: "p.2-3".to_string(),
            prefix: String::new(),
            text: "chunk b text overlapping a".to_string(),
            token_count: 5,
        };
        let chunk_b_id = pack.insert_chunk(&chunk_b).unwrap();

        let manifest = test_manifest(0.5, 0.0);
        let candidates = vec![
            // Ranked first (higher fused_score, and first in this single
            // lane's order) -- must be the survivor.
            Candidate {
                chunk_id: chunk_a_id,
                fused_score: 2.0,
                dense_cosine: 0.9,
            },
            Candidate {
                chunk_id: chunk_b_id,
                fused_score: 1.0,
                dense_cosine: 0.85,
            },
        ];
        let hit = PackHit {
            pack: &pack,
            manifest: &manifest,
            candidates,
        };

        let result = assemble(&[hit], Tier::Large).unwrap();
        let RetrievalResult::Grounded { citations, .. } = result else {
            panic!("expected Grounded");
        };
        assert_eq!(
            citations.len(),
            1,
            "the lower-ranked overlapping chunk must be deduped out"
        );
        assert_eq!(
            citations[0].chunk_id, chunk_a_id,
            "the higher-ranked chunk of the overlapping pair must survive"
        );
    }

    // 20. Empty hits -> NoEvidence, no panic.
    #[test]
    fn t20_assemble_empty_hits_yields_no_evidence_no_panic() {
        let result = assemble(&[], Tier::Small).unwrap();
        assert_eq!(result, RetrievalResult::NoEvidence);
    }

    // 21. Fix 1 regression: a candidate that clears its pack's OWN gate but
    // whose chunk_id does not exist in that pack (e.g. its doc/chunk row is
    // missing) must not fall through to a citation-less "Grounded" prompt --
    // assemble must return NoEvidence instead. Uses a pack with no chunks
    // inserted at all, so ANY chunk_id named by the synthetic candidate is
    // guaranteed absent (get_chunk returns None in step 4, leaving both
    // `selected` and `resolved` empty); dense_cosine is set well above the
    // pack's gate_abs_floor so the pack passes its own gate (step 1) and the
    // only thing that fails is resolution.
    #[test]
    fn t21_assemble_all_selected_chunks_unresolvable_yields_no_evidence() {
        let pack = empty_pack("t21-unresolvable-chunk");
        let manifest = test_manifest(0.5, 0.05);
        // n==1: floor alone, well clear of 0.5. chunk_id 999_999 was never
        // inserted into `pack`, so get_chunk(999_999) returns None.
        let candidates = vec![candidate(999_999, 0.9)];

        let hit = PackHit {
            pack: &pack,
            manifest: &manifest,
            candidates,
        };

        let result = assemble(&[hit], Tier::Small).unwrap();
        assert_eq!(
            result,
            RetrievalResult::NoEvidence,
            "a pack that passes its own gate but whose selected chunk_id doesn't resolve to a \
             real chunk/doc row must fall back to NoEvidence, never a citation-less Grounded"
        );
    }

    // 22. retrieve(): end-to-end wiring with MockEmbedder — a permissive
    // gate (abs_floor below anything a real cosine could be, since
    // dot_int8-derived cosine is always clamped to [-1, 1]) on a single
    // pack with one chunk yields Grounded with exactly one citation, and
    // the prompt carries the system contract + a rendered "[1] (...)"
    // line. Deliberately a PLUMBING test (embed -> quantize -> per-pack
    // retrieve -> assemble all get wired together end to end by `retrieve`
    // itself), not a relevance test — MockEmbedder has no real semantics
    // (see embed.rs's own doc comment), so which chunk "wins" is never
    // asserted, only that the pipeline as a whole produces a well-formed
    // Grounded result.
    #[test]
    fn t22_retrieve_end_to_end_grounded_with_mock_embedder() {
        let dir = unique_dir("t22-retrieve-grounded");
        let out_path = dir.join("t22.kpack");
        let embedder = MockEmbedder::new(TEST_DIMS);
        let cfg = ChunkConfig::default();
        let meta = test_meta();

        let sources = vec![SourceInput {
            title: "Only Doc".to_string(),
            source_type: "md".to_string(),
            content: SourceContent::Raw(
                "# Only Doc\n\nThis pack has exactly one chunk of content.\n".to_string(),
            ),
        }];
        build_pack(&sources, &embedder, &meta, &out_path, &cfg).unwrap();
        let pack = Pack::open(&out_path).unwrap();

        // gate_abs_floor well below any cosine's possible range -- ANY
        // candidate passes, isolating this test to the wiring rather than a
        // specific mock cosine value. Pinned to Curated so this extreme -2.0
        // floor is actually honored: a Personal-tier manifest would have its
        // floor overridden by PERSONAL_RUNTIME_GATE_ABS_FLOOR (0.30), silently
        // recoupling this wiring test to whether the mock cosine clears 0.30
        // (mirrors t16/t17/t23).
        let mut manifest = test_manifest(-2.0, 0.0);
        manifest.pack_tier = PackTier::Curated;

        let packs = vec![(pack, manifest)];
        let result = retrieve("only doc content", &packs, &embedder, Tier::Small).unwrap();

        match result {
            RetrievalResult::Grounded { prompt, citations } => {
                assert!(
                    prompt.contains(contract::system_contract().trim_end()),
                    "prompt must contain the system contract text"
                );
                assert!(
                    prompt.contains("[1] ("),
                    "prompt must contain the rendered numbered source line: {prompt}"
                );
                assert_eq!(citations.len(), 1);
            }
            RetrievalResult::NoEvidence => panic!("expected Grounded with an abs_floor of -2.0"),
        }
    }

    // 23. retrieve(): an impossible gate_abs_floor (2.0, outside a cosine's
    // [-1, 1] range) always fails every pack's gate regardless of query or
    // candidates -- proving `retrieve` surfaces `assemble`'s NoEvidence
    // path end to end, not just its Grounded one. CURATED tier, so this
    // pins the manifest's own 2.0 floor rather than being overridden by
    // PERSONAL_RUNTIME_GATE_ABS_FLOOR (0.30), which a real MockEmbedder
    // cosine could plausibly clear.
    #[test]
    fn t23_retrieve_end_to_end_no_evidence_when_gate_impossible() {
        let dir = unique_dir("t23-retrieve-no-evidence");
        let out_path = dir.join("t23.kpack");
        let embedder = MockEmbedder::new(TEST_DIMS);
        let cfg = ChunkConfig::default();
        let meta = test_meta();

        let sources = vec![SourceInput {
            title: "Only Doc".to_string(),
            source_type: "md".to_string(),
            content: SourceContent::Raw(
                "# Only Doc\n\nThis pack has exactly one chunk of content.\n".to_string(),
            ),
        }];
        build_pack(&sources, &embedder, &meta, &out_path, &cfg).unwrap();
        let pack = Pack::open(&out_path).unwrap();

        let mut manifest = test_manifest(2.0, 0.0); // no real cosine can ever reach 2.0
        manifest.pack_tier = PackTier::Curated;

        let packs = vec![(pack, manifest)];
        let result = retrieve("only doc content", &packs, &embedder, Tier::Small).unwrap();
        assert_eq!(result, RetrievalResult::NoEvidence);
    }

    /// An `Embedder` whose `embed_query` always errors -- exists only to
    /// prove t24's error-propagation path; `embed_passage`/`token_count`
    /// are never exercised by `retrieve` (which only calls `embed_query`)
    /// but are implemented plausibly so this stays a well-formed `Embedder`.
    struct FailingEmbedder;

    impl Embedder for FailingEmbedder {
        fn dims(&self) -> usize {
            TEST_DIMS
        }

        fn embed_passage(&self, _text: &str) -> std::result::Result<Vec<f32>, crate::embed::EmbedError> {
            Ok(vec![0.0; TEST_DIMS])
        }

        fn embed_query(&self, _text: &str) -> std::result::Result<Vec<f32>, crate::embed::EmbedError> {
            Err(crate::embed::EmbedError::Backend("boom".to_string()))
        }

        fn token_count(&self, text: &str) -> usize {
            text.split_whitespace().count()
        }

        fn max_input_tokens(&self) -> usize {
            usize::MAX
        }
    }

    // 24. retrieve(): an Embedder that fails embed_query surfaces as
    // Error::Embed (via this module's own `From<EmbedError>` impl), not a
    // panic -- proving the `?` on `embedder.embed_query(query)` actually
    // propagates. `packs` is empty since embed_query runs BEFORE any
    // per-pack loop, so this fails before ever touching a pack.
    #[test]
    fn t24_retrieve_propagates_embed_error_not_panic() {
        let result = retrieve("any query", &[], &FailingEmbedder, Tier::Small);
        assert!(
            matches!(result, Err(Error::Embed(_))),
            "expected Error::Embed, got {result:?}"
        );
    }

    // ---- RAG-quality quick wins: personal-pack runtime gate + doc-title
    // context (see PERSONAL_RUNTIME_GATE_ABS_FLOOR's doc comment). ----

    // 25. Gentler personal gate: a PERSONAL pack whose manifest bakes
    // gate_abs_floor=0.5 (the build-time placeholder), with a single
    // candidate at dense_cosine=0.40 -- BELOW the baked 0.5 floor, so
    // pack_passes_gate_manifest(&candidates, &manifest) would fail -- now
    // PASSES under assemble's PERSONAL_RUNTIME_GATE_ABS_FLOOR (0.30)
    // override, yielding Grounded rather than the NoEvidence the baked
    // value alone would produce.
    #[test]
    fn t25_assemble_personal_pack_runtime_floor_admits_below_manifest_floor() {
        let pack = empty_pack("t25-personal-runtime-floor");
        let chunk_id = insert_test_chunk(&pack, "Intro", "p.1", "hello world");

        let manifest = test_manifest(0.5, 0.05); // baked placeholder; pack_tier is Personal
        assert!(
            !pack_passes_gate_manifest(&[candidate(chunk_id, 0.40)], &manifest),
            "sanity check: 0.40 must fail the BAKED manifest floor of 0.5"
        );

        let hit = PackHit {
            pack: &pack,
            manifest: &manifest,
            candidates: vec![candidate(chunk_id, 0.40)],
        };
        let result = assemble(&[hit], Tier::Small).unwrap();
        match result {
            RetrievalResult::Grounded { citations, .. } => {
                assert_eq!(citations.len(), 1);
                assert_eq!(citations[0].chunk_id, chunk_id);
            }
            RetrievalResult::NoEvidence => {
                panic!("expected Grounded -- the personal runtime floor (0.30) should admit 0.40")
            }
        }
    }

    // 26. Gentler personal gate, lower bound: a candidate at dense_cosine
    // 0.2746 (the §4-measured out-of-scope Mongolia-vs-hemostasis case) is
    // still below PERSONAL_RUNTIME_GATE_ABS_FLOOR (0.30) -- must still
    // yield NoEvidence, proving the lenient floor doesn't admit everything.
    #[test]
    fn t26_assemble_personal_pack_runtime_floor_still_rejects_out_of_scope() {
        let pack = empty_pack("t26-personal-out-of-scope");
        let chunk_id = insert_test_chunk(&pack, "Intro", "p.1", "hello world");

        let manifest = test_manifest(0.5, 0.05);
        let hit = PackHit {
            pack: &pack,
            manifest: &manifest,
            candidates: vec![candidate(chunk_id, 0.2746)],
        };
        let result = assemble(&[hit], Tier::Small).unwrap();
        assert_eq!(
            result,
            RetrievalResult::NoEvidence,
            "0.2746 is below the 0.30 personal runtime floor and must still refuse"
        );
    }

    // 27. Curated packs are unaffected by the personal runtime override: a
    // CURATED manifest with gate_abs_floor=0.6, a candidate at 0.5 -- above
    // the personal runtime floor (0.30) but below the curated pack's OWN
    // manifest floor (0.6) -- must still yield NoEvidence, proving assemble
    // reads the manifest's own thresholds for a non-Personal pack rather
    // than applying the personal override universally.
    #[test]
    fn t27_assemble_curated_pack_still_uses_manifest_floor() {
        let pack = empty_pack("t27-curated-manifest-floor");
        let chunk_id = insert_test_chunk(&pack, "Intro", "p.1", "hello world");

        let mut manifest = test_manifest(0.6, 0.05);
        manifest.pack_tier = PackTier::Curated;

        let hit = PackHit {
            pack: &pack,
            manifest: &manifest,
            candidates: vec![candidate(chunk_id, 0.5)],
        };
        let result = assemble(&[hit], Tier::Small).unwrap();
        assert_eq!(
            result,
            RetrievalResult::NoEvidence,
            "a curated pack must be gated on its OWN manifest floor (0.6), not the personal \
             runtime override -- 0.5 clears the personal floor but not this pack's own"
        );
    }

    /// Inserts one doc (with the given `title`) + one chunk into `pack`,
    /// returning the chunk's id -- like `insert_test_chunk`, but lets
    /// doc-title-context tests control the title directly (`sample_doc()`
    /// always uses a fixed "Test Doc" title).
    fn insert_test_chunk_with_title(pack: &Pack, title: &str, section_path: &str, locator: &str, text: &str) -> i64 {
        let mut doc = sample_doc();
        doc.title = title.to_string();
        let doc_id = pack.insert_doc(&doc).unwrap();
        let chunk = Chunk {
            id: 0,
            doc_id,
            section_path: section_path.to_string(),
            locator: locator.to_string(),
            prefix: String::new(),
            text: text.to_string(),
            token_count: text.split_whitespace().count() as i64,
        };
        pack.insert_chunk(&chunk).unwrap()
    }

    // 28. Doc-title context: a Grounded result's prompt contains "These
    // sources are excerpts from:" followed by the cited doc's title,
    // inserted between the system contract and the rendered sources block.
    #[test]
    fn t28_assemble_grounded_prompt_carries_doc_title_context() {
        let pack = empty_pack("t28-doc-title-context");
        let chunk_id =
            insert_test_chunk_with_title(&pack, "The Great Cookbook", "Intro", "p.1", "hello world");

        let manifest = test_manifest(0.5, 0.05);
        let hit = PackHit {
            pack: &pack,
            manifest: &manifest,
            candidates: vec![candidate(chunk_id, 0.9)],
        };
        let result = assemble(&[hit], Tier::Small).unwrap();
        let RetrievalResult::Grounded { prompt, .. } = result else {
            panic!("expected Grounded");
        };
        assert!(
            prompt.contains("These sources are excerpts from: The Great Cookbook."),
            "prompt should state the cited doc's title as context: {prompt}"
        );
        // Placement: between the system contract and the sources block, per
        // assemble's doc comment -- the contract text appears BEFORE the
        // doc-context line, which appears BEFORE the numbered source line.
        let contract_pos = prompt.find(contract::system_contract().trim_end()).unwrap();
        let context_pos = prompt.find("These sources are excerpts from:").unwrap();
        let sources_pos = prompt.find("[1] (").unwrap();
        assert!(contract_pos < context_pos, "contract must precede doc context");
        assert!(context_pos < sources_pos, "doc context must precede the sources block");
    }

    // 29. Doc-title context, empty title: a build with an empty doc title
    // omits the context line entirely -- no doubled blank lines, no
    // dangling "from: .". Falls back to the original two-part
    // contract+blank-line+sources layout.
    #[test]
    fn t29_assemble_grounded_prompt_omits_doc_context_for_empty_title() {
        let pack = empty_pack("t29-empty-doc-title");
        let chunk_id = insert_test_chunk_with_title(&pack, "", "Intro", "p.1", "hello world");

        let manifest = test_manifest(0.5, 0.05);
        let hit = PackHit {
            pack: &pack,
            manifest: &manifest,
            candidates: vec![candidate(chunk_id, 0.9)],
        };
        let result = assemble(&[hit], Tier::Small).unwrap();
        let RetrievalResult::Grounded { prompt, .. } = result else {
            panic!("expected Grounded");
        };
        assert!(
            !prompt.contains("These sources are excerpts from"),
            "an empty title must omit the doc-context line entirely: {prompt}"
        );
        assert!(!prompt.contains("from: ."), "must not render a dangling empty title: {prompt}");
        assert!(
            !prompt.contains("\n\n\n"),
            "must not leave a doubled blank line where the omitted context line was: {prompt}"
        );
        // Falls back to exactly the original two-part layout: contract +
        // one blank line + sources block.
        let expected_prefix = format!("{}\n\n", contract::system_contract().trim_end());
        assert!(
            prompt.starts_with(&expected_prefix),
            "must fall back to the original contract+blank-line+sources layout: {prompt}"
        );
    }
}
