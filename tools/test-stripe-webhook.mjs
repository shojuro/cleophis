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
// Reads STRIPE_WEBHOOK_SECRET from ~/.env (required — the tester can't sign
// anything without it). SUPABASE_ACCESS_TOKEN is optional; when present and
// the mode is the default or --replay, this script also queries the
// resulting entitlements row via the Supabase Management API's database
// query endpoint and prints it, asserting source='purchase'.
//
// Secret values (STRIPE_WEBHOOK_SECRET, SUPABASE_ACCESS_TOKEN) are never
// logged — only variable names, ids, HTTP statuses, and response bodies
// that this script itself generated (which never contain either secret).

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
    'usage: node tools/test-stripe-webhook.mjs <user_id> <model_id> [--tamper|--wrong-amount|--replay]',
  );
  process.exit(1);
}

const argv = process.argv.slice(2);
const positional = argv.filter((a) => !a.startsWith('--'));
const flagArgs = argv.filter((a) => a.startsWith('--'));

const [userId, modelId] = positional;
if (!userId || !modelId) usageAndExit();

const VALID_FLAGS = new Set(['--tamper', '--wrong-amount', '--replay']);
for (const f of flagArgs) {
  if (!VALID_FLAGS.has(f)) {
    console.error(`error: unknown flag ${f}`);
    usageAndExit();
  }
}
if (flagArgs.length > 1) {
  console.error('error: only one of --tamper, --wrong-amount, --replay may be given');
  usageAndExit();
}
// mode: 'default' | 'tamper' | 'wrong-amount' | 'replay'
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

/** Queries the resulting entitlements row via the Management API's
 * database query endpoint and prints it. Only called when
 * SUPABASE_ACCESS_TOKEN is present; never logs the token itself. Returns
 * true if the row confirms source='purchase' (or if the check couldn't be
 * performed as a hard failure — see below), false on a clear assertion
 * failure. */
async function verifyEntitlementRow() {
  const sql =
    `select source, expires_at from public.entitlements ` +
    `where user_id='${sqlLiteral(userId)}' and model_id='${sqlLiteral(modelId)}'`;
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

// --- main --------------------------------------------------------------

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
  const rowOk = await verifyEntitlementRow();
  ok = ok && rowOk;
} else if (mode === 'default' || mode === 'replay') {
  console.log('SUPABASE_ACCESS_TOKEN not set — skipping entitlements row verification');
}

process.exit(ok ? 0 : 1);
