// supabase/functions/stripe-webhook/index.ts
//
// Single writer of `source='purchase'` entitlements. Stripe calls this
// endpoint directly with each event; there is no Supabase JWT to check
// (Stripe cannot send one), so security here is entirely Stripe signature
// verification — this function is deployed with verify_jwt: OFF (S6).
//
// Task S4: code + commit only, no deployment. Deployment (verify_jwt: OFF)
// plus secrets (STRIPE_WEBHOOK_SECRET) happen in S6, alongside webhook
// registration in the Stripe dashboard/CLI.
//
// Import: pinned to jsr:@supabase/supabase-js@2 and npm:stripe, matching
// create-checkout. Deno has no synchronous crypto, so signature
// verification uses Stripe's async SubtleCrypto-based provider instead of
// the Node-only sync path the Stripe SDK uses by default.
import { createClient } from "jsr:@supabase/supabase-js@2.110.7";
import Stripe from "npm:stripe@22.3.2";

const cryptoProvider = Stripe.createSubtleCryptoProvider();

// In-source subscription-validation registry. Keep in sync with
// create-checkout's MODELS registry (supabase/functions/create-checkout/index.ts)
// and the catalog on the client side (src/app.js / src-tauri). Subscription
// entitlements are validated by PRICE ID (stronger than amount; immune to
// proration).
const SUB_MODELS: Record<string, { priceEnv: string }> = {
  "socratic-tutor": { priceEnv: "STRIPE_PRICE_SOCRATIC_MONTHLY" },
};

const USER_ID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

function jsonResponse(body: unknown, status: number): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

/** Returns the env value, or null if unset/empty. Never logs the value. */
function readEnv(name: string): string | null {
  const value = Deno.env.get(name);
  return value && value.length > 0 ? value : null;
}

/** Narrows an unknown value to a plain object record, or null otherwise. */
function asRecord(value: unknown): Record<string, unknown> | null {
  return typeof value === "object" && value !== null ? (value as Record<string, unknown>) : null;
}

/** Like asRecord, but null unless the record has at least one own key. */
function nonEmptyRecord(value: unknown): Record<string, unknown> | null {
  const rec = asRecord(value);
  return rec !== null && Object.keys(rec).length > 0 ? rec : null;
}

// Task Sub2: handles invoice.paid — the recurring-billing counterpart to the
// checkout.session.completed branch below. Stripe delivers invoice.paid once
// per billing period for the life of a subscription; each delivery extends
// the entitlement's expires_at via the RPC rather than re-granting it from
// scratch, which makes replays and out-of-order delivery harmless.
async function handleInvoicePaid(
  event: Stripe.Event,
  supabaseUrl: string,
  serviceRoleKey: string,
): Promise<Response> {
  const invoice = asRecord(event.data.object) ?? {};

  // Subscription id extraction is defensive: newer Stripe API versions moved
  // this field from invoice.subscription to
  // invoice.parent.subscription_details.subscription. The webhook endpoint's
  // configured API version (Stripe dashboard/CLI setting, independent of
  // this file's pinned SDK apiVersion) decides which shape actually arrives,
  // so both are checked. Absent/non-string means this invoice isn't tied to
  // a subscription at all (e.g. a one-off invoice) — not an error, just not
  // ours to act on.
  let subscriptionId: string | null = null;
  if (typeof invoice.subscription === "string") {
    subscriptionId = invoice.subscription;
  } else {
    const parent = asRecord(invoice.parent);
    const parentSubDetails = asRecord(parent?.subscription_details);
    const nested = parentSubDetails?.subscription;
    subscriptionId = typeof nested === "string" ? nested : null;
  }
  if (subscriptionId === null) {
    console.error("invoice without subscription — ignored");
    return jsonResponse({ received: true }, 200);
  }

  // Metadata extraction is likewise defensive across API-version shapes, and
  // additionally falls back to the first line item's metadata (Stripe copies
  // subscription metadata onto invoice line items). First non-empty source
  // wins. Missing entirely, or missing the user_id/model_id keys, means this
  // is not one of our subscriptions — 200-ignore rather than 400, since a
  // 400 would make Stripe retry forever for an invoice we'll never claim.
  const linesRecord = asRecord(invoice.lines);
  const lineItems = Array.isArray(linesRecord?.data) ? (linesRecord!.data as unknown[]) : [];
  const directSubDetails = asRecord(invoice.subscription_details);
  const parentSubDetailsForMeta = asRecord(asRecord(invoice.parent)?.subscription_details);
  const firstLine = asRecord(lineItems[0]);

  const metadata =
    nonEmptyRecord(directSubDetails?.metadata) ??
    nonEmptyRecord(parentSubDetailsForMeta?.metadata) ??
    nonEmptyRecord(firstLine?.metadata);

  if (
    metadata === null ||
    !Object.hasOwn(metadata, "user_id") ||
    !Object.hasOwn(metadata, "model_id")
  ) {
    console.error(`invoice metadata missing/incomplete — ignored: subscription=${subscriptionId}`);
    return jsonResponse({ received: true }, 200);
  }
  const rawUserId = metadata.user_id;
  const rawModelId = metadata.model_id;

  // Every check below must pass before an entitlement period is ever
  // extended. Mirrors the checkout.session.completed validation below:
  // early-return per check (so TS narrows userId/modelId to `string`
  // afterward), and logging WHICH check failed is safe — subscription id,
  // user_id, model_id are ours, no secret material involved.
  if (typeof rawUserId !== "string" || !USER_ID_RE.test(rawUserId)) {
    console.error(
      `stripe-webhook invoice.paid validation failed: check=user_id subscription=${subscriptionId}`,
    );
    return jsonResponse({ error: "validation_failed" }, 400);
  }
  if (typeof rawModelId !== "string" || !Object.hasOwn(SUB_MODELS, rawModelId)) {
    console.error(
      `stripe-webhook invoice.paid validation failed: check=model_id subscription=${subscriptionId} ` +
        `user_id=${rawUserId}`,
    );
    return jsonResponse({ error: "validation_failed" }, 400);
  }
  const userId = rawUserId;
  const modelId = rawModelId;

  if (invoice.paid !== true) {
    console.error(
      `stripe-webhook invoice.paid validation failed: check=paid subscription=${subscriptionId} ` +
        `user_id=${userId} model_id=${modelId}`,
    );
    return jsonResponse({ error: "validation_failed" }, 400);
  }
  if (invoice.currency !== "usd") {
    console.error(
      `stripe-webhook invoice.paid validation failed: check=currency subscription=${subscriptionId} ` +
        `user_id=${userId} model_id=${modelId}`,
    );
    return jsonResponse({ error: "validation_failed" }, 400);
  }

  // Price check: at least one line item's price must match the model's
  // configured monthly price. The price env var is resolved only now that
  // model_id is known — mirrors how create-checkout resolves its per-model
  // priceEnv only after its registry fence. Missing env is a deploy
  // misconfiguration (500, name only), not a bad event (400).
  const priceEnvName = SUB_MODELS[modelId].priceEnv;
  const expectedPriceId = readEnv(priceEnvName);
  if (!expectedPriceId) {
    console.error(`stripe-webhook misconfigured: missing env ${priceEnvName}`);
    return jsonResponse({ error: "misconfigured" }, 500);
  }

  // Single pass over line items resolves both the price check and periodEnd:
  // matched-price periods are collected preferentially, falling back to
  // every line's period only if none of the matched line(s) carry a usable
  // period — "entries where the price matched if easy, else all lines" per
  // brief.
  let priceMatched = false;
  const matchedPeriods: number[] = [];
  const allPeriods: number[] = [];
  for (const line of lineItems) {
    const lineRec = asRecord(line);
    if (!lineRec) continue;

    const priceRec = asRecord(lineRec.price);
    const directPriceId = typeof priceRec?.id === "string" ? priceRec.id : undefined;
    const pricingRec = asRecord(lineRec.pricing);
    const priceDetailsRec = asRecord(pricingRec?.price_details);
    const pricingPriceId =
      typeof priceDetailsRec?.price === "string" ? priceDetailsRec.price : undefined;
    const linePriceId = directPriceId ?? pricingPriceId;
    const isMatch = linePriceId === expectedPriceId;
    if (isMatch) priceMatched = true;

    const periodRec = asRecord(lineRec.period);
    const periodEndRaw = periodRec?.end;
    if (typeof periodEndRaw === "number" && Number.isFinite(periodEndRaw)) {
      allPeriods.push(periodEndRaw);
      if (isMatch) matchedPeriods.push(periodEndRaw);
    }
  }
  if (!priceMatched) {
    console.error(
      `stripe-webhook invoice.paid validation failed: check=price subscription=${subscriptionId} ` +
        `user_id=${userId} model_id=${modelId}`,
    );
    return jsonResponse({ error: "validation_failed" }, 400);
  }

  const periodCandidates = matchedPeriods.length > 0 ? matchedPeriods : allPeriods;
  const periodEnd = periodCandidates.length > 0 ? Math.max(...periodCandidates) : null;
  // ~400 days — longest plausible billing period + slack; an inflated
  // period can never be walked back by the extend-only writer, and this
  // also prevents a Date RangeError on absurd values.
  const periodEndMax = Math.floor(Date.now() / 1000) + 400 * 86400;
  if (periodEnd === null || !(periodEnd > 0) || periodEnd > periodEndMax) {
    console.error(
      `stripe-webhook invoice.paid validation failed: check=period_end subscription=${subscriptionId} ` +
        `user_id=${userId} model_id=${modelId}`,
    );
    return jsonResponse({ error: "validation_failed" }, 400);
  }
  const periodEndIso = new Date(periodEnd * 1000).toISOString();

  const supabase = createClient(supabaseUrl, serviceRoleKey);

  // Entitlement write via the RPC, NOT a direct upsert — the RPC owns expiry
  // semantics (extend-only, grandfathered-lifetime-preserving; see
  // apply_subscription_period in supabase/migrations/0006_subscriptions.sql)
  // and this function must not duplicate that logic. Replay-safe: the RPC is
  // convergent/monotonic, so retrying the same or an out-of-order period is
  // harmless.
  const { error: rpcErr } = await supabase.rpc("apply_subscription_period", {
    p_user_id: userId,
    p_model_id: modelId,
    p_period_end: periodEndIso,
  });
  if (rpcErr) {
    // DB error → 500 so Stripe retries; the RPC is convergent/monotonic so
    // retry is safe.
    console.error(`apply_subscription_period rpc failed: ${rpcErr.message}`);
    return jsonResponse({ error: "storage_failed" }, 500);
  }

  // Customer mapping: AFTER the entitlement write, and best-effort — the
  // entitlement write above is the critical operation; a failure here
  // (including a cross-user customer_id unique clash) must not fail the
  // event or make Stripe retry an entitlement grant that already succeeded.
  if (typeof invoice.customer === "string") {
    const { error: mapErr } = await supabase
      .from("stripe_customers")
      .upsert({ user_id: userId, customer_id: invoice.customer }, { onConflict: "user_id" });
    if (mapErr) {
      console.error(`stripe_customers upsert failed: ${mapErr.message}`);
    }
  }

  return jsonResponse({ received: true }, 200);
}

Deno.serve(async (req: Request) => {
  if (req.method !== "POST") {
    return jsonResponse({ error: "method_not_allowed" }, 405);
  }

  // Base env check happens right after the method check, before the raw
  // body is even read — mirrors create-checkout/download-url: a
  // misconfigured deployment fails fast and deterministically. Names only,
  // never values, per brief.
  const supabaseUrl = readEnv("SUPABASE_URL");
  const serviceRoleKey = readEnv("SUPABASE_SERVICE_ROLE_KEY");
  const stripeSecretKey = readEnv("STRIPE_SECRET_KEY");
  const webhookSecret = readEnv("STRIPE_WEBHOOK_SECRET");

  const missing: string[] = [];
  if (!supabaseUrl) missing.push("SUPABASE_URL");
  if (!serviceRoleKey) missing.push("SUPABASE_SERVICE_ROLE_KEY");
  if (!stripeSecretKey) missing.push("STRIPE_SECRET_KEY");
  if (!webhookSecret) missing.push("STRIPE_WEBHOOK_SECRET");
  if (missing.length > 0) {
    console.error(`stripe-webhook misconfigured: missing env ${missing.join(", ")}`);
    return jsonResponse({ error: "misconfigured" }, 500);
  }

  // RAW body first — never parse before signature verification. Stripe
  // signs the exact bytes it sent; re-serializing a parsed body (or
  // reading req.json()) would produce different bytes and break every
  // signature.
  const body = await req.text();

  const sig = req.headers.get("stripe-signature");
  if (!sig) {
    return jsonResponse({ error: "bad_request" }, 400);
  }

  // Stripe client constructed lazily, only once env validation has passed
  // — mirrors create-checkout.
  const stripe = new Stripe(stripeSecretKey!, {
    apiVersion: "2024-06-20",
    httpClient: Stripe.createFetchHttpClient(),
  });

  let event: Stripe.Event;
  try {
    event = await stripe.webhooks.constructEventAsync(
      body,
      sig,
      webhookSecret!,
      undefined,
      cryptoProvider,
    );
  } catch (err) {
    // Log the error message only — NEVER the signature, the webhook
    // secret, or the raw body.
    const message = err instanceof Error ? err.message : String(err);
    console.error(`Stripe signature verification failed: ${message}`);
    return jsonResponse({ error: "invalid_signature" }, 400);
  }

  // Test-deployment guard: this webhook is bootstrapped against Stripe test
  // mode first (S6); reject any livemode event so a premature dashboard
  // misconfiguration can't grant a real-money purchase an entitlement
  // before the live cutover. Remove this block (and its comment) at the
  // live flip — see S10.
  if (event.livemode === true) {
    console.error("livemode event rejected (test deployment)");
    return jsonResponse({ error: "invalid_signature" }, 400);
  }

  // Ignore-list: this endpoint acts on exactly one event type —
  // invoice.paid (subscription period grants/renewals, Task Sub2).
  if (event.type === "invoice.paid") {
    return await handleInvoicePaid(event, supabaseUrl!, serviceRoleKey!);
  }

  // Every other event type is acknowledged and ignored. checkout.session.completed
  // is still emitted (subscription checkouts fire it), but the subscription grant
  // is handled ENTIRELY by invoice.paid — so there is nothing to do here. The
  // former one-time-payment grant branch was removed in Phase-1 hardening:
  // checkout is subscription-only, and that branch granted a lifetime entitlement
  // on amount-only validation with no price-ID check (a latent over-grant if
  // payment mode ever returned). Existing grandfathered lifetime rows are
  // untouched — they live in the DB; this only removes the code that MINTED them.
  return jsonResponse({ received: true }, 200);
});
