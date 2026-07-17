# CDN milestone (model delivery over B2 + Supabase) — Verification Record

Date: 2026-07-17 · Branch: `feat/cdn-delivery` · Project: `isltexsxpysxqewjsryv`
(Cleophis) · B2 bucket: `cleophis-models`

**Build under test:**
`src-tauri/target/release/bundle/msi/Cleophis_0.1.0_x64_en-US.msi` —
**37,285,888 bytes** (thin installer, models excluded from the bundle),
down from the fat installer's **2,021,289,984 bytes** (single hero model
bundled in-app, Milestone A baseline).

This milestone replaces the bundled-model install with an on-demand,
entitlement-gated download from a private B2 bucket, minted through the
Supabase edge function `download-url` (`supabase/functions/download-url/index.ts`).
Every row below traces to `.superpowers/sdd/progress.md`'s CDN MILESTONE
section and/or the commit/task report it cites.

## Result summary

| # | Check | Result | Evidence |
|---|---|---|---|
| 1 | Unit test suite (Rust, `cargo test`) | **PASS** | Final count **81 passed**, 0 failed, 1 ignored (pre-existing live-network test), zero warnings on `cargo build` and `cargo test`. Built up across the milestone: 72 after C1/C2 catalog+engine work, 80 after C3b's download-worker TDD slice (71 existing + 9 of the brief's 10 required tests — disk-space preflight #9 skipped as not portably testable), 81 after the C7 fast-fail fix added `repeated_403_fast_fails_with_server_refusal_message` (commit `03088ef`) |
| 2 | Catalog integrity fields | **PASS** | `sha256`/`version` added to `CatalogEntry`; hero entry's sha256 (`6c1a2b41...`) independently verified against the uploaded file (commit `e1c4e17`) |
| 3 | Live curl matrix against the deployed edge function | **PASS** | 401 no-JWT · 404 unknown model id · 404 `__proto__` (prototype-pollution probe on the `MODELS` lookup, fenced by `Object.hasOwn`) · 403 unentitled · 201 entitlement grant · 200 mint (valid JWT + entitled) · 206 ranged GGUF fetch against the minted B2 token · 401 tokenless B2 fetch. Throwaway test user cleaned up server-side after the run |
| 4 | Model upload to B2 | **PASS** | Hero model (`Llama-3.2-3B-Instruct-Q4_K_M.gguf`, 2,019,377,696 bytes) uploaded via `curl -T` single-shot (`b2_authorize_account` → `b2_get_upload_url` → upload with `X-Bz-Content-Sha1`); B2 server-side sha1 verification passed; ~11.7 MB/s |
| 5 | Fresh real download → chat (user-verified, installed thin app) | **PASS** | Live progress events observed during download; on completion the app auto-slid straight into a working chat session against the freshly downloaded model |
| 6 | Offline relaunch after download | **PASS** | Relaunching the installed app with the model already on disk works fully offline — no re-download, no network dependency for a model already present |
| 7 | Live resume from a partial download | **PASS** | Seeded a `.part` file at 445 MB, relaunched the download — resumed from 23% (matching the seeded byte offset) via HTTP Range, completed, and passed sha256 verification against the catalog-pinned hash |
| 8 | B2 daily download-cap incident | **FOUND + FIXED** | At the C7 checkpoint, B2's free-tier daily cap was hit mid-testing: every request returned `HTTP 403 {"code":"download_cap_exceeded",...}`. The download worker's retry path originally reported this as the generic "check your connection" message even though the connection was fine and B2 was definitively refusing. Fixed in commit `03088ef`: 403/404 are now classified as a "definitive refusal" (`FailureKind::Status`) distinct from a transport error, the give-up message names the HTTP status and calls out a possible account limit, and two consecutive definitive refusals fast-fail after the 2nd attempt instead of a 3rd. Re-reviewed and approved. The bucket's daily cap was subsequently raised to **$3/day** by the account owner |
| 9 | Demo-tag install check | **DEFERRED BY CONSTRUCTION** | The `v0.1.0-demo` tag's build artifact is immutable (fat installer, model bundled); installing it would replace the thin install currently under test, so this check is not run as part of this milestone — noted, not a gap requiring follow-up |

## Deferred to later milestones

- **Multi-model switching** — the edge function's `MODELS` registry
  (`supabase/functions/download-url/index.ts`) currently carries a single
  entry (`socratic-tutor`); this milestone delivers the hero model's
  delivery pipeline only, not a UI/backend flow for switching between
  multiple downloaded models.
- **Cloudflare domain swap** — the delivery scheme is built to be
  hostname-agnostic (`B2_DOWNLOAD_BASE_URL` env override, see
  `docs/ops/model-delivery-runbook.md`), but no CDN domain has actually
  been put in front of the B2 bucket yet; downloads currently resolve to
  B2's native `downloadUrl`.
- **Stripe integration landmines** (recorded in the ledger ahead of
  Milestone B's payment work):
  - The Stripe webhook **must UPSERT** on `(user_id, model_id)` via
    `service_role` — a plain insert conflicts with client-side "library"
    rows and would silently drop real purchases.
  - The `download-url` edge function must require `source='purchase'` (or
    a per-model allow-list of sources) once payments are real — today,
    self-granting an entitlement yields a free download, accepted **by
    design** as a pre-Stripe trust-model decision, but not safe to carry
    into a milestone with real money moving.
  - `model_id` caps/FK constraints should land before money is involved.
- **Minor triage leftovers** (non-blocking, flagged across task reviews
  for a later cleanup pass):
  - Redundant catalog reads in the engine spawn path (C2).
  - Test comment numbering order in the C3a test file — cosmetic.
  - From C3b's review: 4xx fast-fail UX polish beyond the C7 fix,
    `Content-Range` response validation, cancel-during-rehash latency,
    stale-`.part` preflight math, `bytes_per_sec` cosmetic rounding.
  - From C4's review: the "✓ Paid" label never actually paints, a brief
    (≤500 ms) blank progress bar on drawer reopen, and the done-phase
    auto-enter-chat behavior firing regardless of context — spec'd for the
    attended demo flow, flagged as a design question for an unattended
    flow later.
