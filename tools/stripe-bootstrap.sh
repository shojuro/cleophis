#!/usr/bin/env bash
# tools/stripe-bootstrap.sh — idempotent Stripe environment setup:
#   1. ensures the one-time Price for the Socratic Math Tutor exists
#   2. ensures the monthly subscription Price for the Socratic Math Tutor
#      exists, reusing the product resolved in step 1
#   3. ensures a webhook endpoint pointed at stripe-webhook exists, subscribed
#      to both checkout.session.completed and invoice.paid with api_version
#      pinned to 2024-06-20, with its signing secret captured in ~/.env
#      (Stripe only returns the signing secret at creation time, never on a
#      later GET)
#   4. ensures a default billing portal configuration exists (best-effort;
#      never fails the script — see step 4 below)
#   5. pushes STRIPE_SECRET_KEY, STRIPE_WEBHOOK_SECRET,
#      STRIPE_PRICE_SOCRATIC, and STRIPE_PRICE_SOCRATIC_MONTHLY to the
#      Supabase project as function secrets via the Management API
#
# Task S5/Sub4a: written and reviewed now (code-only stage); NOT executed in
# this task — bootstrap runs in S6.
#
# Usage:
#   tools/stripe-bootstrap.sh
#
# Idempotency: safe to re-run.
#   - Prices (matched by lookup_key) are reused rather than recreated.
#   - The product backing both prices is only created once: if the one-time
#     price already exists, its `product` field (already present in the same
#     GET response used to check existence — no extra call) is reused to
#     create the monthly price; a new product is created only when neither
#     price exists yet.
#   - The webhook endpoint (matched by url) is reused if it already has both
#     enabled_events and the pinned api_version. If it's missing an event but
#     the api_version is already correct, it's updated in place (a POST
#     update does not rotate the signing secret). If the api_version is
#     missing/wrong (api_version cannot be changed via update) — or if no
#     signing secret is stored locally (e.g. ~/.env was lost/rotated out from
#     under it) — the endpoint is deleted and recreated to obtain a fresh
#     secret.
#   - The default billing portal configuration is reused if one already
#     exists (is_default=true); creation failures are logged as a warning
#     and do not fail the script, since a portal configuration can also be
#     saved once by hand in the Stripe Dashboard.
#
# Reads STRIPE_SECRET_KEY and SUPABASE_ACCESS_TOKEN from ~/.env; fails fast
# if either is missing. All Stripe API calls use HTTP Basic auth with the
# secret key as the username and an empty password (Stripe's convention),
# so the key never appears on a URL or in a header curl would echo.
#
# Secret values (STRIPE_SECRET_KEY, STRIPE_WEBHOOK_SECRET) are never
# echoed — only variable names, ids (product/price/webhook-endpoint/
# portal-configuration ids are not secrets), and HTTP status codes.

set -euo pipefail

PROJECT_REF="isltexsxpysxqewjsryv"
SUPABASE_API_BASE="https://api.supabase.com"
STRIPE_API_BASE="https://api.stripe.com"
WEBHOOK_TARGET_URL="https://$PROJECT_REF.supabase.co/functions/v1/stripe-webhook"
PRICE_LOOKUP_KEY="socratic-tutor-onetime"
PRICE_LOOKUP_KEY_MONTHLY="socratic-tutor-monthly"
WEBHOOK_API_VERSION="2024-06-20"
PORTAL_RETURN_URL="https://shojuro.github.io/cleophis/pay/portal-return.html"

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
# WEBHOOK_TARGET_URL subscribed to checkout.session.completed and
# invoice.paid, with api_version pinned (api_version can only be set at
# creation, never changed via update — see step 3 below), then appends its
# signing secret to ~/.env. Appending (rather than rewriting the file)
# preserves the file's existing permissions (expected chmod 600) since the
# inode is untouched.
create_new_webhook_endpoint() {
  local create_body="$TMP_DIR/webhook-create.json"
  local status
  status="$(stripe_request POST /v1/webhook_endpoints "$create_body" \
    "url=$WEBHOOK_TARGET_URL" \
    "enabled_events[]=checkout.session.completed" \
    "enabled_events[]=invoice.paid" \
    "api_version=$WEBHOOK_API_VERSION")"
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
  # Resolve the product id from the same lookup response (a Price object
  # always carries its product id) so step 2 (monthly price) never needs an
  # extra API call, and a new product is created only when neither price
  # exists yet.
  product_id="$(json_get "$price_lookup_body" 'd["data"][0]["product"]')"
  if [[ -z "$product_id" ]]; then
    echo "error: existing price lookup response had no product id" >&2
    exit 1
  fi
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

# --- step 2: monthly subscription price -----------------------------------

echo "Checking for existing Price (lookup_key=$PRICE_LOOKUP_KEY_MONTHLY) ..."
monthly_price_lookup_body="$TMP_DIR/monthly-price-lookup.json"
status="$(stripe_request GET /v1/prices "$monthly_price_lookup_body" \
  "lookup_keys[]=$PRICE_LOOKUP_KEY_MONTHLY" "limit=1")"
if [[ "$status" -lt 200 || "$status" -ge 300 ]]; then
  echo "monthly price lookup failed: HTTP $status" >&2
  exit 1
fi

monthly_price_id="$(json_get "$monthly_price_lookup_body" 'd["data"][0]["id"]')"

if [[ -n "$monthly_price_id" ]]; then
  echo "monthly price: $monthly_price_id (reused)"
else
  echo "Creating monthly price for product $product_id ..."
  monthly_price_body="$TMP_DIR/monthly-price-create.json"
  status="$(stripe_request POST /v1/prices "$monthly_price_body" \
    "product=$product_id" "unit_amount=2000" "currency=usd" \
    "recurring[interval]=month" "lookup_key=$PRICE_LOOKUP_KEY_MONTHLY")"
  if [[ "$status" -lt 200 || "$status" -ge 300 ]]; then
    echo "monthly price creation failed: HTTP $status" >&2
    exit 1
  fi
  monthly_price_id="$(json_get "$monthly_price_body" 'd["id"]')"
  if [[ -z "$monthly_price_id" ]]; then
    echo "error: monthly price creation response had no id" >&2
    exit 1
  fi
  echo "monthly price: $monthly_price_id (created)"
fi

# --- step 3: webhook endpoint ----------------------------------------------

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
  # Inspect the matched endpoint's enabled_events + api_version from the
  # same listing response already fetched above — no extra GET needed — to
  # decide reuse vs. update-in-place vs. delete+recreate.
  existing_events="$(json_get "$webhooks_body" \
    '",".join(next((e.get("enabled_events", []) for e in d["data"] if e["id"] == target), []))' \
    "target=$existing_endpoint_id")"
  existing_api_version="$(json_get "$webhooks_body" \
    'next((e.get("api_version") or "" for e in d["data"] if e["id"] == target), "")' \
    "target=$existing_endpoint_id")"
  existing_events_padded=",$existing_events,"
  has_checkout_event=false
  has_invoice_event=false
  [[ "$existing_events_padded" == *",checkout.session.completed,"* ]] && has_checkout_event=true
  [[ "$existing_events_padded" == *",invoice.paid,"* ]] && has_invoice_event=true
  has_pinned_api_version=false
  [[ "$existing_api_version" == "$WEBHOOK_API_VERSION" ]] && has_pinned_api_version=true

  if [[ -z "${STRIPE_WEBHOOK_SECRET:-}" ]]; then
    echo "webhook endpoint exists (id=$existing_endpoint_id) but no stored secret; recreating to obtain one ..."
    delete_body="$TMP_DIR/webhook-delete.json"
    status="$(stripe_request DELETE "/v1/webhook_endpoints/$existing_endpoint_id" "$delete_body")"
    if [[ "$status" -lt 200 || "$status" -ge 300 ]]; then
      echo "webhook endpoint delete failed: HTTP $status" >&2
      exit 1
    fi
    create_new_webhook_endpoint
  elif [[ "$has_checkout_event" == true && "$has_invoice_event" == true && "$has_pinned_api_version" == true ]]; then
    echo "webhook endpoint exists; secret already stored; events and api_version unchanged"
  elif [[ "$has_pinned_api_version" != true ]]; then
    # api_version can only be set at creation, never changed via a later
    # update, so a missing/mismatched pinned version can only be fixed by
    # deleting and recreating — which mints a new signing secret.
    echo "webhook endpoint exists (id=$existing_endpoint_id) but api_version is not pinned to $WEBHOOK_API_VERSION; recreating (signing secret will rotate) ..."
    delete_body="$TMP_DIR/webhook-delete.json"
    status="$(stripe_request DELETE "/v1/webhook_endpoints/$existing_endpoint_id" "$delete_body")"
    if [[ "$status" -lt 200 || "$status" -ge 300 ]]; then
      echo "webhook endpoint delete failed: HTTP $status" >&2
      exit 1
    fi
    create_new_webhook_endpoint
  else
    # api_version is already correct; only an enabled_events entry is
    # missing, so update in place — this does NOT rotate the signing secret.
    echo "webhook endpoint exists (id=$existing_endpoint_id) but is missing an enabled event; updating in place ..."
    update_body="$TMP_DIR/webhook-update.json"
    status="$(stripe_request POST "/v1/webhook_endpoints/$existing_endpoint_id" "$update_body" \
      "enabled_events[]=checkout.session.completed" "enabled_events[]=invoice.paid")"
    if [[ "$status" -lt 200 || "$status" -ge 300 ]]; then
      echo "webhook endpoint update failed: HTTP $status" >&2
      exit 1
    fi
    echo "endpoint updated in place; secret unchanged"
  fi
else
  echo "No existing webhook endpoint found; creating ..."
  create_new_webhook_endpoint
fi

# Re-read ~/.env after step 3: the freshest source of truth for
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

# --- step 4: billing portal configuration -----------------------------------

# run_portal_step: reuses the default billing portal configuration
# (is_default=true) if one exists, else creates one. Called from inside an
# `if !`, which — per bash's documented set -e behavior — exempts every
# command in this function from triggering script exit on failure, so a
# failure at any point here (lookup or creation) falls through to the
# warning below and the script continues, matching the requirement that a
# subscriber-facing customer portal can also be configured once by hand in
# the Stripe Dashboard.
run_portal_step() {
  local list_body="$TMP_DIR/portal-list.json"
  local status
  status="$(stripe_request GET /v1/billing_portal/configurations "$list_body" \
    "is_default=true" "limit=1")"
  if [[ "$status" -lt 200 || "$status" -ge 300 ]]; then
    echo "billing portal configuration lookup failed: HTTP $status" >&2
    return 1
  fi

  local portal_config_id
  portal_config_id="$(json_get "$list_body" 'd["data"][0]["id"]')"
  if [[ -n "$portal_config_id" ]]; then
    echo "billing portal configuration: $portal_config_id (reused)"
    return 0
  fi

  echo "Creating default billing portal configuration ..."
  local create_body="$TMP_DIR/portal-create.json"
  status="$(stripe_request POST /v1/billing_portal/configurations "$create_body" \
    "features[invoice_history][enabled]=true" \
    "features[subscription_cancel][enabled]=true" \
    "features[subscription_cancel][mode]=at_period_end" \
    "default_return_url=$PORTAL_RETURN_URL" \
    "business_profile[headline]=Cleophis")"
  if [[ "$status" -lt 200 || "$status" -ge 300 ]]; then
    echo "billing portal configuration creation failed: HTTP $status" >&2
    return 1
  fi

  portal_config_id="$(json_get "$create_body" 'd["id"]')"
  if [[ -z "$portal_config_id" ]]; then
    echo "billing portal configuration creation response had no id" >&2
    return 1
  fi
  echo "billing portal configuration: $portal_config_id (created)"
}

echo "Checking for existing default billing portal configuration ..."
if ! run_portal_step; then
  echo "warning: could not verify/create a default billing portal configuration automatically; it must be saved once by hand in the Stripe Dashboard (Settings -> Billing -> Customer portal) before subscription customers can access it" >&2
fi

# --- step 5: supabase function secrets -----------------------------------

# STRIPE_PRICE_SOCRATIC and STRIPE_PRICE_SOCRATIC_MONTHLY are not themselves
# read from ~/.env — they're this run's resolved price ids — but they're
# exported here so the python3 helper below can read every value uniformly
# via os.environ, matching deploy-download-url.sh's secrets_set discipline:
# secret VALUES are never passed as argv (which a local `ps` could see),
# only names are.
export STRIPE_PRICE_SOCRATIC="$price_id"
export STRIPE_PRICE_SOCRATIC_MONTHLY="$monthly_price_id"
secret_names=(STRIPE_SECRET_KEY STRIPE_WEBHOOK_SECRET STRIPE_PRICE_SOCRATIC STRIPE_PRICE_SOCRATIC_MONTHLY)

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
