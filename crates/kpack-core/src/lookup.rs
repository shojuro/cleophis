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
//! 3. Otherwise **contained title** (Phase 1h M5b): when the normalised
//!    query has at least [`CONTAINED_MIN_CHARS`] chars and a word outside
//!    [`CONTAINED_STOP_WORDS`], every document with a normalised title, slug
//!    (hyphens as spaces) or variant containing the query as a whole-word
//!    sequence is a hit. One document → `Found` (same chunk selection);
//!    several → `DidYouMean` with up to [`DID_YOU_MEAN_MAX`] display titles
//!    ordered by title length (chars) ascending, then normalised title, then
//!    doc id — the shortest containing title is the likeliest intent
//!    ("anxiety" → "Health anxiety", "Social anxiety", …).
//! 4. Otherwise (no contained hit) **did-you-mean**: restricted Damerau-Levenshtein (optimal
//!    string alignment) distance between the normalised query and every
//!    normalised title/slug/variant; documents within [`max_edits`] of the
//!    query, ordered by (distance, normalised title, doc id), up to
//!    [`DID_YOU_MEAN_MAX`] → [`LexicalOutcome::DidYouMean`].
//! 5. Otherwise [`LexicalOutcome::NotFound`].
//! 6. A pack without the `titles` table (a v1 personal pack) →
//!    [`LexicalOutcome::Unavailable`], never an error.
//!
//! No embedder, no cosine, no gate floors: the lexical gate IS the title
//! match (design ruling I4 — it removes the circular calibration). NO_EVIDENCE
//! and did-you-mean are scripted by the caller; no model runs for either.
//! [`assemble_lexical`] renders a `Found` page into the grounded prompt via
//! `contract::assemble_system` over the TRIAGE contract ([`lookup_contract`]).

use crate::contract;
use crate::format::{Chunk, Error, Pack, TitleEntry};
use crate::retrieve::{render_grounded, CitationSource, GroundedItem, RetrievalResult};

/// The most chunks a lexical `Found` ever carries — `retrieve::Tier::Small`'s
/// budget (k = 3, the 2048-token floor-tier budget, design ruling I1).
pub const LEXICAL_MAX_K: usize = 3;

/// The most did-you-mean candidates ever returned.
pub const DID_YOU_MEAN_MAX: usize = 5;

/// The outcome of one lexical lookup. See the module doc comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LexicalOutcome {
    /// The page exists: its title-index `doc_id`, display `title`, where
    /// it came from (`source`: URL and retrieval date for an `nhs-web`
    /// page, empty otherwise — Phase 1h M4b), and its first ≤ k chunks in
    /// section order (never empty).
    Found {
        doc_id: i64,
        title: String,
        source: CitationSource,
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

/// The shortest normalised query (in chars) the contained-title tier runs
/// for: a one- or two-letter query ("a", "d3") would be inside half the
/// index.
pub const CONTAINED_MIN_CHARS: usize = 3;

/// Words that on their own never make a contained-title query (Phase 1h
/// M5b): a query made only of these ("and", "in the", "for") skips the
/// tier, because it sits inside hundreds of titles and says nothing about
/// the page wanted. Normalised form, sorted. Population words ("children",
/// "adults") are NOT stop words: they are real, if broad, queries.
pub const CONTAINED_STOP_WORDS: &[&str] = &[
    "a", "about", "after", "an", "and", "are", "as", "at", "be", "before", "by", "can", "do", "during",
    "for", "from", "how", "i", "if", "in", "is", "it", "its", "me", "my", "not", "of", "on", "or",
    "that", "the", "this", "to", "we", "what", "when", "who", "why", "with", "you", "your",
];

/// Whether the contained-title tier runs for a NORMALISED query: at least
/// [`CONTAINED_MIN_CHARS`] chars and at least one word outside
/// [`CONTAINED_STOP_WORDS`].
pub fn uses_contained_tier(normalised_query: &str) -> bool {
    normalised_query.chars().count() >= CONTAINED_MIN_CHARS
        && normalised_query
            .split(' ')
            .any(|w| !CONTAINED_STOP_WORDS.contains(&w))
}

/// Whether `normalised_query` occurs in `normalised_key` as a whole-word
/// sequence (a word boundary on both sides). Both are already normalised,
/// so words are separated by exactly one space.
fn contains_words(normalised_key: &str, normalised_query: &str) -> bool {
    format!(" {normalised_key} ").contains(&format!(" {normalised_query} "))
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
            1 => return found(pack, hits[0], &q, k),
            _ => {
                let mut ranked: Vec<(usize, &TitleEntry)> =
                    hits.into_iter().map(|e| (0, e)).collect();
                return Ok(did_you_mean(&mut ranked));
            }
        }
    }

    // Contained title (Phase 1h M5b): the query as a whole-word sequence
    // inside any of an entry's keys. It runs before the edit tier, so a
    // coincidental edit-distance hit (a brand name) never outranks a page
    // whose title names the query.
    if uses_contained_tier(&q) {
        let mut hits: Vec<&TitleEntry> = keyed
            .iter()
            .filter(|(_, keys)| keys.iter().flatten().any(|key| contains_words(key, &q)))
            .map(|(e, _)| *e)
            .collect();
        // One entry per document (the titles table's key), but stay honest
        // about a hand-inserted duplicate.
        hits.sort_by_key(|e| e.doc_id);
        hits.dedup_by_key(|e| e.doc_id);
        match hits.len() {
            0 => {}
            1 => return found(pack, hits[0], &q, k),
            _ => {
                // The shortest containing title is the likeliest intent.
                hits.sort_by(|a, b| {
                    a.title
                        .chars()
                        .count()
                        .cmp(&b.title.chars().count())
                        .then_with(|| normalise_title(&a.title).cmp(&normalise_title(&b.title)))
                        .then_with(|| a.doc_id.cmp(&b.doc_id))
                });
                let mut candidates: Vec<String> = Vec::new();
                for e in hits {
                    if candidates.len() >= DID_YOU_MEAN_MAX {
                        break;
                    }
                    if !candidates.contains(&e.title) {
                        candidates.push(e.title.clone());
                    }
                }
                return Ok(LexicalOutcome::DidYouMean { candidates });
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

/// `Found` for one matched entry: its first `k` chunks in section order
/// ([`select_page_chunks`]) and its source. A title with no body is no
/// evidence (`NotFound`).
fn found(pack: &Pack, e: &TitleEntry, normalised_query: &str, k: usize) -> Result<LexicalOutcome, Error> {
    let chunks = select_page_chunks(pack, e.doc_id, normalised_query, k)?;
    if chunks.is_empty() {
        return Ok(LexicalOutcome::NotFound);
    }
    let source = pack
        .get_doc(e.doc_id)?
        .map(|doc| CitationSource::from_doc(&doc))
        .unwrap_or_default();
    Ok(LexicalOutcome::Found {
        doc_id: e.doc_id,
        title: e.title.clone(),
        source,
        chunks,
    })
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

/// The contract the lookup path assembles its grounded prompt with, and
/// whose `no_evidence_marker` / `refusal_with_offer` the caller scripts:
/// the TRIAGE contract (`contracts/prompt-contract.triage.v1.toml`, Task
/// M3) — no arithmetic delegation, doses quoted exactly as cited, refusal
/// refers the user on. The one place the lookup path picks its contract.
pub fn lookup_contract() -> &'static contract::PromptContract {
    contract::contract_for(contract::ContractId::Triage)
}

/// Render a [`LexicalOutcome::Found`] page as the grounded prompt:
/// `contract::assemble_system(lookup_contract() /* triage */, doc_context =
/// the page title's doc-context line, the page's ≤ 3 chunks)` — THE
/// assembler, reached through `retrieve::render_grounded` (which adds only
/// the citations). Citations carry `pack_id` and the page's `source` (URL
/// and retrieval date, from `Found::source`). Empty `chunks` → `NoEvidence`.
pub fn assemble_lexical(pack_id: &str, title: &str, source: &CitationSource, chunks: &[Chunk]) -> RetrievalResult {
    let items: Vec<GroundedItem<'_>> = chunks
        .iter()
        .take(LEXICAL_MAX_K)
        .map(|chunk| GroundedItem {
            pack_id,
            chunk,
            title,
            source,
        })
        .collect();
    render_grounded(lookup_contract(), &items)
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
            assemble_lexical("reference-test", &title, &CitationSource::default(), &chunks)
        else {
            panic!("expected Grounded")
        };
        // Byte-for-byte THE assembler over the TRIAGE contract, with the
        // page title as the doc-context line.
        let render: Vec<contract::RenderChunk<'_>> = chunks
            .iter()
            .map(|c| contract::RenderChunk {
                source_title: &title,
                section_path: &c.section_path,
                locator: &c.locator,
                text: &c.text,
            })
            .collect();
        let triage = contract::contract_for(contract::ContractId::Triage);
        let expected = contract::assemble_system(
            triage,
            contract::doc_context_line([title.as_str()]).as_deref(),
            &render,
        )
        .unwrap();
        assert_eq!(prompt, expected);
        assert_eq!(lookup_contract().contract_id, "triage");
        assert!(prompt.starts_with(triage.system_contract.trim_end()));
        assert!(
            !prompt.contains(contract::system_contract().trim_end()),
            "never the tutor contract"
        );
        assert!(prompt.contains("These sources are excerpts from: Ibuprofen."));
        assert!(prompt.contains("Ibuprofen is an anti-inflammatory."));
        assert_eq!(citations.len(), chunks.len());
        assert_eq!(citations[0].n, 1);
        assert_eq!(citations[0].pack_id, "reference-test");
        assert_eq!(citations[0].doc_title, "Ibuprofen");
        assert_eq!(assemble_lexical("p", "t", &CitationSource::default(), &[]), RetrievalResult::NoEvidence);
    }

    // --- the contained-title tier (Phase 1h M5b) --------------------------

    /// Pages whose titles CONTAIN common one-word queries, but none of
    /// comparable length: the case M5's exact and edit tiers both miss.
    fn contained_corpus() -> Vec<SourceInput> {
        vec![
            md("Anxiety in children", "# Anxiety in children\n\nWorry in children.\n"),
            md("Generalised anxiety disorder in adults", "# GAD\n\nLong-term worry.\n"),
            md("Social anxiety", "# Social anxiety\n\nFear of social situations.\n"),
            md("Anxiety in pregnancy", "# Anxiety in pregnancy\n\nWorry while pregnant.\n"),
            md("Health anxiety", "# Health anxiety\n\nWorry about illness.\n"),
            md("Anxiety and panic", "# Anxiety and panic\n\nPanic attacks.\n"),
            md(
                "Attention deficit hyperactivity disorder (ADHD) in adults",
                "# ADHD in adults\n\nA condition affecting behaviour.\n",
            ),
            md("Heartburn and acid reflux", "# Heartburn and acid reflux\n\nA burning feeling.\n"),
            // A brand-name variant 2 edits from "reflux" (the M4b re-review's case).
            md("Cefalexin", "---\nvariants: [Keflex]\n---\n# Cefalexin\n\nAn antibiotic.\n"),
            md("Piles", "---\nslug: haemorrhoids-piles\n---\n# Piles\n\nSwollen blood vessels.\n"),
            md(
                "Cold sores",
                "---\nvariants: [herpes simplex labialis]\n---\n# Cold sores\n\nSmall blisters.\n",
            ),
            md("Vitamin B12 and D3", "# Vitamin B12 and D3\n\nTwo vitamins.\n"),
            md("Travel and the heat", "# Travel and the heat\n\nKeep cool.\n"),
            md("Paracetamol for adults", "# Paracetamol for adults\n\nA painkiller.\n"),
        ]
    }

    fn contained_pack(name: &str) -> Pack {
        let dir = unique_dir(name);
        let path = dir.join("contained.kpack");
        build_pack(
            &contained_corpus(),
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
        Pack::mount_lexical(&path, &ctx).unwrap().0
    }

    fn dym(titles: &[&str]) -> LexicalOutcome {
        LexicalOutcome::DidYouMean {
            candidates: titles.iter().map(|t| t.to_string()).collect(),
        }
    }

    // Rule 2: several containing pages -> did-you-mean, up to 5, by title
    // length ascending then alphabetically (normalised). "Health anxiety"
    // and "Social anxiety" tie on length (14); "Anxiety and panic" and
    // "Anxiety in children" come next; the longest two are cut by the cap.
    #[test]
    fn contained_query_in_several_titles_is_did_you_mean_shortest_first() {
        let pack = contained_pack("contained-many");
        assert_eq!(
            retrieve_lexical(&pack, "anxiety", 3).unwrap(),
            dym(&[
                "Health anxiety",
                "Social anxiety",
                "Anxiety and panic",
                "Anxiety in children",
                "Anxiety in pregnancy",
            ])
        );
        // Normalised like the exact tier: case and punctuation do not matter.
        assert_eq!(
            retrieve_lexical(&pack, "  ANXIETY! ", 3).unwrap(),
            retrieve_lexical(&pack, "anxiety", 3).unwrap()
        );
        // A multi-word query is matched as a whole-word sequence.
        assert_eq!(
            retrieve_lexical(&pack, "anxiety in", 3).unwrap(),
            dym(&["Anxiety in children", "Anxiety in pregnancy"])
        );
    }

    // Rule 2: exactly one containing page -> Found, same chunk selection as
    // the exact tier.
    #[test]
    fn contained_query_in_one_title_is_found_with_the_usual_chunks() {
        let pack = contained_pack("contained-one");
        let o = retrieve_lexical(&pack, "ADHD", 3).unwrap();
        let LexicalOutcome::Found { doc_id, title, chunks, .. } = &o else {
            panic!("expected Found, got {o:?}")
        };
        assert_eq!(title, "Attention deficit hyperactivity disorder (ADHD) in adults");
        assert_eq!(*chunks, select_page_chunks(&pack, *doc_id, "adhd", 3).unwrap());
        assert!(!chunks.is_empty());
        assert_eq!(
            found_title(&retrieve_lexical(&pack, "generalised anxiety", 3).unwrap()),
            "Generalised anxiety disorder in adults"
        );
    }

    // Rule 1: the slug (hyphens as spaces) and the variants count too, and
    // hits are deduplicated by document.
    #[test]
    fn contained_query_matches_slug_and_variants_once_per_document() {
        let pack = contained_pack("contained-keys");
        assert_eq!(found_title(&retrieve_lexical(&pack, "haemorrhoids", 3).unwrap()), "Piles");
        assert_eq!(found_title(&retrieve_lexical(&pack, "labialis", 3).unwrap()), "Cold sores");
        // "reflux" is in the title and (derived) slug of the same page: one hit.
        assert_eq!(
            found_title(&retrieve_lexical(&pack, "reflux", 3).unwrap()),
            "Heartburn and acid reflux"
        );
    }

    // Rule 1: whole words only — a partial word never matches.
    #[test]
    fn contained_query_needs_word_boundaries() {
        let pack = contained_pack("contained-words");
        for q in ["anxiet", "nxiety", "anxiety disorders", "paracetamol for adult"] {
            let o = retrieve_lexical(&pack, q, 3).unwrap();
            assert!(!matches!(o, LexicalOutcome::Found { .. }), "{q:?} -> {o:?}");
        }
        // "anxiet" is 1 edit from nothing of its length: NotFound, not a
        // contained hit on the six anxiety pages.
        assert_eq!(retrieve_lexical(&pack, "anxiet", 3).unwrap(), LexicalOutcome::NotFound);
        // "paracetamol for adult" stays the edit tier's did-you-mean.
        assert_eq!(
            retrieve_lexical(&pack, "paracetamol for adult", 3).unwrap(),
            dym(&["Paracetamol for adults"])
        );
    }

    // Rule 3: a query under 3 characters, or of stop words only, never uses
    // the tier.
    #[test]
    fn short_or_stop_word_queries_never_use_the_contained_tier() {
        let pack = contained_pack("contained-stop");
        // "d3" and "b12"-like tokens: "d3" is 2 chars (contained in
        // "vitamin b12 and d3") -> NotFound.
        assert_eq!(retrieve_lexical(&pack, "d3", 3).unwrap(), LexicalOutcome::NotFound);
        // 3 chars is enough.
        assert_eq!(found_title(&retrieve_lexical(&pack, "b12", 3).unwrap()), "Vitamin B12 and D3");
        for q in ["and", "for", "and the", "in", "the", "And The!"] {
            assert_eq!(retrieve_lexical(&pack, q, 3).unwrap(), LexicalOutcome::NotFound, "{q:?}");
        }
        // One content word is enough to use the tier.
        assert_eq!(found_title(&retrieve_lexical(&pack, "and the heat", 3).unwrap()), "Travel and the heat");
    }

    #[test]
    fn stop_list_is_small_lowercase_normalised_and_sorted() {
        assert!(CONTAINED_STOP_WORDS.len() <= 60);
        for w in CONTAINED_STOP_WORDS {
            assert_eq!(normalise_title(w), *w, "a stop word must be in normalised form");
            assert!(!w.contains(' '));
        }
        assert!(CONTAINED_STOP_WORDS.windows(2).all(|p| p[0] < p[1]), "sorted, no duplicates");
        for w in ["a", "and", "the", "of", "in", "for", "is", "with"] {
            assert!(CONTAINED_STOP_WORDS.contains(&w), "{w}");
        }
        // Medical words and population words are content words.
        for w in ["anxiety", "children", "adults", "pain"] {
            assert!(!CONTAINED_STOP_WORDS.contains(&w), "{w}");
        }
        assert!(uses_contained_tier("anxiety"));
        assert!(uses_contained_tier("the heat"));
        assert!(!uses_contained_tier("ab"));
        assert!(!uses_contained_tier("and the"));
    }

    // Rule 4: the edit tier runs only when the contained tier has no hits.
    // Without the contained tier "reflux" is 2 edits from the brand variant
    // "keflex" and would suggest Cefalexin.
    #[test]
    fn contained_hit_outranks_an_edit_distance_brand_hit() {
        let pack = contained_pack("contained-vs-edit");
        assert_eq!(damerau_levenshtein("reflux", "keflex"), 2);
        let o = retrieve_lexical(&pack, "reflux", 3).unwrap();
        assert_eq!(found_title(&o), "Heartburn and acid reflux");
        // With no contained hit the edit tier is unchanged: "keflux" is in no
        // title and is 1 edit from the variant "keflex" -> Cefalexin.
        assert_eq!(retrieve_lexical(&pack, "keflux", 3).unwrap(), dym(&["Cefalexin"]));
        // And the exact tier still decides first: a whole title is Found
        // even though other titles contain it.
        assert_eq!(found_title(&retrieve_lexical(&pack, "social anxiety", 3).unwrap()), "Social anxiety");
    }

    // Phase 1h M4b ruling 2: a `Found` page from an `nhs-web` doc carries
    // its URL and retrieval date (a docs join), and so does every citation
    // `assemble_lexical` builds from it. The pack's other docs carry none.
    #[test]
    fn found_page_and_its_citations_carry_the_nhs_web_source() {
        use crate::format::{Doc, SOURCE_TYPE_NHS_WEB};
        let dir = unique_dir("nhs-web-source");
        let pack = Pack::open_or_create(dir.join("ref.kpack"), 8).unwrap();
        let mut ids = Vec::new();
        for (title, st, path, date) in [
            ("Gout", SOURCE_TYPE_NHS_WEB, Some("https://www.nhs.uk/conditions/gout/"), Some("2026-09-26")),
            ("Notes", "md", Some("/home/me/notes.md"), Some("2026-01-01")),
        ] {
            let doc_id = pack
                .insert_doc(&Doc {
                    id: 0,
                    title: title.to_string(),
                    source_type: Some(st.to_string()),
                    sha256: "00".repeat(32),
                    source_path: path.map(str::to_string),
                    source_size: None,
                    source_mtime: date.map(str::to_string),
                    extraction_quality: None,
                    added_at: "2026-09-26T00:00:00Z".to_string(),
                })
                .unwrap();
            pack.insert_title(&TitleEntry {
                doc_id,
                title: title.to_string(),
                normalised_title: normalise_title(title),
                slug: slugify(title),
                variants: vec![],
            })
            .unwrap();
            for section in ["Symptoms", "Treatment"] {
                pack.insert_chunk(&Chunk {
                    id: 0,
                    doc_id,
                    section_path: format!("{title} > {section}"),
                    locator: "L1".to_string(),
                    prefix: String::new(),
                    text: format!("{title} {section} text."),
                    token_count: 3,
                })
                .unwrap();
            }
            ids.push(doc_id);
        }
        let LexicalOutcome::Found { title, source, chunks, .. } = retrieve_lexical(&pack, "gout", 3).unwrap() else {
            panic!("expected Found");
        };
        assert_eq!(
            source,
            CitationSource {
                url: Some("https://www.nhs.uk/conditions/gout/".to_string()),
                retrieved_at: Some("2026-09-26".to_string()),
            }
        );
        let RetrievalResult::Grounded { citations, .. } = assemble_lexical("reference-test", &title, &source, &chunks) else {
            panic!("expected Grounded");
        };
        assert_eq!(citations.len(), 2);
        for c in &citations {
            assert_eq!(c.url.as_deref(), Some("https://www.nhs.uk/conditions/gout/"));
            assert_eq!(c.retrieved_at.as_deref(), Some("2026-09-26"));
        }
        let LexicalOutcome::Found { source, .. } = retrieve_lexical(&pack, "notes", 3).unwrap() else {
            panic!("expected Found");
        };
        assert_eq!(source, CitationSource::default());
    }
}
