# Payments runbook — Cleophis Stripe integration

How a $20 one-time model purchase gets from "user clicks Get" to a paid
download, and how to add another paid model to the pipeline. Source of
truth for the pieces this document describes:
`supabase/functions/create-checkout/index.ts`,
`supabase/functions/stripe-webhook/index.ts`,
`supabase/functions/download-url/index.ts`, `tools/stripe-bootstrap.sh`,
`tools/deploy-function.sh`, `tools/test-stripe-webhook.mjs`.

## Architecture in 5 lines

1. `create-checkout` mints a Stripe Checkout Session for `(user_id,
   model_id)` — an `idempotencyKey: checkout-${userId}-${modelId}` means
   rapid double-clicks return the *same* session instead of double-charging,
   and an existing `source='purchase'` row short-circuits with `409
   already_purchased` before Stripe is ever called.
2. The app opens that session's URL in the system browser
   (`tauri-plugin-opener`, Rust-side only — no capability/CSP change) and
   the user completes checkout there, not inside the app.
3. Stripe calls `stripe-webhook` directly (no Supabase JWT — security is
   entirely Stripe signature verification, `verify_jwt: false`). On a
   validated `checkout.session.completed` with `payment_status: paid`, it
   **UPSERTs** — never plain-inserts — an `entitlements` row with
   `source='purchase'`, keyed on `onConflict: "user_id,model_id"`. This is
   the single writer of purchase entitlements and the fix for the landmine
   the CDN milestone flagged in advance: a plain insert would 23505-conflict
   against a client-writable `library`/`trial` row for the same pair, or a
   delete+insert would race a concurrent `download-url` entitlement check.
4. Back in the app, `beginPaymentPoll` calls `list_entitlements` every 3
   seconds (10-minute deadline) waiting for a `source === 'purchase'` hit on
   the model just bought, then auto-starts the download.
5. `download-url` fences each model on which `entitlements.source` values
   are accepted for it (`allowedSources`, checked via `.in("source",
   model.allowedSources)` in the entitlement query) — paid models require
   `["purchase"]`; a free model would list `["library"]`. This is per-model,
   not global, so free and paid models can coexist in the same registry.

## Adding a paid model

Four files change together — they're independent in-source registries by
design (no shared schema), so each one needs its own entry:

1. **Stripe product + price**, following the bootstrap pattern in
   `tools/stripe-bootstrap.sh` (reuse-or-create by `lookup_key`, e.g.
   `<model-id>-onetime`) — either extend the script to loop over a list of
   models, or run the same two Stripe API calls (`POST /v1/products`, then
   `POST /v1/prices` with `lookup_key`) by hand for the new model and note
   the resulting price id.

2. **`stripe-webhook`'s `EXPECTED` registry**
   (`supabase/functions/stripe-webhook/index.ts`) — the amount/currency a
   completed Checkout Session must match exactly before an entitlement is
   granted:
   ```ts
   const EXPECTED: Record<string, { amount: number; currency: string }> = {
     "socratic-tutor": { amount: 2000, currency: "usd" },
     // "<new-model-id>": { amount: <cents>, currency: "usd" },
   };
   ```

3. **`create-checkout`'s `MODELS` registry**
   (`supabase/functions/create-checkout/index.ts`) — maps the model id to
   the Supabase secret name holding its Stripe Price id:
   ```ts
   const MODELS: Record<string, { priceEnv: string; display: string }> = {
     "socratic-tutor": { priceEnv: "STRIPE_PRICE_SOCRATIC", display: "Socratic Math Tutor" },
     // "<new-model-id>": { priceEnv: "STRIPE_PRICE_<NEW>", display: "<Display Name>" },
   };
   ```
   Then push `STRIPE_PRICE_<NEW>` to the Supabase project as a function
   secret (same pattern as `tools/stripe-bootstrap.sh`'s step 3 — value via
   `os.environ`, never argv).

4. **`download-url`'s `allowedSources`**
   (`supabase/functions/download-url/index.ts`) — the new model's `MODELS`
   entry needs `allowedSources: ["purchase"]` alongside its existing
   `prefix`/`file`/`bytes` fields (see
   `docs/ops/model-delivery-runbook.md` for adding the B2/catalog side of a
   new model).

5. **Catalog price copy** (`src-tauri/resources/catalog.json`) — the
   model's `"price"` field (e.g. `"$20"`) is display text only, read by
   `src/app.js` for the Get/Download button label and drawer meta line
   (`m.price`, several call sites). It is **not** wired to the Stripe
   amount programmatically — keep it in sync with `EXPECTED[...].amount`
   by hand.

6. Redeploy the two changed functions:
   ```bash
   tools/deploy-function.sh create-checkout
   tools/deploy-function.sh stripe-webhook --no-verify-jwt
   tools/deploy-function.sh download-url   # deploy-download-url.sh also works, per its own runbook
   ```

## Subscriptions (monthly plan)

The hero model's $20 one-time purchase became a $20/mo subscription in the
subscriptions milestone (Stripe Checkout `mode: subscription`, price env
`STRIPE_PRICE_SOCRATIC_MONTHLY`, lookup_key `socratic-tutor-monthly`, test
price `price_1TuK9DDK47Qi4BpkZCnrk1Iv`). Source of truth for this section:
`supabase/migrations/0006_subscriptions.sql`,
`supabase/functions/stripe-webhook/index.ts`'s `handleInvoicePaid`,
`supabase/functions/create-checkout/index.ts`'s subscription-mode session,
`supabase/functions/customer-portal/index.ts`.

### Event flow

1. `create-checkout` mints a Checkout Session with `mode: "subscription"`
   against `STRIPE_PRICE_SOCRATIC_MONTHLY`, carrying
   `subscription_data.metadata: { user_id, model_id }` (the session-level
   `metadata`/`client_reference_id` used by the payment branch are
   unchanged). Its idempotency key is `subcheckout-${userId}-${modelId}` —
   a distinct prefix from the one-time flow's `checkout-${userId}-${modelId}`,
   so a 24h-cached key from before this milestone can never collide with
   one minted after it.
2. `stripe-webhook`'s **`invoice.paid`** is the single authoritative
   subscription event — the first invoice on a new subscription and every
   renewal invoice after it fire the same event type; there is no separate
   "subscription created" code path. On a matched invoice the webhook
   extracts the subscription id and `{user_id, model_id}` metadata
   defensively (the payload shape varies by the endpoint's configured
   Stripe API version, so both the classic `invoice.subscription` /
   `invoice.subscription_details.metadata` shape and the newer
   `invoice.parent.subscription_details.*` shape are read; a payload that
   matches neither is a 200-ignore, not ours), validates `invoice.paid ===
   true`, `currency === "usd"`, and that a line's `price.id` matches
   `STRIPE_PRICE_SOCRATIC_MONTHLY`, then computes `periodEnd` as the max
   `line.period.end` across the matched lines — capped at `now + 400 days`
   upstream in the webhook, so a corrupt or absurd Stripe value can never
   mint a multi-year grant.
3. The webhook calls `apply_subscription_period(user_id, model_id,
   period_end)` — the **only** code path allowed to write a non-null
   `expires_at`. AFTER that write succeeds, it best-effort upserts
   `stripe_customers(user_id, customer_id)` (`onConflict: "user_id"`); any
   error there, including a cross-user `customer_id` unique clash, is
   logged and swallowed rather than failing the event — the entitlement
   write is the critical one, the customer mapping only powers the billing
   portal.
4. **Renewal is the same event.** Nothing in the webhook distinguishes a
   subscription's first `invoice.paid` from its second, third, etc. — the
   RPC's extend-only semantics below are what make that safe.

### Replay / out-of-order safety

`apply_subscription_period` is an `INSERT ... ON CONFLICT (user_id,
model_id) DO UPDATE` whose expiry math is `GREATEST(entitlements.expires_at,
excluded.expires_at)`. A redelivered event (Stripe retry) or two renewal
events arriving out of order both converge to the same final `expires_at`
(the later period end) — it can only extend, never shrink. Proven live via
the synthetic tester's `--sub-replay` and `--sub-out-of-order` modes (see
`docs/superpowers/verification-milestone-subscriptions.md`).

### Lifetime grandfathering

Rows written by the original one-time-purchase milestone — `source =
'purchase'` with `expires_at IS NULL` — get an explicit branch in the RPC:
```sql
when entitlements.source = 'purchase' and entitlements.expires_at is null
  then null
```
`GREATEST()` ignores `NULL` operands, so without this branch a lifetime row
would silently receive whatever `expires_at` arrived on the next
`invoice.paid` for that `(user_id, model_id)` pair. This branch is why that
can't happen: lifetime rows are never migrated and never touched by
anything in this milestone.

### Soft lapse

Subscription expiry gates **downloads only** — `download-url`'s entitlement
query (`.or("expires_at.is.null,expires_at.gt.<now>")`, unchanged in shape
since the CDN milestone and now also serving subscription rows) is the sole
enforcement point. A model already downloaded/installed keeps chatting
forever once a subscription lapses; there is no revocation of local model
files and no client-side lapse UX — the app has no code path that tells a
user their subscription has lapsed. `create-checkout` runs the identical
expiry filter for its already-subscribed 409 check, so a lapsed row also
falls through to a fresh Checkout Session instead of blocking re-subscribe.

### Manual revocation

To force-expire a subscription row for support/testing (e.g. to simulate a
lapse without waiting for a real billing cycle):

```sql
update public.entitlements
set expires_at = now()
where user_id = '<uuid>'
  and model_id = '<model>'
  and source = 'purchase'
  and expires_at is not null;
```

**Never touch a row where `expires_at IS NULL`.** That is a grandfathered
lifetime purchase, not a lapsed subscription — the `and expires_at is not
null` guard above is what keeps this statement from being able to touch
one even by accident. Double-check the `where` clause, especially the
`<uuid>`/`<model>` values, before running this against production.

After running it, the user can re-subscribe immediately —
`create-checkout`'s expiry-aware 409 only blocks a *future* or *null*
expiry, so a row manually forced to `now()` falls through to a fresh
Checkout Session exactly like a naturally lapsed one.

### Known caveats

- **Idempotency-key staleness window.** `create-checkout`'s subscription
  idempotency key (`subcheckout-${userId}-${modelId}`) is deterministic, so
  Stripe serves back the *same* cached Checkout Session for up to 24h after
  the key's first use. A user who re-subscribes within 24h of their
  original checkout click could be handed a stale session. Not exploitable
  at this milestone's actual monthly cadence (a lapse is ~30 days after the
  last successful charge, far outside any 24h window) — revisit only if a
  sub-24h expiry or testing path is ever added to the product surface.
- **Cap-trigger + webhook-retry interaction.** The 100-row-per-user soft
  cap (`enforce_entitlement_cap()`,
  `supabase/migrations/0005_entitlement_caps.sql`) fires as a `BEFORE
  INSERT` trigger on `entitlements`, which also fires for
  `apply_subscription_period`'s `INSERT ... ON CONFLICT DO UPDATE`
  (Postgres runs `BEFORE INSERT` triggers ahead of conflict resolution). A
  first-time subscriber landing exactly at the 100-row cap hits `raise
  exception` and gets a webhook 500, which Stripe retries for 3 days — all
  of which also 500, so the customer is paid but never entitled. This is
  the same pre-existing class flagged for the one-time-purchase path in
  `docs/superpowers/verification-milestone-stripe.md`; the `not exists()`
  renewal exemption added in migration 0005 (fix `05640b1`) already
  protects renewals on both paths equally, since 0006's RPC writes through
  the identical `INSERT ... ON CONFLICT` shape and hits the same trigger —
  it needed no guard of its own. Only a genuinely new `(user_id, model_id)`
  pair landing at the cap is exposed, on either path, and only reachable
  via deliberate self-flooding (100 distinct `model_id` rows for one user).
- **Residual dunning-window case (two subs, one customer).** `create-checkout`
  now reuses the mapped Stripe Customer on re-subscribe (see its
  `stripe_customers` lookup ahead of the Checkout Session call), which
  closes the orphaned-customer version of this problem. But if a user
  re-subscribes while their old subscription is still inside Stripe's
  smart-retry (dunning) window, they end up with TWO subscriptions on the
  SAME customer — the lapsed one Stripe hasn't given up on yet, plus the
  new one just created. Both are now portal-visible and self-serve
  cancellable, so the support answer is simple: cancel the stale
  subscription in the Billing Portal or the Stripe Dashboard. Future
  hardening option: have `create-checkout` cancel any non-canceled prior
  subscription on the customer at re-checkout time, instead of leaving two
  live.
- **Stale/deleted Stripe Customer id blocks checkout.** If a
  `stripe_customers` row points at a customer that no longer exists in the
  active Stripe mode (test-data wipe, or test-mode rows surviving the
  live flip), `create-checkout`'s `customer` param makes Stripe reject the
  session (`resource_missing`) and the user gets a 502
  `payment_provider_unavailable` on every attempt — self-perpetuating,
  because checkout never completes so no `invoice.paid` fires to rewrite
  the mapping. Remedy: delete the offending row
  (`delete from public.stripe_customers where user_id = '<uuid>';`) — the
  next checkout then omits `customer` and mints a fresh one. See also
  LIVE-MODE FLIP step 6.

## LIVE-MODE FLIP checklist

Everything above and everything currently deployed runs against **Stripe
test mode**. Before real money can move, in order:

1. **Set the account's public business name in the Stripe Dashboard.**
   Stripe's API refuses to set this programmatically in test mode — it's a
   one-time manual step in the Dashboard UI, and Checkout will not
   represent the business correctly to real customers until it's done.
2. **Rotate four secrets to their live-mode values**: `STRIPE_SECRET_KEY`,
   `STRIPE_WEBHOOK_SECRET`, `STRIPE_PRICE_SOCRATIC`, and (subscriptions
   milestone) `STRIPE_PRICE_SOCRATIC_MONTHLY`. Live and test Stripe
   objects are entirely separate — a live secret key, a live webhook
   endpoint (with its own live signing secret), and live Price ids for
   both the one-time and monthly products (even though each represents the
   "same" product test mode already has) are all needed. `~/.env` ends
   up with the test-mode `STRIPE_WEBHOOK_SECRET=` line still present from
   the original `tools/stripe-bootstrap.sh` run (step 4 below appends the
   live one rather than replacing it in place) — prune the stale test line
   by hand so a future read of `~/.env` doesn't pick up the wrong secret.
   Optionally disable (rather than delete) the test-mode webhook endpoint
   in the Stripe Dashboard once the live one is confirmed working, so test
   deliveries stop arriving at a production-adjacent function.
3. **Remove the livemode guard** in
   `supabase/functions/stripe-webhook/index.ts` — the block reading
   ```ts
   if (event.livemode === true) {
     console.error("livemode event rejected (test deployment)");
     return jsonResponse({ error: "invalid_signature" }, 400);
   }
   ```
   exists specifically to stop a premature Dashboard misconfiguration from
   granting a real charge an entitlement while the build-out was still in
   test mode. Leaving it in place after the flip would silently reject every
   real purchase, so it must come out (delete the block and its comment)
   as part of the same change that rotates the secrets.
4. **Re-run `tools/stripe-bootstrap.sh` against the live secret key** (i.e.
   with a live `STRIPE_SECRET_KEY` in `~/.env`) to create the live-mode
   product/price and webhook endpoint and push the resulting live secrets —
   the script's reuse-by-`lookup_key`/reuse-by-`url` idempotency means this
   is safe to run again; it will not touch or duplicate the test-mode
   objects, which live in Stripe's separate test-mode data. Since the
   subscriptions milestone, the same re-run also: (a) reuses-or-creates the
   **live monthly price** under the same `socratic-tutor-monthly`
   lookup_key and pushes it as `STRIPE_PRICE_SOCRATIC_MONTHLY`; (b) creates
   the live webhook endpoint subscribed to **both**
   `checkout.session.completed` and `invoice.paid`, pinned to the same
   `api_version` the script hard-codes for test mode (`2024-06-20` at time
   of writing — confirm this still matches the bootstrap script before
   re-running, since `api_version` is creation-only and a mismatch forces a
   delete-and-recreate that rotates the signing secret again); (c)
   reuses-or-creates a **live-mode billing portal configuration** as the
   account default (`is_default=true`) — portal configurations are
   mode-scoped, so the test-mode default created earlier does not carry
   over. The portal step is designed to degrade to a stderr warning rather
   than abort the script on failure; if it warns, save a configuration once
   by hand in the live Stripe Dashboard before any live subscriber clicks
   "Manage billing" — without a live default configuration,
   `customer-portal`'s session-create call fails and the function returns
   its generic Stripe-failure response (502 `payment_provider_unavailable`).
5. Redeploy `create-checkout` and `stripe-webhook` (step 3 above changes
   their code) via `tools/deploy-function.sh`. `customer-portal` needs no
   redeploy for the flip — it has no test/live branching of its own, so it
   picks up live-mode behavior automatically once `STRIPE_SECRET_KEY` is
   rotated in step 2.
6. **Purge test-mode `stripe_customers` rows before the first live sale.**
   Every row written before the flip holds a *test-mode* `cus_…` id, which
   does not exist in live mode. Because `create-checkout` now passes the
   mapped id as `customer`, a stale row makes Stripe reject the session
   (`resource_missing`) and the user is blocked from subscribing with a 502
   `payment_provider_unavailable` — and stays blocked, since no
   `invoice.paid` can fire to correct the mapping. Truncate the table (and
   any test entitlement rows being retired) as part of the flip:
   `delete from public.stripe_customers;` via the SQL editor.
7. Verify customer reuse behavior in live mode: run the re-subscribe flow
   with a live-mode card and confirm it lands on the existing Customer (no
   new one minted) and that the Billing Portal shows the full subscription
   history for that customer.

**No app release is needed for this flip.** Nothing in the Tauri client
(`src-tauri`, `src/app.js`) encodes test-vs-live mode, a secret value, or a
Price id — the client only ever calls `start_checkout`/`create-checkout`
by `model_id` and opens whatever URL comes back. The entire flip is
server-side (Supabase function secrets + code) and Stripe-Dashboard-side
(business name, webhook endpoint); an already-installed app keeps working
unmodified once the steps above land.

## Return pages (GitHub Pages, pay.cleophis.com)

The Stripe Checkout success/cancel redirect targets are **static HTML on
the `gh-pages` branch**, not a Supabase Edge Function, served on the
custom domain (Cloudflare milestone — DNS-only CNAME `pay` →
`shojuro.github.io`, `CNAME` file in the branch, HTTPS enforced):
- `https://pay.cleophis.com/pay/success.html`
- `https://pay.cleophis.com/pay/cancelled.html`
- `https://pay.cleophis.com/pay/portal-return.html`

The pre-cutover `https://shojuro.github.io/cleophis/pay/…` URLs
permanently redirect to the custom domain, so Stripe sessions and portal
configurations minted before the cutover still land correctly.

**Why not Supabase:** the return pages originally lived as a same-repo
Edge Function (`checkout-return`, now deleted). Supabase's shared
`*.supabase.co` domain unconditionally rewrites any `GET` response with
`Content-Type: text/html` to `text/plain` (+ `X-Content-Type-Options:
nosniff`) — a documented platform-level anti-phishing behavior, confirmed
against Supabase's own docs and independently reproduced live even after
the function's code was verified correct. It affects every function on the
default domain, not just this one; the only fixes are a custom domain
(Supabase Pro) or moving the page off-platform. Static hosting was chosen
as the simpler option since these pages need no server logic at all (no
secrets, no auth, no entitlement writes — the webhook grants the
entitlement out-of-band regardless of whether the user's browser ever
reaches the return page).

`create-checkout`'s `success_url`/`cancel_url` point directly at the two
URLs above; `SUPABASE_URL` is no longer used for that purpose (it's still
used to construct the Supabase client for auth).

## Test tooling

- **`tools/test-stripe-webhook.mjs <user_id> <model_id> [--tamper|--wrong-amount|--replay]`**
  — dependency-free (Node builtins only: `node:crypto` for HMAC, global
  `fetch`). Builds a synthetic `checkout.session.completed` event, signs it
  the way Stripe would (`t=<unix-seconds>,v1=<hmac-sha256 hex>` over
  `${t}.${body}`), and POSTs straight to the deployed `stripe-webhook` — no
  real Stripe account or Checkout Session involved.
  - *(default)* — expect **200**; valid signature, valid amount.
  - `--tamper` — flips one hex nibble of the signature after signing;
    expect **400** (`invalid_signature`).
  - `--wrong-amount` — `amount_total: 500` instead of `2000`; expect **400**
    (`validation_failed`).
  - `--replay` — the *same* signed body POSTed twice (true redelivery, not
    two different sessions); expect **200 both times**, exercising the
    idempotent UPSERT.
  - Reads `STRIPE_WEBHOOK_SECRET` from `~/.env` (required). When
    `SUPABASE_ACCESS_TOKEN` is also set and the mode is default/`--replay`,
    it additionally queries the resulting `entitlements` row via the
    Management API and asserts `source='purchase'` — that check is folded
    into the process exit code. If that optional network call itself fails
    (observed live: Node's `fetch`/undici to `api.supabase.com` can be
    edge-blocked in some environments even though `curl` to the same host
    and the webhook delivery itself succeed), the script degrades to
    printing the SQL you'd run manually and leaves the exit code decided by
    the delivery-status checks alone.

- **`tools/stripe-bootstrap.sh`** — idempotent Stripe + Supabase
  environment setup, safe to re-run: (1) reuse-or-create the Price by
  `lookup_key`; (2) reuse-or-create the webhook endpoint by `url` (deletes
  and recreates if the endpoint exists but no signing secret is stored
  locally, since Stripe only returns it at creation); (3) pushes
  `STRIPE_SECRET_KEY`/`STRIPE_WEBHOOK_SECRET`/`STRIPE_PRICE_SOCRATIC` to the
  Supabase project as function secrets. All Stripe calls use HTTP Basic
  auth (`curl -u "$STRIPE_SECRET_KEY:"`) so the key never appears on a URL
  or in anything `curl` would echo; secret values are never printed, only
  names, ids, and HTTP status codes.

- **`tools/deploy-function.sh <slug> [--no-verify-jwt]`** — generic
  Management-API deploy for any `supabase/functions/<slug>/index.ts`
  (generalizes the house `deploy-download-url.sh` template's `deploy`
  step). Used for all three Stripe functions; `--no-verify-jwt` is required
  for `stripe-webhook` (Stripe sends no Supabase JWT).

## Secrets inventory

Names and locations only — no values below.

| Secret | Lives in | Read by |
|---|---|---|
| `STRIPE_SECRET_KEY` | `~/.env` (local) **and** Supabase function secrets | `tools/stripe-bootstrap.sh`, `create-checkout`, `stripe-webhook` |
| `STRIPE_WEBHOOK_SECRET` | `~/.env` (appended by bootstrap) **and** Supabase function secrets | `tools/test-stripe-webhook.mjs`, `stripe-webhook` |
| `STRIPE_PRICE_SOCRATIC` | Supabase function secrets only (resolved value, not persisted to `~/.env`) | `create-checkout` |
| `SUPABASE_ACCESS_TOKEN` | `~/.env` (local) | `tools/stripe-bootstrap.sh`, `tools/deploy-function.sh`, `tools/test-stripe-webhook.mjs` (optional, for the entitlements-row check) — the Management API credential, distinct from any Stripe secret |
| `SUPABASE_URL`, `SUPABASE_SERVICE_ROLE_KEY` | Supabase-provided function env (automatic, not set by any tool in this repo) | `create-checkout`, `stripe-webhook`, `download-url` |
| `B2_KEY_ID`, `B2_APP_KEY`, `B2_BUCKET_ID`, `B2_BUCKET_NAME`, `B2_DOWNLOAD_BASE_URL` (optional) | Supabase function secrets, set via `tools/deploy-download-url.sh secrets_set` | `download-url` — see `docs/ops/model-delivery-runbook.md` |
