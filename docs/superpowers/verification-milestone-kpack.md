# Verification — §1 Shared Core: the Knowledge-Pack Format (`kpack-core`)

Branch `feat/kpack-core` (PR #11). Act two of the same product: the on-device
RAG knowledge-pack core the rest of `on-device-rag-spec-v1.3.md` builds on. The
demo is unchanged; this is additive. Subagent-driven, one review gate per slice.

## What shipped

| Slice | What | Key commits |
|---|---|---|
| K0 | Cargo workspace; `kpack-core` (network-free, compiler-enforced) + `kpack-embed` scaffolds | d7242f9 |
| K1 | `.kpack` SQLite format: full §1.1 schema, `vec0` (int8) + `fts5` dual lanes, **static** sqlite-vec registration (the only path that reaches iOS), golden-DDL lock | 89041b6, 57d2f65 |
| K2 | Typed manifest + load-time gate (embedder-hash → dims → schema → vec-format), trust injected, never from the pack | 68fe751, 4c12580 |
| K3 | ed25519 curated-pack signature verify (`verify_strict`, injected key, fail-closed, release-can't-link-test-key guardrail, bytes-only pre-open gate) | d981f3f, 145308b |
| K4a | `Embedder` trait + int8 quantization math (L2-norm → int8 → dot ≈ cosine) + drift-guarded BGE prefix + deterministic mock | 5753db3 |
| K4b | **Real** linked llama.cpp BGE embedder (CLS pooling, dims 768), behind the `real` feature | b242b0a, ae8ebe2 |
| K5 | Structure-aware chunking: sentence-snapped 400-token windows, 18% overlap, tables row-wise, code atomic, D6 citations | 2775a02, c434a6b |
| K6 | Markdown + plain-text parsers → heading-tree Document | f5c5cef, 2d571dd |
| K7 | Prompt-contract loader + citation renderer, byte-pinned golden (`contracts/prompt-contract.v1.toml`, D5) | 9c0ed2d, dbdfa47, c8dd232 |
| K8 | End-to-end `build_pack` + golden-pack / build-determinism / no-socket tests | 738b165, 51f86c6 |
| K8b | Real-embedder build determinism (§5) — **byte-identical** | 051dba6 |
| K9 | Tauri seam: `mount_pack`, `build_personal_pack`; app carries the real embedder | 12c3922 |

## Live verification

- **The pipeline works end to end.** A document parses to a heading tree, chunks
  with correct sentence boundaries, embeds through the **real** BGE model,
  quantizes to int8, indexes into the `vec0` + `fts5` lanes of a signed,
  integrity-gated `.kpack`, and mounts back queryable — proven by K8's golden
  test (builds a real MD fixture, mounts through the full K2 gate, queries both
  lanes with exact-distance assertions) and K9's app round-trip.
- **The embedder is real, not noise.** K4b's semantic-sanity test: cosine(similar
  pair) = **0.8351** vs cosine(dissimilar) = **0.2206** — clean separation. CLS
  pooling confirmed against the GGUF's own `bert`/`embedding_length=768` metadata.
- **Reproducible to the byte.** K8b builds the same corpus twice through the real
  tokenizer → **byte-identical** stored int8 vectors and identical chunk sets (6
  chunks), no tolerance needed. This is the §5 cross-build determinism proof.
- **Network-free by construction.** `kpack-core`'s `Cargo.toml` links no HTTP /
  socket / async-runtime crate; `cargo tree` confirms none transitively. The
  spec's "no network in the build/retrieval path" is a dependency-graph
  invariant, not a comment. The native embedder is contained to `kpack-embed`'s
  `real` feature, so the default workspace builds with no toolchain.
- **Test suite:** `cargo test -p kpack-core --features test-util` → **89 passing,
  zero warnings**. Real-embedder integration tests (`kpack-embed --features
  real -- --ignored`, needs the GGUF + LLVM/cmake): K4b semantic sanity, K8b
  determinism, K9 round-trip — all pass. The existing app's 115 tests are
  unaffected by the workspace restructure.

## Reviews (every slice gated; the ones that caught real bugs)

Each slice had an independent Opus/Sonnet review. Findings that mattered, all
fixed and re-verified: K1 shipped an incomplete `docs` schema (missing sha256,
token_count, etc. — the on-disk contract) → completed + golden-DDL-locked; K2's
`vec_format_version` was recorded but not enforced → made a real gate check; K3's
signature gate parsed untrusted SQLite before verifying → added the bytes-only
pre-open `verify_file` + documented residual; K5's sentence splitter merged
"Appendix A."/"5." boundaries and its overlap ran away to ~85% → precise
boundaries + window-fraction-bounded overlap; K6 glued loose-list paragraph text
together → separator fix; K8 **silently duplicated content** when a stale `.part`
survived a crash → unconditional clear + regression test. The security slice (K3)
and the money-adjacent nothing here; the format contract (K1) and the
anti-hallucination artifact (K7) got the strictest passes.

## Deferred (later milestones, by design)

- **Curated pack signing** (§2.6): `kpack-core` ships the verify path + is
  adversarially tested; `CURATOR_PUBLIC_KEY` is `None` (fail-closed) until a real
  key is pinned. See `docs/ops/pack-signing-runbook.md`. The parse-before-verify
  residual's fix (wrapper calls `verify_file` before open on CDN packs) is wired
  at §2.6.
- **Gate calibration** (§2.4 / §3.4): manifest gate thresholds are documented
  placeholders (`0.5`/`0.05`, `gate_calibrated=false`); the refusal-biased
  personal defaults + local mini-calibration are the scheduled fast-follow.
- **PDF/EPUB/DOCX/HTML parsers, OCR, contextual prefixes** (§2.3/§3.2): §1 ships
  Markdown + plain-text; the prefix column is empty (quick build).
- **The retrieval runtime** (§4: RRF fusion, the per-pack gate, NO_EVIDENCE
  emission), the **builder/retrieval UX**, and the **server pipeline** (§2) are
  their own milestones — K9's commands are a thin seam, not the product UX.

## Note

The MSI grows ~118 MB (the bundled BGE embedder, D3) — the "thin installer" is no
longer thin. If size matters later, the embedder can move to CDN delivery (the
download machinery already exists); the decision was bundle-for-simplicity now.
