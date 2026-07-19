//! BGE embedder smoke test — loads the real bundled `bge-base-en-v1.5`
//! Q8_0 GGUF via `llama-cpp-2` (embedding mode, CLS pooling) and proves the
//! whole real path works: dims, a real passage embedding, the query-prefix
//! drift guard, and semantic sanity (similar text scores higher than
//! dissimilar text). Mirrors `src-tauri/tests/engine_smoke.rs`'s
//! real-artifact, `#[ignore]`d pattern — no mocks.
//!
//! Requires: `node tools/fetch-embedder.mjs` (writes the GGUF this test
//! reads) and the `real` Cargo feature (native llama.cpp build — needs
//! libclang + cmake). Run (Windows, after `$env:LIBCLANG_PATH` is set):
//!   cargo test -p kpack-embed --features real -- --ignored --test-threads=1 --nocapture
#![cfg(feature = "real")]

use std::path::Path;

use kpack_core::embed::{l2_normalize, Embedder};
use kpack_embed::BgeEmbedder;

#[test]
#[ignore]
fn real_embedder_smoke() {
    let model_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("src-tauri")
        .join("resources")
        .join("embedders")
        .join("bge-base-en-v1.5-q8_0.gguf");
    assert!(
        model_path.exists(),
        "model missing: {} — run `node tools/fetch-embedder.mjs` first",
        model_path.display()
    );

    let embedder = BgeEmbedder::new(&model_path).expect("failed to load BgeEmbedder");

    // dims == 768
    assert_eq!(embedder.dims(), 768, "bge-base-en-v1.5 must report 768 dims");

    // embed_passage returns 768 finite f32.
    let passage = embedder
        .embed_passage("The quick brown fox jumps over the lazy dog.")
        .expect("embed_passage failed");
    assert_eq!(passage.len(), 768);
    assert!(
        passage.iter().all(|x| x.is_finite()),
        "embed_passage produced non-finite values"
    );

    // embed_query applies the BGE prefix (kpack_core::embed::query_input)
    // -> differs from embed_passage on identical input text.
    let text = "How does vitamin K interact with warfarin?";
    let as_passage = embedder.embed_passage(text).expect("embed_passage failed");
    let as_query = embedder.embed_query(text).expect("embed_query failed");
    assert_ne!(
        as_passage, as_query,
        "embed_query must differ from embed_passage on identical text (BGE prefix applied)"
    );

    // Semantic sanity: cosine(similar pair) > cosine(dissimilar pair).
    // Vectors are L2-normalized first so a plain dot product IS cosine
    // similarity (kpack-core's embed.rs quant-math convention).
    let anchor = l2_normalize(&embedder.embed_passage("The cat sat on the mat.").unwrap());
    let similar = l2_normalize(
        &embedder
            .embed_passage("A cat was sitting on a rug.")
            .unwrap(),
    );
    let dissimilar = l2_normalize(
        &embedder
            .embed_passage("Quarterly earnings rose 12 percent on strong cloud revenue.")
            .unwrap(),
    );

    let cos_similar = dot(&anchor, &similar);
    let cos_dissimilar = dot(&anchor, &dissimilar);

    eprintln!("k4b: cosine(similar) = {cos_similar:.4}, cosine(dissimilar) = {cos_dissimilar:.4}");

    assert!(
        cos_similar > cos_dissimilar,
        "semantic sanity failed: cosine(similar)={cos_similar} should exceed cosine(dissimilar)={cos_dissimilar}"
    );

    // token_count is the real tokenizer's count (CLS + words + SEP), not a
    // word-count approximation like MockEmbedder's.
    let n = embedder.token_count("hello world");
    eprintln!("k4b: token_count(\"hello world\") = {n}");
    assert!(
        n >= 2,
        "token_count should be at least the word count (real tokenizer adds CLS/SEP on top): got {n}"
    );
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Task B-chunkcap's defensive net: `max_input_tokens()` reports BGE's real
/// 512-token ceiling, and an input tokenizing well past it now TRUNCATES
/// (`Ok`, still 768 dims) instead of hard-erroring — kpack-core's chunker
/// is supposed to keep chunks within the ceiling before they ever reach
/// here, but this proves a chunk that somehow slips through can't abort an
/// entire pack build.
#[test]
#[ignore]
fn real_embedder_over_ceiling_input_truncates_instead_of_erroring() {
    let model_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("src-tauri")
        .join("resources")
        .join("embedders")
        .join("bge-base-en-v1.5-q8_0.gguf");
    assert!(
        model_path.exists(),
        "model missing: {} — run `node tools/fetch-embedder.mjs` first",
        model_path.display()
    );

    let embedder = BgeEmbedder::new(&model_path).expect("failed to load BgeEmbedder");

    assert_eq!(
        embedder.max_input_tokens(),
        512,
        "BgeEmbedder's ceiling must be bge-base-en-v1.5's real 512-token context window"
    );

    // Repeat a common word well past the ceiling (real tokenizer: ~1
    // token/word for "hello", plus CLS/SEP overhead).
    let long_text = "hello ".repeat(600);
    let n = embedder.token_count(&long_text);
    assert!(
        n > 512,
        "test input must tokenize over 512 tokens to exercise the truncation path, got {n}"
    );

    let result = embedder.embed_passage(&long_text);
    assert!(
        result.is_ok(),
        "over-ceiling input must truncate (Ok), not hard-error: {:?}",
        result.err()
    );
    let vec = result.unwrap();
    assert_eq!(vec.len(), 768, "truncated embedding must still report 768 dims");
    assert!(
        vec.iter().all(|x| x.is_finite()),
        "truncated embedding produced non-finite values"
    );
}
