// supabase/functions/customer-portal/index.ts
//
// Mints a Stripe billing-portal session for the authenticated user, so they
// can manage or cancel their subscription without emailing support. Called
// by the Rust client's portal entry point (src-tauri/src/cloud/rest.rs,
// Sub7) — POST with no body, a Supabase user JWT in the Authorization
// header. Returns the Stripe-hosted portal URL for the client to open
// (opener plugin), mirroring create-checkout's shape.
//
// stripe_customers (supabase/migrations/0006_subscriptions.sql) is
// populated ONLY by stripe-webhook, on the first invoice.paid delivery for
// a user's subscription (Sub2) — so a user who has never subscribed has no
// row there yet, and 404 no_billing_account is the normal, expected answer
// for them, not an error condition.
//
// The portal's return_url points at a static page on GitHub Pages
// (gh-pages branch, https://pay.cleophis.com/pay/portal-return.html)
// — same reasoning as create-checkout's success/cancel pages (S10):
// Supabase's shared *.supabase.co domain force-serves HTML as text/plain
// (platform anti-phishing behavior), so return pages live outside Supabase
// entirely.
//
// Task Sub6: code + commit only, no deployment. Deployment (verify_jwt: ON,
// same as create-checkout) plus pushing the gh-pages return page happen at
// coordinator/deploy time.
//
// Import: pinned to jsr:@supabase/supabase-js@2 and npm:stripe, matching
// create-checkout.
import { createClient } from "jsr:@supabase/supabase-js@2";
import Stripe from "npm:stripe";

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
  // Env fail-fast happens first, before the method gate: a misconfigured
  // deployment should surface its real problem (500 misconfigured) even for
  // a non-POST probe request, rather than reporting a misleading 405.
  // Mirrors create-checkout's readEnv pattern and error discipline exactly;
  // only the position relative to the method gate differs, per brief.
  const supabaseUrl = readEnv("SUPABASE_URL");
  const anonKey = readEnv("SUPABASE_ANON_KEY");
  const serviceRoleKey = readEnv("SUPABASE_SERVICE_ROLE_KEY");
  const stripeSecretKey = readEnv("STRIPE_SECRET_KEY");

  const missing: string[] = [];
  if (!supabaseUrl) missing.push("SUPABASE_URL");
  if (!anonKey) missing.push("SUPABASE_ANON_KEY");
  if (!serviceRoleKey) missing.push("SUPABASE_SERVICE_ROLE_KEY");
  if (!stripeSecretKey) missing.push("STRIPE_SECRET_KEY");
  if (missing.length > 0) {
    // Names only — never values — per brief.
    console.error(`customer-portal misconfigured: missing env ${missing.join(", ")}`);
    return jsonResponse({ error: "misconfigured" }, 500);
  }

  if (req.method !== "POST") {
    return jsonResponse({ error: "method_not_allowed" }, 405);
  }

  // No request body is read: the portal needs no input beyond the caller's
  // identity carried in the Authorization header. The client mirrors
  // start_checkout's POST shape but sends an empty body.

  const authHeader = req.headers.get("Authorization") ?? "";
  const jwt = authHeader.startsWith("Bearer ") ? authHeader.slice("Bearer ".length).trim() : "";
  if (!jwt) {
    return jsonResponse({ error: "unauthorized" }, 401);
  }

  // Anon-key client, used ONLY to validate the caller's JWT via
  // auth.getUser() — distinct from the service-role client below, which is
  // the one actually allowed to touch stripe_customers (clients have zero
  // grants on that table, by design — see migrations/0006_subscriptions.sql).
  // Extraction, try/catch, and 401 mapping below are otherwise identical to
  // create-checkout's auth flow.
  const supabaseAuth = createClient(supabaseUrl!, anonKey!);

  let userId: string;
  try {
    const { data: userData, error: userErr } = await supabaseAuth.auth.getUser(jwt);
    if (userErr || !userData?.user) {
      return jsonResponse({ error: "unauthorized" }, 401);
    }
    userId = userData.user.id;
  } catch (err) {
    // Never log the jwt itself.
    console.error(`auth.getUser failed: ${String(err)}`);
    return jsonResponse({ error: "unauthorized" }, 401);
  }

  // Service-role client: the only client type with grants on
  // stripe_customers. A 404 here is the expected, common case for anyone
  // who has never completed a subscription checkout — see header comment.
  const supabaseService = createClient(supabaseUrl!, serviceRoleKey!);

  const { data: customerRow, error: lookupErr } = await supabaseService
    .from("stripe_customers")
    .select("customer_id")
    .eq("user_id", userId)
    .maybeSingle();

  if (lookupErr) {
    // Log the error message only — never row values.
    console.error(`stripe_customers lookup failed: ${lookupErr.message}`);
    return jsonResponse({ error: "lookup_failed" }, 500);
  }
  if (!customerRow) {
    return jsonResponse({ error: "no_billing_account" }, 404);
  }
  const customerId = customerRow.customer_id;

  // Stripe client constructed lazily, only once every prior check has
  // passed — mirrors create-checkout.
  const stripe = new Stripe(stripeSecretKey!, {
    apiVersion: "2024-06-20",
    httpClient: Stripe.createFetchHttpClient(),
  });

  let session: Stripe.BillingPortal.Session;
  try {
    // No `configuration` param: the bootstrap-created billing portal
    // configuration (tools/stripe-bootstrap.sh) is the account default
    // (verified live), so Stripe applies it automatically. No idempotency
    // key: unlike Checkout Sessions, portal sessions are cheap and
    // short-lived — a double-click just opens two equally-valid portal
    // links rather than risking a double charge.
    session = await stripe.billingPortal.sessions.create({
      customer: customerId,
      return_url: "https://pay.cleophis.com/pay/portal-return.html",
    });
  } catch (err) {
    // Log the error message only — never the Stripe secret key. Same
    // status/error code as create-checkout's Stripe failure path.
    const message = err instanceof Error ? err.message : String(err);
    console.error(`Stripe billing portal session creation failed: ${message}`);
    return jsonResponse({ error: "payment_provider_unavailable" }, 502);
  }

  // Never log session.url — it is a capability URL (bearer-equivalent
  // access to the customer's billing portal).
  return jsonResponse({ url: session.url }, 200);
});
