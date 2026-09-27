//! `kpack-cli lookup` (Phase 1h M5b addendum): exactly what the phone's
//! lexical lookup returns for a query over a signed pack, as JSON, so the
//! triage repo's probe bank (Task T4) can pin real chunk ids.
//!
//! The pack is mounted as the app mounts the bundled pack
//! (`Pack::mount_lexical`: the curated signature is checked against the
//! pinned production curator key unless `--curator-key` names another
//! public key), then `kpack_core::lookup::retrieve_lexical(pack, query, k)`
//! runs unchanged. One record per query:
//!
//! ```text
//! { "query", "normalised_query",
//!   "outcome": "found" | "did_you_mean" | "not_found" | "unavailable",
//!   "doc": { "doc_id", "title", "slug" },                      // found only
//!   "chunks": [ { "chunk_id", "section_path", "locator", "url",
//!                 "retrieved_at", "token_count", "content_sha" } ],   // found only
//!   "candidates": [ "<display title>", ... ],                 // did_you_mean only
//!   "pack": { "pack_id", "version", "content_sha256" } }
//! ```
//!
//! `chunks` are in the order `assemble_lexical` numbers them ([1], [2], …).
//! `content_sha` is the `chunks.jsonl` rule ([`kpack_core::chunk_content_sha256`]
//! over text, section path, locator and page title), so a record joins
//! `chunks.jsonl` on `chunk_id` and `content_sha`. `url`/`retrieved_at` are
//! `null` when the page has none. Keys are emitted sorted; no timestamps;
//! the same pack and query always give the same bytes.

use crate::CliError;
use kpack_core::lookup::{normalise_title, retrieve_lexical, LexicalOutcome};
use kpack_core::{chunk_content_sha256, LoadContext, Manifest, Pack};
use serde_json::{json, Value};
use std::path::Path;

/// The `k` used when `--k` is not given — the phone's (`LEXICAL_MAX_K`).
pub const DEFAULT_LOOKUP_K: usize = kpack_core::lookup::LEXICAL_MAX_K;

/// A pack mounted for lookup, with its identity for the `pack` field.
pub struct LookupPack {
    pack: Pack,
    pack_json: Value,
}

impl LookupPack {
    /// Mount `path` lexically. `curator_key` is the ed25519 public key a
    /// curated pack's `<path>.sig` must verify against; `None` means the
    /// pinned production key (`kpack_core::curator_verifying_key`), the one
    /// the app uses.
    pub fn mount(path: &Path, curator_key: Option<[u8; 32]>) -> Result<LookupPack, CliError> {
        let key = match curator_key {
            Some(bytes) => Some(
                ed25519_dalek::VerifyingKey::from_bytes(&bytes)
                    .map_err(|e| CliError(format!("--curator-key is not an ed25519 public key: {e}")))?,
            ),
            None => kpack_core::curator_verifying_key(),
        };
        let ctx = LoadContext {
            available_embedder_sha256: &[],
            curator_key: key,
        };
        let (pack, manifest) = Pack::mount_lexical(path, &ctx)
            .map_err(|e| CliError(format!("cannot mount {} for lookup: {e}", path.display())))?;
        let pack_json = pack_identity(&pack, &manifest)?;
        Ok(LookupPack { pack, pack_json })
    }

    /// The lookup record for one query (see the module doc).
    pub fn lookup(&self, query: &str, k: usize) -> Result<Value, CliError> {
        let e = |err: kpack_core::Error| CliError(format!("lookup {query:?}: {err}"));
        let outcome = retrieve_lexical(&self.pack, query, k).map_err(e)?;
        let mut rec = json!({
            "query": query,
            "normalised_query": normalise_title(query),
            "pack": self.pack_json,
        });
        let obj = rec.as_object_mut().expect("an object literal");
        match outcome {
            LexicalOutcome::Found { doc_id, title, source, chunks } => {
                let slug = self
                    .pack
                    .titles()
                    .map_err(e)?
                    .into_iter()
                    .find(|t| t.doc_id == doc_id)
                    .map(|t| t.slug)
                    .ok_or_else(|| CliError(format!("found doc {doc_id} has no title-index row")))?;
                let chunks: Vec<Value> = chunks
                    .iter()
                    .map(|c| {
                        json!({
                            "chunk_id": c.id,
                            "section_path": c.section_path,
                            "locator": c.locator,
                            "url": source.url,
                            "retrieved_at": source.retrieved_at,
                            "token_count": c.token_count,
                            "content_sha": chunk_content_sha256(&c.text, &c.section_path, &c.locator, &title),
                        })
                    })
                    .collect();
                obj.insert("outcome".into(), json!("found"));
                obj.insert("doc".into(), json!({ "doc_id": doc_id, "title": title, "slug": slug }));
                obj.insert("chunks".into(), Value::Array(chunks));
            }
            LexicalOutcome::DidYouMean { candidates } => {
                obj.insert("outcome".into(), json!("did_you_mean"));
                obj.insert("candidates".into(), json!(candidates));
            }
            LexicalOutcome::NotFound => {
                obj.insert("outcome".into(), json!("not_found"));
            }
            LexicalOutcome::Unavailable => {
                obj.insert("outcome".into(), json!("unavailable"));
            }
        }
        Ok(rec)
    }
}

fn pack_identity(pack: &Pack, manifest: &Manifest) -> Result<Value, CliError> {
    let content = pack
        .manifest_get(kpack_core::lexical_build::MANIFEST_CONTENT_SHA256)
        .map_err(|e| CliError(format!("reading the pack manifest: {e}")))?;
    Ok(json!({
        "pack_id": manifest.pack_id,
        "version": manifest.pack_version,
        "content_sha256": content,
    }))
}

/// Parse a 64-hex-character ed25519 public key.
pub fn parse_public_key_hex(s: &str) -> Result<[u8; 32], CliError> {
    let s = s.trim();
    if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(CliError("--curator-key must be 64 hex characters (an ed25519 public key)".into()));
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).expect("checked hex");
    }
    Ok(out)
}

/// One batch input line: a JSON string (the query) or an object
/// `{"query": "...", "k": 3}` (`k` optional, else `default_k`). Blank lines
/// are skipped; anything else is an error naming the line.
pub fn parse_batch_line(line: &str, line_no: usize, default_k: usize) -> Result<Option<(String, usize)>, CliError> {
    if line.trim().is_empty() {
        return Ok(None);
    }
    let bad = |why: &str| CliError(format!("batch line {line_no}: {why}"));
    let v: Value = serde_json::from_str(line).map_err(|e| bad(&format!("not JSON: {e}")))?;
    match v {
        Value::String(q) => Ok(Some((q, default_k))),
        Value::Object(o) => {
            if let Some(extra) = o.keys().find(|k| *k != "query" && *k != "k") {
                return Err(bad(&format!("unknown key {extra:?}")));
            }
            let q = o.get("query").and_then(Value::as_str).ok_or_else(|| bad("\"query\" must be a string"))?;
            let k = match o.get("k") {
                None => default_k,
                Some(k) => k.as_u64().ok_or_else(|| bad("\"k\" must be a whole number"))? as usize,
            };
            Ok(Some((q.to_string(), k)))
        }
        _ => Err(bad("expected a JSON string or {\"query\": ..., \"k\": ...}")),
    }
}
