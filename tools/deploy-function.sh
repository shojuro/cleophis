#!/usr/bin/env bash
# tools/deploy-function.sh — generalized Supabase Edge Function deploy via
# the Management API. Generalizes tools/deploy-download-url.sh's `deploy`
# step (kept as-is, untouched) to any function slug under
# supabase/functions/<slug>/index.ts, with a --no-verify-jwt switch for
# functions like stripe-webhook that Stripe calls without a Supabase JWT.
#
# Task S5: written and reviewed now (code-only stage); NOT executed in this
# task — deployment happens in S6 once Stripe env vars are bootstrapped.
#
# Usage:
#   tools/deploy-function.sh <slug> [--no-verify-jwt]
#
# Examples:
#   tools/deploy-function.sh create-checkout
#   tools/deploy-function.sh stripe-webhook --no-verify-jwt
#
# Same Management API call as deploy-download-url.sh's deploy step:
# POST /v1/projects/{ref}/functions/deploy?slug=<slug> with multipart fields
# "metadata" (JSON: name, entrypoint_path, verify_jwt) and "file" (source).
#
# Reads SUPABASE_ACCESS_TOKEN from ~/.env (kept out of shell history/repo,
# same pattern as deploy-download-url.sh).
#
# Secret values are never echoed — only the slug, the verify_jwt flag, and
# HTTP status codes/response bodies (deploy responses carry no secrets).

set -euo pipefail

PROJECT_REF="isltexsxpysxqewjsryv"
API_BASE="https://api.supabase.com"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

usage() {
  echo "usage: $0 <slug> [--no-verify-jwt]" >&2
  exit 1
}

if [[ $# -lt 1 || $# -gt 2 || "$1" == --* ]]; then
  usage
fi

slug="$1"
verify_jwt="true"

if [[ $# -eq 2 ]]; then
  if [[ "$2" != "--no-verify-jwt" ]]; then
    usage
  fi
  verify_jwt="false"
fi

FUNCTION_SRC="$REPO_ROOT/supabase/functions/$slug/index.ts"

if [[ -f "$HOME/.env" ]]; then
  set -a
  # shellcheck disable=SC1090
  source "$HOME/.env"
  set +a
fi

if [[ -z "${SUPABASE_ACCESS_TOKEN:-}" ]]; then
  echo "error: SUPABASE_ACCESS_TOKEN is not set (expected in ~/.env)" >&2
  exit 1
fi

if [[ ! -f "$FUNCTION_SRC" ]]; then
  echo "error: function source not found at $FUNCTION_SRC" >&2
  exit 1
fi

# Temp files for response bodies; cleaned up on exit. The body is only
# printed on failure — deploy responses/logs carry no secret material.
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

# Build metadata JSON with python3 (available in this WSL image; jq is
# not) so the slug is properly JSON-escaped rather than hand-interpolated
# into a JSON string, matching the secrets_set discipline in
# deploy-download-url.sh even though slug is not itself secret.
metadata="$(python3 - "$slug" "$verify_jwt" <<'PY'
import json, sys
name, verify_jwt = sys.argv[1], sys.argv[2] == "true"
print(json.dumps({"name": name, "entrypoint_path": "index.ts", "verify_jwt": verify_jwt}))
PY
)"

resp_body="$TMP_DIR/deploy-response.json"
status=""

echo "Deploying $slug from $FUNCTION_SRC (verify_jwt=$verify_jwt) ..."
status="$(curl -sS -o "$resp_body" -w '%{http_code}' \
  -X POST "$API_BASE/v1/projects/$PROJECT_REF/functions/deploy?slug=$slug" \
  -H "Authorization: Bearer $SUPABASE_ACCESS_TOKEN" \
  -F "metadata=$metadata;type=application/json" \
  -F "file=@$FUNCTION_SRC;filename=index.ts;type=application/typescript")"
echo "deploy: HTTP $status"

if [[ "$status" -lt 200 || "$status" -ge 300 ]]; then
  echo "deploy failed; response body:" >&2
  cat "$resp_body" >&2 || true
  exit 1
fi
