# Verification — §4 On-Device Retrieval Runtime

Branch `feat/rag-retrieval` (PR #12). The query-time brain on top of the §1 pack
core: retrieve → per-pack calibrated gate → cross-pack fuse → cite-or-`NO_EVIDENCE`
→ grounded prompt. This is where the anti-hallucination behavior becomes real and
measurable. **Scope: Rust brain + Tauri command only** (the visible chat-with-packs
UX is §3); generate uses the current base Llama (the contract-trained adapter is
the parked track); gate thresholds are the §1 placeholders until calibration.

## What shipped

| Slice | What | Commits |
|---|---|---|
| R1 | Per-pack retrieval + RRF fusion (k_rrf=60, rank-based) + `Pack::get_embedding` + fts5-safe query + dense-cosine (`dot_int8/16129`) | dfbaee9, 2143c1f |
| R2 | Per-pack **calibrated gate on dense cosine** (Δ1: `top1≥floor` AND `top1−cos@rank10≥margin`; per-pack, pre-fusion) | 930c76a, 2143c1f |
| R3 | Cross-pack fusion + `NO_EVIDENCE` + tier select (N=3/5) + dedupe + K7-contract assembly | 027b357, ed02823 |
| R4 | `retrieve()` entry point + `rag_query` Tauri command + real-embedder integration tests | 33e5830 |

## Live verification (the numbers that matter)

- **The pipeline discriminates — measured with the real BGE embedder:** an
  in-corpus query scores top-1 cosine **0.8064**; an out-of-scope query (a
  Mongolia question against a hemostasis corpus) scores **0.2746**. With the gate
  floor set to the measured midpoint (a calibration stand-in), the in-corpus query
  returns a **Grounded** cited answer and the out-of-scope query returns
  **`NO_EVIDENCE`**. This is the anti-hallucination behavior working end to end.
- **Δ1 holds with real vectors (the safety lock):** mount a strict pack (whose
  chunk scored 0.8064) and a loose pack; set the strict pack's own floor above its
  best score so it fails ITS gate. Result: the strict pack contributes **nothing**,
  and only the loose pack's citation survives — even though the strict chunk
  out-scored everything. A loosely-gated pack can never resurrect a chunk a
  strictly-gated pack refused. This is the property the whole multi-pack safety
  story rests on.
- **Fail-closed everywhere** (each caught by review, all fixed): an empty query
  skips the lexical lane instead of erroring; a `NaN` cosine fails the gate closed
  (never open); a query where every selected chunk fails to resolve returns
  `NO_EVIDENCE`, never a citation-less "grounded" answer that would tell the model
  to cite sources that don't exist.
- **Tests:** `kpack-core` **116 passing, zero warnings**; the app (`cleophis`)
  builds clean and its 120 tests pass; the real-embedder integration suite
  (`kpack-embed --features real -- --ignored`) is **6/6** (bge smoke, build
  determinism, and the 3 new RAG tests). No network dep entered `kpack-core` — the
  whole retrieval path is network-free by construction.

## Reviews

Each slice reviewed (R2's gate got an adversarial safety pass). Findings, all
fixed: R1 hard-errored on an empty query; R2's gate failed *open* on a NaN cosine
(unreachable but a safety-contract violation); R3 could emit a citation-less
"grounded" answer when chunks failed to resolve. The Δ1 gate-before-fusion
ordering — the one thing that must not regress — was traced live and confirmed
airtight, and is locked by the multi-pack regression test (synthetic in R3,
real-embedder in R4).

## The command

`rag_query(query, pack_paths)` (src-tauri): mounts the packs, embeds the query with
the bundled BGE model, runs `retrieve()`, and returns `{ status: "grounded" |
"noEvidence", prompt, citations }`. It does NOT generate — the chat-send/generate
wiring and the pack-picker UI are §3. Nothing in the app calls it yet, so **no MSI
rebuild is needed for §4** (the command is compiled in; it lights up when §3 wires
the chat).

## Deferred (own milestones, by design)

- **Gate calibration** (§2.4 curated / §3.4 personal mini-calibration) sets the
  *real* per-pack floors — the shipped `0.5`/`0.05` are documented placeholders, so
  out-of-scope *rejection quality* is rough until then. The mechanism is proven;
  the tuning is the fast-follow.
- **The chat wiring + citation UI** (§3), and the **contract-trained adapter**
  (the parked hot-swap track) that makes cite-or-refuse crisp.
- **Latency**: the dense-cosine lookup is one query per candidate — a `TODO(§5-perf)`
  batching win, to be measured on §5's floor machine.
