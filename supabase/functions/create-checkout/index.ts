// supabase/functions/create-checkout/index.ts
//
// Creates a Stripe Checkout Session for a one-time model purchase. Called by
// the Rust client's checkout entry point (src-tauri/src/cloud/rest.rs, S7) —
// POST { model_id } with a Supabase user JWT in the Authorization header.
// Returns the Stripe-hosted checkout URL for the client to open (opener
// plugin, S7). The user lands back on checkout-return/index.ts (this repo,
// verify_jwt: OFF) after paying or cancelling; the actual entitlement grant
// happens out-of-band via stripe-webhook (S4), not on this request.
//
// Task S3: code + commit only, no deployment. Deployment (verify_jwt: ON)
// plus secrets (STRIPE_SECRET_KEY, STRIPE_PRICE_SOCRATIC) happen in S6.
//
// Import: pinned to jsr:@supabase/supabase-js@2, matching download-url.
import { createClient } from "jsr:@supabase/supabase-js@2";
import Stripe from "npm:stripe";

// In-source model→Stripe-price registry. Keep in sync with the catalog on
// the client side (src/app.js / src-tauri) and with download-url's own
// MODELS registry — this function is the source of truth for which Stripe
// Price ID backs a purchase of a given model_id.
const MODELS: Record<string, { priceEnv: string; display: string }> = {
  "socratic-tutor": { priceEnv: "STRIPE_PRICE_SOCRATIC", display: "Socratic Math Tutor" },
};

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

  let payload: unknown;
  try {
    payload = await req.json();
  } catch {
    return jsonResponse({ error: "bad_request" }, 400);
  }
  if (typeof payload !== "object" || payload === null) {
    return jsonResponse({ error: "bad_request" }, 400);
  }
  const modelId = (payload as Record<string, unknown>).model_id;
  if (typeof modelId !== "string" || modelId.length === 0) {
    return jsonResponse({ error: "bad_request" }, 400);
  }

  // Base env check happens right after basic request validation, before any
  // network calls, mirroring download-url — a misconfigured deployment fails
  // fast regardless of which downstream step would have needed the missing
  // var. The model's priceEnv is intentionally NOT in this pass: its name
  // isn't known until the registry fence below resolves modelId to a model.
  const supabaseUrl = readEnv("SUPABASE_URL");
  const serviceRoleKey = readEnv("SUPABASE_SERVICE_ROLE_KEY");
  const stripeSecretKey = readEnv("STRIPE_SECRET_KEY");

  const missing: string[] = [];
  if (!supabaseUrl) missing.push("SUPABASE_URL");
  if (!serviceRoleKey) missing.push("SUPABASE_SERVICE_ROLE_KEY");
  if (!stripeSecretKey) missing.push("STRIPE_SECRET_KEY");
  if (missing.length > 0) {
    // Names only — never values — per brief.
    console.error(`create-checkout misconfigured: missing env ${missing.join(", ")}`);
    return jsonResponse({ error: "misconfigured" }, 500);
  }

  const authHeader = req.headers.get("Authorization") ?? "";
  const jwt = authHeader.startsWith("Bearer ") ? authHeader.slice("Bearer ".length).trim() : "";
  if (!jwt) {
    return jsonResponse({ error: "unauthorized" }, 401);
  }

  const supabase = createClient(supabaseUrl!, serviceRoleKey!);

  let userId: string;
  try {
    const { data: userData, error: userErr } = await supabase.auth.getUser(jwt);
    if (userErr || !userData?.user) {
      return jsonResponse({ error: "unauthorized" }, 401);
    }
    userId = userData.user.id;
  } catch (err) {
    // Never log the jwt itself.
    console.error(`auth.getUser failed: ${String(err)}`);
    return jsonResponse({ error: "unauthorized" }, 401);
  }

  // Object.hasOwn fences the lookup against prototype-chain keys
  // ("__proto__", "constructor", "toString", ...) that would otherwise
  // return a truthy value from MODELS[modelId] and bypass this 404 gate —
  // mirrors download-url's registry fence.
  if (!Object.hasOwn(MODELS, modelId)) {
    return jsonResponse({ error: "unknown_model" }, 404);
  }
  const model = MODELS[modelId];

  // The Stripe Price ID env var is model-specific, so it can only be
  // resolved once the registry fence above has confirmed modelId is known.
  const priceId = readEnv(model.priceEnv);
  if (!priceId) {
    // Name only — never a value — per brief.
    console.error(`create-checkout misconfigured: missing env ${model.priceEnv}`);
    return jsonResponse({ error: "misconfigured" }, 500);
  }

  // Already-purchased check: a completed purchase (source = 'purchase')
  // blocks a second Checkout Session outright, before Stripe is ever
  // called — avoids double-charging and duplicate entitlement rows.
  const { data: entRows, error: entErr } = await supabase
    .from("entitlements")
    .select("id")
    .eq("user_id", userId)
    .eq("model_id", modelId)
    .eq("source", "purchase");

  if (entErr) {
    // Not one of the brief's named error cases (DB/network failure on the
    // entitlement query) — assumption: mirror download-url's treatment, a
    // generic internal error distinct from "misconfigured" (env-only) and
    // "payment_provider_unavailable" (Stripe-specific).
    console.error(`entitlements query failed: ${entErr.message}`);
    return jsonResponse({ error: "internal_error" }, 500);
  }
  if (entRows && entRows.length > 0) {
    return jsonResponse(
      { error: "already_purchased", message: "You already own this model." },
      409,
    );
  }

  // Stripe client constructed lazily here, only once every prior check has
  // passed and it's actually about to be used — mirrors how download-url
  // builds its clients after env validation rather than at module load.
  const stripe = new Stripe(stripeSecretKey!, {
    apiVersion: "2024-06-20",
    httpClient: Stripe.createFetchHttpClient(),
  });

  let session: Stripe.Checkout.Session;
  try {
    session = await stripe.checkout.sessions.create({
      mode: "payment",
      line_items: [{ price: priceId, quantity: 1 }],
      metadata: { user_id: userId, model_id: modelId },
      client_reference_id: userId,
      success_url: `${supabaseUrl}/functions/v1/checkout-return?status=success`,
      cancel_url: `${supabaseUrl}/functions/v1/checkout-return?status=cancel`,
    });
  } catch (err) {
    // Log the error message only — never the Stripe secret key.
    const message = err instanceof Error ? err.message : String(err);
    console.error(`Stripe checkout session creation failed: ${message}`);
    return jsonResponse({ error: "payment_provider_unavailable" }, 502);
  }

  if (!session.url) {
    // Not one of the brief's named cases (the Stripe SDK types session.url
    // as nullable) — assumption: treat a session created without a redirect
    // URL as a provider failure rather than returning a body with url: null.
    console.error("Stripe checkout session created without a url");
    return jsonResponse({ error: "payment_provider_unavailable" }, 502);
  }

  return jsonResponse({ url: session.url }, 200);
});
