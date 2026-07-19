//! Real-embedder cross-build determinism (K8b, spec §5 "Cross-build
//! determinism") — the genuine §5 proof that `kpack-core`'s own
//! `t2_build_pack_is_deterministic_mock_plumbing_only_not_section_5` test
//! (`crates/kpack-core/src/build.rs`) explicitly disclaims: that test locks
//! `build_pack`'s CODE path (no randomness, no ordering drift) under
//! `MockEmbedder`, whose `token_count` is a crude word-count approximation,
//! not a real tokenizer — chunk boundaries built from it don't prove
//! anything about real token boundaries. This file re-runs the same shape
//! of test with the REAL `BgeEmbedder` (real WordPiece tokenizer, real
//! llama.cpp CPU inference), which is what §5 actually demands: "server
//! pipeline and device builder given the same corpus + embedder must
//! produce identical chunk sets and near-identical (quantization-tolerance)
//! vectors." The full server-pipeline-vs-device-builder comparison awaits
//! §2's server pipeline (not built yet); what's provable now — and what
//! needs the real tokenizer for — is that the on-device builder itself
//! (`kpack_core::build::build_pack`) is byte-reproducible across two
//! independent builds (two independently loaded `BgeEmbedder` instances,
//! mirroring two separate build processes) with real token boundaries.
//!
//! Mirrors `bge_smoke.rs`'s pattern: model path resolution + skip-with-
//! message if the bundled GGUF is absent, `#[ignore]`d (needs the real
//! model + native llama.cpp build), gated on the `real` Cargo feature so a
//! default `cargo build -p kpack-embed` (no features) never touches this
//! file. Run (Windows, after `$env:LIBCLANG_PATH` is set):
//!   cargo test -p kpack-embed --features real -- --ignored --test-threads=1 --nocapture
#![cfg(feature = "real")]

use std::path::{Path, PathBuf};

use kpack_core::build::{build_pack, passage_input, BuildMeta, SourceContent, SourceInput};
use kpack_core::chunk::ChunkConfig;
use kpack_core::embed::{l2_normalize, quantize_int8, Embedder};
use kpack_core::format::{Chunk, Pack};
use kpack_core::manifest::{LoadContext, PackTier};
use kpack_embed::BgeEmbedder;

/// Same temp-dir convention as `kpack-core/src/build.rs`'s and
/// `bge_smoke.rs`'s test helpers (this repo hand-rolls temp dirs instead of
/// depending on `tempfile`) — duplicated per-file since it's private to each.
fn unique_dir(name: &str) -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "cleophis-kpack-embed-determinism-test-{}-{}-{}",
        std::process::id(),
        unique,
        name
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Mirrors `bge_smoke.rs`'s model path resolution exactly: the bundled
/// `bge-base-en-v1.5` Q8_0 GGUF, fetched by `node tools/fetch-embedder.mjs`.
fn model_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("src-tauri")
        .join("resources")
        .join("embedders")
        .join("bge-base-en-v1.5-q8_0.gguf")
}

/// A small fixed corpus: two MD documents with headings (so `section_path`
/// is exercised) plus one paragraph long enough — under a deliberately
/// small `target_tokens` — that real WordPiece token boundaries actually
/// force a multi-chunk split. `ChunkConfig::default()`'s `target_tokens`
/// (400) would need an unwieldy amount of fixture prose to exceed; a small
/// custom `cfg` (see `test_chunk_config`) makes the same point with a
/// realistic paragraph, exactly as `kpack-core/src/chunk.rs`'s own tests do
/// (`target_tokens: 20`).
fn fixture_sources() -> Vec<SourceInput> {
    vec![
        SourceInput {
            title: "Vitamin K Basics".to_string(),
            source_type: "md".to_string(),
            content: SourceContent::Raw(
                "\
# Chapter 1

## Vitamin K

Vitamin K is a fat-soluble vitamin involved in blood clotting, and it also \
plays a supporting role in bone metabolism through the carboxylation of \
osteocalcin. Clinically, the most important interaction is with warfarin: \
because warfarin works by antagonizing vitamin K's role in activating \
several clotting factors, a sudden change in dietary vitamin K intake can \
significantly shift a patient's INR in either direction, which is why \
anticoagulation clinics generally counsel patients to keep their vitamin K \
intake roughly consistent from week to week rather than eliminating it \
outright.

## Dosing Notes

Typical adult dietary reference intakes are expressed in micrograms per \
day, and the vitamin is fat-soluble, so absorption improves when it is \
consumed alongside a meal containing some dietary fat.
"
                .to_string(),
            ),
        },
        SourceInput {
            title: "Getting Started".to_string(),
            source_type: "md".to_string(),
            content: SourceContent::Raw(
                "\
# Getting Started

## Installation

Run the install script to set up the build tool on this machine, then \
verify the installation by checking the reported version string against \
the release notes for this build.

```
cargo install kpack-cli
```

Verify the installation by checking the reported version string.
"
                .to_string(),
            ),
        },
    ]
}

/// Deliberately small `target_tokens` so the Vitamin K paragraph (well over
/// 20 real WordPiece tokens once CLS/SEP and subword splitting are counted)
/// forces at least one multi-chunk split under the REAL tokenizer — the
/// exact thing `MockEmbedder`'s word-count approximation could get wrong
/// (see the optional `real_token_count_differs_from_mock_approximation`
/// test below).
fn test_chunk_config() -> ChunkConfig {
    ChunkConfig {
        target_tokens: 40,
        overlap_pct: 18,
    }
}

fn test_meta() -> BuildMeta {
    BuildMeta {
        pack_id: "k8b-real-determinism".to_string(),
        pack_version: "2026.07.1".to_string(),
        pack_tier: PackTier::Personal,
        embedder_name: "bge-base-en-v1.5-q8_0.gguf".to_string(),
        // Placeholder — this test never exercises the embedder-hash gate's
        // real security property, only that both mounts use the SAME
        // declared hash so `Pack::mount` accepts them (see `mount_ctx`).
        embedder_sha256: "k8btest0000000000000000000000000000000000000000000000000000".to_string(),
        built_by: "k8b-determinism-test".to_string(),
    }
}

/// Walk `chunks` ids `1, 2, 3, ...` via the public `get_chunk` API until the
/// first `None` — mirrors `kpack-core/src/build.rs`'s own `all_chunks` test
/// helper (duplicated here since that one is private to that module).
fn all_chunks(pack: &Pack) -> Vec<Chunk> {
    let mut out = Vec::new();
    let mut id = 1;
    while let Some(chunk) = pack.get_chunk(id).unwrap() {
        out.push(chunk);
        id += 1;
    }
    out
}

#[test]
#[ignore]
fn real_build_is_deterministic() {
    let model_path = model_path();
    if !model_path.exists() {
        eprintln!(
            "SKIPPED: model missing at {} — run `node tools/fetch-embedder.mjs` first",
            model_path.display()
        );
        return;
    }

    // Two independently loaded `BgeEmbedder` instances — mirrors two
    // separate build processes (device vs. a hypothetical second build),
    // not one embedder reused across both builds, so this actually
    // exercises "does loading the model twice and running real llama.cpp
    // CPU inference twice give the same answer," not just "is one loaded
    // model deterministic across repeated calls."
    let embedder_a = BgeEmbedder::new(&model_path).expect("failed to load BgeEmbedder (build A)");
    let embedder_b = BgeEmbedder::new(&model_path).expect("failed to load BgeEmbedder (build B)");

    let dir = unique_dir("real_build_is_deterministic");
    let path_a = dir.join("a.kpack");
    let path_b = dir.join("b.kpack");
    let cfg = test_chunk_config();
    let meta = test_meta();
    let sources = fixture_sources();

    build_pack(&sources, &embedder_a, &meta, &path_a, &cfg).expect("build A failed");
    build_pack(&sources, &embedder_b, &meta, &path_b, &cfg).expect("build B failed");

    let available = vec![meta.embedder_sha256.clone()];
    let ctx = LoadContext {
        available_embedder_sha256: &available,
        curator_key: None,
    };
    let (pack_a, manifest_a) = Pack::mount(&path_a, &ctx).expect("mount A failed");
    let (pack_b, manifest_b) = Pack::mount(&path_b, &ctx).expect("mount B failed");

    // --- docs.sha256: same source bytes -> same hash, in both packs. ---
    let doc_a1 = pack_a.get_doc(1).unwrap().expect("pack A doc 1 should exist");
    let doc_b1 = pack_b.get_doc(1).unwrap().expect("pack B doc 1 should exist");
    assert_eq!(doc_a1.sha256, doc_b1.sha256, "doc 1 sha256 should match across builds");
    let doc_a2 = pack_a.get_doc(2).unwrap().expect("pack A doc 2 should exist");
    let doc_b2 = pack_b.get_doc(2).unwrap().expect("pack B doc 2 should exist");
    assert_eq!(doc_a2.sha256, doc_b2.sha256, "doc 2 sha256 should match across builds");
    assert!(pack_a.get_doc(3).unwrap().is_none(), "fixture corpus has exactly 2 docs");

    // --- chunk COUNT + (section_path, locator, text, token_count) identical. ---
    let chunks_a = all_chunks(&pack_a);
    let chunks_b = all_chunks(&pack_b);
    assert!(!chunks_a.is_empty(), "fixture corpus should produce at least one chunk");
    assert_eq!(chunks_a.len(), chunks_b.len(), "chunk COUNT must match across builds");
    eprintln!("K8b: chunk count = {}", chunks_a.len());
    assert!(
        chunks_a.len() > 1,
        "fixture + small target_tokens should force multiple chunks (real token boundaries \
         matter) — got only {}; the paragraph fixture may need to be longer",
        chunks_a.len()
    );

    let key = |c: &Chunk| (c.section_path.clone(), c.locator.clone(), c.text.clone(), c.token_count);
    let keys_a: Vec<_> = chunks_a.iter().map(key).collect();
    let keys_b: Vec<_> = chunks_b.iter().map(key).collect();
    assert_eq!(
        keys_a, keys_b,
        "the two builds' (section_path, locator, text, token_count) chunk sets must match \
         exactly — real-tokenizer chunk boundaries must be stable across independent builds"
    );

    // --- stored int8 vectors: byte-identical (the strong claim) vs. tight
    // tolerance (the fallback), reported explicitly either way. ---
    let mut all_exact = true;
    let mut max_abs_diff: i32 = 0;
    for (chunk_a, chunk_b) in chunks_a.iter().zip(chunks_b.iter()) {
        let input = passage_input(&chunk_a.prefix, &chunk_a.text);

        // Recompute the exact fp32 -> normalize -> quantize pipeline
        // `build_pack` ran, independently, from EACH embedder instance —
        // this is a direct `Vec<i8>` equality check in Rust (not an SQL
        // distance trick), and it's the thing that actually determines
        // what each pack's `insert_embedding` call wrote to disk.
        let raw_a = embedder_a.embed_passage(&input).expect("embed_passage (A) failed");
        let raw_b = embedder_b.embed_passage(&input).expect("embed_passage (B) failed");
        let quant_a = quantize_int8(&l2_normalize(&raw_a));
        let quant_b = quantize_int8(&l2_normalize(&raw_b));
        assert_eq!(quant_a.len(), quant_b.len(), "vector dims must match");

        if quant_a != quant_b {
            all_exact = false;
            for (x, y) in quant_a.iter().zip(quant_b.iter()) {
                max_abs_diff = max_abs_diff.max((*x as i32 - *y as i32).abs());
            }
        }

        // Cross-check against what's ACTUALLY on disk in each pack: vec0
        // nearest-neighbor search for the recomputed vector should return
        // this exact chunk at distance 0 in its own pack — i.e. what
        // build_pack stored IS embed(input) quantized, not merely close.
        let hit_a = pack_a.vec_search(&quant_a, 1).unwrap();
        assert_eq!(hit_a.len(), 1);
        assert_eq!(hit_a[0].0, chunk_a.id, "pack A's nearest match to its own recomputed vector should be itself");
        assert_eq!(hit_a[0].1, 0.0, "pack A's stored vector should exactly equal the recomputed one");

        let hit_b = pack_b.vec_search(&quant_b, 1).unwrap();
        assert_eq!(hit_b.len(), 1);
        assert_eq!(hit_b[0].0, chunk_b.id, "pack B's nearest match to its own recomputed vector should be itself");
        assert_eq!(hit_b[0].1, 0.0, "pack B's stored vector should exactly equal the recomputed one");
    }

    if all_exact {
        eprintln!(
            "K8b RESULT: stored int8 vectors were BYTE-IDENTICAL across two independent \
             real-embedder builds ({} chunks) — the strong claim, and the expected result on CPU.",
            chunks_a.len()
        );
    } else {
        eprintln!(
            "K8b RESULT: stored int8 vectors were NOT byte-identical; max |diff| = {max_abs_diff} \
             int8 units across {} chunks — falling back to a tight tolerance assertion.",
            chunks_a.len()
        );
    }
    assert!(
        all_exact,
        "llama.cpp CPU embedding was expected to be bit-deterministic across independent builds; \
         got a max int8 diff of {max_abs_diff}. If this genuinely reproduces, the assertion above \
         should be relaxed to a tolerance (e.g. `max_abs_diff <= 1`) rather than exact equality — \
         see this test's module doc comment."
    );

    // Manifest embedding_dims should agree with the embedder in both packs
    // (sanity — not itself a determinism claim).
    assert_eq!(manifest_a.embedding_dims, manifest_b.embedding_dims);
    assert_eq!(manifest_a.embedding_dims, embedder_a.dims() as u32);
}

/// Documents WHY the §5 determinism test must use the REAL embedder: the
/// real tokenizer's token_count (CLS + WordPiece subwords + SEP) differs
/// from `MockEmbedder`'s crude whitespace word-count approximation on a
/// realistic sentence — so chunk boundaries derived from the mock (K8's own
/// `t2_build_pack_is_deterministic_mock_plumbing_only_not_section_5` test)
/// would NOT match what a real build produces. Reimplements the mock's
/// exact approximation formula inline
/// (`text.split_whitespace().count()`, `kpack-core/src/embed.rs`'s
/// `MockEmbedder::token_count`) rather than depending on `MockEmbedder`
/// itself, since that type is only exported from `kpack-core` under
/// `cfg(test)` / the `test-util` feature, which this crate's `Cargo.toml`
/// deliberately does not enable as a dependency feature for this test.
#[test]
#[ignore]
fn real_token_count_differs_from_mock_approximation() {
    let model_path = model_path();
    if !model_path.exists() {
        eprintln!(
            "SKIPPED: model missing at {} — run `node tools/fetch-embedder.mjs` first",
            model_path.display()
        );
        return;
    }

    let embedder = BgeEmbedder::new(&model_path).expect("failed to load BgeEmbedder");

    let sentence = "Anticoagulation clinics generally counsel patients to keep their \
                     vitamin K intake roughly consistent, since warfarin's effectiveness \
                     is sensitive to sudden dietary changes.";

    let mock_approx = sentence.split_whitespace().count();
    let real_count = embedder.token_count(sentence);

    eprintln!(
        "K8b: mock word-count approximation = {mock_approx}, real tokenizer token_count = {real_count}"
    );
    assert_ne!(
        real_count, mock_approx,
        "real tokenizer token_count should differ from the mock's word-count approximation on a \
         realistic sentence — this is exactly why the §5 determinism test can't be backed by \
         MockEmbedder (see kpack-core/src/embed.rs's MockEmbedder::token_count doc comment)"
    );
}
