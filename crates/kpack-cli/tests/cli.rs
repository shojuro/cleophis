//! kpack-cli over a 3-page fixture corpus (`tests/fixtures/`, rendered with
//! `fetch_nhs.py`'s own writer and `clusters.py`'s own `entry_names`): the
//! command-line interface, the contract files, reproducibility, `verify`,
//! and the input checks.

mod common;

use common::*;
use kpack_core::lookup::{normalise_title, retrieve_lexical, LexicalOutcome};
use kpack_core::{chunk_content_sha256, pack_content_sha256, LoadContext, Pack, PackTier};
use serde_json::Value;
use std::path::{Path, PathBuf};

const EPOCH: &str = "1790380800"; // 2026-09-26T00:00:00Z

fn fixtures() -> (PathBuf, PathBuf) {
    let f = manifest_dir().join("tests").join("fixtures");
    (f.join("corpus"), f.join("clusters.json"))
}

fn build(out: &Path) -> std::process::Output {
    let (corpus, clusters) = fixtures();
    run_cli(
        &[
            "build-reference",
            "--corpus",
            corpus.to_str().unwrap(),
            "--clusters",
            clusters.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ],
        Some(EPOCH),
    )
}

fn lines(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn build_reference_writes_the_pack_and_both_contract_files() {
    let out = unique_dir("build");
    let o = build(&out);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let stdout = String::from_utf8_lossy(&o.stdout);
    assert!(stdout.contains("docs            3"), "{stdout}");
    for f in ["reference-uk-v1.kpack", "chunks.jsonl", "titles.json"] {
        assert!(out.join(f).is_file(), "{f}");
    }
    assert!(!out.join("reference-uk-v1.kpack.part").exists());
}

#[test]
fn chunks_jsonl_is_exactly_the_packs_chunks_in_pack_order() {
    let out = unique_dir("chunks");
    assert!(build(&out).status.success());
    let rows = lines(&out.join("chunks.jsonl"));
    let pack = Pack::open(out.join("reference-uk-v1.kpack")).unwrap();
    let mut expected = Vec::new();
    for doc_id in 1..=3 {
        let doc = pack.get_doc(doc_id).unwrap().unwrap();
        for c in pack.chunks_for_doc(doc_id).unwrap() {
            expected.push((doc.clone(), c));
        }
    }
    assert!(pack.get_doc(4).unwrap().is_none());
    assert_eq!(rows.len(), expected.len());
    let keys = [
        "chunk_id", "doc_id", "slug", "title", "section_path", "locator", "url", "retrieved_at", "text",
        "token_count", "content_sha",
    ];
    // Field order is the contract: check it on the raw line (a parsed
    // serde_json map sorts its keys).
    let raw = std::fs::read_to_string(out.join("chunks.jsonl")).unwrap();
    for line in raw.lines() {
        let at: Vec<usize> = keys.iter().map(|k| line.find(&format!("\"{k}\":")).expect(k)).collect();
        assert!(at.windows(2).all(|w| w[0] < w[1]), "field order: {line}");
    }
    let mut shas = Vec::new();
    for (row, (doc, c)) in rows.iter().zip(&expected) {
        assert_eq!(row.as_object().unwrap().len(), keys.len());
        assert_eq!(row["chunk_id"], c.id);
        assert_eq!(row["doc_id"], c.doc_id);
        assert_eq!(row["title"], doc.title.as_str());
        assert_eq!(row["section_path"], c.section_path.as_str());
        assert_eq!(row["locator"], c.locator.as_str());
        assert_eq!(row["text"], c.text.as_str());
        assert_eq!(row["token_count"], c.token_count);
        assert_eq!(row["url"], doc.source_path.as_deref().unwrap());
        assert_eq!(row["retrieved_at"], doc.source_mtime.as_deref().unwrap());
        let sha = chunk_content_sha256(&c.text, &c.section_path, &c.locator, &doc.title);
        assert_eq!(row["content_sha"], sha.as_str());
        shas.push(sha);
    }
    // Pack order: (slug, section) — chronic-obstructive..., gout, paracetamol-for-adults.
    let slugs: Vec<&str> = rows.iter().map(|r| r["slug"].as_str().unwrap()).collect();
    let mut sorted = slugs.clone();
    sorted.sort();
    assert_eq!(slugs, sorted);
    assert_eq!(slugs[0], "chronic-obstructive-pulmonary-disease-copd");
    // The content sha: manifest == titles header == recomputed.
    let content = pack_content_sha256(shas.iter().map(String::as_str));
    assert_eq!(pack.manifest_get("content_sha256").unwrap().as_deref(), Some(content.as_str()));
    let titles: Value = serde_json::from_slice(&std::fs::read(out.join("titles.json")).unwrap()).unwrap();
    assert_eq!(titles["header"]["content_sha256"], content.as_str());
    assert_eq!(titles["header"]["chunks"], rows.len());
}

#[test]
fn titles_json_has_one_entry_per_page_with_merged_variants_and_sources() {
    let out = unique_dir("titles");
    assert!(build(&out).status.success());
    let t: Value = serde_json::from_slice(&std::fs::read(out.join("titles.json")).unwrap()).unwrap();
    let h = &t["header"];
    assert_eq!(h["schema"], "cleophis/reference-titles/v1");
    assert_eq!(h["pack_id"], "reference-uk-v1");
    assert_eq!(h["pack_file"], "reference-uk-v1.kpack");
    assert_eq!(h["source_date_epoch"], 1_790_380_800u64);
    assert_eq!(h["titles"], 3);
    let entries = t["titles"].as_array().unwrap();
    assert_eq!(entries.len(), 3);
    let copd = &entries[0];
    assert_eq!(copd["title"], "Chronic obstructive pulmonary disease (COPD)");
    assert_eq!(copd["normalised_title"], "chronic obstructive pulmonary disease copd");
    // The title's parenthetical, from the clusters file.
    assert!(copd["variants"].as_array().unwrap().contains(&Value::from("COPD")));
    assert_eq!(copd["retrieved_at"], "2026-09-25");
    let gout = &entries[1];
    assert!(gout["variants"].as_array().unwrap().contains(&Value::from("Gouty arthritis")));
    assert_eq!(gout["url"], "https://www.nhs.uk/conditions/gout/");
    let para = &entries[2];
    let v = para["variants"].as_array().unwrap();
    assert!(v.contains(&Value::from("Calpol")) && v.contains(&Value::from("Panadol")), "brands are variants");
    assert!(!v.contains(&Value::from("paracetamol")), "a unique population core is not a variant");
    assert!(copd["variants"].as_array().unwrap().contains(&Value::from("Chronic obstructive pulmonary disease")));
    assert_eq!(h["merge_target_tokens"], 150);
    assert_eq!(h["merge_max_tokens"], 256);
    assert_eq!(para["section"], "medicines");
}

#[test]
fn a_rebuild_is_byte_identical_and_verify_passes_then_catches_tampering() {
    let a = unique_dir("det-a");
    let b = unique_dir("det-b");
    assert!(build(&a).status.success());
    assert!(build(&b).status.success());
    for f in ["reference-uk-v1.kpack", "chunks.jsonl", "titles.json"] {
        assert_eq!(std::fs::read(a.join(f)).unwrap(), std::fs::read(b.join(f)).unwrap(), "{f}");
    }
    let (corpus, clusters) = fixtures();
    let verify = |dir: &Path| {
        run_cli(
            &[
                "verify",
                "--corpus",
                corpus.to_str().unwrap(),
                "--clusters",
                clusters.to_str().unwrap(),
                "--dir",
                dir.to_str().unwrap(),
            ],
            None,
        )
    };
    let o = verify(&a);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(String::from_utf8_lossy(&o.stdout).contains("pack compared   yes"));
    // A different epoch changes the pack bytes, not the content.
    let c = unique_dir("det-c");
    let (corpus_s, clusters_s) = (corpus.to_str().unwrap(), clusters.to_str().unwrap());
    let o = run_cli(
        &["build-reference", "--corpus", corpus_s, "--clusters", clusters_s, "--out", c.to_str().unwrap()],
        Some("1790380801"),
    );
    assert!(o.status.success());
    assert_ne!(std::fs::read(a.join("reference-uk-v1.kpack")).unwrap(), std::fs::read(c.join("reference-uk-v1.kpack")).unwrap());
    assert_eq!(std::fs::read(a.join("chunks.jsonl")).unwrap(), std::fs::read(c.join("chunks.jsonl")).unwrap());
    // Tampering is caught.
    let mut chunks = std::fs::read_to_string(b.join("chunks.jsonl")).unwrap();
    chunks = chunks.replacen("painkiller", "pain-killer", 1);
    std::fs::write(b.join("chunks.jsonl"), chunks).unwrap();
    let o = verify(&b);
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("chunks.jsonl differs"));
}

#[test]
fn the_built_pack_mounts_lexically_and_answers_found_did_you_mean_not_found() {
    let out = unique_dir("mount");
    assert!(build(&out).status.success());
    let path = out.join("reference-uk-v1.kpack");
    let key = sign(&path);
    let ctx = LoadContext {
        available_embedder_sha256: &[],
        curator_key: Some(key),
    };
    let (pack, manifest) = Pack::mount_lexical(&path, &ctx).unwrap();
    assert_eq!(manifest.pack_id, "reference-uk-v1");
    assert_eq!(manifest.pack_tier, PackTier::Curated);
    assert_eq!(manifest.chunk_target_tokens, 400);
    let LexicalOutcome::Found { title, source, chunks, .. } = retrieve_lexical(&pack, "COPD", 3).unwrap() else {
        panic!("COPD should be found");
    };
    assert_eq!(title, "Chronic obstructive pulmonary disease (COPD)");
    assert_eq!(source.retrieved_at.as_deref(), Some("2026-09-25"));
    assert!(!chunks.is_empty());
    assert!(matches!(retrieve_lexical(&pack, "Calpol", 3).unwrap(), LexicalOutcome::Found { .. }));
    // The parenthetical-free title resolves. The fixture's only paracetamol
    // page is population-specific ("for adults"), so its bare core name is
    // NOT a variant (fix round 1, I2). It is still Found — through the
    // contained-title tier (Phase 1h M5b), because that page's title is the
    // only one containing "paracetamol".
    let row = pack.titles().unwrap().into_iter().find(|t| t.title == "Paracetamol for adults").unwrap();
    assert!(!row.variants.iter().any(|v| normalise_title(v) == "paracetamol"), "{:?}", row.variants);
    let LexicalOutcome::Found { title, .. } = retrieve_lexical(&pack, "paracetamol", 3).unwrap() else {
        panic!("paracetamol: the one containing title")
    };
    assert_eq!(title, "Paracetamol for adults");
    assert!(matches!(
        retrieve_lexical(&pack, "chronic obstructive pulmonary disease", 3).unwrap(),
        LexicalOutcome::Found { .. }
    ));
    let o = retrieve_lexical(&pack, "paracetamol for adult", 3).unwrap();
    assert_eq!(o, LexicalOutcome::DidYouMean { candidates: vec!["Paracetamol for adults".to_string()] });
    assert_eq!(retrieve_lexical(&pack, "Zeltrofen", 3).unwrap(), LexicalOutcome::NotFound);
    // A wrong key refuses the pack: the signature check is not relaxed.
    let other = ed25519_dalek::SigningKey::from_bytes(&[9u8; 32]).verifying_key();
    let bad = LoadContext {
        available_embedder_sha256: &[],
        curator_key: Some(other),
    };
    assert!(Pack::mount_lexical(&path, &bad).is_err());
}

#[test]
fn source_date_epoch_is_required_and_bad_arguments_are_refused() {
    let out = unique_dir("args");
    let (corpus, clusters) = fixtures();
    let (c, k, o) = (corpus.to_str().unwrap(), clusters.to_str().unwrap(), out.to_str().unwrap());
    let r = run_cli(&["build-reference", "--corpus", c, "--clusters", k, "--out", o], None);
    assert!(!r.status.success());
    assert!(String::from_utf8_lossy(&r.stderr).contains("SOURCE_DATE_EPOCH must be set"));
    let r = run_cli(&["build-reference", "--corpus", c, "--clusters", k, "--out", o], Some("soon"));
    assert!(!r.status.success());
    let r = run_cli(&["build-reference", "--corpus", c, "--out", o], Some(EPOCH));
    assert!(String::from_utf8_lossy(&r.stderr).contains("--clusters is required"));
    let r = run_cli(&["build-reference", "--corpus", c, "--clusters", k, "--out", o, "--bogus", "x"], Some(EPOCH));
    assert!(String::from_utf8_lossy(&r.stderr).contains("unknown argument"));
    let r = run_cli(&["build-reference", "--corpus", c, "--clusters", k, "--out", o, "--pack-id", "Ref UK"], Some(EPOCH));
    assert!(!r.status.success());
    assert!(!out.join("reference-uk-v1.kpack").exists());
}

/// Copy the fixture corpus somewhere it can be damaged.
fn corpus_copy(name: &str) -> (PathBuf, PathBuf) {
    let (corpus, clusters) = fixtures();
    let dir = unique_dir(name);
    for section in ["medicines", "conditions"] {
        std::fs::create_dir_all(dir.join("corpus").join(section)).unwrap();
        for f in std::fs::read_dir(corpus.join(section)).unwrap() {
            let f = f.unwrap().path();
            std::fs::copy(&f, dir.join("corpus").join(section).join(f.file_name().unwrap())).unwrap();
        }
    }
    std::fs::copy(corpus.join("index.json"), dir.join("corpus/index.json")).unwrap();
    std::fs::copy(&clusters, dir.join("clusters.json")).unwrap();
    (dir.join("corpus"), dir.join("clusters.json"))
}

fn build_from(corpus: &Path, clusters: &Path) -> std::process::Output {
    let out = corpus.parent().unwrap().join("out");
    run_cli(
        &[
            "build-reference",
            "--corpus",
            corpus.to_str().unwrap(),
            "--clusters",
            clusters.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ],
        Some(EPOCH),
    )
}

#[test]
fn a_corpus_body_that_disagrees_with_the_index_is_refused() {
    let (corpus, clusters) = corpus_copy("tamper-body");
    let p = corpus.join("conditions/gout.md");
    let text = std::fs::read_to_string(&p).unwrap().replace("sudden severe pain", "mild pain");
    std::fs::write(&p, text).unwrap();
    let o = build_from(&corpus, &clusters);
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("body sha256 differs"));
}

#[test]
fn a_clusters_file_that_disagrees_with_the_corpus_is_refused() {
    let (corpus, clusters) = corpus_copy("tamper-clusters");
    let mut v: Value = serde_json::from_slice(&std::fs::read(&clusters).unwrap()).unwrap();
    // Drop a page's own name from its cluster.
    let names = v["clusters"][1]["names"].as_array_mut().unwrap();
    names.retain(|n| n != "Gouty arthritis");
    std::fs::write(&clusters, serde_json::to_vec(&v).unwrap()).unwrap();
    let o = build_from(&corpus, &clusters);
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("lacks \"Gouty arthritis\""), "{}", String::from_utf8_lossy(&o.stderr));
    // A cluster name that belongs to no page.
    let (corpus, clusters) = corpus_copy("tamper-clusters-2");
    let mut v: Value = serde_json::from_slice(&std::fs::read(&clusters).unwrap()).unwrap();
    v["clusters"][1]["names"].as_array_mut().unwrap().push(Value::from("Podagra"));
    std::fs::write(&clusters, serde_json::to_vec(&v).unwrap()).unwrap();
    let o = build_from(&corpus, &clusters);
    assert!(String::from_utf8_lossy(&o.stderr).contains("belongs to none of its pages"));
}
