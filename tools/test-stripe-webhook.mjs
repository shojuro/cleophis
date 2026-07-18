#!/usr/bin/env node
// tools/test-stripe-webhook.mjs — signed synthetic Stripe webhook tester.
// Builds a checkout.session.completed event, signs it the way Stripe would
// (t=<unix seconds>, HMAC-SHA256 of "${t}.${body}" with the webhook signing
// secret), and POSTs it straight to the deployed stripe-webhook function —
// no real Stripe account/checkout involved. Dependency-free: only Node
// builtins (node:crypto for HMAC, global fetch for HTTP).
//
// Task S5: written and reviewed now (code-only stage); NOT executed in
// this task — real runs happen in S6 against the deployed function.
//
// Usage:
//   node tools/test-stripe-webhook.mjs <user_id> <model_id> [--tamper|--wrong-amount|--replay]
//
// Modes (mutually exclusive; default = plain valid delivery):
//   (none)          expect HTTP 200 — valid signature, valid amount.
//   --tamper        expect HTTP 400 — signature flipped after signing, so
//                   verification must fail (never touches the secret).
//   --wrong-amount  expect HTTP 400 — amount_total set to 500 instead of
//                   2000, so the webhook's amount check must reject it.
//   --replay        expect HTTP 200 twice — the SAME signed event body is
//                   POSTed twice, exercising the webhook's idempotent
//                   UPSERT (see supabase/functions/stripe-webhook/index.ts).
//
// Task Sub3 adds six --sub* modes, all building synthetic invoice.paid
// events (the recurring-billing counterpart to checkout.session.completed
// above) against handleInvoicePaid (Task Sub2):
//   --sub                   expect HTTP 200 — periodEnd = now+30d; verifies
//                           source=purchase and expires_at ~ +30d.
//   --sub-replay            expect HTTP 200 twice — ONE signed invoice.paid
//                           event (periodEnd +30d) POSTed twice; verifies
//                           expires_at is unchanged between deliveries.
//   --sub-out-of-order      expect HTTP 200 twice — two invoice.paid events,
//                           periodEnd +60d then +30d; verifies the final
//                           expires_at stays ~+60d (GREATEST/extend-only
//                           proof).
//   --sub-lifetime-protect  expect HTTP 200 — invoice.paid periodEnd +30d
//                           against a user who must ALREADY hold a lifetime
//                           (source=purchase, expires_at IS NULL) row;
//                           verifies expires_at IS STILL NULL. Precondition:
//                           the coordinator arranges the lifetime row first.
//   --sub-wrong-price       expect HTTP 400 — invoice.paid with a price id
//                           that doesn't match the configured monthly price.
//   --sub-no-metadata       expect HTTP 200 (ignore path) — invoice.paid
//                           with no subscription_details.metadata; verifies
//                           NO entitlements row is created (meaningful only
//                           for a fresh user_id/model_id pair).
//
// Reads STRIPE_WEBHOOK_SECRET from ~/.env (required — the tester can't sign
// anything without it). SUPABASE_ACCESS_TOKEN is optional; when present and
// the mode is the default or --replay, this script also queries the
// resulting entitlements row via the Supabase Management API's database
// query endpoint and prints it, asserting source='purchase'.
//
// S6 finding: node's fetch (undici) to api.supabase.com can be edge-blocked
// in some network environments where `curl` to the same host succeeds —
// observed live during S6. That only affects this optional row-verification
// step (the webhook deliveries themselves go to
// isltexsxpysxqewjsryv.supabase.co, a different host, and were unaffected).
// The verification call is therefore wrapped in try/catch: on ANY failure —
// network-unreachable or otherwise — this script prints the SQL it would
// have run and leaves the process exit code untouched, since delivery
// expectations alone decide pass/fail. A row that WAS reached but has the
// wrong contents is a different case and still fails the run (see
// verifyEntitlementRow's return value).
//
// Secret values (STRIPE_WEBHOOK_SECRET, SUPABASE_ACCESS_TOKEN) are never
// logged — only variable names, ids, HTTP statuses, and response bodies
// that this script itself generated (which never contain either secret).
//
// Task Sub3: the --sub* modes additionally require STRIPE_SECRET_KEY (also
// from ~/.env, also never logged) to resolve the monthly subscription Price
// id from Stripe — except --sub-wrong-price, which never resolves a real
// price. See the "Sub3: synthetic invoice.paid tester modes" section below
// for the shared event builder, price-id resolution, and per-mode dispatch.

import { readFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { createHmac } from 'node:crypto';

const PROJECT_REF = 'isltexsxpysxqewjsryv';
const WEBHOOK_URL = `https://${PROJECT_REF}.supabase.co/functions/v1/stripe-webhook`;
const MANAGEMENT_API_BASE = 'https://api.supabase.com';

// --- minimal ~/.env reader ------------------------------------------------
// Not a full shell interpreter — just enough to read simple KEY=VALUE (or
// export KEY=VALUE) lines, matching how the .sh tools in this repo `source`
// the same file. Comments and blank lines are skipped; a single layer of
// matching quotes is stripped.
function parseEnvFile(text) {
  const vars = {};
  for (const rawLine of text.split(/\r?\n/)) {
    let line = rawLine.trim();
    if (!line || line.startsWith('#')) continue;
    if (line.startsWith('export ')) line = line.slice('export '.length).trim();
    const eq = line.indexOf('=');
    if (eq === -1) continue;
    const key = line.slice(0, eq).trim();
    let value = line.slice(eq + 1).trim();
    if (
      (value.startsWith('"') && value.endsWith('"') && value.length >= 2) ||
      (value.startsWith("'") && value.endsWith("'") && value.length >= 2)
    ) {
      value = value.slice(1, -1);
    }
    vars[key] = value;
  }
  return vars;
}

function loadEnvFile() {
  const envPath = join(homedir(), '.env');
  let text;
  try {
    text = readFileSync(envPath, 'utf8');
  } catch {
    return {};
  }
  return parseEnvFile(text);
}

const envVars = loadEnvFile();

function requireEnv(name) {
  const value = envVars[name] ?? process.env[name];
  if (!value) {
    console.error(`error: ${name} is not set (expected in ~/.env)`);
    process.exit(1);
  }
  return value;
}

function optionalEnv(name) {
  const value = envVars[name] ?? process.env[name];
  return value && value.length > 0 ? value : null;
}

// --- argv parsing ----------------------------------------------------------

function usageAndExit() {
  console.error(
    'usage: node tools/test-stripe-webhook.mjs <user_id> <model_id> ' +
      '[--tamper|--wrong-amount|--replay|--sub|--sub-replay|--sub-out-of-order|' +
      '--sub-lifetime-protect|--sub-wrong-price|--sub-no-metadata]',
  );
  process.exit(1);
}

const argv = process.argv.slice(2);
const positional = argv.filter((a) => !a.startsWith('--'));
const flagArgs = argv.filter((a) => a.startsWith('--'));

const [userId, modelId] = positional;
if (!userId || !modelId) usageAndExit();

// Task Sub3: six --sub* flags added alongside the original three. The "at
// most one flag" contract (flagArgs.length > 1 check below) is unchanged —
// it just no longer names only the original three in its message.
const VALID_FLAGS = new Set([
  '--tamper',
  '--wrong-amount',
  '--replay',
  '--sub',
  '--sub-replay',
  '--sub-out-of-order',
  '--sub-lifetime-protect',
  '--sub-wrong-price',
  '--sub-no-metadata',
]);
for (const f of flagArgs) {
  if (!VALID_FLAGS.has(f)) {
    console.error(`error: unknown flag ${f}`);
    usageAndExit();
  }
}
if (flagArgs.length > 1) {
  console.error('error: only one mode flag may be given');
  usageAndExit();
}
// mode: 'default' | 'tamper' | 'wrong-amount' | 'replay' | 'sub' |
// 'sub-replay' | 'sub-out-of-order' | 'sub-lifetime-protect' |
// 'sub-wrong-price' | 'sub-no-metadata'
const mode = flagArgs.length === 1 ? flagArgs[0].slice(2) : 'default';

const webhookSecret = requireEnv('STRIPE_WEBHOOK_SECRET');
const supabaseAccessToken = optionalEnv('SUPABASE_ACCESS_TOKEN');

const EXPECTED_STATUS = { default: 200, tamper: 400, 'wrong-amount': 400, replay: 200 };

// --- event construction -----------------------------------------------------

// process.hrtime.bigint(): nanosecond-resolution monotonic counter, used
// only to make test ids unique per run — not a security token.
function nanos() {
  return process.hrtime.bigint().toString();
}

function buildEvent() {
  const amountTotal = mode === 'wrong-amount' ? 500 : 2000;
  const idSuffix = nanos();
  return {
    id: `evt_test_${idSuffix}`,
    type: 'checkout.session.completed',
    livemode: false,
    data: {
      object: {
        id: `cs_test_${idSuffix}`,
        mode: 'payment',
        payment_status: 'paid',
        amount_total: amountTotal,
        currency: 'usd',
        metadata: { user_id: userId, model_id: modelId },
      },
    },
  };
}

/** HMAC-SHA256(secret, `${t}.${body}`) hex, per Stripe's signing scheme. */
function signBody(body, secret, t) {
  return createHmac('sha256', secret).update(`${t}.${body}`).digest('hex');
}

/** Flips one hex nibble so the signature no longer verifies. Operates only
 * on the derived signature — never touches the secret itself. */
function tamperSignature(signedHex) {
  const nibble = parseInt(signedHex[0], 16);
  const flipped = (nibble ^ 0x1).toString(16);
  return flipped + signedHex.slice(1);
}

// --- HTTP ------------------------------------------------------------------

async function postEvent(body, header, label) {
  const res = await fetch(WEBHOOK_URL, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'Stripe-Signature': header,
    },
    body,
  });
  const text = await res.text();
  console.log(`${label}: POST ${WEBHOOK_URL} -> HTTP ${res.status}`);
  if (text) console.log(`${label}: body: ${text}`);
  return res.status;
}

function sqlLiteral(s) {
  return String(s).replace(/'/g, "''");
}

/** The SQL SELECT the row-verification step runs (or would run). Factored
 * out so the try/catch around the verification call can print it verbatim
 * as a manual fallback when the fetch itself fails — see call site. */
function entitlementsSql() {
  return (
    `select source, expires_at from public.entitlements ` +
    `where user_id='${sqlLiteral(userId)}' and model_id='${sqlLiteral(modelId)}'`
  );
}

/** Queries the resulting entitlements row via the Management API's
 * database query endpoint and prints it. Only called when
 * SUPABASE_ACCESS_TOKEN is present; never logs the token itself. Returns
 * true if the row confirms source='purchase', false on a clear assertion
 * failure (query reached the API but the row was missing/wrong). Network-
 * level failures (fetch throws) are NOT caught here — they propagate to
 * the caller, which treats "couldn't even reach it" differently from
 * "reached it and the row was wrong". */
async function verifyEntitlementRow() {
  const sql = entitlementsSql();
  const res = await fetch(`${MANAGEMENT_API_BASE}/v1/projects/${PROJECT_REF}/database/query`, {
    method: 'POST',
    headers: {
      Authorization: `Bearer ${supabaseAccessToken}`,
      'Content-Type': 'application/json',
    },
    body: JSON.stringify({ query: sql }),
  });
  console.log(`entitlements query: HTTP ${res.status}`);
  let rows;
  try {
    rows = await res.json();
  } catch {
    rows = null;
  }
  if (!res.ok) {
    console.error(`entitlements query failed: ${JSON.stringify(rows)}`);
    return false;
  }
  console.log(`entitlements row(s): ${JSON.stringify(rows)}`);
  if (Array.isArray(rows) && rows.length === 1 && rows[0].source === 'purchase') {
    console.log('entitlements assertion: source=purchase OK');
    return true;
  }
  console.error('entitlements assertion FAILED: expected exactly one row with source=purchase');
  return false;
}

// --- Sub3: synthetic invoice.paid tester modes ----------------------------
// The --sub* flags below build and sign synthetic invoice.paid events
// against the SAME webhook endpoint, exercising handleInvoicePaid in
// supabase/functions/stripe-webhook/index.ts (Task Sub2) the way the modes
// above exercise the checkout.session.completed branch. Every field
// buildInvoiceEvent sets is a field handleInvoicePaid actually reads —
// verified against index.ts at HEAD: data.object.subscription (subscription
// id extraction), .subscription_details.metadata (user_id/model_id — the
// direct-shape branch of the handler's defensive extraction), .paid,
// .currency, .lines.data[].price.id (price match) + .period.end
// (periodEnd), and .customer (stripe_customers upsert). Nothing else in the
// built object is read by the handler.
//
// periodEnd cap note: index.ts rejects periodEnd beyond ~400 days out
// (periodEndMax = now + 400*86400 — guards against an inflated period the
// extend-only RPC could never walk back, and against a Date RangeError on
// absurd values). Every mode below uses +30d or +60d, both far under that
// cap, so none of these synthetic events can trip it.

const DAY_SECONDS = 86400;
const PRICE_TEST_WRONG = 'price_test_wrong';

function daysFromNow(days) {
  return Math.floor(Date.now() / 1000) + days * DAY_SECONDS;
}

function isoDatePrefix(periodEndSec) {
  return new Date(periodEndSec * 1000).toISOString().slice(0, 10);
}

/** Shared synthetic invoice.paid event builder (Task Sub3 brief). Parameter
 * names deliberately match the outer userId/modelId consts (safe plain
 * shadowing — this is a function parameter, not a closure needing the
 * outer values) so call sites can use object-shorthand `{ userId, modelId,
 * ... }` exactly as the brief's pseudocode shows. */
function buildInvoiceEvent({
  userId,
  modelId,
  priceId,
  periodEndSec,
  paid = true,
  currency = 'usd',
  withMetadata = true,
}) {
  return {
    id: `evt_test_${nanos()}`,
    type: 'invoice.paid',
    livemode: false,
    data: {
      object: {
        id: `in_test_${nanos()}`,
        subscription: `sub_test_${nanos()}`,
        customer: 'cus_test_synthetic',
        paid,
        currency,
        subscription_details: withMetadata ? { metadata: { user_id: userId, model_id: modelId } } : {},
        lines: {
          data: [
            {
              price: { id: priceId },
              period: { start: Math.floor(Date.now() / 1000), end: periodEndSec },
            },
          ],
        },
      },
    },
  };
}

/** Signs and POSTs a freshly-built invoice.paid event; returns the HTTP
 * status, matching postEvent's own contract. Every --sub* mode below
 * otherwise repeats "stringify, sign now, POST" verbatim. */
async function signAndPost(event, label) {
  const body = JSON.stringify(event);
  const t = Math.floor(Date.now() / 1000);
  const header = `t=${t},v1=${signBody(body, webhookSecret, t)}`;
  return postEvent(body, header, label);
}

/** Resolves the socratic-tutor monthly subscription Price id from Stripe
 * (Task Sub3 brief): GET /v1/prices?lookup_keys[]=socratic-tutor-monthly
 * with Basic auth (STRIPE_SECRET_KEY:), returns data[0].id. Hard errors —
 * process.exit(1) — with a message pointing at stripe-bootstrap if the key
 * is absent or no matching Price comes back; --sub-wrong-price is the only
 * mode that never calls this. */
async function resolveMonthlyPriceId() {
  const stripeSecretKey = requireEnv('STRIPE_SECRET_KEY');
  const url = 'https://api.stripe.com/v1/prices?lookup_keys[]=socratic-tutor-monthly&limit=1';
  const auth = Buffer.from(`${stripeSecretKey}:`).toString('base64');
  const res = await fetch(url, { headers: { Authorization: `Basic ${auth}` } });
  let json;
  try {
    json = await res.json();
  } catch {
    json = null;
  }
  const priceId = json && Array.isArray(json.data) && json.data[0] ? json.data[0].id : undefined;
  if (!res.ok || typeof priceId !== 'string') {
    console.error(
      `error: could not resolve the socratic-tutor-monthly price id (HTTP ${res.status}) — ` +
        'run stripe-bootstrap first',
    );
    console.error(`(response: ${JSON.stringify(json)})`);
    process.exit(1);
  }
  return priceId;
}

/** Task Sub3 counterpart to verifyEntitlementRow: same request shape and
 * error-handling contract (throws on a network-level failure, left
 * uncaught here by design — see verifyBestEffort below, which wraps every
 * call site the same way the original default/replay verification block
 * wraps verifyEntitlementRow), but returns the parsed rows array instead of
 * asserting a fixed shape, since each --sub* mode's expires_at expectation
 * differs and a single fixed assertion doesn't fit all of them. */
async function fetchEntitlementRows() {
  const sql = entitlementsSql();
  const res = await fetch(`${MANAGEMENT_API_BASE}/v1/projects/${PROJECT_REF}/database/query`, {
    method: 'POST',
    headers: {
      Authorization: `Bearer ${supabaseAccessToken}`,
      'Content-Type': 'application/json',
    },
    body: JSON.stringify({ query: sql }),
  });
  console.log(`entitlements query: HTTP ${res.status}`);
  let rows;
  try {
    rows = await res.json();
  } catch {
    rows = null;
  }
  if (!res.ok) {
    throw new Error(`entitlements query failed: ${JSON.stringify(rows)}`);
  }
  console.log(`entitlements row(s): ${JSON.stringify(rows)}`);
  return rows;
}

function assertPurchaseExpiry(rows, expectedPeriodEndSec, label) {
  if (!Array.isArray(rows) || rows.length !== 1) {
    console.error(
      `${label} assertion FAILED: expected exactly one entitlements row, got ${JSON.stringify(rows)}`,
    );
    return false;
  }
  const row = rows[0];
  const expectedPrefix = isoDatePrefix(expectedPeriodEndSec);
  const actualPrefix = typeof row.expires_at === 'string' ? row.expires_at.slice(0, 10) : null;
  if (row.source !== 'purchase' || actualPrefix !== expectedPrefix) {
    console.error(
      `${label} assertion FAILED: expected source=purchase expires_at~${expectedPrefix}, ` +
        `got source=${row.source} expires_at=${row.expires_at}`,
    );
    return false;
  }
  console.log(`${label} assertion: source=purchase expires_at~${expectedPrefix} OK`);
  return true;
}

function assertLifetimeProtected(rows, label) {
  if (!Array.isArray(rows) || rows.length !== 1) {
    console.error(
      `${label} assertion FAILED: expected exactly one entitlements row, got ${JSON.stringify(rows)}`,
    );
    return false;
  }
  const row = rows[0];
  if (row.source !== 'purchase' || row.expires_at !== null) {
    console.error(
      `${label} assertion FAILED: expected source=purchase expires_at=null (untouched lifetime ` +
        `row), got source=${row.source} expires_at=${JSON.stringify(row.expires_at)}`,
    );
    return false;
  }
  console.log(`${label} assertion: expires_at IS STILL NULL OK`);
  return true;
}

function assertNoRow(rows, label) {
  if (Array.isArray(rows) && rows.length === 0) {
    console.log(`${label} assertion: no entitlements row OK`);
    return true;
  }
  console.error(`${label} assertion FAILED: expected no row, got ${JSON.stringify(rows)}`);
  return false;
}

function assertExpiresAtUnchanged(before, after, label) {
  if (before === undefined || after === undefined) {
    console.error(
      `${label} assertion FAILED: missing expires_at snapshot (before=${JSON.stringify(before)}, ` +
        `after=${JSON.stringify(after)})`,
    );
    return false;
  }
  if (before !== after) {
    console.error(`${label} assertion FAILED: expires_at changed between deliveries (${before} -> ${after})`);
    return false;
  }
  console.log(`${label} assertion: expires_at unchanged (${after}) OK`);
  return true;
}

/** Runs `check()` under the SAME best-effort contract as the original
 * default/replay verification block below (S6 finding — see file header):
 * only attempted when SUPABASE_ACCESS_TOKEN is present; a network-level
 * failure prints the fallback SQL and is reported as not-attempted rather
 * than failed, so it never flips a passing delivery to FAIL. Returns
 * {attempted, ok}; callers only fold `ok` into their own pass/fail state
 * when `attempted` is true. */
async function verifyBestEffort(check, label) {
  if (!supabaseAccessToken) {
    console.log('SUPABASE_ACCESS_TOKEN not set — skipping entitlements row verification');
    return { attempted: false, ok: false };
  }
  try {
    const ok = await check();
    return { attempted: true, ok };
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    console.log(`row verification unavailable (network)${label ? ` [${label}]` : ''} — verify manually:`);
    console.log(entitlementsSql());
    console.log(`(underlying error: ${message})`);
    return { attempted: false, ok: false };
  }
}

/** Dispatches one of the six --sub* modes; every branch prints
 * expect/delivery/PASS-FAIL like the existing modes above and terminates
 * via process.exit, so nothing below this function's call site in main ever
 * runs for a --sub* invocation. */
async function runSubMode(subMode) {
  const periodEnd30 = daysFromNow(30);
  const periodEnd60 = daysFromNow(60);

  if (subMode === 'sub') {
    const priceId = await resolveMonthlyPriceId();
    const event = buildInvoiceEvent({ userId, modelId, priceId, periodEndSec: periodEnd30 });
    console.log('expect: HTTP 200');
    const status = await signAndPost(event, 'delivery');
    let ok = status === 200;
    console.log(ok ? 'PASS' : 'FAIL');
    const v = await verifyBestEffort(
      async () => assertPurchaseExpiry(await fetchEntitlementRows(), periodEnd30, 'sub'),
      'sub',
    );
    if (v.attempted) ok = ok && v.ok;
    process.exit(ok ? 0 : 1);
  }

  if (subMode === 'sub-replay') {
    const priceId = await resolveMonthlyPriceId();
    const event = buildInvoiceEvent({ userId, modelId, priceId, periodEndSec: periodEnd30 });
    const body = JSON.stringify(event);
    const t = Math.floor(Date.now() / 1000);
    const header = `t=${t},v1=${signBody(body, webhookSecret, t)}`;

    console.log('expect: HTTP 200 (both deliveries)');
    const s1 = await postEvent(body, header, 'delivery 1');

    let expiresAfter1;
    const v1 = await verifyBestEffort(async () => {
      const rows = await fetchEntitlementRows();
      expiresAfter1 = Array.isArray(rows) && rows.length === 1 ? rows[0].expires_at : undefined;
      return true; // snapshot only — nothing to pass/fail yet
    }, 'sub-replay after delivery 1');

    const s2 = await postEvent(body, header, 'delivery 2');
    let ok = s1 === 200 && s2 === 200;
    console.log(ok ? 'PASS' : 'FAIL');

    if (v1.attempted) {
      const v2 = await verifyBestEffort(async () => {
        const rows = await fetchEntitlementRows();
        const expiresAfter2 = Array.isArray(rows) && rows.length === 1 ? rows[0].expires_at : undefined;
        return assertExpiresAtUnchanged(expiresAfter1, expiresAfter2, 'sub-replay');
      }, 'sub-replay after delivery 2');
      if (v2.attempted) ok = ok && v2.ok;
    } else {
      console.log('sub-replay: verification after delivery 2 skipped (no baseline from delivery 1)');
    }
    process.exit(ok ? 0 : 1);
  }

  if (subMode === 'sub-out-of-order') {
    const priceId = await resolveMonthlyPriceId();
    console.log('expect: HTTP 200 (both deliveries)');
    const event60 = buildInvoiceEvent({ userId, modelId, priceId, periodEndSec: periodEnd60 });
    const s1 = await signAndPost(event60, 'delivery 1 (+60d)');
    const event30 = buildInvoiceEvent({ userId, modelId, priceId, periodEndSec: periodEnd30 });
    const s2 = await signAndPost(event30, 'delivery 2 (+30d, out of order)');
    let ok = s1 === 200 && s2 === 200;
    console.log(ok ? 'PASS' : 'FAIL');
    const v = await verifyBestEffort(
      async () =>
        assertPurchaseExpiry(
          await fetchEntitlementRows(),
          periodEnd60,
          'sub-out-of-order (GREATEST proof: final expires_at must stay ~+60d)',
        ),
      'sub-out-of-order',
    );
    if (v.attempted) ok = ok && v.ok;
    process.exit(ok ? 0 : 1);
  }

  if (subMode === 'sub-lifetime-protect') {
    console.log(
      'PRECONDITION: this mode assumes the target user_id/model_id already holds a lifetime ' +
        "row (source='purchase', expires_at IS NULL) BEFORE this run — the coordinator arranges " +
        'that; this script does not create it. If no such row exists yet, the assertion below is ' +
        'meaningless.',
    );
    const priceId = await resolveMonthlyPriceId();
    const event = buildInvoiceEvent({ userId, modelId, priceId, periodEndSec: periodEnd30 });
    console.log('expect: HTTP 200');
    const status = await signAndPost(event, 'delivery');
    let ok = status === 200;
    console.log(ok ? 'PASS' : 'FAIL');
    const v = await verifyBestEffort(
      async () => assertLifetimeProtected(await fetchEntitlementRows(), 'sub-lifetime-protect'),
      'sub-lifetime-protect',
    );
    if (v.attempted) ok = ok && v.ok;
    process.exit(ok ? 0 : 1);
  }

  if (subMode === 'sub-wrong-price') {
    const event = buildInvoiceEvent({
      userId,
      modelId,
      priceId: PRICE_TEST_WRONG,
      periodEndSec: periodEnd30,
    });
    console.log('expect: HTTP 400 (validation_failed)');
    const status = await signAndPost(event, 'delivery');
    const ok = status === 400;
    console.log(ok ? 'PASS' : 'FAIL');
    process.exit(ok ? 0 : 1);
  }

  if (subMode === 'sub-no-metadata') {
    const priceId = await resolveMonthlyPriceId();
    const event = buildInvoiceEvent({
      userId,
      modelId,
      priceId,
      periodEndSec: periodEnd30,
      withMetadata: false,
    });
    console.log('expect: HTTP 200 (ignore path — missing metadata, no entitlement write)');
    const status = await signAndPost(event, 'delivery');
    let ok = status === 200;
    console.log(ok ? 'PASS' : 'FAIL');
    console.log(
      'NOTE: the "no entitlements row" assertion below is only meaningful against a ' +
        'user_id/model_id pair with no prior row — pass a fresh user_id for a clean signal.',
    );
    const v = await verifyBestEffort(
      async () => assertNoRow(await fetchEntitlementRows(), 'sub-no-metadata'),
      'sub-no-metadata',
    );
    if (v.attempted) ok = ok && v.ok;
    process.exit(ok ? 0 : 1);
  }

  throw new Error(`unhandled sub mode: ${subMode}`);
}

// --- main --------------------------------------------------------------

// Task Sub3: --sub* modes take a completely separate path (a different
// event shape entirely — invoice.paid, not checkout.session.completed) and
// each one calls process.exit itself, so nothing below this block ever runs
// for a --sub* invocation.
if (mode.startsWith('sub')) {
  await runSubMode(mode);
}

const event = buildEvent();
const body = JSON.stringify(event);
const t = Math.floor(Date.now() / 1000);
let signedHex = signBody(body, webhookSecret, t);
if (mode === 'tamper') signedHex = tamperSignature(signedHex);
const header = `t=${t},v1=${signedHex}`;

let ok = true;

if (mode === 'replay') {
  console.log('expect: HTTP 200 (both deliveries)');
  const s1 = await postEvent(body, header, 'delivery 1');
  const s2 = await postEvent(body, header, 'delivery 2');
  ok = s1 === EXPECTED_STATUS.replay && s2 === EXPECTED_STATUS.replay;
} else {
  const expected = EXPECTED_STATUS[mode];
  console.log(`expect: HTTP ${expected}`);
  const s = await postEvent(body, header, 'delivery');
  ok = s === expected;
}

console.log(ok ? 'PASS' : 'FAIL');

if (supabaseAccessToken && (mode === 'default' || mode === 'replay')) {
  try {
    const rowOk = await verifyEntitlementRow();
    ok = ok && rowOk;
  } catch (err) {
    // ANY failure here (observed live in S6: node's fetch to
    // api.supabase.com can be edge-blocked while curl to the same host
    // works) is treated as "couldn't check" rather than "checked and it's
    // wrong" — exit code is deliberately left alone; the webhook delivery
    // expectations checked above already determined it.
    const message = err instanceof Error ? err.message : String(err);
    console.log('row verification unavailable (network) — verify manually:');
    console.log(entitlementsSql());
    console.log(`(underlying error: ${message})`);
  }
} else if (mode === 'default' || mode === 'replay') {
  console.log('SUPABASE_ACCESS_TOKEN not set — skipping entitlements row verification');
}

process.exit(ok ? 0 : 1);
