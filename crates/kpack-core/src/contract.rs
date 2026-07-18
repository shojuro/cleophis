//! The versioned prompt contract (spec §4.2, plan D5) — the anti-hallucination
//! artifact shared, byte-for-byte, between the on-device runtime (this
//! module) and the (future) adapter-training track. Both import the SAME
//! file, `contracts/prompt-contract.v1.toml`; neither re-types a copy.
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

/// The contract file, baked into the binary at compile time. Path is
/// relative to this file (`crates/kpack-core/src/contract.rs`): three
/// levels up reaches the repo root, where `contracts/` lives.
const CONTRACT_TOML: &str = include_str!("../../../contracts/prompt-contract.v1.toml");

/// The parsed prompt contract — see the module doc comment and
/// `contracts/prompt-contract.v1.toml` for what each field means and how
/// it is used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptContract {
    pub contract_version: u32,
    pub system_contract: String,
    pub citation_line: String,
    pub citation_source_sep: String,
    pub no_evidence_marker: String,
    pub refusal_with_offer: String,
}

fn required_str(table: &toml::Table, key: &str) -> String {
    table
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| {
            panic!("prompt-contract.v1.toml: missing or non-string key `{key}` (compiled-in contract — this is a build-time bug, not attacker input)")
        })
        .to_string()
}

fn parse_contract() -> PromptContract {
    let table: toml::Table = CONTRACT_TOML.parse().expect(
        "prompt-contract.v1.toml: malformed TOML (compiled-in contract — this is a build-time bug, not attacker input)",
    );

    let contract_version = table
        .get("contract_version")
        .and_then(|v| v.as_integer())
        .expect("prompt-contract.v1.toml: missing or non-integer key `contract_version`");
    let contract_version = u32::try_from(contract_version)
        .expect("prompt-contract.v1.toml: `contract_version` out of u32 range");

    PromptContract {
        contract_version,
        system_contract: required_str(&table, "system_contract"),
        citation_line: required_str(&table, "citation_line"),
        citation_source_sep: required_str(&table, "citation_source_sep"),
        no_evidence_marker: required_str(&table, "no_evidence_marker"),
        refusal_with_offer: required_str(&table, "refusal_with_offer"),
    }
}

/// The parsed contract, parsed exactly once (`OnceLock`) and shared for the
/// lifetime of the process.
fn contract() -> &'static PromptContract {
    static CONTRACT: OnceLock<PromptContract> = OnceLock::new();
    CONTRACT.get_or_init(parse_contract)
}

/// The contract's format version. Bump-tracked in the training track's
/// adapter metadata (which contract it speaks) — see plan D5.
pub fn contract_version() -> u32 {
    contract().contract_version
}

/// The system prompt prepended to every grounded request.
pub fn system_contract() -> &'static str {
    &contract().system_contract
}

/// The marker the runtime injects in place of the sources block when the
/// per-pack retrieval gate returns NO_EVIDENCE (spec §4.1).
pub fn no_evidence_marker() -> &'static str {
    &contract().no_evidence_marker
}

/// The shape of the refusal the adapter is trained to produce on
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

/// Render `chunks` into the numbered citation block the system contract
/// refers to (`cite its supporting source as [n]`): one `citation_line` per
/// chunk, in order, 1-based, joined by `"\n"`. Empty input renders `""`.
///
/// `{source}` is composed per chunk from the NON-EMPTY parts of
/// `[source_title, section_path, locator]`, joined by
/// `citation_source_sep` — a root chunk (D6) with an empty `section_path`
/// drops out of the join cleanly, with no dangling separator.
pub fn render_sources(chunks: &[RenderChunk<'_>]) -> String {
    let c = contract();
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
}
