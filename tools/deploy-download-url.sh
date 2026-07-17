#!/usr/bin/env bash
# tools/deploy-download-url.sh — deploy helper for the download-url edge
# function, via the Supabase Management API.
#
# Task C6a: written and reviewed now (code-only stage); NOT executed in
# this task. The coordinator runs it in Task C6b once the user's Backblaze
# B2 keys are available.
#
# Usage:
#   tools/deploy-download-url.sh deploy        # push supabase/functions/download-url/index.ts
#   tools/deploy-download-url.sh secrets_set    # set B2_* secrets from env
#
# Assumption on invocation shape: the brief lists "deploy" and "secrets_set"
# as two numbered steps/modes of this one script; this implementation reads
# the mode from $1. If the intent was instead a fixed "deploy" subcommand
# that also accepts a literal "secrets_set" as $2, the coordinator adapts
# at C6b.
#
# NOTE on the deploy endpoint shape: `POST /v1/projects/{ref}/functions/deploy?slug=<slug>`
# with multipart fields "metadata" (JSON) and "file" (source) is the
# best-known form as of this writing per the Supabase Management API docs.
# If the real call rejects the field names/shape, the coordinator adapts
# this script at C6b and documents the fix here.
#
# Reads SUPABASE_ACCESS_TOKEN from ~/.env (kept out of shell history/repo,
# same pattern as other tokens used in this project).
#
# Secret values are never echoed — only variable names, and only HTTP
# status codes from responses.

set -euo pipefail

PROJECT_REF="isltexsxpysxqewjsryv"
API_BASE="https://api.supabase.com"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
FUNCTION_SRC="$REPO_ROOT/supabase/functions/download-url/index.ts"

mode="${1:-}"
if [[ "$mode" != "deploy" && "$mode" != "secrets_set" ]]; then
  echo "usage: $0 {deploy|secrets_set}" >&2
  exit 1
fi

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

# Temp files for response bodies; cleaned up on exit. Bodies are only
# printed on failure, and only for the deploy step (function deploy
# responses/logs, not secret material).
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

deploy() {
  if [[ ! -f "$FUNCTION_SRC" ]]; then
    echo "error: function source not found at $FUNCTION_SRC" >&2
    exit 1
  fi

  local metadata='{"name":"download-url","entrypoint_path":"index.ts","verify_jwt":true}'
  local resp_body="$TMP_DIR/deploy-response.json"
  local status

  echo "Deploying download-url from $FUNCTION_SRC ..."
  status="$(curl -sS -o "$resp_body" -w '%{http_code}' \
    -X POST "$API_BASE/v1/projects/$PROJECT_REF/functions/deploy?slug=download-url" \
    -H "Authorization: Bearer $SUPABASE_ACCESS_TOKEN" \
    -F "metadata=$metadata;type=application/json" \
    -F "file=@$FUNCTION_SRC;filename=index.ts;type=application/typescript")"
  echo "deploy: HTTP $status"

  if [[ "$status" -lt 200 || "$status" -ge 300 ]]; then
    echo "deploy failed; response body:" >&2
    cat "$resp_body" >&2 || true
    exit 1
  fi
}

secrets_set() {
  local required_vars=(B2_KEY_ID B2_APP_KEY B2_BUCKET_ID B2_BUCKET_NAME)
  local optional_vars=(B2_DOWNLOAD_BASE_URL)
  local missing=()
  local present=()

  for v in "${required_vars[@]}"; do
    if [[ -z "${!v:-}" ]]; then
      missing+=("$v")
    else
      present+=("$v")
    fi
  done
  if [[ "${#missing[@]}" -gt 0 ]]; then
    echo "error: missing required secret env vars: ${missing[*]}" >&2
    exit 1
  fi
  for v in "${optional_vars[@]}"; do
    if [[ -n "${!v:-}" ]]; then
      present+=("$v")
    fi
  done

  echo "Setting secrets: ${present[*]} (values not shown)"

  # Build the {name, value} JSON array with python3 (available in this WSL
  # image; jq is not) so secret values are properly JSON-escaped rather
  # than hand-interpolated into a JSON string.
  local payload
  payload="$(python3 - "${present[@]}" <<'PY'
import json, os, sys
names = sys.argv[1:]
print(json.dumps([{"name": n, "value": os.environ[n]} for n in names]))
PY
)"

  local resp_body="$TMP_DIR/secrets-response.json"
  local status
  status="$(curl -sS -o "$resp_body" -w '%{http_code}' \
    -X POST "$API_BASE/v1/projects/$PROJECT_REF/secrets" \
    -H "Authorization: Bearer $SUPABASE_ACCESS_TOKEN" \
    -H "Content-Type: application/json" \
    -d "$payload")"
  echo "secrets_set: HTTP $status"

  if [[ "$status" -lt 200 || "$status" -ge 300 ]]; then
    # Deliberately not dumping the response body here: unlike the deploy
    # step, this endpoint's request body contains secret values, and we
    # don't assume the error response can't reflect part of the request
    # back. Status code plus the on-disk temp file (removed on exit) is
    # the safe amount of detail for a code-only-stage script.
    echo "secrets_set failed (see HTTP status above)" >&2
    exit 1
  fi
}

case "$mode" in
  deploy) deploy ;;
  secrets_set) secrets_set ;;
esac
