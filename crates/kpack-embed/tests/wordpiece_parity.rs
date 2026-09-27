//! Phase 1h M4b: `kpack_core::wordpiece` (the tokenizer-only BGE counter the
//! lexical reference pack is chunked with) against the REAL llama.cpp
//! tokenizer (`BgeEmbedder::token_count`) — the desktop's own count.
//!
//! Compared, every one:
//! - every committed reference-corpus file, whole and paragraph by paragraph;
//! - every chunk text in the committed `tools/reference/build/chunks.jsonl`,
//!   whose recorded `token_count` must equal llama.cpp's count exactly.
//!
//! Requires the `real` feature (libclang + cmake) and the bundled GGUF.
//! `KPACK_BGE_GGUF` overrides the model path (a worktree has no
//! `src-tauri/resources/embedders/`):
//!   KPACK_BGE_GGUF=/path/to/bge-base-en-v1.5-q8_0.gguf \
//!   cargo test -p kpack-embed --features real --test wordpiece_parity -- --ignored --nocapture
#![cfg(feature = "real")]

use std::path::{Path, PathBuf};

use kpack_core::embed::Embedder;
use kpack_core::WordPieceTokenizer;
use kpack_embed::BgeEmbedder;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn model_path() -> PathBuf {
    std::env::var_os("KPACK_BGE_GGUF").map(PathBuf::from).unwrap_or_else(|| {
        repo().join("src-tauri/resources/embedders/bge-base-en-v1.5-q8_0.gguf")
    })
}

fn md_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for section in std::fs::read_dir(dir).expect("corpus dir") {
        let section = section.unwrap().path();
        if section.is_dir() {
            for f in std::fs::read_dir(&section).unwrap() {
                let f = f.unwrap().path();
                if f.extension().map_or(false, |e| e == "md") {
                    out.push(f);
                }
            }
        }
    }
    out.sort();
    out
}

#[test]
#[ignore]
fn wordpiece_counts_equal_llama_cpp_over_the_corpus_and_chunks() {
    let path = model_path();
    assert!(path.exists(), "model missing: {} (set KPACK_BGE_GGUF)", path.display());
    let real = BgeEmbedder::new(&path).expect("load BgeEmbedder");
    let port = WordPieceTokenizer::bge_base_en_v1_5();
    assert_eq!(real.max_input_tokens(), kpack_core::wordpiece::BGE_MAX_INPUT_TOKENS);

    let mut compared = 0usize;
    let mut mismatches: Vec<String> = Vec::new();
    let mut check = |label: &str, text: &str| {
        compared += 1;
        let (a, b) = (real.token_count(text), port.count(text));
        if a != b && mismatches.len() < 20 {
            let head: String = text.chars().take(120).collect();
            mismatches.push(format!("{label}: llama.cpp {a} vs port {b}: {head:?}"));
        }
    };

    let files = md_files(&repo().join("tools/reference/corpus"));
    assert_eq!(files.len(), 941, "the committed corpus");
    for f in &files {
        let text = std::fs::read_to_string(f).unwrap();
        let name = f.file_name().unwrap().to_string_lossy().to_string();
        check(&name, &text);
        for (i, para) in text.split("\n\n").enumerate() {
            check(&format!("{name}#{i}"), para);
        }
    }

    let chunks_path = repo().join("tools/reference/build/chunks.jsonl");
    let mut chunk_count = 0usize;
    if chunks_path.exists() {
        for line in std::fs::read_to_string(&chunks_path).unwrap().lines() {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            let text = v["text"].as_str().unwrap();
            let recorded = v["token_count"].as_u64().unwrap() as usize;
            let id = v["chunk_id"].as_i64().unwrap();
            let a = real.token_count(text);
            compared += 1;
            chunk_count += 1;
            if a != recorded && mismatches.len() < 20 {
                mismatches.push(format!("chunk {id}: llama.cpp {a} vs recorded {recorded}"));
            }
        }
    }

    println!("compared {compared} texts ({} files, {chunk_count} chunks)", files.len());
    assert!(mismatches.is_empty(), "{} mismatches:\n{}", mismatches.len(), mismatches.join("\n"));
}
