# Subscriptions milestone (Socratic Math Tutor monthly plan) — Verification Record

Date: 2026-07-18 · Branch: `feat/subscriptions` · Project:
`isltexsxpysxqewjsryv` (Cleophis) · Mode: **Stripe test**

This milestone turns the Stripe milestone's one-time $20 purchase gate for
the hero model (`socratic-tutor`) into a $20/mo subscription (Stripe
Checkout `mode: subscription`, price env `STRIPE_PRICE_SOCRATIC_MONTHLY`,
lookup_key `socratic-tutor-monthly`, test price
`price_1TuK9DDK47Qi4BpkZCnrk1Iv`). A new migration (`0006_subscriptions.sql`)
adds `stripe_customers` and the single subscription writer,
`apply_subscription_period` (extend-only, lifetime-protecting, null-
rejecting); the webhook's new `invoice.paid` branch is the single
authoritative subscription event, covering both first payment and every
renewal; `create-checkout`'s already-purchased 409 became expiry-aware so a
lapsed subscriber can re-subscribe; a new `customer-portal` edge function
and a Rust `open_billing_portal` command let a subscriber manage or cancel
their plan via Stripe's hosted billing portal. Every row below traces to
`.superpowers/sdd/progress.md`'s SUBSCRIPTIONS MILESTONE section and/or the
task report it cites.

## Result summary

| # | Check | Result | Evidence |
|---|---|---|---|
| 1 | Migration 0006 — `stripe_customers` + `apply_subscription_period` RPC | **PASS** | `supabase/migrations/0006_subscriptions.sql` (commit `b453998`) adds `stripe_customers` (service-role-only, zero client grants) and the extend-only `apply_subscription_period(user_id, model_id, period_end)` RPC with the explicit lifetime-grandfathering CASE branch. Coordinator review caught that a NULL `period_end` would mint an un-expirable row indistinguishable from a lifetime purchase; same-day fix added a `raise exception` null guard (commit `0c76bdf`). Not applied by the implementer — applied live by the coordinator ahead of Sub4's synthetic suite |
| 2 | `stripe-webhook`'s `invoice.paid` branch | **PASS** | `supabase/functions/stripe-webhook/index.ts` (commit `cbd323a`) adds the `SUB_MODELS` price-id registry and `handleInvoicePaid` — defensive subscription-id/metadata extraction across API-version-dependent payload shapes, validation (UUID, `Object.hasOwn`, `paid===true`, `currency==="usd"`, price-id match), `periodEnd` from the max matched line period, write via the RPC (never a direct upsert), then a best-effort `stripe_customers` upsert after the entitlement write. Opus review approved with one Important: an unbounded `periodEnd` from Stripe could over-grant past the RPC's extend-only design's ability to walk back — fixed same-day with a ~400-day upper cap alongside the existing `>0` check (commit `9387e93`). The pre-existing `checkout.session.completed` branch confirmed byte-identical via `git diff --unified=0` (zero removed/modified lines in its span) |
| 3 | Tester `--sub*` synthetic modes | **PASS** | `tools/test-stripe-webhook.mjs` (commit `1c68c0b`, +429/-4 lines) adds `buildInvoiceEvent`, `resolveMonthlyPriceId` (looks up the live `socratic-tutor-monthly` price by lookup_key), and six new flags — `--sub`, `--sub-replay`, `--sub-out-of-order`, `--sub-lifetime-protect`, `--sub-wrong-price`, `--sub-no-metadata`. Review approved with an independent fidelity re-derivation (every field `handleInvoicePaid` reads is present in the synthetic payload, nothing extra); minors only (stale header claim re response bodies, a replay-snapshot race note). All 4 pre-existing modes confirmed byte-identical; `node --check` clean |
| 4 | Bootstrap — monthly price, dual webhook events, portal config; deploy + live synthetic suite | **PASS** | `tools/stripe-bootstrap.sh` (commit `01b3247`) adds: reuse-or-create the monthly price by `lookup_key=socratic-tutor-monthly`; webhook-endpoint idempotency now inspects `enabled_events`/`api_version` and updates-in-place or recreates as needed to land on **both** `checkout.session.completed` and `invoice.paid` pinned to `api_version=2024-06-20`; a new non-fatal portal-configuration step. Review caught a rotation-path bug (duplicate `STRIPE_WEBHOOK_SECRET=` lines in `~/.env` on an api_version-mismatch recreate); fixed same-day with a `sed -i` dedupe before the append (commit `ecdbdd5`). Live run (coordinator, no separate task report — recorded directly on the ledger): `stripe-webhook` redeployed via `tools/deploy-function.sh` (the Management-API deploy path every function deploy this and the Stripe milestone confirms as HTTP 201); bootstrap produced monthly price `price_1TuK9DDK47Qi4BpkZCnrk1Iv`, webhook endpoint recreated with both events + pinned `api_version` (secret rotated and deduped, confirmed 1 line), portal config `bpc_1TuK9N...`, 4 secrets pushed. **Full synthetic suite PASS (8/8)**, including `--sub-lifetime-protect` against the real grandfathered row (`expires_at` still `NULL`, server-verified) and `--sub-out-of-order` proving `GREATEST` (final row landed at the earlier-delivered +60d value, not the later +30d). Throwaway test data cleaned |
| 5 | `create-checkout` subscription mode + expiry-aware 409 | **PASS** | `supabase/functions/create-checkout/index.ts` (commit `ad325aa`): `MODELS.priceEnv` repointed to `STRIPE_PRICE_SOCRATIC_MONTHLY`; already-purchased query gains `.or("expires_at.is.null,expires_at.gt.<now>")` chained after `.eq("source","purchase")` (null or future expiry 409s, a lapsed row falls through to a fresh session); `mode: "subscription"` with `subscription_data.metadata`; idempotency key reprefixed `subcheckout-` (can never collide with a pre-milestone `checkout-` key). Review approved, zero Critical/Important; a stale header comment fixed by the coordinator (`ce6e361`). Deployed: HTTP 201 (`verify_jwt` on). **Portal probe** (ahead of Sub6's function existing): bootstrap config `bpc_1TuK9NDK47Qi4Bpkki7vvvwG` confirmed `is_default=true`; `billingPortal.sessions.create` **without** a `configuration` param minted a `https://billing.stripe.com/p/session/...` URL — no portal-config secret needed. Throwaway probe customer deleted |
| 6 | `customer-portal` edge function + portal-return page | **PASS** | New file `supabase/functions/customer-portal/index.ts` (commit `cada8a7`) mirrors `create-checkout`'s structure/error discipline: env fail-fast, POST-only, anon-client `getUser` auth, service-role `stripe_customers` lookup (404 `no_billing_account` for never-subscribed users — the expected answer, not an error), no `configuration` param, capability URL never logged. Opus adversarial review approved, zero Critical/Important — IDOR closed (lookup bound solely to server-validated `user.id`, request body never read), service-role key used in exactly one parameterized query. Deployed: HTTP 201 (`verify_jwt` on); unauthenticated POST confirmed 401 (platform gate). `pay/portal-return.html` pushed to `gh-pages` (commit `53fdb23`) — **live at HTTP 200** after Pages rebuild |
| 7 | Rust portal seam — `open_billing_portal` | **PASS** | `src-tauri/src/cloud/rest.rs`/`session.rs`/`commands.rs`/`main.rs` (commit `2debdab`) mirror the checkout seam layer-for-layer: `PortalSessionResponse` (no `Debug` — capability URL), `Cloud::create_portal_session` with the identical fresh-access/retry-once shape as `create_checkout`, `portal_outcome` gating any URL on `https://billing.stripe.com/` before it ever reaches the opener, 404 mapped to `NoBillingAccount` (not an error). 4 new tests. Review approved, zero findings — confirmed a single construction/consumption site for `PortalOutcome::Url` (no ungated path to the opener) and a byte-untouched checkout seam; a pre-existing doc nit fixed by the coordinator (`7d10297`). **Rust suite: 97 total (93 pre-existing + 4 new), serial run clean** — 96 passed, 0 failed, 1 ignored (a documented pre-existing parallel mock-`TcpListener` flake, not a regression, confirmed by rerunning serially) |
| 8 | FE — $20/mo copy + Billing button | **PASS** | `src-tauri/resources/catalog.json` (hero `"price"` → `"$20/mo"`), `src/index.html` (`billingBtn`, hidden by default, beside `signOutBtn`), `src/app.js` (commit `a2af328`) — every `signOutBtn.style.display` toggle site mirrored for `billingBtn` (session-restore included), click handler disables/re-enables around `invoke('open_billing_portal')`, `noBillingAccount` → toast, `String(e)` catch matching the house idiom. Review approved, zero findings — exhaustive toggle census (no other class/hidden/container toggle sites), boot path safe (`display:none` default), checkout flow/poll timer/`state.mine`/download-chat gating/drawer logic confirmed byte-untouched, `catalog.json` valid |

## Deferred / known items

Carried on the ledger, not gaps introduced by this milestone:

- **Real E2E is pending user-assist** — see the checklist below. Everything
  above is synthetic-signed-event or code-review verification; no real
  Stripe test-card subscription has been run through the app yet this
  milestone.
- **Test clocks were skipped.** Stripe Test Clocks (which simulate real
  billing-cycle passage) were not used — the synthetic tester's
  `--sub-replay`/`--sub-out-of-order` modes cover the renewal-math
  invariants (idempotent convergence, `GREATEST` extend-only) directly
  against the RPC/webhook without needing a real subscription to age.
- **Cancel-path verification, carried from the Stripe milestone.** The
  Stripe milestone's doc flagged the Checkout *cancel* flow (user backs out
  of payment) as folded into a whole-branch review rather than
  independently verified. This milestone adds a *second*, distinct
  cancel-path question — cancelling an active subscription via the billing
  portal (`subscription_cancel` at `mode: at_period_end`, configured by
  `tools/stripe-bootstrap.sh`'s portal step) — which is likewise unverified
  live; both are covered by the real-E2E checklist below.
- **Business name = user dashboard action.** Still open, unchanged from the
  Stripe milestone — Stripe's API refuses to set this programmatically in
  test mode; required before the live flip (see
  `docs/ops/payments-runbook.md`'s LIVE-MODE FLIP checklist, extended this
  milestone with the monthly price/dual-event/portal-config steps).
- **Cap-trigger + webhook-retry interaction** (documented in
  `docs/ops/payments-runbook.md`'s Known caveats) — pre-existing class from
  the Stripe milestone, now confirmed to apply identically to the
  subscription RPC path; not fixed, not newly introduced, recorded there
  rather than here.

## Pending — real E2E (user-assist)

None of the following have been run yet. All server-side/synthetic
verification above is a necessary but not sufficient substitute — Stripe's
actual Checkout UI, actual invoice/webhook timing, and the actual Tauri
opener/portal round-trip have not been exercised end-to-end this milestone.

- [ ] Real $20/mo checkout with a Stripe test card (`4242 4242 4242 4242`)
      completes successfully
- [ ] Resulting `entitlements` row shows `expires_at` ≈ now + 1 month
- [ ] `stripe_customers` row exists for the user (populated by the
      post-entitlement best-effort upsert)
- [ ] Download works (model mints and downloads normally post-subscribe)
- [ ] The Billing button opens the Stripe customer portal
- [ ] Cancelling in the portal (cancel-at-period-end) leaves the
      `entitlements` row **unchanged** — it lapses naturally at
      `expires_at`, not immediately
- [ ] Re-checkout while the subscription is still active returns **409**
      (expiry-aware already-subscribed gate)
- [ ] SQL-force the row's `expires_at` into the past (Manual revocation,
      `docs/ops/payments-runbook.md`)
- [ ] Post-force, download **403**s (soft lapse enforced)
- [ ] Post-force, re-subscribing mints a **fresh** Checkout Session (not a
      stale/cached one)

Note: Stripe Test Clocks (simulated billing-cycle passage) were deliberately
skipped for this milestone — the synthetic tester's `--sub-replay`/
`--sub-out-of-order` modes already cover the renewal math (idempotent
convergence, extend-only `GREATEST`) directly against the RPC/webhook, so
the checklist above exists to prove the real Stripe UI/webhook/opener
round-trip, not to re-derive math already proven synthetically.

## Multi-model landmines

Carried forward from `docs/superpowers/verification-milestone-stripe.md`
(both still unfixed — this milestone didn't touch either code path, per
Sub8's explicit landmine list):

(a) **FE owned-check vs. server fence.** `state.mine` (`src/app.js`)
    hydrates from **all** entitlement sources with no source filter, and
    `runGetFlow` short-circuits any model already in `state.mine` straight
    to download. Once a free (`library`-sourced) model and a paid model
    coexist, a self-granted `library` row on a paid model becomes a
    purchase dead end: the front end believes the model is owned and skips
    checkout, but the server's per-model source fence rejects the download
    with 403.
(b) **`beginPaymentPoll` leaks on a second concurrent purchase.**
    `beginPaymentPoll` (`src/app.js`) never clears a pre-existing
    `setInterval` before starting a new one, and `state.pay`/`state.dl` are
    single-slot — starting a purchase on a second purchasable model while a
    first model's poll is still running leaks the first poll's timer.

New this milestone, found by cross-referencing the expiry-aware 409/download
fence (Sub5, `download-url`'s `.or("expires_at.is.null,expires_at.gt.<now>")`
filter) against `state.mine`'s actual population code:

(c) **FE owned-check is also expiry-blind.** `state.mine = new Set((info
    .entitlements || []).map((e) => e.modelId))` (`src/app.js:507`) is a
    flat set of model ids — it carries no `expires_at`. `runGetFlow`'s
    `if (state.mine.has(m.id) || state.dl.partBytes > 0) { startDownload();
    return; }` (`src/app.js:173`) therefore treats a **lapsed** subscription
    exactly like an active one: once a subscription's `expires_at` passes,
    the row still exists (soft lapse leaves it in place by design) so
    `state.mine` still reports the model as owned, the front end skips
    checkout, and `startDownload` calls straight into `download-url`'s
    expiry-aware fence — which now 403s. The user lands on a download
    failure with no Buy/re-subscribe button offered, the same dead-end
    shape as landmine (a) but triggered by time instead of source. This
    didn't exist before this milestone because one-time purchases never
    expired. Fix at the next milestone alongside (a): gate paid-model
    download eligibility client-side on an entitlement's actual expiry, not
    on mere presence in `state.mine`.
