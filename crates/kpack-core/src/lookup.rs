//! Lexical-only reference lookup (Phase 1h M5) — "does the page exist".
//!
//! The phone runs no embedder (the 4 GB floor device holds only the Q6_K
//! base), so reference lookup is LEXICAL ONLY and deterministic:
//!
//! 1. [`normalise_title`] the query.
//! 2. **Exact / normalised match** against the pack's `titles` table (schema
//!    v2): the normalised title, then the normalised slug, then each
//!    normalised variant — the first tier with any match decides. One
//!    document at that tier → [`LexicalOutcome::Found`] with that page's
//!    first `k` (≤ [`LEXICAL_MAX_K`]) chunks in section order, fts5 rank
//!    against the query breaking ties WITHIN a section. More than one
//!    document at the same tier (an ambiguous alias) →
//!    [`LexicalOutcome::DidYouMean`] listing them — never a silent pick.
//! 3. Otherwise **did-you-mean**: restricted Damerau-Levenshtein (optimal
//!    string alignment) distance between the normalised query and every
//!    normalised title/slug/variant; documents within [`max_edits`] of the
//!    query, ordered by (distance, normalised title, doc id), up to
//!    [`DID_YOU_MEAN_MAX`] → [`LexicalOutcome::DidYouMean`].
//! 4. Otherwise [`LexicalOutcome::NotFound`].
//! 5. A pack without the `titles` table (a v1 personal pack) →
//!    [`LexicalOutcome::Unavailable`], never an error.
//!
//! No embedder, no cosine, no gate floors: the lexical gate IS the title
//! match (design ruling I4 — it removes the circular calibration). NO_EVIDENCE
//! and did-you-mean are scripted by the caller; no model runs for either.
//! [`assemble_lexical`] renders a `Found` page into the grounded prompt via
//! `retrieve`'s one assembler.

use crate::contract;
use crate::format::{Chunk, Error, Pack, TitleEntry};
use crate::retrieve::{render_grounded, GroundedItem, RetrievalResult};

/// The most chunks a lexical `Found` ever carries — `retrieve::Tier::Small`'s
/// budget (k = 3, the 2048-token floor-tier budget, design ruling I1).
pub const LEXICAL_MAX_K: usize = 3;

/// The most did-you-mean candidates ever returned.
pub const DID_YOU_MEAN_MAX: usize = 5;

/// The outcome of one lexical lookup. See the module doc comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LexicalOutcome {
    /// The page exists: its title-index `doc_id`, display `title`, and its
    /// first ≤ k chunks in section order (never empty).
    Found {
        doc_id: i64,
        title: String,
        chunks: Vec<Chunk>,
    },
    /// No exact match; these display titles are close (or the query was an
    /// ambiguous alias). Non-empty, ≤ [`DID_YOU_MEAN_MAX`], deterministic.
    DidYouMean { candidates: Vec<String> },
    /// No page and nothing close.
    NotFound,
    /// The pack has no title index (schema v1) — lookup cannot run on it.
    Unavailable,
}

/// The query/title normalisation rule, shared by build (the stored
/// `normalised_title`) and lookup:
///
/// 1. Lowercase (Unicode `to_lowercase`).
/// 2. Fold common Latin diacritics to ASCII (`é`→`e`, `ß`→`ss`, `æ`→`ae`, …).
/// 3. Drop possessives: an apostrophe (`'`, `’`, `‘`, `ʼ`) followed by `s`
///    and then a non-alphanumeric character or the end is removed together
///    with the `s` (`crohn's` → `crohn`).
/// 4. Every remaining character that is not alphanumeric becomes a space
///    (hyphens and slashes included: `co-codamol` → `co codamol`).
/// 5. Collapse runs of whitespace to one space; trim.
pub fn normalise_title(s: &str) -> String {
    let folded: Vec<char> = s.to_lowercase().chars().flat_map(fold_diacritic).collect();
    let is_apostrophe = |c: char| matches!(c, '\'' | '\u{2019}' | '\u{2018}' | '\u{02BC}');
    let mut out = String::with_capacity(folded.len());
    let mut i = 0;
    while i < folded.len() {
        let c = folded[i];
        if is_apostrophe(c)
            && folded.get(i + 1) == Some(&'s')
            && folded.get(i + 2).map_or(true, |n| !n.is_alphanumeric())
        {
            i += 2; // possessive `'s`: dropped entirely
            continue;
        }
        out.push(if c.is_alphanumeric() { c } else { ' ' });
        i += 1;
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// URL-style slug of a title: [`normalise_title`] with spaces as hyphens.
pub fn slugify(title: &str) -> String {
    normalise_title(title).replace(' ', "-")
}

/// Restricted Damerau-Levenshtein (optimal string alignment) distance over
/// Unicode scalar values: insertions, deletions, substitutions and adjacent
/// transpositions each cost 1.
pub fn damerau_levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (n, m) = (a.len(), b.len());
    // d[i][j] = distance between a[..i] and b[..j]; three rolling rows.
    let mut prev2 = vec![0usize; m + 1];
    let mut prev: Vec<usize> = (0..=m).collect();
    let mut cur = vec![0usize; m + 1];
    for i in 1..=n {
        cur[0] = i;
        for j in 1..=m {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut v = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                v = v.min(prev2[j - 2] + 1);
            }
            cur[j] = v;
        }
        std::mem::swap(&mut prev2, &mut prev);
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[m]
}

/// The did-you-mean edit budget for a normalised query of `len` chars —
/// at most 2 (the spec's bound), tightened for short queries so a 3-letter
/// query does not "match" half the index: 0 for ≤ 2 chars, 1 for 3–5, 2 for
/// 6+ (the Elasticsearch `AUTO` fuzziness rule).
pub fn max_edits(len: usize) -> usize {
    match len {
        0..=2 => 0,
        3..=5 => 1,
        _ => 2,
    }
}

/// Run the lexical lookup for `query` over `pack`, returning at most
/// `k.clamp(1, LEXICAL_MAX_K)` chunks on `Found`. See the module doc comment
/// for the algorithm. Errors only on pack I/O failure.
pub fn retrieve_lexical(pack: &Pack, query: &str, k: usize) -> Result<LexicalOutcome, Error> {
    if !pack.has_titles()? {
        return Ok(LexicalOutcome::Unavailable);
    }
    let q = normalise_title(query);
    if q.is_empty() {
        return Ok(LexicalOutcome::NotFound);
    }
    let k = k.clamp(1, LEXICAL_MAX_K);
    let entries = pack.titles()?;

    // Every entry's normalised keys, by tier: 0 title, 1 slug, 2 variants.
    let keyed: Vec<(&TitleEntry, [Vec<String>; 3])> = entries
        .iter()
        .map(|e| {
            let slug = normalise_title(&e.slug);
            let variants = e.variants.iter().map(|v| normalise_title(v)).collect();
            // The stored normalised_title is trusted only if it matches the
            // rule; recomputing keeps a hand-inserted row honest.
            (e, [vec![normalise_title(&e.title)], vec![slug], variants])
        })
        .collect();

    // Exact tier match: the first tier with any hit decides.
    for tier in 0..3 {
        let hits: Vec<&TitleEntry> = keyed
            .iter()
            .filter(|(_, keys)| keys[tier].iter().any(|key| *key == q))
            .map(|(e, _)| *e)
            .collect();
        match hits.len() {
            0 => continue,
            1 => {
                let e = hits[0];
                let chunks = select_page_chunks(pack, e.doc_id, &q, k)?;
                if chunks.is_empty() {
                    // A title with no body is no evidence.
                    return Ok(LexicalOutcome::NotFound);
                }
                return Ok(LexicalOutcome::Found {
                    doc_id: e.doc_id,
                    title: e.title.clone(),
                    chunks,
                });
            }
            _ => {
                let mut ranked: Vec<(usize, &TitleEntry)> =
                    hits.into_iter().map(|e| (0, e)).collect();
                return Ok(did_you_mean(&mut ranked));
            }
        }
    }

    // Did-you-mean: best distance per entry over all its keys.
    let budget = max_edits(q.chars().count());
    if budget == 0 {
        return Ok(LexicalOutcome::NotFound);
    }
    let q_len = q.chars().count();
    let mut ranked: Vec<(usize, &TitleEntry)> = keyed
        .iter()
        .filter_map(|(e, keys)| {
            keys.iter()
                .flatten()
                .filter(|key| key.chars().count().abs_diff(q_len) <= budget)
                .map(|key| damerau_levenshtein(&q, key))
                .min()
                .filter(|d| *d <= budget)
                .map(|d| (d, *e))
        })
        .collect();
    if ranked.is_empty() {
        return Ok(LexicalOutcome::NotFound);
    }
    Ok(did_you_mean(&mut ranked))
}

/// Sort `(distance, entry)` pairs by (distance, normalised title, doc id),
/// dedupe display titles, cap at [`DID_YOU_MEAN_MAX`].
fn did_you_mean(ranked: &mut [(usize, &TitleEntry)]) -> LexicalOutcome {
    ranked.sort_by(|(da, a), (db, b)| {
        da.cmp(db)
            .then_with(|| normalise_title(&a.title).cmp(&normalise_title(&b.title)))
            .then_with(|| a.doc_id.cmp(&b.doc_id))
    });
    let mut candidates: Vec<String> = Vec::new();
    for (_, e) in ranked.iter() {
        if candidates.len() >= DID_YOU_MEAN_MAX {
            break;
        }
        if !candidates.contains(&e.title) {
            candidates.push(e.title.clone());
        }
    }
    LexicalOutcome::DidYouMean { candidates }
}

/// Fold one lowercase char's Latin diacritic to ASCII (a small, fixed table
/// covering Latin-1 and the common Latin Extended-A letters); any other
/// char passes through unchanged.
fn fold_diacritic(c: char) -> Vec<char> {
    let s: &str = match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
        'æ' => "ae",
        'ç' | 'ć' | 'č' | 'ĉ' | 'ċ' => "c",
        'ď' | 'đ' | 'ð' => "d",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => "e",
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => "g",
        'ĥ' | 'ħ' => "h",
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => "i",
        'ĵ' => "j",
        'ķ' => "k",
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => "l",
        'ñ' | 'ń' | 'ņ' | 'ň' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => "o",
        'œ' => "oe",
        'ŕ' | 'ŗ' | 'ř' => "r",
        'ś' | 'ŝ' | 'ş' | 'š' => "s",
        'ß' => "ss",
        'ţ' | 'ť' | 'ŧ' => "t",
        'þ' => "th",
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => "u",
        'ŵ' => "w",
        'ý' | 'ÿ' | 'ŷ' => "y",
        'ź' | 'ż' | 'ž' => "z",
        _ => return vec![c],
    };
    s.chars().collect()
}

/// A page's first `k` chunks in section order (sections by first
/// appearance in chunk-id order; within a section, fts5 rank against
/// `normalised_query` first, unmatched after, then chunk id).
fn select_page_chunks(
    pack: &Pack,
    doc_id: i64,
    normalised_query: &str,
    k: usize,
) -> Result<Vec<Chunk>, Error> {
    let chunks = pack.chunks_for_doc(doc_id)?;
    let fts_query = normalised_query
        .split_whitespace()
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ");
    let fts_rank: Vec<i64> = if fts_query.is_empty() {
        Vec::new()
    } else {
        pack.fts_search_in_doc(&fts_query, doc_id)?
    };
    let mut section_order: Vec<&str> = Vec::new();
    let mut keyed: Vec<(usize, usize, i64, &Chunk)> = chunks
        .iter()
        .map(|c| {
            let section = match section_order.iter().position(|s| *s == c.section_path) {
                Some(p) => p,
                None => {
                    section_order.push(&c.section_path);
                    section_order.len() - 1
                }
            };
            let rank = fts_rank
                .iter()
                .position(|id| *id == c.id)
                .unwrap_or(usize::MAX);
            (section, rank, c.id, c)
        })
        .collect();
    keyed.sort_by_key(|(section, rank, id, _)| (*section, *rank, *id));
    Ok(keyed
        .into_iter()
        .take(k)
        .map(|(_, _, _, c)| c.clone())
        .collect())
}

/// Render a [`LexicalOutcome::Found`] page as the grounded prompt through
/// `retrieve`'s one assembler (the same sources rendering and
/// `doc_context` line as [`crate::retrieve::assemble`]; `doc_context` is the
/// page title). Citations carry `pack_id`. Empty `chunks` → `NoEvidence`.
///
/// SEAM (M3): the system text is `contract::system_contract()` today. When
/// the triage contract lands (`contract::assemble_system`, Task M3), pass
/// its system text here instead — `render_grounded` takes it as a
/// parameter, so that is a one-argument change.
pub fn assemble_lexical(pack_id: &str, title: &str, chunks: &[Chunk]) -> RetrievalResult {
    let items: Vec<GroundedItem<'_>> = chunks
        .iter()
        .take(LEXICAL_MAX_K)
        .map(|chunk| GroundedItem {
            pack_id,
            chunk,
            title,
        })
        .collect();
    render_grounded(contract::contract_for(contract::ContractId::Tutor), &items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::{build_pack, BuildMeta, SourceContent, SourceInput};
    use crate::chunk::ChunkConfig;
    use crate::embed::MockEmbedder;
    use crate::manifest::{LoadContext, PackTier};
    use std::path::PathBuf;

    fn unique_dir(name: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "cleophis-kpack-lookup-test-{}-{}-{}",
            std::process::id(),
            unique,
            name
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn meta() -> BuildMeta {
        BuildMeta {
            pack_id: "reference-test".to_string(),
            pack_version: "2026.09.1".to_string(),
            pack_tier: PackTier::Personal,
            embedder_name: "mock-embedder-8d".to_string(),
            embedder_sha256: "mockhash".repeat(8),
            built_by: "lookup-test".to_string(),
        }
    }

    fn md(title: &str, body: &str) -> SourceInput {
        SourceInput {
            content: SourceContent::Raw(body.to_string()),
            title: title.to_string(),
            source_type: "md".to_string(),
        }
    }

    /// A small reference corpus. Paracetamol has four sections, one of them
    /// long enough to split into several windows; the others are one page
    /// each. Front matter carries a slug and variants.
    fn corpus() -> Vec<SourceInput> {
        let long_dose: String = (0..120)
            .map(|i| format!("Dosing sentence number {i} for adults."))
            .collect::<Vec<_>>()
            .join(" ");
        vec![
            md(
                "Paracetamol",
                &format!(
                    "---\nslug: paracetamol-for-adults\nvariants: [acetaminophen, \"Tylenol\"]\n---\n\
                     # Paracetamol\n\nParacetamol is a common painkiller.\n\n\
                     ## Dose\n\n{long_dose} The maximum daily dose matters.\n\n\
                     ## Side effects\n\nSide effects are rare.\n\n\
                     ## Overdose\n\nGo to A&E if you took too much.\n"
                ),
            ),
            md("Ibuprofen", "# Ibuprofen\n\nIbuprofen is an anti-inflammatory.\n"),
            md(
                "Crohn's disease",
                "---\naliases:\n  - Crohns\n  - regional enteritis\n---\n# Crohn's disease\n\nA long-term condition.\n",
            ),
            md("Ménière's disease", "# Ménière's disease\n\nAn inner ear condition.\n"),
            // Two pages sharing a variant: an ambiguous alias.
            md("Cold sores", "---\nvariants: [herpes]\n---\n# Cold sores\n\nSmall blisters.\n"),
            md("Genital herpes", "---\nvariants: [herpes]\n---\n# Genital herpes\n\nAn STI.\n"),
            md("Flu", "# Flu\n\nA viral infection.\n"),
            md("Asthma", "# Asthma\n\nA lung condition.\n"),
            md("Angina", "# Angina\n\nChest pain.\n"),
        ]
    }

    fn built_pack(name: &str) -> Pack {
        let dir = unique_dir(name);
        let path = dir.join("ref.kpack");
        let m = meta();
        build_pack(
            &corpus(),
            &MockEmbedder::new(8),
            &m,
            &path,
            &ChunkConfig::default(),
        )
        .unwrap();
        let ctx = LoadContext {
            available_embedder_sha256: &[],
            curator_key: None,
        };
        Pack::mount_lexical(&path, &ctx).unwrap().0
    }

    fn found_title(o: &LexicalOutcome) -> &str {
        match o {
            LexicalOutcome::Found { title, .. } => title,
            other => panic!("expected Found, got {other:?}"),
        }
    }

    // --- normalisation --------------------------------------------------

    #[test]
    fn normalise_rule() {
        assert_eq!(normalise_title("  Paracetamol  "), "paracetamol");
        assert_eq!(normalise_title("Crohn's disease"), "crohn disease");
        assert_eq!(normalise_title("CROHN’S DISEASE"), "crohn disease");
        assert_eq!(normalise_title("Ménière's disease"), "meniere disease");
        assert_eq!(normalise_title("co-codamol"), "co codamol");
        assert_eq!(normalise_title("A&E: what / when?"), "a e what when");
        assert_eq!(normalise_title("patients' rights"), "patients rights");
        assert_eq!(
            normalise_title("it'sy"),
            "it sy",
            "not a possessive: s is followed by a letter"
        );
        assert_eq!(normalise_title("Straße"), "strasse");
        assert_eq!(normalise_title("!!!"), "");
        assert_eq!(slugify("Crohn's disease"), "crohn-disease");
    }

    #[test]
    fn damerau_levenshtein_distances() {
        assert_eq!(damerau_levenshtein("", ""), 0);
        assert_eq!(damerau_levenshtein("abc", ""), 3);
        assert_eq!(damerau_levenshtein("asthma", "asthma"), 0);
        assert_eq!(
            damerau_levenshtein("asthma", "ashtma"),
            1,
            "adjacent transposition"
        );
        assert_eq!(
            damerau_levenshtein("asthma", "athsma"),
            2,
            "a non-adjacent move is two edits"
        );
        assert_eq!(damerau_levenshtein("paracetamol", "paracetmol"), 1);
        assert_eq!(damerau_levenshtein("paracetamol", "parcetamoll"), 2);
        assert_eq!(damerau_levenshtein("kitten", "sitting"), 3);
        assert_eq!(damerau_levenshtein("é", "e"), 1, "counts chars, not bytes");
    }

    #[test]
    fn max_edits_budget() {
        assert_eq!(max_edits(2), 0);
        assert_eq!(max_edits(3), 1);
        assert_eq!(max_edits(5), 1);
        assert_eq!(max_edits(6), 2);
        assert_eq!(max_edits(40), 2);
    }

    // --- the pack's title index ------------------------------------------

    #[test]
    fn build_writes_the_title_index_from_title_and_front_matter() {
        let pack = built_pack("index");
        let titles = pack.titles().unwrap();
        assert_eq!(titles.len(), corpus().len());
        let para = &titles[0];
        assert_eq!(para.title, "Paracetamol");
        assert_eq!(para.normalised_title, "paracetamol");
        assert_eq!(para.slug, "paracetamol-for-adults");
        assert_eq!(
            para.variants,
            vec!["acetaminophen".to_string(), "Tylenol".to_string()]
        );
        let crohn = titles
            .iter()
            .find(|t| t.title == "Crohn's disease")
            .unwrap();
        assert_eq!(crohn.slug, "crohn-disease");
        assert_eq!(
            crohn.variants,
            vec!["Crohns".to_string(), "regional enteritis".to_string()]
        );
        // Front matter is stripped before parsing: no chunk carries it.
        for c in pack.chunks_for_doc(para.doc_id).unwrap() {
            assert!(
                !c.text.contains("slug:"),
                "front matter leaked into {:?}",
                c.text
            );
        }
    }

    // --- outcomes --------------------------------------------------------

    #[test]
    fn exact_title_match_is_found() {
        let pack = built_pack("exact");
        let o = retrieve_lexical(&pack, "Paracetamol", 3).unwrap();
        assert_eq!(found_title(&o), "Paracetamol");
    }

    #[test]
    fn normalised_match_is_found() {
        let pack = built_pack("normalised");
        for q in [
            "CROHN’S   disease!",
            "crohn disease",
            "meniere's disease",
            "Meniere disease",
        ] {
            let o = retrieve_lexical(&pack, q, 3).unwrap();
            assert!(matches!(o, LexicalOutcome::Found { .. }), "{q:?} -> {o:?}");
        }
        // No apostrophe = not a possessive: "crohns" is one edit away, so
        // it is a did-you-mean, not a silent match.
        assert_eq!(
            retrieve_lexical(&pack, "crohns disease", 3).unwrap(),
            LexicalOutcome::DidYouMean {
                candidates: vec!["Crohn's disease".to_string()]
            }
        );
    }

    #[test]
    fn slug_and_variant_matches_are_found() {
        let pack = built_pack("variant");
        for q in ["paracetamol-for-adults", "Acetaminophen", "tylenol"] {
            let o = retrieve_lexical(&pack, q, 3).unwrap();
            assert_eq!(found_title(&o), "Paracetamol", "{q:?}");
        }
        let o = retrieve_lexical(&pack, "regional enteritis", 3).unwrap();
        assert_eq!(found_title(&o), "Crohn's disease");
    }

    #[test]
    fn found_chunks_are_in_section_order_capped_at_k() {
        let pack = built_pack("order");
        let o = retrieve_lexical(&pack, "paracetamol", 3).unwrap();
        let LexicalOutcome::Found { doc_id, chunks, .. } = o else {
            panic!("expected Found")
        };
        let all = pack.chunks_for_doc(doc_id).unwrap();
        assert!(
            all.len() > 3,
            "fixture must have more chunks than k: {}",
            all.len()
        );
        assert_eq!(chunks.len(), 3);
        // Sections appear in document order: the intro first.
        assert!(
            chunks[0].text.contains("common painkiller"),
            "{:?}",
            chunks[0].text
        );
        let order: Vec<usize> = chunks
            .iter()
            .map(|c| {
                all.iter()
                    .position(|a| a.section_path == c.section_path)
                    .unwrap()
            })
            .collect();
        assert!(
            order.windows(2).all(|w| w[0] <= w[1]),
            "section order: {order:?}"
        );

        // k is capped at LEXICAL_MAX_K, and floored at 1.
        let o = retrieve_lexical(&pack, "paracetamol", 50).unwrap();
        let LexicalOutcome::Found { chunks, .. } = o else {
            panic!()
        };
        assert_eq!(chunks.len(), LEXICAL_MAX_K);
        let o = retrieve_lexical(&pack, "paracetamol", 0).unwrap();
        let LexicalOutcome::Found { chunks, .. } = o else {
            panic!()
        };
        assert_eq!(chunks.len(), 1);
    }

    #[test]
    fn fts_rank_breaks_ties_within_a_section() {
        let pack = built_pack("tiebreak");
        // "Paracetamol maximum" matches the dose section's LAST window (it
        // alone says "maximum"), so within the dose section that window is
        // promoted ahead of its earlier siblings. Section order still holds.
        let LexicalOutcome::Found { doc_id, .. } =
            retrieve_lexical(&pack, "paracetamol", 3).unwrap()
        else {
            panic!()
        };
        let all = pack.chunks_for_doc(doc_id).unwrap();
        let dose: Vec<&Chunk> = all
            .iter()
            .filter(|c| c.section_path.contains("Dose"))
            .collect();
        assert!(dose.len() >= 2, "dose section must split: {}", dose.len());
        let with_max = dose.iter().find(|c| c.text.contains("maximum")).unwrap();
        assert_ne!(
            with_max.id, dose[0].id,
            "fixture: 'maximum' must not be in the first window"
        );

        let chunks = select_page_chunks(&pack, doc_id, "maximum", 3).unwrap();
        assert!(
            chunks[0].text.contains("common painkiller"),
            "intro section stays first"
        );
        assert_eq!(
            chunks[1].id, with_max.id,
            "the matching dose window leads its section"
        );
    }

    #[test]
    fn typo_gives_did_you_mean_in_deterministic_order() {
        let pack = built_pack("typo");
        let o = retrieve_lexical(&pack, "paracetmol", 3).unwrap();
        assert_eq!(
            o,
            LexicalOutcome::DidYouMean {
                candidates: vec!["Paracetamol".to_string()]
            }
        );

        // "angna" (5 chars, budget 1) is 1 from "angina"; "asthma"/"flu" are far.
        let o = retrieve_lexical(&pack, "angna", 3).unwrap();
        assert_eq!(
            o,
            LexicalOutcome::DidYouMean {
                candidates: vec!["Angina".to_string()]
            }
        );

        // Ordered by distance, then normalised title: "ibuprofin" is 1 from
        // ibuprofen. A variant typo resolves to its page's display title.
        let o = retrieve_lexical(&pack, "acetaminophin", 3).unwrap();
        assert_eq!(
            o,
            LexicalOutcome::DidYouMean {
                candidates: vec!["Paracetamol".to_string()]
            }
        );

        // Same result every call.
        let a = retrieve_lexical(&pack, "ibuprofin", 3).unwrap();
        let b = retrieve_lexical(&pack, "ibuprofin", 3).unwrap();
        assert_eq!(a, b);
        assert_eq!(
            a,
            LexicalOutcome::DidYouMean {
                candidates: vec!["Ibuprofen".to_string()]
            }
        );
    }

    #[test]
    fn did_you_mean_orders_by_distance_then_title_and_caps_at_five() {
        let dir = unique_dir("dym-order");
        let path = dir.join("dym.kpack");
        // Distances from "abcdefgh": abcdefgx=1, abcdefxx=2, abcdefgy=1,
        // abcdefhg=1 (transposition), zzzzzzzz far, plus more distance-2s.
        let titles = [
            "Abcdefxx", "Abcdefgy", "Zzzzzzzz", "Abcdefgx", "Abcdefhg", "Abcdexxh", "Abcdxfgx",
        ];
        let sources: Vec<SourceInput> = titles
            .iter()
            .map(|t| md(t, &format!("# {t}\n\nBody.\n")))
            .collect();
        build_pack(
            &sources,
            &MockEmbedder::new(8),
            &meta(),
            &path,
            &ChunkConfig::default(),
        )
        .unwrap();
        let ctx = LoadContext {
            available_embedder_sha256: &[],
            curator_key: None,
        };
        let pack = Pack::mount_lexical(&path, &ctx).unwrap().0;
        let o = retrieve_lexical(&pack, "abcdefgh", 3).unwrap();
        assert_eq!(
            o,
            LexicalOutcome::DidYouMean {
                candidates: vec![
                    "Abcdefgx".to_string(),
                    "Abcdefgy".to_string(),
                    "Abcdefhg".to_string(),
                    "Abcdefxx".to_string(),
                    "Abcdexxh".to_string(),
                ]
            }
        );
    }

    #[test]
    fn ambiguous_alias_is_did_you_mean_not_a_silent_pick() {
        let pack = built_pack("ambiguous");
        let o = retrieve_lexical(&pack, "herpes", 3).unwrap();
        assert_eq!(
            o,
            LexicalOutcome::DidYouMean {
                candidates: vec!["Cold sores".to_string(), "Genital herpes".to_string()]
            }
        );
    }

    #[test]
    fn unknown_entity_is_not_found() {
        let pack = built_pack("notfound");
        for q in ["zorbanex", "", "   ", "!!", "fl"] {
            assert_eq!(
                retrieve_lexical(&pack, q, 3).unwrap(),
                LexicalOutcome::NotFound,
                "{q:?}"
            );
        }
    }

    #[test]
    fn old_schema_pack_is_unavailable_not_an_error() {
        let dir = unique_dir("v1");
        let path = dir.join("v1.kpack");
        build_pack(
            &corpus(),
            &MockEmbedder::new(8),
            &meta(),
            &path,
            &ChunkConfig::default(),
        )
        .unwrap();
        // Rewrite it into a v1 pack: no titles table, schema_version 1.
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(
                "DROP TABLE titles; UPDATE manifest SET value = '1' WHERE key = 'schema_version';",
            )
            .unwrap();
        }
        let ctx = LoadContext {
            available_embedder_sha256: &[],
            curator_key: None,
        };
        let (pack, manifest) = Pack::mount_lexical(&path, &ctx).unwrap();
        assert_eq!(manifest.schema_version, 1);
        assert_eq!(
            retrieve_lexical(&pack, "Paracetamol", 3).unwrap(),
            LexicalOutcome::Unavailable
        );
    }

    #[test]
    fn assemble_lexical_reuses_the_one_assembler() {
        let pack = built_pack("assemble");
        let LexicalOutcome::Found { title, chunks, .. } =
            retrieve_lexical(&pack, "ibuprofen", 3).unwrap()
        else {
            panic!()
        };
        let RetrievalResult::Grounded { prompt, citations } =
            assemble_lexical("reference-test", &title, &chunks)
        else {
            panic!("expected Grounded")
        };
        assert!(prompt.starts_with(contract::system_contract().trim_end()));
        assert!(prompt.contains("These sources are excerpts from: Ibuprofen."));
        assert!(prompt.contains("Ibuprofen is an anti-inflammatory."));
        assert_eq!(citations.len(), chunks.len());
        assert_eq!(citations[0].n, 1);
        assert_eq!(citations[0].pack_id, "reference-test");
        assert_eq!(citations[0].doc_title, "Ibuprofen");
        assert_eq!(assemble_lexical("p", "t", &[]), RetrievalResult::NoEvidence);
    }
}
