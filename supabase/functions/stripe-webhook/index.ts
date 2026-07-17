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
import { createClient } from "jsr:@supabase/supabase-js@2";
import Stripe from "npm:stripe";

const cryptoProvider = Stripe.createSubtleCryptoProvider();

// In-source purchase-validation registry. Keep in sync with create-checkout's
// MODELS registry (supabase/functions/create-checkout/index.ts) and with the
// catalog on the client side (src/app.js / src-tauri) — this is the amount
// and currency this webhook will accept as a valid completed purchase for a
// given model_id. A Checkout Session that doesn't match exactly is rejected
// rather than trusted, so a stale/misconfigured Stripe Price can't silently
// grant an entitlement for the wrong price.
const EXPECTED: Record<string, { amount: number; currency: string }> = {
  "socratic-tutor": { amount: 2000, currency: "usd" },
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

  // Ignore-list: this endpoint subscribes to exactly one event type. Every
  // other event Stripe might deliver to this endpoint (including
  // async_payment_succeeded / async_payment_failed) is deliberately
  // unhandled — card payments complete synchronously, so
  // checkout.session.completed with payment_status: 'paid' is already the
  // final state for the payment methods this app accepts. Returning 200
  // here (rather than 400) tells Stripe delivery succeeded, so it won't
  // retry an event this function will never act on.
  if (event.type !== "checkout.session.completed") {
    return jsonResponse({ received: true }, 200);
  }

  const session = event.data.object as Stripe.Checkout.Session;

  // Only a completed, paid, one-time payment counts — anything else
  // (subscription mode, unpaid/no-payment-required sessions) is ignored
  // rather than treated as an error, since Stripe can legitimately deliver
  // checkout.session.completed for sessions this app doesn't sell through.
  if (session.mode !== "payment" || session.payment_status !== "paid") {
    return jsonResponse({ received: true }, 200);
  }

  const rawUserId = session.metadata?.user_id;
  const rawModelId = session.metadata?.model_id;

  // Every check below must pass before an entitlement is ever written.
  // Each branch returns immediately (rather than accumulating a flag) so
  // TypeScript can narrow userId/modelId to `string` afterward without a
  // cast. Logging WHICH check failed is safe: model_id/user_id are ours,
  // and no secret material is involved.
  if (typeof rawUserId !== "string" || !USER_ID_RE.test(rawUserId)) {
    console.error(
      `stripe-webhook validation failed: check=user_id session=${session.id} ` +
        `user_id=${String(rawUserId)} model_id=${String(rawModelId)}`,
    );
    return jsonResponse({ error: "validation_failed" }, 400);
  }
  if (typeof rawModelId !== "string" || !Object.hasOwn(EXPECTED, rawModelId)) {
    console.error(
      `stripe-webhook validation failed: check=model_id session=${session.id} ` +
        `user_id=${rawUserId} model_id=${String(rawModelId)}`,
    );
    return jsonResponse({ error: "validation_failed" }, 400);
  }
  const userId = rawUserId;
  const modelId = rawModelId;
  const expected = EXPECTED[modelId];
  if (session.amount_total !== expected.amount) {
    console.error(
      `stripe-webhook validation failed: check=amount_total session=${session.id} ` +
        `user_id=${userId} model_id=${modelId}`,
    );
    return jsonResponse({ error: "validation_failed" }, 400);
  }
  if (session.currency !== expected.currency) {
    console.error(
      `stripe-webhook validation failed: check=currency session=${session.id} ` +
        `user_id=${userId} model_id=${modelId}`,
    );
    return jsonResponse({ error: "validation_failed" }, 400);
  }

  const supabase = createClient(supabaseUrl!, serviceRoleKey!);

  // UPSERT via service-role client — THE landmine fix. A client-writable
  // 'trial'/'library' row for this (user_id, model_id) must be upgraded to
  // 'purchase' in place, never dropped and recreated: a plain insert would
  // 23505-conflict on the unique(user_id, model_id) constraint, and a
  // delete+insert would race a concurrent download-url entitlement check
  // and briefly show the user as unentitled for a model they just paid
  // for (or already owned via trial/library).
  //
  // created_at is omitted deliberately: on conflict, Postgres leaves the
  // existing column value untouched, so created_at keeps its original
  // "first entitled at" meaning rather than resetting to the purchase
  // time.
  //
  // Replay safety: this UPSERT is convergent — redelivering the same
  // Stripe event re-writes the same (user_id, model_id, source: 'purchase',
  // expires_at: null) values, a no-op in effect. No event-dedup table is
  // needed this milestone.
  const { error: upsertErr } = await supabase
    .from("entitlements")
    .upsert(
      { user_id: userId, model_id: modelId, source: "purchase", expires_at: null },
      { onConflict: "user_id,model_id" },
    );

  if (upsertErr) {
    // DB error → 500 so Stripe retries; retrying an idempotent UPSERT is
    // correct and may heal a transient failure.
    console.error(`entitlements upsert failed: ${upsertErr.message}`);
    return jsonResponse({ error: "storage_failed" }, 500);
  }

  return jsonResponse({ received: true }, 200);
});
