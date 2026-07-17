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

## LIVE-MODE FLIP checklist

Everything above and everything currently deployed runs against **Stripe
test mode**. Before real money can move, in order:

1. **Set the account's public business name in the Stripe Dashboard.**
   Stripe's API refuses to set this programmatically in test mode — it's a
   one-time manual step in the Dashboard UI, and Checkout will not
   represent the business correctly to real customers until it's done.
2. **Rotate three secrets to their live-mode values**: `STRIPE_SECRET_KEY`,
   `STRIPE_WEBHOOK_SECRET`, `STRIPE_PRICE_SOCRATIC`. Live and test Stripe
   objects are entirely separate — a live secret key, a live webhook
   endpoint (with its own live signing secret), and a live Price id (even
   if it represents the "same" $20 product) are all needed. `~/.env` ends
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
   objects, which live in Stripe's separate test-mode data.
5. Redeploy `create-checkout` and `stripe-webhook` (step 3 above changes
   their code) via `tools/deploy-function.sh`.

**No app release is needed for this flip.** Nothing in the Tauri client
(`src-tauri`, `src/app.js`) encodes test-vs-live mode, a secret value, or a
Price id — the client only ever calls `start_checkout`/`create-checkout`
by `model_id` and opens whatever URL comes back. The entire flip is
server-side (Supabase function secrets + code) and Stripe-Dashboard-side
(business name, webhook endpoint); an already-installed app keeps working
unmodified once the five steps above land.

## Return pages (GitHub Pages)

The Stripe Checkout success/cancel redirect targets are **static HTML on
the `gh-pages` branch**, not a Supabase Edge Function:
- `https://shojuro.github.io/cleophis/pay/success.html`
- `https://shojuro.github.io/cleophis/pay/cancelled.html`

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
