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
//! - M5b: the contained-title tier — one-word queries ("anxiety", "adhd")
//!   reach the pages whose titles contain them, and every content word of
//!   every title (3+ chars, not a stop word) resolves (never NotFound).
//!
//! The build runs once (~90 s unoptimised) and is shared by these tests.

mod common;

use common::*;
use kpack_cli::{build_reference, core_name_detail, fake_names, keep_core, strip_parentheticals, Options};
use kpack_core::lookup::{
    normalise_title, population_tail, retrieve_lexical, LexicalOutcome, CONTAINED_MIN_CHARS,
    CONTAINED_STOP_WORDS,
};
use kpack_core::{LoadContext, Pack};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::OnceLock;

/// sha256 of `reference-uk-v1.kpack` built from the committed corpus with
/// SOURCE_DATE_EPOCH=1790380800. Update it (and the report) only together
/// with a corpus, clusters or builder change.
const PACK_SHA256: &str = "5c7b2c98337118ecd8a6fbd07887a639be81371b4e325504997768b41cff1853";

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

/// A copy of the built pack signed with a throwaway key: the curated
/// signature check runs.
fn signed_copy() -> (PathBuf, ed25519_dalek::VerifyingKey) {
    let b = built();
    let dir = unique_dir("real-mount");
    let path = dir.join("reference-uk-v1.kpack");
    std::fs::copy(&b.pack, &path).unwrap();
    let key = sign(&path);
    (path, key)
}

fn mounted() -> Pack {
    let (path, key) = signed_copy();
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

/// Lookup recall: every page's title without parentheticals ("irritable
/// bowel syndrome") and every KEPT core name ("paracetamol", "salbutamol")
/// reach it — Found, or a did-you-mean when several pages share the name.
/// A core name that removed a population tail and belongs to one page only
/// (fix round 1, I2) is not a variant: that bare name never resolves Found
/// through a derived variant to that population-specific page (nor, M5b fix
/// round 1, through the contained-title tier: a lone population-scoped
/// hit is a one-title did-you-mean). Only the page's own slug may find it.
#[test]
fn every_pages_core_name_and_bare_title_resolve() {
    let pack = mounted();
    let t: Value = serde_json::from_slice(&std::fs::read(committed().2.join("titles.json")).unwrap()).unwrap();
    let entries = t["titles"].as_array().unwrap();
    let mut counts = std::collections::BTreeMap::new();
    for e in entries {
        let (core, _) = core_name_detail(e["title"].as_str().unwrap(), e["section"].as_str().unwrap()).unwrap();
        *counts.entry(core).or_insert(0usize) += 1;
    }
    let mut dropped = 0;
    for e in entries {
        let (title, section, slug) =
            (e["title"].as_str().unwrap(), e["section"].as_str().unwrap(), e["slug"].as_str().unwrap());
        let (core, population) = core_name_detail(title, section).unwrap();
        let mut queries = vec![strip_parentheticals(title)];
        if keep_core(population, counts[&core]) {
            queries.push(core);
        } else {
            dropped += 1;
            // Dropped: only the page's own slug (the NHS's URL for it, M5's
            // slug tier) may still find it by that bare name.
            if let LexicalOutcome::Found { title: got, .. } = retrieve_lexical(&pack, &core, 3).unwrap() {
                assert!(got != title || normalise_title(slug) == normalise_title(&core), "{core:?} -> {got}");
            }
        }
        for q in queries {
            match retrieve_lexical(&pack, &q, 3).unwrap() {
                LexicalOutcome::Found { .. } => {}
                LexicalOutcome::DidYouMean { candidates } => assert!(!candidates.is_empty()),
                other => panic!("{q:?} ({title}) -> {other:?}"),
            }
        }
    }
    assert!(dropped > 0);
    // The reviewer's cases (M4b fix round 1, I2): no derived variant makes
    // them Found; the contained-title tier (M5b) lists the pages whose
    // titles (or variants) contain them, shortest title first.
    for (q, want) in [
        (
            "anxiety",
            vec![
                "Health anxiety",
                "Anxiety in pregnancy",
                "Anxiety disorders in children",
                "Social anxiety (social phobia)",
                "Generalised anxiety disorder (GAD)",
            ],
        ),
        ("adhd", vec!["ADHD in adults", "ADHD in children and young people"]),
        (
            "developmental co ordination disorder",
            vec!["Developmental co-ordination disorder (dyspraxia) in children", "Dyspraxia in adults"],
        ),
        // Title hits before Esomeprazole, which matches only through its
        // brand variant "Guardium Acid Reflux Control" (fix round 1, M1).
        // The M4b re-review's edit-tier ["Cefalexin"] (via the brand
        // "Keflex") can no longer win: the edit tier runs only when nothing
        // contains the query.
        ("reflux", vec!["Reflux in babies", "Heartburn and acid reflux", "Esomeprazole"]),
        // Only one title contains "anxiety disorders", and it is scoped to
        // children: offered, never Found (fix round 1, C1; the M4b I2 rule).
        ("anxiety disorders", vec!["Anxiety disorders in children"]),
    ] {
        let want: Vec<String> = want.into_iter().map(String::from).collect();
        assert_eq!(retrieve_lexical(&pack, q, 3).unwrap(), LexicalOutcome::DidYouMean { candidates: want }, "{q}");
    }
    // Found through the page's OWN slug (its NHS URL), not a derived
    // variant — unchanged since round 1, pinned so any change is visible.
    for (q, want) in [("cataracts", "Cataracts in adults"), ("nephrotic syndrome", "Nephrotic syndrome in children")] {
        let LexicalOutcome::Found { title, .. } = retrieve_lexical(&pack, q, 3).unwrap() else { panic!("{q}") };
        assert_eq!(title, want);
    }
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
/// chunk exceeds 256 tokens, and a merged chunk's locator spans a line range
/// (fix round 1, I1). (Never crossing a section holds by construction and is
/// tested in kpack-core, t24.)
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
            let loc = v["locator"].as_str().unwrap();
            let (a, b) = loc.split_once('-').expect("a merged chunk spans lines");
            let n = |s: &str| s.strip_prefix('L').unwrap().parse::<u64>().unwrap();
            assert!(n(a) < n(b), "{loc}");
        }
    }
}

/// M5b fix round 1: kpack-core's `population_tail` (the lookup's
/// population rule) agrees with `core_name_detail`'s population flag (the
/// variant derivation's) on every committed title. No pack build needed.
#[test]
fn the_lookup_population_rule_agrees_with_the_variant_derivation() {
    let t: Value = serde_json::from_slice(&std::fs::read(committed().2.join("titles.json")).unwrap()).unwrap();
    let entries = t["titles"].as_array().unwrap();
    assert_eq!(entries.len(), 941);
    let mut scoped = 0;
    for e in entries {
        let (title, section) = (e["title"].as_str().unwrap(), e["section"].as_str().unwrap());
        let (_, population) = core_name_detail(title, section).unwrap();
        assert_eq!(population_tail(title).is_some(), population, "{title} ({section})");
        scoped += usize::from(population);
    }
    assert!(scoped > 20, "{scoped}");
}

/// M5b recall gate: every content word of every page title — 3+ chars, not
/// a contained-tier stop word — reaches at least its own page (Found, or a
/// did-you-mean when several titles contain it), never NotFound.
#[test]
fn every_title_content_word_resolves() {
    let pack = mounted();
    let titles = pack.titles().unwrap();
    let mut words = std::collections::BTreeSet::new();
    for t in &titles {
        for w in normalise_title(&t.title).split(' ') {
            if w.chars().count() >= CONTAINED_MIN_CHARS && !CONTAINED_STOP_WORDS.contains(&w) {
                words.insert(w.to_string());
            }
        }
    }
    assert!(words.len() > 1200, "{}", words.len());
    for w in &words {
        match retrieve_lexical(&pack, w, 3).unwrap() {
            LexicalOutcome::Found { .. } => {}
            LexicalOutcome::DidYouMean { candidates } => assert!(!candidates.is_empty() && candidates.len() <= 5),
            other => panic!("{w:?} -> {other:?}"),
        }
    }
}

/// M5b addendum: `kpack-cli lookup` over the real signed pack — the
/// two-page did-you-mean, and a Found page whose chunk ids (and every other
/// chunk field) join the committed `chunks.jsonl`.
#[test]
fn lookup_subcommand_over_the_real_pack() {
    let (path, key) = signed_copy();
    let hex: String = key.to_bytes().iter().map(|b| format!("{b:02x}")).collect();
    let batch = path.parent().unwrap().join("q.jsonl");
    std::fs::write(&batch, "\"paracetamol\"\n\"Gout\"\n").unwrap();
    let o = run_cli(
        &["lookup", "--pack", path.to_str().unwrap(), "--curator-key", &hex, "--batch", batch.to_str().unwrap()],
        None,
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let recs: Vec<Value> = String::from_utf8(o.stdout).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(recs.len(), 2);
    let t: Value = serde_json::from_slice(&std::fs::read(committed().2.join("titles.json")).unwrap()).unwrap();
    for r in &recs {
        assert_eq!(r["pack"]["pack_id"], "reference-uk-v1");
        assert_eq!(r["pack"]["content_sha256"], t["header"]["content_sha256"]);
        assert_eq!(r["pack"]["version"], t["header"]["pack_version"]);
    }
    assert_eq!(recs[0]["outcome"], "did_you_mean");
    assert_eq!(
        recs[0]["candidates"],
        serde_json::json!(["Paracetamol for adults", "Paracetamol for children (Calpol)"])
    );
    let gout = &recs[1];
    assert_eq!(gout["outcome"], "found");
    assert_eq!(gout["doc"]["title"], "Gout");
    assert_eq!(gout["doc"]["slug"], "gout");
    let chunks = gout["chunks"].as_array().unwrap();
    assert!(!chunks.is_empty() && chunks.len() <= 3);
    let rows: Vec<Value> = std::fs::read_to_string(committed().2.join("chunks.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    for c in chunks {
        let row = rows.iter().find(|r| r["chunk_id"] == c["chunk_id"]).expect("the chunk id is in chunks.jsonl");
        assert_eq!(row["doc_id"], gout["doc"]["doc_id"]);
        for f in ["section_path", "locator", "url", "retrieved_at", "token_count", "content_sha"] {
            assert_eq!(c[f], row[f], "{f}");
        }
    }
    // The phone's order: the same ids retrieve_lexical returns.
    let LexicalOutcome::Found { chunks: want, .. } = retrieve_lexical(&mounted(), "Gout", 3).unwrap() else {
        panic!("Gout")
    };
    let ids: Vec<i64> = chunks.iter().map(|c| c["chunk_id"].as_i64().unwrap()).collect();
    assert_eq!(ids, want.iter().map(|c| c.id).collect::<Vec<_>>());
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
