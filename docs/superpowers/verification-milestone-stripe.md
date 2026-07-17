# Stripe milestone (Socratic Math Tutor purchase flow) — Verification Record

Date: 2026-07-17 – 2026-07-18 · Branch: `feat/stripe-payments` · Project:
`isltexsxpysxqewjsryv` (Cleophis) · Mode: **Stripe test**

This milestone turns the pre-Stripe "self-grant = free download" trust model
(accepted by design in the CDN milestone) into a real one-time $20 purchase
gate for the hero model (`socratic-tutor`). A Supabase edge function
(`create-checkout`) mints a Stripe Checkout Session; a second edge function
(`stripe-webhook`) is the **single writer** of `source='purchase'`
entitlement rows, signature-verified; `download-url` now fences paid models
on `entitlements.source` instead of trusting any row that exists. Every row
below traces to `.superpowers/sdd/progress.md`'s STRIPE MILESTONE section
and/or the commit/task report it cites.

## Result summary

| # | Date | Check | Result | Evidence |
|---|---|---|---|---|
| 1 | 2026-07-17 | Migration 0005 applied + constraint fires | **PASS** | `supabase/migrations/0005_entitlement_caps.sql` (commit `f92945f`) adds `entitlements_model_id_length` (CHECK `char_length(model_id) between 1 and 64`) and a soft per-user 100-row cap trigger. Reviewer caught a revenue-path bug in the original trigger (a BEFORE INSERT cap check would abort the webhook's ON CONFLICT UPSERT at the 100-row mark); fixed same-day with a `not exists()` exemption for rows that already exist (commit `05640b1`). Applied live via the Management API (HTTP 201); pre-check found 0 pre-existing bad rows; the length CHECK was verified firing on a 65-char `model_id` insert |
| 2 | 2026-07-17 | `entitlements()` synthetic-merge fix | **PASS** | `merge_pending_synthetics` (`src-tauri/src/cloud/session.rs`, commit `5448239`) stops a freshly-polled server entitlement list from transiently dropping a still-queued optimistic synthetic grant — needed once the app starts polling `list_entitlements` every 3s during checkout (S8). Rust suite **86/86** at landing (82 pre-existing + 4 new), 0 failed, 1 ignored; grew to **92/92** by S7 (row 6) as the checkout surface added its own tests on top of this fix |
| 3 | 2026-07-17 → 2026-07-18 | Function deploys | **PASS** | Three Edge Functions deployed via `tools/deploy-function.sh` (Management API, HTTP 201 each): `create-checkout` (`verify_jwt: true`), `stripe-webhook` (`verify_jwt: false` — Stripe has no Supabase JWT to send), and the now-removed `checkout-return` (`verify_jwt: false`) |
| 4 | 2026-07-17 → 2026-07-18 | Bootstrap results | **PASS** | `tools/stripe-bootstrap.sh` (idempotent): product `prod_Uu3a...`, price `price_1TuFTRDK47Qi4BpkEKxgLKrs` (lookup_key `socratic-tutor-onetime`, $20.00 usd one-time), webhook endpoint `https://isltexsxpysxqewjsryv.supabase.co/functions/v1/stripe-webhook` subscribed to `checkout.session.completed` — created, signing secret captured to `~/.env` (Stripe only returns it at creation). All three secrets (`STRIPE_SECRET_KEY`, `STRIPE_WEBHOOK_SECRET`, `STRIPE_PRICE_SOCRATIC`) pushed to the Supabase project as function secrets |
| 5 | 2026-07-17 → 2026-07-18 | Signed webhook matrix | **PASS** | `tools/test-stripe-webhook.mjs` against the deployed `stripe-webhook`: default mode → **200** + `entitlements` row confirmed `source='purchase'`; `--replay` (same signed body redelivered) → **200 twice**, same row unchanged (convergent UPSERT, no dedup table needed); `--tamper` (flipped signature nibble) → **400** `invalid_signature`; `--wrong-amount` (`amount_total: 500` against a valid UUID) → **400** `validation_failed`; a bonus manual probe with a non-UUID `user_id` → **400** `validation_failed` (the `USER_ID_RE` regex check) |
| 6 | 2026-07-18 | Rust suite (`cargo test`) | **PASS** | **92 passed**, 0 failed, 1 ignored (pre-existing live-network integration test), 0 compiler warnings on both `cargo build` and `cargo test --no-run` — commit `3909ad0` (S7: `create_checkout`/`CheckoutOutcome`/opener-plugin surface; 86 pre-existing + 6 new: request-shape and malformed-200 in `rest.rs`, happy-path/409-already-owned/non-Stripe-URL-rejected/401-no-infinite-retry in `session.rs`) |
| 7 | 2026-07-18 | REAL test-card purchase (user-verified) | **PASS** | Live Stripe Checkout Session completed end-to-end with a Stripe test card: `payment_status: paid`, `amount_total: 2000`, `currency: usd`. The webhook UPSERTed the `entitlements` row; the app's 3-second payment poll auto-detected `source === 'purchase'`, auto-started the download, and landed the user in a working chat session against the purchased model — the full attended flow, user-confirmed (S9) |
| 8 | 2026-07-18 | Return-page platform finding + GitHub Pages relocation | **FOUND + FIXED** | The same-repo `checkout-return` Edge Function served its `text/html` response as `text/plain` (+ `X-Content-Type-Options: nosniff`) on the live `*.supabase.co` domain, even though the committed code set `Content-Type: text/html; charset=utf-8` correctly. Root-caused (Supabase docs + two GitHub discussions) as an **unconditional platform-level rewrite on the shared, non-custom domain** — a documented anti-phishing behavior, not a code bug, with no in-function fix available. Resolved by moving both return pages to static HTML on the `gh-pages` branch (`pay/success.html`, `pay/cancelled.html` — commit `d28f382`) at `https://shojuro.github.io/cleophis/pay/success.html` and `.../cancelled.html`; `checkout-return/index.ts` deleted and `create-checkout`'s `success_url`/`cancel_url` repointed (commit `54e8b1b`). Both URLs verified live: HTTP 200, `Content-Type: text/html; charset=utf-8` |
| 9 | 2026-07-18 | LIVE fence matrix (incl. in-place library→purchase upgrade) | **PASS** | Against the redeployed `download-url` (per-model `allowedSources` fence, commit `54e8b1b`): a test user's `source='library'` row → **403** `not_entitled`; a signed webhook delivery for the same `(user_id, model_id)` **upgraded that library row to `source='purchase'` in place** — the UPSERT-not-insert design proven live under real conditions, not just unit-tested; the now-`purchase` row → **200** mint with the correct `fileBytes`. Throwaway test user cleaned up server-side afterward |
| 10 | 2026-07-18 (pre-S9) | Trial-row cleanup | **DONE** | Leftover `source='trial'` rows deleted ahead of the S9 real-purchase test, so the test exercised a clean unentitled → purchased transition instead of colliding with a pre-existing trial grant that would have masked the fence |

## Deferred / known items

Carried on the ledger, not gaps introduced by this milestone:

- **S4 minors** (`stripe-webhook`, opus-approved with no Critical/Important):
  no top-level `try`/`catch` (Deno's default 500 on an uncaught exception is
  considered self-healing here); Stripe's 3-day webhook retry schedule will
  generate retry noise against *permanent* failures (a dropped FK, a
  hard-capped `entitlements` row) that can never succeed — accepted, not
  fixed; a cosmetic mismatch where the livemode-rejection log line doesn't
  exactly mirror the response body.
- **Cross-drawer note — resolved.** S8's review caught that a payment-poll
  hit could take over the *wrong* model's open drawer if the user switched
  drawers mid-poll. Fixed same-task via `state.drawerId` identity tracking
  (commit `bb2c3ce`) — gated, not outstanding.
- **Business name = user dashboard action.** Stripe's test-mode API refuses
  to set the account's public business name programmatically; this is a
  one-time manual step in the Stripe Dashboard, required before the live
  flip (see `docs/ops/payments-runbook.md`'s LIVE-MODE FLIP checklist).
- **Cancel-path re-check — pending.** The Checkout cancel flow (user backs
  out of payment) was folded into the final whole-branch review round
  rather than independently re-verified during S9/S10; still open at time
  of writing.
- **Live-mode flip checklist** (full detail in `docs/ops/payments-runbook.md`):
  rotate the three Stripe secrets (`STRIPE_SECRET_KEY`,
  `STRIPE_WEBHOOK_SECRET`, `STRIPE_PRICE_SOCRATIC`) to live values, remove
  the `event.livemode === true` guard block in
  `supabase/functions/stripe-webhook/index.ts` (present specifically to
  block a premature real charge from being entitled during the test-mode
  build-out), and re-run `tools/stripe-bootstrap.sh` against the live
  secret key.
- **Carried from the CDN milestone, now closed by this one:** the "Stripe
  webhook must UPSERT" and "`download-url` must require `source='purchase'`"
  landmines recorded in `docs/superpowers/verification-milestone-cdn.md`
  are both addressed here (rows 1, 2, 9 above) — the free-self-grant trust
  model no longer applies to the hero model.
- **Other standing backlog notes** (non-blocking, from individual task
  reviews): replica-lag on `entitlements()` reads is transient, not a
  regression, for retired rows; `entitlements()` doesn't refresh
  `last_online_auth` on a poll (grace-period clock unaffected); S7 flagged
  a pre-existing fire-and-forget device-upsert thread
  (`apply_and_sync`) that can leak across the test lock and cause
  intermittent parallel-test flakiness — unrelated to Stripe, not fixed in
  this milestone.

### MULTI-MODEL LANDMINES (next milestone inherits these)

This milestone ships exactly one paid model (`socratic-tutor`) with no
free model in the same catalog yet. The following are correctness gaps
that don't bite at that scope but will as soon as a free (library) model
and a paid model coexist, or more than one purchase can be in flight —
found during final whole-branch review, recorded here rather than fixed
now since fixing them now would mean building against a shape (multi-model
catalog) this milestone doesn't have yet:

(a) **FE owned-check vs. server fence.** `state.mine` (`src/app.js`)
    hydrates from **all** entitlement sources, and `runGetFlow`
    short-circuits any model already in `state.mine` straight to download.
    Once a free (`library`-sourced) model and a paid model coexist, a
    self-granted `library` row on a *paid* model becomes a purchase dead
    end: the front end believes the model is owned and skips checkout
    entirely, but `download-url`'s per-model source fence (the source
    fence cutover, commit `54e8b1b`) rejects the download with 403 — the
    user can neither download the model nor get offered a Buy button for
    it. Fix at the next milestone: gate paid-model download eligibility
    client-side on `source === 'purchase'` specifically, not on mere
    presence in `state.mine`.
(b) **`beginPaymentPoll` leaks on a second concurrent purchase.**
    `beginPaymentPoll` (`src/app.js`) never clears a pre-existing
    `setInterval` before starting a new one, and `state.pay`/`state.dl`
    are single-slot. Starting a purchase on a second purchasable model
    while a first model's poll is still running leaks the first poll's
    timer — it keeps firing against the now-overwritten single-slot state
    rather than being cancelled. Fix when generalizing beyond one
    in-flight purchase at a time.
(c) Two more edge cases worth recording now rather than rediscovering
    later: at the 100-row per-user `entitlements` cap
    (`enforce_entitlement_cap()`,
    `supabase/migrations/0005_entitlement_caps.sql`), a paying customer's
    webhook UPSERT hits the cap trigger and 500s forever — paid but never
    entitled. Only reachable via deliberate self-flooding (100 distinct
    `model_id` rows for one user), not a normal-use path, but worth
    recognizing as a support signature if it's ever reported. Separately:
    changing a Stripe Price's amount strands any Checkout Session already
    minted at the old amount — `stripe-webhook`'s `EXPECTED` validation
    (`supabase/functions/stripe-webhook/index.ts`) will permanently reject
    that in-flight session's completion once the registry moves to the new
    amount, since `amount_total` can never match again. Drain in-flight
    sessions (or accept the loss) before changing a live Price.
(d) Platform undeploy of the old `checkout-return` Edge Function instance
    is confirmed (HTTP 200, coordinator-run) — closes out the item flagged
    as outstanding in the S10 task report ("No removal of the deployed
    `checkout-return` Edge Function instance (coordinator's job)").
