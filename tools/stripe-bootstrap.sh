#!/usr/bin/env bash
# tools/stripe-bootstrap.sh — idempotent Stripe environment setup:
#   1. ensures the one-time Price for the Socratic Math Tutor exists
#   2. ensures a webhook endpoint pointed at stripe-webhook exists, with its
#      signing secret captured in ~/.env (Stripe only returns the signing
#      secret at creation time, never on a later GET)
#   3. pushes STRIPE_SECRET_KEY, STRIPE_WEBHOOK_SECRET, and
#      STRIPE_PRICE_SOCRATIC to the Supabase project as function secrets via
#      the Management API
#
# Task S5: written and reviewed now (code-only stage); NOT executed in this
# task — bootstrap runs in S6.
#
# Usage:
#   tools/stripe-bootstrap.sh
#
# Idempotency: safe to re-run. An existing Price (matched by lookup_key) and
# an existing webhook endpoint (matched by url) are reused rather than
# recreated — except when a webhook endpoint exists but no signing secret is
# stored locally (e.g. ~/.env was lost/rotated out from under it), in which
# case the endpoint is deleted and recreated to obtain a fresh secret.
#
# Reads STRIPE_SECRET_KEY and SUPABASE_ACCESS_TOKEN from ~/.env; fails fast
# if either is missing. All Stripe API calls use HTTP Basic auth with the
# secret key as the username and an empty password (Stripe's convention),
# so the key never appears on a URL or in a header curl would echo.
#
# Secret values (STRIPE_SECRET_KEY, STRIPE_WEBHOOK_SECRET) are never
# echoed — only variable names, ids (product/price/webhook-endpoint ids are
# not secrets), and HTTP status codes.

set -euo pipefail

PROJECT_REF="isltexsxpysxqewjsryv"
SUPABASE_API_BASE="https://api.supabase.com"
STRIPE_API_BASE="https://api.stripe.com"
WEBHOOK_TARGET_URL="https://$PROJECT_REF.supabase.co/functions/v1/stripe-webhook"
PRICE_LOOKUP_KEY="socratic-tutor-onetime"

ENV_FILE="$HOME/.env"

if [[ -f "$ENV_FILE" ]]; then
  set -a
  # shellcheck disable=SC1090
  source "$ENV_FILE"
  set +a
fi

if [[ -z "${STRIPE_SECRET_KEY:-}" ]]; then
  echo "error: STRIPE_SECRET_KEY is not set (expected in ~/.env)" >&2
  exit 1
fi
if [[ -z "${SUPABASE_ACCESS_TOKEN:-}" ]]; then
  echo "error: SUPABASE_ACCESS_TOKEN is not set (expected in ~/.env)" >&2
  exit 1
fi

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

# --- helpers -----------------------------------------------------------

# stripe_request METHOD PATH OUT_FILE [FIELD...]
# Performs a Stripe API call. FIELD entries (e.g. "name=value" or
# "name[]=value") are sent via --data-urlencode: as query params (-G) for
# GET, as the form body otherwise. Writes the response body to OUT_FILE and
# prints the HTTP status code on stdout. Auth is via -u (Basic), so the
# secret key never appears in anything curl would print or that ends up in
# a URL.
stripe_request() {
  local method="$1" path="$2" out_file="$3"
  shift 3
  local curl_args=(-sS -o "$out_file" -w '%{http_code}' -u "$STRIPE_SECRET_KEY:")
  if [[ "$method" == "GET" ]]; then
    curl_args+=(-G)
  elif [[ "$method" != "POST" ]]; then
    curl_args+=(-X "$method")
  fi
  local field
  for field in "$@"; do
    curl_args+=(--data-urlencode "$field")
  done
  curl_args+=("$STRIPE_API_BASE$path")
  curl "${curl_args[@]}"
}

# json_get FILE PYEXPR [NAME=VALUE...]
# Parses FILE as JSON into `d`, evaluates PYEXPR against a namespace of `d`
# plus any NAME=VALUE string bindings, and prints the result. An empty line
# means "not found" (KeyError/IndexError/TypeError/StopIteration) or a JSON
# null. Only used to pull ids/urls (never secret values) out of Stripe API
# responses.
json_get() {
  local file="$1" expr="$2"
  shift 2
  python3 - "$file" "$expr" "$@" <<'PY'
import json, sys
file, expr = sys.argv[1], sys.argv[2]
extra = {}
for kv in sys.argv[3:]:
    name, _, value = kv.partition("=")
    extra[name] = value
with open(file) as f:
    d = json.load(f)
ns = {"d": d}
ns.update(extra)
# Single-dict eval (globals only, locals defaults to it): generator
# expressions compile to a nested scope whose free variables resolve via
# closure over the enclosing *frame*, not via a separately-passed locals
# dict — eval(expr, {}, ns) would raise NameError for `target` inside the
# genexpr below. Globals lookups don't have that restriction, so folding
# everything into one dict (used as both) is what makes `target` visible.
try:
    result = eval(expr, ns)
except (KeyError, IndexError, TypeError, StopIteration):
    result = ""
if result is None:
    result = ""
print(result)
PY
}

# create_new_webhook_endpoint: POSTs a new webhook endpoint for
# WEBHOOK_TARGET_URL subscribed to checkout.session.completed, then appends
# its signing secret to ~/.env. Appending (rather than rewriting the file)
# preserves the file's existing permissions (expected chmod 600) since the
# inode is untouched.
create_new_webhook_endpoint() {
  local create_body="$TMP_DIR/webhook-create.json"
  local status
  status="$(stripe_request POST /v1/webhook_endpoints "$create_body" \
    "url=$WEBHOOK_TARGET_URL" "enabled_events[]=checkout.session.completed")"
  if [[ "$status" -lt 200 || "$status" -ge 300 ]]; then
    echo "webhook endpoint creation failed: HTTP $status" >&2
    exit 1
  fi
  local secret
  secret="$(json_get "$create_body" 'd["secret"]')"
  if [[ -z "$secret" ]]; then
    echo "error: webhook endpoint creation response had no secret" >&2
    exit 1
  fi
  printf 'STRIPE_WEBHOOK_SECRET=%s\n' "$secret" >>"$ENV_FILE"
  echo "webhook secret stored in ~/.env"
}

# --- step 1: price -------------------------------------------------------

echo "Checking for existing Price (lookup_key=$PRICE_LOOKUP_KEY) ..."
price_lookup_body="$TMP_DIR/price-lookup.json"
status="$(stripe_request GET /v1/prices "$price_lookup_body" \
  "lookup_keys[]=$PRICE_LOOKUP_KEY" "limit=1")"
if [[ "$status" -lt 200 || "$status" -ge 300 ]]; then
  echo "price lookup failed: HTTP $status" >&2
  exit 1
fi

price_id="$(json_get "$price_lookup_body" 'd["data"][0]["id"]')"

if [[ -n "$price_id" ]]; then
  echo "price: $price_id (existing)"
else
  echo "Creating product 'Socratic Math Tutor' ..."
  product_body="$TMP_DIR/product-create.json"
  status="$(stripe_request POST /v1/products "$product_body" "name=Socratic Math Tutor")"
  if [[ "$status" -lt 200 || "$status" -ge 300 ]]; then
    echo "product creation failed: HTTP $status" >&2
    exit 1
  fi
  product_id="$(json_get "$product_body" 'd["id"]')"
  if [[ -z "$product_id" ]]; then
    echo "error: product creation response had no id" >&2
    exit 1
  fi
  echo "product: $product_id (created)"

  echo "Creating price for product $product_id ..."
  price_body="$TMP_DIR/price-create.json"
  status="$(stripe_request POST /v1/prices "$price_body" \
    "product=$product_id" "unit_amount=2000" "currency=usd" "lookup_key=$PRICE_LOOKUP_KEY")"
  if [[ "$status" -lt 200 || "$status" -ge 300 ]]; then
    echo "price creation failed: HTTP $status" >&2
    exit 1
  fi
  price_id="$(json_get "$price_body" 'd["id"]')"
  if [[ -z "$price_id" ]]; then
    echo "error: price creation response had no id" >&2
    exit 1
  fi
  echo "price: $price_id (created)"
fi

# --- step 2: webhook endpoint --------------------------------------------

echo "Checking for existing webhook endpoint ($WEBHOOK_TARGET_URL) ..."
webhooks_body="$TMP_DIR/webhooks-list.json"
status="$(stripe_request GET /v1/webhook_endpoints "$webhooks_body" "limit=100")"
if [[ "$status" -lt 200 || "$status" -ge 300 ]]; then
  echo "webhook endpoint lookup failed: HTTP $status" >&2
  exit 1
fi

existing_endpoint_id="$(json_get "$webhooks_body" \
  'next((e["id"] for e in d["data"] if e.get("url") == target), "")' \
  "target=$WEBHOOK_TARGET_URL")"

if [[ -n "$existing_endpoint_id" ]]; then
  if [[ -n "${STRIPE_WEBHOOK_SECRET:-}" ]]; then
    echo "webhook endpoint exists; secret already stored"
  else
    echo "webhook endpoint exists (id=$existing_endpoint_id) but no stored secret; recreating to obtain one ..."
    delete_body="$TMP_DIR/webhook-delete.json"
    status="$(stripe_request DELETE "/v1/webhook_endpoints/$existing_endpoint_id" "$delete_body")"
    if [[ "$status" -lt 200 || "$status" -ge 300 ]]; then
      echo "webhook endpoint delete failed: HTTP $status" >&2
      exit 1
    fi
    create_new_webhook_endpoint
  fi
else
  echo "No existing webhook endpoint found; creating ..."
  create_new_webhook_endpoint
fi

# Re-read ~/.env after step 2: the freshest source of truth for
# STRIPE_WEBHOOK_SECRET is the file itself, whether this run just appended
# it moments ago or it was already present from a prior run.
set -a
# shellcheck disable=SC1090
source "$ENV_FILE"
set +a

if [[ -z "${STRIPE_WEBHOOK_SECRET:-}" ]]; then
  echo "error: STRIPE_WEBHOOK_SECRET missing after webhook endpoint step" >&2
  exit 1
fi

# --- step 3: supabase function secrets -----------------------------------

# STRIPE_PRICE_SOCRATIC is not itself read from ~/.env — it's this run's
# resolved price id — but it's exported here so the python3 helper below
# can read every value uniformly via os.environ, matching
# deploy-download-url.sh's secrets_set discipline: secret VALUES are never
# passed as argv (which a local `ps` could see), only names are.
export STRIPE_PRICE_SOCRATIC="$price_id"
secret_names=(STRIPE_SECRET_KEY STRIPE_WEBHOOK_SECRET STRIPE_PRICE_SOCRATIC)

echo "Setting Supabase function secrets: ${secret_names[*]} (values not shown)"

payload="$(python3 - "${secret_names[@]}" <<'PY'
import json, os, sys
names = sys.argv[1:]
print(json.dumps([{"name": n, "value": os.environ[n]} for n in names]))
PY
)"

secrets_resp_body="$TMP_DIR/secrets-response.json"
status="$(curl -sS -o "$secrets_resp_body" -w '%{http_code}' \
  -X POST "$SUPABASE_API_BASE/v1/projects/$PROJECT_REF/secrets" \
  -H "Authorization: Bearer $SUPABASE_ACCESS_TOKEN" \
  -H "Content-Type: application/json" \
  -d "$payload")"
echo "secrets_set: HTTP $status"

if [[ "$status" -lt 200 || "$status" -ge 300 ]]; then
  # Deliberately not dumping the response body here: this endpoint's
  # request body contains secret values, and we don't assume the error
  # response can't reflect part of the request back. Status code plus the
  # on-disk temp file (removed on exit) is the safe amount of detail.
  echo "secrets_set failed (see HTTP status above)" >&2
  exit 1
fi

echo "stripe-bootstrap: done"
