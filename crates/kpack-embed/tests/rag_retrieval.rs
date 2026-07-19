//! Real-embedder RAG retrieval integration tests (spec §4, R4) — proves
//! `kpack_core::retrieve::retrieve` end to end against the REAL, linked
//! `BgeEmbedder`, not `MockEmbedder`: an in-corpus query is grounded with
//! real citations, the real embedder + gate actually DISCRIMINATE between
//! an in-corpus and an out-of-scope query (measured, not assumed), and the
//! Δ1 multi-pack regression (a strict-gated pack's own gate can never be
//! bypassed by a loosely-gated pack winning the fused ranking) holds with
//! real cosines, not synthetic ones.
//!
//! Mirrors `bge_smoke.rs`/`build_determinism.rs`'s pattern: model path
//! resolution + skip-with-message if the bundled GGUF is absent, `#[ignore]`d
//! (needs the real model + native llama.cpp build), gated on the `real`
//! Cargo feature so a default `cargo build -p kpack-embed` (no features)
//! never touches this file. Run (Windows, after `$env:LIBCLANG_PATH` is
//! set):
//!   cargo test -p kpack-embed --features real -- --ignored --test-threads=1 --nocapture
//!
//! ## On the gate floors used here
//! `kpack_core::build::build_pack` always writes the K8 placeholder gate
//! values (`PLACEHOLDER_GATE_ABS_FLOOR` = 0.5, `PLACEHOLDER_GATE_REL_MARGIN`
//! = 0.05) into a fresh pack's manifest — there is no calibration pipeline
//! wired in yet (spec §2.4/§3.4, later milestones). Tests 2 and 3 below need
//! SPECIFIC gate values to prove discrimination and the Δ1 property, so
//! `set_gate` edits the manifest table directly, post-build: `Pack::open`
//! (not `mount`, which would immediately re-validate against the
//! still-placeholder values) → `Manifest::read` → mutate the two threshold
//! fields → `Manifest::write` back. This is explicitly a TEST-ONLY stand-in
//! for real calibration, not a claim about production gate values — the
//! SHIPPED placeholder is untouched by this file. `Manifest::read`'s
//! `require_gate_threshold` rejects negative values (a negative
//! `gate_abs_floor` would fail OPEN in the safety gate), so every floor this
//! file computes is clamped to `>= 0.0`.
#![cfg(feature = "real")]

use std::path::{Path, PathBuf};

use kpack_core::build::{build_pack, BuildMeta, SourceInput};
use kpack_core::chunk::ChunkConfig;
use kpack_core::contract;
use kpack_core::embed::{l2_normalize, quantize_int8, Embedder};
use kpack_core::format::Pack;
use kpack_core::manifest::{LoadContext, Manifest, PackTier};
use kpack_core::retrieve::{retrieve, retrieve_pack, RetrievalResult, Tier};
use kpack_embed::BgeEmbedder;

/// Same temp-dir convention as `kpack-core`'s and this crate's other test
/// files (this repo hand-rolls temp dirs instead of depending on
/// `tempfile`) — duplicated per-file since it's private to each.
fn unique_dir(name: &str) -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "cleophis-kpack-embed-rag-retrieval-test-{}-{}-{}",
        std::process::id(),
        unique,
        name
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Mirrors `bge_smoke.rs`/`build_determinism.rs`'s model path resolution
/// exactly: the bundled `bge-base-en-v1.5` Q8_0 GGUF, fetched by `node
/// tools/fetch-embedder.mjs`.
fn model_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("src-tauri")
        .join("resources")
        .join("embedders")
        .join("bge-base-en-v1.5-q8_0.gguf")
}

fn test_meta(pack_id: &str) -> BuildMeta {
    BuildMeta {
        pack_id: pack_id.to_string(),
        pack_version: "2026.07.1".to_string(),
        pack_tier: PackTier::Personal,
        embedder_name: "bge-base-en-v1.5-q8_0.gguf".to_string(),
        // Placeholder — these tests never exercise the embedder-hash gate's
        // real security property, only that mount uses the SAME declared
        // hash the build wrote (see mount_ctx below).
        embedder_sha256: "r4test0000000000000000000000000000000000000000000000000000".to_string(),
        built_by: "r4-rag-retrieval-test".to_string(),
    }
}

/// A small corpus on a specific, narrow topic (vitamin K / blood clotting /
/// warfarin) — the "in-corpus" domain for the in-corpus queries below.
fn vitamin_k_sources() -> Vec<SourceInput> {
    vec![
        SourceInput {
            title: "Vitamin K Basics".to_string(),
            source_type: "md".to_string(),
            content: "\
# Vitamin K

Vitamin K is a fat-soluble vitamin that plays a central role in blood \
clotting: it activates several clotting factors in the liver, and without \
enough of it, blood does not coagulate properly. It also supports bone \
metabolism through the carboxylation of osteocalcin. Warfarin, a common \
anticoagulant, works by antagonizing vitamin K's role in the clotting \
cascade, so patients on warfarin are usually told to keep their dietary \
vitamin K intake roughly consistent from week to week rather than \
eliminating it outright.\n"
                .to_string(),
        },
        SourceInput {
            title: "Getting Started".to_string(),
            source_type: "md".to_string(),
            content: "\
# Getting Started

Run the install script to set up the build tool on this machine, then \
verify the installation by checking the reported version string against \
the release notes for this build.\n"
                .to_string(),
        },
    ]
}

/// A corpus entirely OUTSIDE the vitamin-K/clotting domain — used as the
/// "loose pack" in test 3, and as the source of the out-of-scope query's
/// (weak) matching material in test 2's pack.
fn getting_started_only_sources() -> Vec<SourceInput> {
    vec![SourceInput {
        title: "Getting Started".to_string(),
        source_type: "md".to_string(),
        content: "\
# Getting Started

Run the install script to set up the build tool on this machine, then \
verify the installation by checking the reported version string against \
the release notes for this build.\n"
            .to_string(),
    }]
}

/// Re-open `path`, overwrite its manifest's `gate_abs_floor`/`gate_rel_margin`,
/// and write it back — the test-only calibration stand-in this file's module
/// doc comment describes. Panics loudly (via `.expect`) on any I/O failure;
/// these are test fixtures, not production code paths.
fn set_gate(path: &Path, gate_abs_floor: f64, gate_rel_margin: f64) {
    let pack = Pack::open(path).expect("re-open for gate edit failed");
    let mut manifest = Manifest::read(&pack).expect("manifest read failed");
    manifest.gate_abs_floor = gate_abs_floor;
    manifest.gate_rel_margin = gate_rel_margin;
    manifest.write(&pack).expect("manifest gate write failed");
}

/// `embedder.embed_query(text)` → l2_normalize → quantize_int8 — the exact
/// pipeline `kpack_core::retrieve::retrieve` runs internally for the dense
/// lane (spec §1.4), duplicated here so tests can measure a raw dense
/// cosine directly via `retrieve_pack` before deciding a gate floor.
fn embed_query_i8(embedder: &BgeEmbedder, text: &str) -> Vec<i8> {
    let raw = embedder.embed_query(text).expect("embed_query failed");
    quantize_int8(&l2_normalize(&raw))
}

fn mount(path: &Path, meta: &BuildMeta) -> (Pack, Manifest) {
    let available = vec![meta.embedder_sha256.clone()];
    let ctx = LoadContext {
        available_embedder_sha256: &available,
        curator_key: None,
    };
    Pack::mount(path, &ctx).expect("mount failed")
}

/// 1. In-corpus grounded: a query clearly answerable from the corpus →
/// `RetrievalResult::Grounded`; the prompt contains the system contract, the
/// relevant chunk's text (via its doc title showing up in the rendered
/// citation), and a `[1] (...)` citation line mapping to it. Uses the K8
/// placeholder gate (unmodified) — the in-corpus match is expected to clear
/// it comfortably; see test 2 for the honest discrimination measurement.
#[test]
#[ignore]
fn t1_in_corpus_query_is_grounded_with_citations() {
    let model_path = model_path();
    if !model_path.exists() {
        eprintln!(
            "SKIPPED: model missing at {} — run `node tools/fetch-embedder.mjs` first",
            model_path.display()
        );
        return;
    }
    let embedder = BgeEmbedder::new(&model_path).expect("failed to load BgeEmbedder");

    let dir = unique_dir("t1-grounded");
    let path = dir.join("t1.kpack");
    let cfg = ChunkConfig::default();
    let meta = test_meta("r4-t1-pack");

    build_pack(&vitamin_k_sources(), &embedder, &meta, &path, &cfg).expect("build_pack failed");
    let (pack, manifest) = mount(&path, &meta);
    let mounted = vec![(pack, manifest)];

    let query = "How does vitamin K interact with warfarin and affect blood clotting?";
    let result = retrieve(query, &mounted, &embedder, Tier::Large).expect("retrieve failed");

    match result {
        RetrievalResult::Grounded { prompt, citations } => {
            eprintln!("R4 t1: grounded prompt =\n{prompt}");
            assert!(
                prompt.contains(contract::system_contract().trim_end()),
                "prompt must contain the system contract"
            );
            assert!(
                prompt.contains("[1] ("),
                "prompt must render at least one numbered citation line: {prompt}"
            );
            assert!(!citations.is_empty(), "grounded result must carry at least one citation");
            for c in &citations {
                assert_eq!(c.pack_id, "r4-t1-pack");
            }
            assert!(
                citations.iter().any(|c| c.doc_title == "Vitamin K Basics"),
                "citations should include the Vitamin K doc for an in-corpus vitamin K query, \
                 got: {citations:?}"
            );
        }
        RetrievalResult::NoEvidence => {
            panic!("expected Grounded for an in-corpus query, got NoEvidence")
        }
    }
}

/// 2. Discrimination (the honest test, given uncalibrated placeholder
/// gates): measure the REAL top-1 dense cosine for an in-corpus query vs. an
/// out-of-scope query against the same pack (both numbers reported via
/// `eprintln!`). Set the pack's `gate_abs_floor` to a value strictly BETWEEN
/// them — a stand-in for real calibration (§2.4/§3.4), NOT the raw K8
/// placeholder (0.5): BGE's baseline similarity can exceed 0.5 even for
/// unrelated text, so asserting `NoEvidence` against 0.5 directly would be
/// dishonest. Then assert: in-corpus → `Grounded`, out-of-scope →
/// `NoEvidence`. This proves the real embedder + gate DISCRIMINATE.
///
/// **The SHIPPED `gate_abs_floor` stays the K8 placeholder (0.5) — this
/// midpoint is a TEST-ONLY stand-in, not a calibration result.** Real
/// out-of-scope rejection quality (i.e. whether 0.5 itself is a good floor)
/// awaits §2.4/§3.4 calibration; this test proves the MECHANISM
/// discriminates, not that the shipped number is well-tuned.
#[test]
#[ignore]
fn t2_gate_discriminates_in_corpus_vs_out_of_scope_real_cosines() {
    let model_path = model_path();
    if !model_path.exists() {
        eprintln!(
            "SKIPPED: model missing at {} — run `node tools/fetch-embedder.mjs` first",
            model_path.display()
        );
        return;
    }
    let embedder = BgeEmbedder::new(&model_path).expect("failed to load BgeEmbedder");

    let dir = unique_dir("t2-discriminate");
    let path = dir.join("t2.kpack");
    let cfg = ChunkConfig::default();
    let meta = test_meta("r4-t2-pack");
    build_pack(&vitamin_k_sources(), &embedder, &meta, &path, &cfg).expect("build_pack failed");

    let in_corpus_query = "How does vitamin K interact with warfarin and affect blood clotting?";
    let out_of_scope_query = "What is the capital city of Mongolia?";

    let in_i8 = embed_query_i8(&embedder, in_corpus_query);
    let out_i8 = embed_query_i8(&embedder, out_of_scope_query);

    let pack_for_measure = Pack::open(&path).expect("open for cosine measurement failed");
    let in_candidates =
        retrieve_pack(&pack_for_measure, in_corpus_query, &in_i8, 20).expect("retrieve_pack (in-corpus) failed");
    let out_candidates = retrieve_pack(&pack_for_measure, out_of_scope_query, &out_i8, 20)
        .expect("retrieve_pack (out-of-scope) failed");

    let top1_in = in_candidates.iter().map(|c| c.dense_cosine).fold(f32::MIN, f32::max);
    let top1_out = out_candidates.iter().map(|c| c.dense_cosine).fold(f32::MIN, f32::max);

    eprintln!(
        "R4 t2: real top-1 dense cosine — in-corpus = {top1_in:.4}, out-of-scope = {top1_out:.4}"
    );

    assert!(
        top1_in > top1_out,
        "the real embedder must rank the in-corpus query's top match above the out-of-scope \
         query's -- got in-corpus={top1_in}, out-of-scope={top1_out}; if this genuinely fails, \
         the fixture queries need to be revisited (this is the discrimination property the whole \
         gate depends on)"
    );

    drop(pack_for_measure);

    // Stand-in for §2.4/§3.4 calibration: strictly between the two measured
    // cosines. Clamped to >= 0.0 -- `Manifest::read`'s `require_gate_threshold`
    // rejects a negative gate_abs_floor (it would fail OPEN).
    let floor = ((top1_in as f64 + top1_out as f64) / 2.0).max(0.0);
    eprintln!(
        "R4 t2: gate_abs_floor set to the midpoint {floor:.4} (test-only stand-in for §2.4/§3.4 \
         calibration -- NOT the shipped placeholder)"
    );

    set_gate(&path, floor, 0.0); // rel_margin 0.0 isolates the absolute-floor discrimination this test targets

    let (pack, manifest) = mount(&path, &meta);
    let mounted = vec![(pack, manifest)];

    let in_result =
        retrieve(in_corpus_query, &mounted, &embedder, Tier::Large).expect("retrieve (in-corpus) failed");
    assert!(
        matches!(in_result, RetrievalResult::Grounded { .. }),
        "in-corpus query must be Grounded once the floor sits below its real cosine, got {in_result:?}"
    );

    let out_result = retrieve(out_of_scope_query, &mounted, &embedder, Tier::Large)
        .expect("retrieve (out-of-scope) failed");
    assert!(
        matches!(out_result, RetrievalResult::NoEvidence),
        "out-of-scope query must be NoEvidence once the floor sits above its real cosine, got {out_result:?}"
    );

    eprintln!(
        "R4 t2: PASS -- floor {floor:.4} correctly separates in-corpus (Grounded) from \
         out-of-scope (NoEvidence); in-corpus cosine={top1_in:.4}, out-of-scope cosine={top1_out:.4}"
    );
}

/// 3. Multi-pack Δ1 (real): a STRICT pack (genuinely on-topic vitamin-K
/// content, gated so high it fails its OWN gate on this query) mounted
/// alongside a LOOSE pack (off-topic install-script content, gated so low
/// it always passes). Even though the strict pack's content is the
/// genuinely relevant one, Δ1 (gate BEFORE cross-pack fusion) means it must
/// contribute NOTHING — only the loose pack's (weak) citations may appear,
/// or the whole result is `NoEvidence`. Proves a loose pack can never
/// surface an answer a strict pack's gate would have refused, with REAL
/// cosines rather than synthetic ones (the synthetic version is R3's
/// `t17_assemble_delta1_gate_before_fusion_regression`).
#[test]
#[ignore]
fn t3_multi_pack_delta1_strict_gate_never_bypassed_by_loose_pack() {
    let model_path = model_path();
    if !model_path.exists() {
        eprintln!(
            "SKIPPED: model missing at {} — run `node tools/fetch-embedder.mjs` first",
            model_path.display()
        );
        return;
    }
    let embedder = BgeEmbedder::new(&model_path).expect("failed to load BgeEmbedder");

    let dir = unique_dir("t3-delta1");
    let strict_path = dir.join("strict.kpack");
    let loose_path = dir.join("loose.kpack");
    let cfg = ChunkConfig::default();

    let strict_meta = test_meta("r4-t3-strict-pack");
    let loose_meta = test_meta("r4-t3-loose-pack");

    // Strict pack: genuinely on-topic content (vitamin K). If gating were
    // done post-fusion on "whichever pack won," this pack's real relevance
    // would let it surface. Δ1 says it must not, because ITS OWN gate refuses.
    build_pack(&vitamin_k_sources(), &embedder, &strict_meta, &strict_path, &cfg)
        .expect("strict build failed");
    // Loose pack: off-topic content (an install-script doc) the query only
    // weakly matches -- but its floor is set so low it always passes.
    build_pack(
        &getting_started_only_sources(),
        &embedder,
        &loose_meta,
        &loose_path,
        &cfg,
    )
    .expect("loose build failed");

    let query = "How does vitamin K interact with warfarin and affect blood clotting?";
    let query_i8 = embed_query_i8(&embedder, query);

    // Self-calibrate the strict floor from the strict pack's OWN measured
    // top-1 cosine for this query, rather than a hardcoded absolute number --
    // guarantees the strict pack fails its own gate regardless of real BGE's
    // exact numeric behavior on this fixture (top1 + 0.05 can never be <=
    // top1, so `top1 >= floor` is false by construction). Clamped to [0, 1]:
    // negative floors are rejected on read-back (see module doc comment),
    // and a cosine ceiling above 1.0 would be meaningless.
    let strict_measure_pack = Pack::open(&strict_path).expect("open strict for cosine measurement");
    let strict_candidates =
        retrieve_pack(&strict_measure_pack, query, &query_i8, 20).expect("retrieve_pack strict failed");
    let strict_top1 = strict_candidates.iter().map(|c| c.dense_cosine).fold(f32::MIN, f32::max);
    eprintln!("R4 t3: strict pack real top-1 dense cosine (in-domain query) = {strict_top1:.4}");
    drop(strict_measure_pack);

    let strict_floor = ((strict_top1 as f64) + 0.05).clamp(0.0, 1.0);

    set_gate(&strict_path, strict_floor, 0.0);
    // 0.0 is the schema's own minimum (see module doc comment) -- the loose
    // pack passes its own gate as long as it has any non-negative top-1
    // cosine, exactly modeling an uncalibrated personal pack (§3.4) that
    // hasn't been through real calibration yet.
    set_gate(&loose_path, 0.0, 0.0);

    let (strict_pack, strict_manifest) = mount(&strict_path, &strict_meta);
    let (loose_pack, loose_manifest) = mount(&loose_path, &loose_meta);
    let mounted = vec![(strict_pack, strict_manifest), (loose_pack, loose_manifest)];

    let result = retrieve(query, &mounted, &embedder, Tier::Large).expect("retrieve failed");

    match result {
        RetrievalResult::Grounded { citations, .. } => {
            eprintln!("R4 t3: Grounded, {} citation(s)", citations.len());
            assert!(
                !citations.is_empty(),
                "expected at least the loose pack's weak citation(s)"
            );
            for c in &citations {
                assert_eq!(
                    c.pack_id, "r4-t3-loose-pack",
                    "Δ1 violation: a citation came from the strict pack ({c:?}), which must have \
                     failed its own gate (floor {strict_floor:.4} > its top-1 cosine \
                     {strict_top1:.4}) and contributed nothing"
                );
            }
            eprintln!("R4 t3: PASS -- all citations came from the loose pack; the strict pack contributed none");
        }
        RetrievalResult::NoEvidence => {
            eprintln!(
                "R4 t3: PASS -- NoEvidence (both packs refused; also a valid Δ1-compliant \
                 outcome per the task brief, as long as the strict pack never surfaces)"
            );
        }
    }
}
