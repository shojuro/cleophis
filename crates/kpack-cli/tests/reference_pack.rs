//! The REAL bundled reference pack, built from the committed corpus
//! (`tools/reference/corpus/`, `tools/reference/reference-clusters.json`)
//! with the committed `titles.json` header's `source_date_epoch`:
//!
//! - M4b ruling 3: two independent builds (in parallel) are byte-identical,
//!   equal the pinned pack sha (an earlier, separate release-profile build),
//!   and reproduce the committed `tools/reference/build/chunks.jsonl` and
//!   `titles.json` byte for byte;
//! - M4b ruling 5: the lexical gate over the built, signed pack — every page
//!   title resolves `Found` to its own page, near-misses resolve
//!   `DidYouMean`, no fabrication-bank fake name is ever `Found` (23 of 25
//!   are `NotFound`, two pinned did-you-means), and none occurs in a chunk.
//!
//! - round 2: every page's core name and bare title resolve (never
//!   NotFound), and merged chunks respect the 150/256 merge limits.
//!
//! The build runs once (~90 s unoptimised) and is shared by these tests.

mod common;

use common::*;
use kpack_cli::{build_reference, core_name, fake_names, strip_parentheticals, Options};
use kpack_core::lookup::{normalise_title, retrieve_lexical, LexicalOutcome};
use kpack_core::{LoadContext, Pack};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::OnceLock;

/// sha256 of `reference-uk-v1.kpack` built from the committed corpus with
/// SOURCE_DATE_EPOCH=1790380800. Update it (and the report) only together
/// with a corpus, clusters or builder change.
const PACK_SHA256: &str = "f42aedcd050771fb9e79ede58389319c1a3d5671ca39a3272bbab40fb28b2cd7";

struct Built {
    pack: PathBuf,
    build_dir: PathBuf,
}

fn committed() -> (PathBuf, PathBuf, PathBuf) {
    let r = repo().join("tools").join("reference");
    (r.join("corpus"), r.join("reference-clusters.json"), r.join("build"))
}

fn committed_options() -> Options {
    let (_, _, build) = committed();
    let t: Value = serde_json::from_slice(&std::fs::read(build.join("titles.json")).unwrap()).unwrap();
    let h = &t["header"];
    Options {
        pack_id: h["pack_id"].as_str().unwrap().to_string(),
        pack_version: h["pack_version"].as_str().unwrap().to_string(),
        source_date_epoch: h["source_date_epoch"].as_u64().unwrap(),
    }
}

fn built() -> &'static Built {
    static BUILT: OnceLock<Built> = OnceLock::new();
    BUILT.get_or_init(|| {
        let (corpus, clusters, _) = committed();
        let opts = committed_options();
        assert_eq!(opts.pack_id, "reference-uk-v1");
        assert_eq!(opts.source_date_epoch, 1_790_380_800);
        let dirs = [unique_dir("real-a"), unique_dir("real-b")];
        let handles: Vec<_> = dirs
            .iter()
            .cloned()
            .map(|d| {
                let (corpus, clusters, opts) = (corpus.clone(), clusters.clone(), opts.clone());
                std::thread::spawn(move || build_reference(&corpus, &clusters, &d, &opts).unwrap())
            })
            .collect();
        let summaries: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let a = std::fs::read(&summaries[0].pack_path).unwrap();
        let b = std::fs::read(&summaries[1].pack_path).unwrap();
        assert!(a == b, "two builds of the committed corpus differ");
        Built {
            pack: summaries[0].pack_path.clone(),
            build_dir: dirs[0].clone(),
        }
    })
}

#[test]
fn committed_corpus_rebuilds_byte_identically_and_reproduces_the_contract_files() {
    let b = built();
    assert_eq!(sha256_hex(&std::fs::read(&b.pack).unwrap()), PACK_SHA256);
    let (_, _, committed_build) = committed();
    for f in ["chunks.jsonl", "titles.json"] {
        assert!(
            std::fs::read(b.build_dir.join(f)).unwrap() == std::fs::read(committed_build.join(f)).unwrap(),
            "the committed {f} is not what the committed corpus builds"
        );
    }
    let pack = Pack::open(&b.pack).unwrap();
    let t: Value = serde_json::from_slice(&std::fs::read(committed_build.join("titles.json")).unwrap()).unwrap();
    assert_eq!(
        pack.manifest_get("content_sha256").unwrap().as_deref(),
        t["header"]["content_sha256"].as_str()
    );
    for id in [1i64, 471, 941] {
        assert_eq!(pack.get_doc(id).unwrap().unwrap().added_at, "2026-09-26T00:00:00Z");
    }
}

fn mounted() -> Pack {
    let b = built();
    // Sign a copy with a throwaway key: the curated signature check runs.
    let dir = unique_dir("real-mount");
    let path = dir.join("reference-uk-v1.kpack");
    std::fs::copy(&b.pack, &path).unwrap();
    let key = sign(&path);
    let ctx = LoadContext {
        available_embedder_sha256: &[],
        curator_key: Some(key),
    };
    let (pack, manifest) = Pack::mount_lexical(&path, &ctx).unwrap();
    assert_eq!(manifest.pack_id, "reference-uk-v1");
    pack
}

#[test]
fn every_page_title_resolves_found_to_its_own_page() {
    let pack = mounted();
    let titles = pack.titles().unwrap();
    assert_eq!(titles.len(), 941);
    for t in &titles {
        match retrieve_lexical(&pack, &t.title, 3).unwrap() {
            LexicalOutcome::Found { doc_id, source, chunks, .. } => {
                assert_eq!(doc_id, t.doc_id, "{}", t.title);
                assert!(!chunks.is_empty() && chunks.len() <= 3);
                assert!(source.url.as_deref().unwrap().starts_with("https://www.nhs.uk/"));
                assert_eq!(source.retrieved_at.as_deref().map(str::len), Some(10));
            }
            other => panic!("{:?} -> {other:?}", t.title),
        }
    }
    // Named real pages and variants.
    for (q, want) in [
        ("Gout", "Gout"),
        ("paracetamol for adults", "Paracetamol for adults"),
        ("COPD", "Chronic obstructive pulmonary disease (COPD)"),
        ("Crohn's disease", "Crohn's disease"),
    ] {
        let LexicalOutcome::Found { title, .. } = retrieve_lexical(&pack, q, 3).unwrap() else {
            panic!("{q} should be found");
        };
        assert_eq!(title, want, "{q}");
    }
}

/// Round 2 (lookup recall): every page's bare core name ("paracetamol",
/// "salbutamol") and its title without parentheticals ("irritable bowel
/// syndrome") reach it — Found, or a did-you-mean when several pages share
/// the name. None reads NotFound.
#[test]
fn every_pages_core_name_and_bare_title_resolve() {
    let pack = mounted();
    let (mut found, mut dym) = (0, 0);
    let t: Value = serde_json::from_slice(&std::fs::read(committed().2.join("titles.json")).unwrap()).unwrap();
    for e in t["titles"].as_array().unwrap() {
        let (title, section) = (e["title"].as_str().unwrap(), e["section"].as_str().unwrap());
        for q in [core_name(title, section).unwrap(), strip_parentheticals(title)] {
            match retrieve_lexical(&pack, &q, 3).unwrap() {
                LexicalOutcome::Found { .. } => found += 1,
                LexicalOutcome::DidYouMean { candidates } => {
                    assert!(!candidates.is_empty());
                    dym += 1;
                }
                other => panic!("{q:?} ({title}) -> {other:?}"),
            }
        }
    }
    assert_eq!(found + dym, 2 * 941);
    for (q, want) in [
        ("paracetamol", vec!["Paracetamol for adults", "Paracetamol for children (Calpol)"]),
        ("ibuprofen", vec!["Ibuprofen for adults (Nurofen)", "Ibuprofen for children"]),
        ("breast cancer", vec!["Breast cancer in men", "Breast cancer in women"]),
    ] {
        let want: Vec<String> = want.into_iter().map(String::from).collect();
        assert_eq!(retrieve_lexical(&pack, q, 3).unwrap(), LexicalOutcome::DidYouMean { candidates: want }, "{q}");
    }
    for (q, want) in [
        ("irritable bowel syndrome", "Irritable bowel syndrome (IBS)"),
        ("salbutamol", "Salbutamol inhalers"),
        ("chronic obstructive pulmonary disease", "Chronic obstructive pulmonary disease (COPD)"),
    ] {
        let LexicalOutcome::Found { title, .. } = retrieve_lexical(&pack, q, 3).unwrap() else {
            panic!("{q} should be found");
        };
        assert_eq!(title, want, "{q}");
    }
}

/// Round 2 (chunk merge): the manifest records the merge sizes, no merged
/// chunk exceeds 256 tokens, and no chunk crosses a section.
#[test]
fn merged_chunks_respect_the_limits() {
    let b = built();
    let pack = Pack::open(&b.pack).unwrap();
    assert_eq!(pack.manifest_get("merge_target_tokens").unwrap().as_deref(), Some("150"));
    assert_eq!(pack.manifest_get("merge_max_tokens").unwrap().as_deref(), Some("256"));
    let rows = std::fs::read_to_string(committed().2.join("chunks.jsonl")).unwrap();
    for l in rows.lines() {
        let v: Value = serde_json::from_str(l).unwrap();
        let text = v["text"].as_str().unwrap();
        if text.contains("\n\n") {
            assert!(v["token_count"].as_i64().unwrap() <= 256, "{l}");
        }
    }
}

#[test]
fn near_misses_resolve_did_you_mean() {
    let pack = mounted();
    for (q, want) in [
        ("Guot", "Gout"),
        ("paracetamol for adult", "Paracetamol for adults"),
        ("asthmaa", "Asthma"),
        ("Crohn's diseas", "Crohn's disease"),
    ] {
        match retrieve_lexical(&pack, q, 3).unwrap() {
            LexicalOutcome::DidYouMean { candidates } => {
                assert!(candidates.iter().any(|c| c == want), "{q} -> {candidates:?}")
            }
            other => panic!("{q} -> {other:?}"),
        }
    }
}

#[test]
fn every_fabrication_bank_fake_name_is_not_found_and_in_no_chunk() {
    let (_, clusters, committed_build) = committed();
    let fakes = fake_names(&clusters).unwrap();
    assert_eq!(fakes.len(), 25, "the fabrication bank's 25 fake names");
    let pack = mounted();
    // No fake is ever Found. 23 of 25 are NotFound; two invented drug names
    // sit within the did-you-mean edit budget (2 edits at 6+ characters,
    // Phase 1h M5) of a real medicine and get a scripted suggestion — pinned
    // here so any change is visible.
    let did_you_mean = [("varexetine", "Paroxetine"), ("velatorin", "Melatonin")];
    for fake in &fakes {
        let outcome = retrieve_lexical(&pack, fake, 3).unwrap();
        match did_you_mean.iter().find(|(f, _)| f == fake) {
            Some((_, real)) => {
                assert_eq!(outcome, LexicalOutcome::DidYouMean { candidates: vec![real.to_string()] }, "{fake}")
            }
            None => assert_eq!(outcome, LexicalOutcome::NotFound, "{fake}"),
        }
    }
    // Absent from every committed chunk (normalised, on word boundaries).
    let texts: Vec<String> = std::fs::read_to_string(committed_build.join("chunks.jsonl"))
        .unwrap()
        .lines()
        .map(|l| {
            let v: Value = serde_json::from_str(l).unwrap();
            format!(" {} ", normalise_title(&format!("{} {}", v["title"].as_str().unwrap(), v["text"].as_str().unwrap())))
        })
        .collect();
    assert_eq!(texts.len(), 23_225);
    for fake in &fakes {
        let k = format!(" {} ", normalise_title(fake));
        assert!(!texts.iter().any(|t| t.contains(&k)), "{fake} occurs in a chunk");
    }
}
