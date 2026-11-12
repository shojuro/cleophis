#!/usr/bin/env bash
# Fetch artifacts from the signed distribution catalog, the way a client does:
# public GET from the cleophis-dist bucket, each artifact sha256-verified
# against the catalog's pinned hash. No auth (app is GET-only).
#
# Two modes:
#
#   ./fetch-artifacts.sh [base_model]        # tutor mode; default: Qwen3-4B
#     base_model ∈ {Llama-3.2-1B, Qwen3-1.7B, Qwen3-4B, Qwen3-8B}
#     Fetches the base + EVERY adapter the catalog lists for that base_model.
#     The 4B is the fullest real composed stack today (base + behavioral +
#     contract; there is no voice adapter in the catalog yet). The 1B is the
#     Llama floor (base + behavioral only).
#
#   ./fetch-artifacts.sh --pinned [bundled_catalog.json] [entry_id]
#     defaults: src-tauri/resources/catalog.triage.json, the first `real` entry
#     Fetches exactly the pair that bundled entry pins: the base whose sha256 is
#     the entry's `sha256` and the adapter whose sha256 is its `adapterSha256`.
#     Selection is BY SHA, never by base_model: once a second base (Q6_K beside
#     Q4_K_M) or a second adapter (triage beside behavioral) is published for
#     the same base_model, `base_model` no longer names one file. Refuses when
#     either sha is absent from the signed catalog, when a sha match has the
#     wrong kind, or when its dist basename differs from the entry's
#     modelFile/adapterFile basename (the app writes `models/<dist basename>`,
#     so a differing name is a file the app would never read as installed).
#
# Output: ~/cleophis-artifacts/<dist basename>  (override with DEST=...)
# run-device-probes.sh reads the pinned mode's output as-is.
set -euo pipefail

# Compiled-in ARTIFACT_BASE_URL from src-tauri/src/catalog_dist.rs.
BASE="https://cleophis-dist.s3.us-east-005.backblazeb2.com"
DEST="${DEST:-$HOME/cleophis-artifacts}"
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"

MODE="tutor"
if [ "${1:-}" = "--pinned" ]; then
  MODE="pinned"
  BUNDLED="${2:-$REPO/src-tauri/resources/catalog.triage.json}"
  ENTRY_ID="${3:-}"
  [ -f "$BUNDLED" ] || { echo "FATAL: bundled catalog not found: $BUNDLED" >&2; exit 1; }
else
  MODEL="${1:-Qwen3-4B}"
fi
mkdir -p "$DEST"

echo "== fetching dist catalog =="
CATALOG_FILE="$(mktemp)"
trap 'rm -f "$CATALOG_FILE"' EXIT
curl -fsSL -o "$CATALOG_FILE" "$BASE/catalog.json"

# Emit "path sha256" lines. Arguments reach Python as argv, never interpolated.
if [ "$MODE" = "tutor" ]; then
  mapfile -t ROWS < <(python3 - "$CATALOG_FILE" "$MODEL" <<'PY'
import sys, json
d = json.load(open(sys.argv[1]))
for a in d['artifacts']:
    if a.get('base_model') == sys.argv[2]:
        print(a['path'], a['sha256'])
PY
)
  [ "${#ROWS[@]}" -gt 0 ] || { echo "no artifacts for base_model=$MODEL in catalog"; exit 1; }
else
  # A refusal here exits non-zero and prints why; `mapfile` would swallow the
  # status, so the rows go through a variable first.
  PINNED_OUT="$(python3 - "$CATALOG_FILE" "$BUNDLED" "$ENTRY_ID" <<'PY'
import sys, json, posixpath
dist = json.load(open(sys.argv[1]))
bundled = json.load(open(sys.argv[2]))
want_id = sys.argv[3]
hits = [e for e in bundled if (e.get("id") == want_id if want_id else e.get("real"))]
if not hits:
    sys.exit(f"FATAL: no {'entry ' + want_id if want_id else 'real entry'} in {sys.argv[2]}")
e = hits[0]
arts = dist.get("artifacts") or []
rows = []
for role, sha_key, file_key, kind in (("base", "sha256", "modelFile", "base"),
                                      ("adapter", "adapterSha256", "adapterFile", "adapter")):
    sha = (e.get(sha_key) or "").lower()
    if not sha:
        sys.exit(f"FATAL: entry {e.get('id')} pins no {role} ({sha_key} empty)")
    matches = [a for a in arts if (a.get("sha256") or "").lower() == sha]
    if not matches:
        sys.exit(f"FATAL: the {role} {sha[:12]}… pinned by {e.get('id')} is not in the signed "
                 f"catalog (catalog_version {dist.get('catalog_version', '?')}). Nothing fetched; publish it first.")
    good = [a for a in matches if a.get("kind") == kind]
    if not good:
        sys.exit(f"FATAL: the {role} {sha[:12]}… is published as kind "
                 f"'{matches[0].get('kind')}', not '{kind}'")
    a = good[0]
    dist_name = posixpath.basename(a["path"])
    bundled_name = posixpath.basename(e.get(file_key) or "")
    if dist_name != bundled_name:
        sys.exit(f"FATAL: the {role}'s dist basename {dist_name} is not the entry's "
                 f"{file_key} basename {bundled_name}; the app would never read it as installed")
    rows.append(f"{a['path']} {a['sha256'].lower()}")
print(f"# entry {e['id']}: {e.get('quant', '?')} base + adapter, selected by sha", file=sys.stderr)
print("\n".join(rows))
PY
)" || exit 1
  mapfile -t ROWS <<< "$PINNED_OUT"
fi

for row in "${ROWS[@]}"; do
  path="${row%% *}"; want="${row##* }"; out="$DEST/$(basename "$path")"
  echo "GET $(basename "$path") ..."
  curl -fL -s -o "$out" "$BASE/$path"
  got="$(sha256sum "$out" | cut -d' ' -f1)"
  if [ "$got" = "$want" ]; then echo "  OK  ($(du -h "$out" | cut -f1))  sha256 verified"
  else echo "  SHA MISMATCH got=$got want=$want"; exit 1; fi
done
echo "== done -> $DEST =="
ls -la "$DEST"/*.gguf
