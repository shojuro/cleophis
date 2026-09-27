//! The versioned prompt contract (spec §4.2, plan D5) — the anti-hallucination
//! artifact shared, byte-for-byte, between the on-device runtime (this
//! module) and the (future) adapter-training track. Both import the SAME
//! file, `contracts/prompt-contract.v1.toml`; neither re-types a copy.
//!
//! Phase 1h adds a second contract, `contracts/prompt-contract.triage.v1.toml`
//! (the bundled medical reference lookup), loaded by [`ContractId`] via
//! [`contract_for`] / [`contract_by_id`]. The free functions
//! (`system_contract()`, `render_sources()`, …) keep reading the tutor
//! contract. [`assemble_system`] is the ONE function that builds the full
//! grounded system message; `retrieve::assemble` calls it, and the golden
//! fixtures in `tests/fixtures/contract-golden/` (see its README.md) pin
//! its bytes for the triage repo's JS and Python ports.
//!
//! The contract is compiled into the binary via `include_str!` (no runtime
//! file dependency — a `.kpack` build or install can never ship without it,
//! and it can never drift from what this binary was built against) and
//! parsed once into a `PromptContract`. A malformed contract is a build-time
//! bug, not attacker-controlled input, so parsing may `expect(...)`.
//!
//! The citation renderer (`render_sources`) is the other half: it substitutes
//! `{n}`/`{source}`/`{text}` into the contract's `citation_line` template in
//! a single left-to-right pass, so a chunk's `text` can never smuggle a
//! second round of substitution through a literal `"{n}"`/`"{text}"`. The
//! golden render test below byte-pins the output — see the crate doc comment
//! and the contract file's own header for why silent drift here is the
//! specific risk this guards against (plan D5's risk note).

use std::sync::OnceLock;

/// The TUTOR contract file, baked into the binary at compile time. Path is
/// relative to this file (`crates/kpack-core/src/contract.rs`): three
/// levels up reaches the repo root, where `contracts/` lives.
const CONTRACT_TOML: &str = include_str!("../../../contracts/prompt-contract.v1.toml");

/// The TRIAGE contract file (the bundled medical reference lookup): no
/// arithmetic delegation, a referral refusal, and renderer fields
/// byte-identical to the tutor's. See the file's own header.
const TRIAGE_CONTRACT_TOML: &str =
    include_str!("../../../contracts/prompt-contract.triage.v1.toml");

/// Which compiled-in contract to use. The tutor contract file predates
/// `contract_id` and carries no such key (its bytes are frozen), so its id
/// is implied; the triage file states `contract_id = "triage"` and the
/// parser asserts it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContractId {
    Tutor,
    Triage,
}

impl ContractId {
    /// The stable string id (`"tutor"` / `"triage"`).
    pub fn as_str(self) -> &'static str {
        match self {
            ContractId::Tutor => "tutor",
            ContractId::Triage => "triage",
        }
    }

    /// Parse a string id; `None` for an unknown id.
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "tutor" => Some(ContractId::Tutor),
            "triage" => Some(ContractId::Triage),
            _ => None,
        }
    }

    fn source(self) -> (&'static str, &'static str) {
        match self {
            ContractId::Tutor => (CONTRACT_TOML, "prompt-contract.v1.toml"),
            ContractId::Triage => (TRIAGE_CONTRACT_TOML, "prompt-contract.triage.v1.toml"),
        }
    }
}

/// The parsed prompt contract — see the module doc comment and the
/// `contracts/prompt-contract*.toml` files for what each field means and
/// how it is used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptContract {
    pub contract_id: String,
    pub contract_version: u32,
    pub system_contract: String,
    pub citation_line: String,
    pub citation_source_sep: String,
    pub no_evidence_marker: String,
    pub refusal_with_offer: String,
}

fn required_str(table: &toml::Table, key: &str, file: &str) -> String {
    table
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| {
            panic!("{file}: missing or non-string key `{key}` (compiled-in contract — this is a build-time bug, not attacker input)")
        })
        .to_string()
}

fn parse_contract(id: ContractId) -> PromptContract {
    let (src, file) = id.source();
    let table: toml::Table = src.parse().unwrap_or_else(|e| {
        panic!("{file}: malformed TOML ({e}) (compiled-in contract — this is a build-time bug, not attacker input)")
    });

    let contract_version = table
        .get("contract_version")
        .and_then(|v| v.as_integer())
        .unwrap_or_else(|| panic!("{file}: missing or non-integer key `contract_version`"));
    let contract_version = u32::try_from(contract_version)
        .unwrap_or_else(|_| panic!("{file}: `contract_version` out of u32 range"));

    // `contract_id` is optional ONLY for the tutor file (frozen bytes, no
    // key); whenever present it must name the contract it was loaded as.
    let contract_id = match table.get("contract_id") {
        None if id == ContractId::Tutor => id.as_str().to_string(),
        None => panic!("{file}: missing key `contract_id`"),
        Some(v) => {
            let v = v
                .as_str()
                .unwrap_or_else(|| panic!("{file}: non-string key `contract_id`"));
            assert_eq!(v, id.as_str(), "{file}: `contract_id` does not match the loaded id");
            v.to_string()
        }
    };

    PromptContract {
        contract_id,
        contract_version,
        system_contract: required_str(&table, "system_contract", file),
        citation_line: required_str(&table, "citation_line", file),
        citation_source_sep: required_str(&table, "citation_source_sep", file),
        no_evidence_marker: required_str(&table, "no_evidence_marker", file),
        refusal_with_offer: required_str(&table, "refusal_with_offer", file),
    }
}

/// The contract for `id`, parsed exactly once per id (`OnceLock`) and shared
/// for the lifetime of the process.
pub fn contract_for(id: ContractId) -> &'static PromptContract {
    static TUTOR: OnceLock<PromptContract> = OnceLock::new();
    static TRIAGE: OnceLock<PromptContract> = OnceLock::new();
    match id {
        ContractId::Tutor => TUTOR.get_or_init(|| parse_contract(ContractId::Tutor)),
        ContractId::Triage => TRIAGE.get_or_init(|| parse_contract(ContractId::Triage)),
    }
}

/// The contract with string id `id` (`"tutor"` / `"triage"`), or `None`.
pub fn contract_by_id(id: &str) -> Option<&'static PromptContract> {
    ContractId::from_id(id).map(contract_for)
}

/// The tutor contract — what every pre-Phase-1h free function reads.
fn contract() -> &'static PromptContract {
    contract_for(ContractId::Tutor)
}

/// The TUTOR contract's format version. Bump-tracked in the training track's
/// adapter metadata (which contract it speaks) — see plan D5.
pub fn contract_version() -> u32 {
    contract().contract_version
}

/// The TUTOR system prompt prepended to every grounded request.
pub fn system_contract() -> &'static str {
    &contract().system_contract
}

/// The marker the runtime injects in place of the sources block when the
/// per-pack retrieval gate returns NO_EVIDENCE (spec §4.1). Identical in
/// both contracts.
pub fn no_evidence_marker() -> &'static str {
    &contract().no_evidence_marker
}

/// The shape of the refusal the (tutor) adapter is trained to produce on
/// NO_EVIDENCE. The runtime does not emit this itself (the model does); it
/// is exposed here only so both tracks share one definition.
pub fn refusal_with_offer() -> &'static str {
    &contract().refusal_with_offer
}

/// One retrieved chunk, ready to be rendered as a numbered citation line by
/// `render_sources`.
pub struct RenderChunk<'a> {
    pub source_title: &'a str,
    pub section_path: &'a str,
    pub locator: &'a str,
    pub text: &'a str,
}

/// [`render_sources_with`] over the TUTOR contract (unchanged behaviour).
pub fn render_sources(chunks: &[RenderChunk<'_>]) -> String {
    render_sources_with(contract(), chunks)
}

/// Render `chunks` into the numbered citation block the system contract
/// refers to (`cite its supporting source as [n]`): one `citation_line` per
/// chunk, in order, 1-based, joined by `"\n"` (no trailing newline). Empty
/// input renders `""`.
///
/// `{source}` is composed per chunk from the NON-EMPTY parts of
/// `[source_title, section_path, locator]`, joined by
/// `citation_source_sep` — a root chunk (D6) with an empty `section_path`
/// drops out of the join cleanly, with no dangling separator. The parts are
/// used as-is (not trimmed).
pub fn render_sources_with(c: &PromptContract, chunks: &[RenderChunk<'_>]) -> String {
    let mut lines = Vec::with_capacity(chunks.len());
    for (i, chunk) in chunks.iter().enumerate() {
        let n = (i + 1).to_string();
        let source = [chunk.source_title, chunk.section_path, chunk.locator]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(&c.citation_source_sep);
        lines.push(substitute_line(&c.citation_line, &n, &source, chunk.text));
    }
    lines.join("\n")
}

/// The doc-context line's fixed text: `PREFIX + titles.join(SEP) + SUFFIX`.
/// A RUNTIME addition, not part of either versioned contract — but part of
/// the assembled system message, so it is pinned by the golden fixtures.
pub const DOC_CONTEXT_PREFIX: &str = "These sources are excerpts from: ";
/// Separator between distinct titles in the doc-context line.
pub const DOC_CONTEXT_TITLE_SEP: &str = "; ";
/// The doc-context line's terminator.
pub const DOC_CONTEXT_SUFFIX: &str = ".";

/// The doc-context line for `titles`: each title trimmed of surrounding
/// whitespace, empty ones dropped, duplicates (after trimming) dropped
/// keeping the FIRST occurrence, joined by [`DOC_CONTEXT_TITLE_SEP`] between
/// [`DOC_CONTEXT_PREFIX`] and [`DOC_CONTEXT_SUFFIX`]. `None` when no title
/// survives (the line is then omitted entirely).
pub fn doc_context_line<'a>(titles: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let mut kept: Vec<&str> = Vec::new();
    for title in titles {
        let title = title.trim();
        if !title.is_empty() && !kept.contains(&title) {
            kept.push(title);
        }
    }
    if kept.is_empty() {
        None
    } else {
        Some(format!(
            "{DOC_CONTEXT_PREFIX}{}{DOC_CONTEXT_SUFFIX}",
            kept.join(DOC_CONTEXT_TITLE_SEP)
        ))
    }
}

/// [`doc_context_line`] over the chunks' `source_title`s, in chunk order —
/// exactly what the runtime's `retrieve::assemble` states (the cited docs'
/// titles).
pub fn doc_context_for(chunks: &[RenderChunk<'_>]) -> Option<String> {
    doc_context_line(chunks.iter().map(|c| c.source_title))
}

/// Why [`assemble_system`] refused to build a grounded system message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssembleError {
    /// Zero chunks: a grounded prompt that instructs the model to cite `[n]`
    /// with nothing to cite is never emitted (the caller takes the
    /// NO_EVIDENCE path instead).
    NoChunks,
}

impl std::fmt::Display for AssembleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AssembleError::NoChunks => {
                f.write_str("refusing to assemble a grounded system message with zero sources")
            }
        }
    }
}

impl std::error::Error for AssembleError {}

/// THE full grounded system message — the one function the runtime, and
/// (by golden fixture) the triage repo's JS and Python ports, agree on
/// byte-for-byte:
///
/// ```text
/// system_contract.trim_end() + "\n\n" [+ doc_context + "\n\n"] + render_sources_with(c, chunks)
/// ```
///
/// - `system_contract` loses ALL trailing whitespace (the TOML multi-line
///   string ends in `"\n"`); leading bytes are kept as-is.
/// - `doc_context` is included iff it is `Some` and non-empty; it is used
///   verbatim (build it with [`doc_context_for`] / [`doc_context_line`]).
/// - The sources block's lines are joined by a single `"\n"`, and the
///   message has NO trailing newline.
/// - Zero chunks is refused ([`AssembleError::NoChunks`]).
pub fn assemble_system(
    c: &PromptContract,
    doc_context: Option<&str>,
    chunks: &[RenderChunk<'_>],
) -> Result<String, AssembleError> {
    if chunks.is_empty() {
        return Err(AssembleError::NoChunks);
    }
    let sources_block = render_sources_with(c, chunks);
    let system = c.system_contract.trim_end();
    Ok(match doc_context.filter(|dc| !dc.is_empty()) {
        Some(dc) => format!("{system}\n\n{dc}\n\n{sources_block}"),
        None => format!("{system}\n\n{sources_block}"),
    })
}

/// Single-pass `{n}` / `{source}` / `{text}` substitution over `template`.
///
/// Walks `template` once, left to right, copying literal spans straight to
/// the output and substituting each recognized placeholder as it is found.
/// A substituted value's bytes are appended directly to the output and the
/// scan resumes AFTER the placeholder in the ORIGINAL template — substituted
/// text is never fed back into the scan, so a chunk `text` containing a
/// literal `"{n}"` or `"{text}"` renders verbatim rather than being
/// re-substituted. Deliberately not implemented as chained `str::replace`
/// calls, which re-scan the whole (growing) output on every call and would
/// happily re-substitute a placeholder that only exists because a prior
/// replacement introduced it.
fn substitute_line(template: &str, n: &str, source: &str, text: &str) -> String {
    let mut out = String::with_capacity(template.len() + source.len() + text.len());
    let mut rest = template;
    while let Some(brace_idx) = rest.find('{') {
        out.push_str(&rest[..brace_idx]);
        let after = &rest[brace_idx + 1..];
        if let Some(remaining) = after.strip_prefix("n}") {
            out.push_str(n);
            rest = remaining;
        } else if let Some(remaining) = after.strip_prefix("source}") {
            out.push_str(source);
            rest = remaining;
        } else if let Some(remaining) = after.strip_prefix("text}") {
            out.push_str(text);
            rest = remaining;
        } else {
            // Unrecognized `{...` — emit the brace literally and keep
            // scanning right after it (still a single forward pass).
            out.push('{');
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_fields_load_non_empty() {
        assert_eq!(contract_version(), 1);
        assert!(!system_contract().is_empty());
        assert!(!no_evidence_marker().is_empty());
        assert!(!refusal_with_offer().is_empty());
    }

    #[test]
    fn golden_render_three_chunks() {
        let chunks = [
            // Full: title + section_path + locator.
            RenderChunk {
                source_title: "Title",
                section_path: "A > B",
                locator: "p. 3",
                text: "body one",
            },
            // Root chunk (D6): empty section_path drops out, no dangling separator.
            RenderChunk {
                source_title: "Title",
                section_path: "",
                locator: "p. 4",
                text: "body two",
            },
            // text contains literal "{n}" and "{text}" — must render verbatim,
            // proving the substitution is single-pass (not re-scanned).
            RenderChunk {
                source_title: "Title",
                section_path: "C",
                locator: "p. 5",
                text: "has {n} and {text} literal",
            },
        ];

        let rendered = render_sources(&chunks);

        let expected = "[1] (Title, A > B, p. 3): body one\n\
                         [2] (Title, p. 4): body two\n\
                         [3] (Title, C, p. 5): has {n} and {text} literal";

        assert_eq!(rendered, expected);
    }

    #[test]
    fn empty_input_renders_empty_string() {
        assert_eq!(render_sources(&[]), "");
    }

    #[test]
    fn no_evidence_marker_matches_contract_value() {
        assert_eq!(no_evidence_marker(), "[[NO_EVIDENCE]]");
    }

    // Degenerate {source}: all three parts empty → renders "()" with no panic
    // and no dangling separators. Locks the "no panic on empty source" property
    // (the renderer never receives this from real data, but the artifact is
    // byte-stable so the guarantee is pinned, not merely inspected).
    #[test]
    fn source_all_parts_empty_renders_empty_parens() {
        let chunks = [RenderChunk {
            source_title: "",
            section_path: "",
            locator: "",
            text: "body",
        }];
        assert_eq!(render_sources(&chunks), "[1] (): body");
    }

    // An unrecognized placeholder in chunk text (e.g. "{foo}") is emitted
    // verbatim — the single forward pass only substitutes {n}/{source}/{text},
    // everything else is literal template/content.
    #[test]
    fn unrecognized_placeholder_in_text_is_literal() {
        let chunks = [RenderChunk {
            source_title: "T",
            section_path: "",
            locator: "p1",
            text: "keep {foo} as-is",
        }];
        assert_eq!(render_sources(&chunks), "[1] (T, p1): keep {foo} as-is");
    }

    // ---- Triage contract + full-system-message golden (Phase 1h M3) ------

    use std::path::PathBuf;

    fn golden_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/contract-golden")
    }

    /// The golden input fixture, parsed: the k=3 chunks (owned) and the
    /// expected doc_context line.
    struct GoldenInput {
        chunks: Vec<[String; 4]>,
        doc_context: String,
        outputs: Vec<(String, String, bool)>, // (file, contract_id, with_doc_context)
    }

    fn golden_input() -> GoldenInput {
        let raw = std::fs::read_to_string(golden_dir().join("chunks-k3.json"))
            .expect("read chunks-k3.json");
        let v: serde_json::Value = serde_json::from_str(&raw).expect("chunks-k3.json is JSON");
        assert_eq!(v["format"], "cleophis-contract-golden/1");
        let s = |c: &serde_json::Value, k: &str| c[k].as_str().expect(k).to_string();
        let chunks: Vec<[String; 4]> = v["chunks"]
            .as_array()
            .expect("chunks array")
            .iter()
            .map(|c| [s(c, "source_title"), s(c, "section_path"), s(c, "locator"), s(c, "text")])
            .collect();
        assert_eq!(v["k"].as_u64(), Some(chunks.len() as u64));
        let outputs = v["outputs"]
            .as_array()
            .expect("outputs array")
            .iter()
            .map(|o| {
                (s(o, "file"), s(o, "contract_id"), o["with_doc_context"].as_bool().expect("bool"))
            })
            .collect();
        GoldenInput { chunks, doc_context: s(&v, "doc_context"), outputs }
    }

    fn render_chunks(owned: &[[String; 4]]) -> Vec<RenderChunk<'_>> {
        owned
            .iter()
            .map(|[t, sp, l, x]| RenderChunk { source_title: t, section_path: sp, locator: l, text: x })
            .collect()
    }

    /// Every golden output, assembled NOW by this build: (file name, bytes).
    fn assemble_golden_outputs() -> Vec<(String, String)> {
        let input = golden_input();
        let chunks = render_chunks(&input.chunks);
        input
            .outputs
            .iter()
            .map(|(file, id, with_dc)| {
                let c = contract_by_id(id).unwrap_or_else(|| panic!("unknown contract id {id}"));
                let dc = with_dc.then_some(input.doc_context.as_str());
                let out = assemble_system(c, dc, &chunks).expect("k=3 assembles");
                (file.clone(), out)
            })
            .collect()
    }

    /// Writes the emitted golden fixtures. Run deliberately, after a
    /// reviewed contract/assembly change:
    ///   cargo test -p kpack-core -- --ignored emit_golden
    #[test]
    #[ignore]
    fn emit_golden() {
        for (file, bytes) in assemble_golden_outputs() {
            std::fs::write(golden_dir().join(&file), bytes.as_bytes()).expect("write golden");
        }
    }

    #[test]
    fn golden_full_system_messages_are_byte_pinned() {
        for (file, bytes) in assemble_golden_outputs() {
            let pinned = std::fs::read(golden_dir().join(&file)).unwrap_or_else(|e| {
                panic!("{file}: {e} — run `cargo test -p kpack-core -- --ignored emit_golden`")
            });
            assert_eq!(
                String::from_utf8(pinned).expect("golden is UTF-8"),
                bytes,
                "{file} drifted from the assembled bytes"
            );
        }
    }

    #[test]
    fn golden_doc_context_is_derived_from_chunk_titles() {
        let input = golden_input();
        let chunks = render_chunks(&input.chunks);
        assert_eq!(doc_context_for(&chunks).as_deref(), Some(input.doc_context.as_str()));
    }

    #[test]
    fn golden_structure_matches_the_documented_rule() {
        // The rule the JS/Python ports implement, spelled out independently
        // of `assemble_system`: system_contract with trailing whitespace
        // trimmed, "\n\n", [doc_context, "\n\n"], the sources block (lines
        // joined by "\n"), and NO trailing newline.
        let input = golden_input();
        let chunks = render_chunks(&input.chunks);
        let t = contract_for(ContractId::Triage);
        let sources = render_sources_with(t, &chunks);
        let with_dc = format!("{}\n\n{}\n\n{}", t.system_contract.trim_end(), input.doc_context, sources);
        let without = format!("{}\n\n{}", t.system_contract.trim_end(), sources);
        assert_eq!(assemble_system(t, Some(&input.doc_context), &chunks).unwrap(), with_dc);
        assert_eq!(assemble_system(t, None, &chunks).unwrap(), without);
        assert!(!with_dc.ends_with('\n'));
        assert!(!with_dc.contains("\n\n\n"));
    }

    #[test]
    fn empty_doc_context_is_the_same_as_none() {
        let input = golden_input();
        let chunks = render_chunks(&input.chunks);
        let t = contract_for(ContractId::Triage);
        assert_eq!(assemble_system(t, Some(""), &chunks), assemble_system(t, None, &chunks));
    }

    #[test]
    fn assemble_system_refuses_zero_chunks() {
        for id in [ContractId::Tutor, ContractId::Triage] {
            let c = contract_for(id);
            assert_eq!(assemble_system(c, None, &[]), Err(AssembleError::NoChunks));
            assert_eq!(assemble_system(c, Some("These sources are excerpts from: X."), &[]), Err(AssembleError::NoChunks));
        }
    }

    #[test]
    fn both_contracts_parse_and_load_by_id() {
        let tutor = contract_for(ContractId::Tutor);
        let triage = contract_for(ContractId::Triage);
        assert_eq!(tutor.contract_id, "tutor");
        assert_eq!(triage.contract_id, "triage");
        assert_eq!(tutor.contract_version, 1);
        assert_eq!(triage.contract_version, 1);
        assert!(std::ptr::eq(contract_by_id("tutor").unwrap(), tutor));
        assert!(std::ptr::eq(contract_by_id("triage").unwrap(), triage));
        assert!(contract_by_id("nope").is_none());
        assert_eq!(ContractId::from_id("triage"), Some(ContractId::Triage));
        assert_eq!(ContractId::Triage.as_str(), "triage");
        // The legacy free functions still read the TUTOR contract.
        assert_eq!(system_contract(), tutor.system_contract);
        assert_eq!(refusal_with_offer(), tutor.refusal_with_offer);
    }

    #[test]
    fn triage_contract_has_no_calc_and_the_referral_refusal() {
        assert!(!TRIAGE_CONTRACT_TOML.to_lowercase().contains("calc"));
        let triage = contract_for(ContractId::Triage);
        assert!(!triage.system_contract.to_lowercase().contains("calc"));
        assert!(!triage.system_contract.to_lowercase().contains("arithmetic"));
        assert_eq!(
            triage.refusal_with_offer,
            "This reference does not cover that. Ask a pharmacist or clinician."
        );
        assert!(triage.system_contract.contains("[n]"));
    }

    #[test]
    fn triage_renderer_fields_match_the_tutor_byte_for_byte() {
        let tutor = contract_for(ContractId::Tutor);
        let triage = contract_for(ContractId::Triage);
        assert_eq!(triage.citation_line, tutor.citation_line);
        assert_eq!(triage.citation_source_sep, tutor.citation_source_sep);
        assert_eq!(triage.no_evidence_marker, tutor.no_evidence_marker);
    }

    #[test]
    fn tutor_contract_still_delegates_arithmetic() {
        // Guard against the triage edit leaking into the tutor file.
        assert!(contract_for(ContractId::Tutor).system_contract.contains("calc()"));
    }
}
