# Verification — §3a Visible RAG Loop (chat with your packs)

Branch `feat/rag-chat` (PR #13). The first cut of §3: it makes the §1 pack core and
the §4 retrieval brain **visible** — build a personal knowledge pack from your own
files, attach it to a chat, and get a **cited answer** or an honest
**`NO_EVIDENCE`** refusal. This is where RAG stops being an invisible command
surface and becomes the product experience. **Scope: the visible loop end to end**,
MD/TXT sources only (the existing parsers); generation uses the current base Llama
(the contract-trained adapter is the parked track); gate thresholds are the §1
placeholders until calibration. Real document parsers (PDF/DOCX/EPUB/HTML) are §3b;
enhanced builds and source staleness are §3c.

## What shipped

| Slice | What | Commits |
|---|---|---|
| A1 | Cached embedder (load-once `BgeEmbedder` in Tauri state) + `build_pack_with_progress` (progress callback + cancel), reused by `rag_query`/build | 3d992fd |
| A2 | File-dialog plugin + app-data pack store: builds into `packs/`, `list_packs`, path-safe `delete_pack`; `build_personal_pack` computes its own path + optional name | e2337f9 |
| A3 | FE **Packs** builder + picker UI: native file picker → build with live progress + cancel, list built packs, delete | a89a83f |
| A4 | FE **chat grounding + citations**: `rag_query` in `sendCompletion` (grounded system-prompt swap / honest no-evidence refusal / cited answers) + Stop-abort fix | 5b48641, ab7a993 |

## The loop the user now has

1. **Build** — signed in → **Packs** (header) → name it → *Choose files & build* →
   pick `.md`/`.txt` → a live progress bar (`Embedding i/N chunks`) with a working
   Cancel → the pack appears in the list; per-pack Delete works.
2. **Attach** — open a chat → **Packs** (chatbar) → check the pack(s) → *Done*; a
   `Packs: N attached` pill shows.
3. **Ask** — an in-corpus question shows *"Searching your packs…"* then a grounded
   answer with a **citation list** under the bubble (`[1] Title · Section ·
   locator`), and a `Grounded in N sources` pill. An out-of-scope question returns
   an honest *"no evidence in your packs"* refusal — no guess. **Unattached chats
   behave exactly as before A4.**

## The two-sided anti-hallucination contract, now visible

- **RAG decides what evidence exists** — the §4 per-pack calibrated gate (Δ1 before
  cross-pack fusion) yields either a grounded prompt or `NO_EVIDENCE`.
- **The grounded prompt** (the §1 K7 contract: system contract + numbered sources)
  instructs the model to cite `[n]` or refuse. The base model follows this
  imperfectly (no adapter yet) — an honest carve-out, set in the UI copy.
- **`NO_EVIDENCE` is handled deterministically in the FE** — a scripted honest
  refusal rather than handing the un-adapted base model the `[[NO_EVIDENCE]]`
  marker and hoping it refuses. When the contract-trained adapter lands, that branch
  can feed the marker to the model instead.

## Load-bearing engineering (verified in review)

- **Embedder cached once (A1)** — without it, grounded chat would reload the 118 MB
  model on every message. Proven: `Arc::ptr_eq` across `get_or_load` calls, one load
  per process; the mutex is held only across the lazy init, and `LlamaModel: Send +
  Sync` was verified against the `llama-cpp-2` source so a shared `Arc` is safe for
  concurrent embeds.
- **`build_pack_with_progress` is pure** — a callback + an `AtomicBool`, no network
  — so `kpack-core` stays network-free by construction.
- **`delete_pack` is path-safe** — canonicalizes both sides, requires the target's
  parent to equal the canonical packs dir and the extension to be `kpack`, and
  removes the *resolved* path. Adversarially reviewed (opus) against the full attack
  matrix (`..` traversal, absolute path, symlink escape, subdir, `.KPACK`/`.kpack.txt`
  case tricks, directory, non-existent, NUL) — it cannot delete anything outside the
  managed dir.
- **The no-packs chat path is provably byte-identical to pre-A4** — the grounding
  block is fully skipped when no packs are attached (`groundedPrompt ?? m.systemPrompt`),
  so normal chat is untouched. Confirmed line-by-line in review.
- **Stop is honored during pack search (A4-fix)** — A4 introduced an `await
  rag_query` before the abortable `fetch`; `sendCompletion` now checks
  `aborter.signal.aborted` after the query resolves (in both the error and result
  paths) and bails without committing a turn.

## Reviews

Every slice reviewed: A1 (sonnet), **A2 (opus, adversarial on `delete_pack`)**, A3
(sonnet), A4 (sonnet). All **Approved**. Findings, all handled: A1 — 3 minors, the
window-close-cancel one folded into A2; A2 — 3 cosmetic nits, path-safety held; A3 —
3 cosmetic minors, one (`hide()` transient reset) folded into A4; A4 — **1 Important
(Stop didn't cancel the in-flight `rag_query`, silently committing a `noEvidence`
turn) → fixed in ab7a993**, plus 2 no-action minors. XSS surface reviewed throughout:
every pack/citation field is `escapeHtml`-escaped in templates and citations render
via `textContent`-only DOM.

## Acceptance E2E (user, on the MSI) — the gate

Because this is the first time the whole loop runs against the **real** model in the
**packaged app**, the acceptance test is a hands-on run on a freshly built MSI:

1. Build a pack from a couple of `.md`/`.txt` files — the progress bar advances and
   Cancel works.
2. The pack appears in the Packs list; Delete removes it.
3. Attach it to a chat; an **in-corpus** question returns an answer **with a citation
   list**.
4. An **out-of-scope** question returns the **no-evidence refusal** (no fabricated
   answer).
5. An **unattached** chat behaves exactly as today.
6. Clicking **Stop** during *"Searching your packs…"* cancels cleanly (no committed
   turn).

## Honest carve-outs (documented, not hidden)

- Generation uses the base Llama; cite-or-refuse is imperfect until the parked
  contract-trained adapter.
- Gate thresholds are the §1 placeholders (`0.5`/`0.05`) — out-of-scope *rejection
  quality* is rough until calibration (§2.4/§3.4). The mechanism is real; the tuning
  is the fast-follow.
- **MD/TXT only.** PDF/DOCX/EPUB/HTML are §3b (including the native-PDF decision and
  the bytes/path parser seam).
- Enhanced builds (contextual prefixes), throttling refinements, and source
  staleness are §3c.

## Deferred (own milestones, by design)

- **§3b** real document parsers (+ the pdfium-vs-lopdf native-dep decision to bring
  to the user).
- **§3c** enhanced build + throttling + source staleness + refusal-biased defaults.
- **§2** curated server pipeline + the real per-pack gate calibration that replaces
  the placeholders.
- The **contract-trained LoRA adapter** (the parked hot-swap track) that makes
  cite-or-refuse crisp — it must train against `contracts/prompt-contract.v1.toml`.
