#!/usr/bin/env bash
# Fetch a hero tier's base + adapters from the signed distribution catalog, the
# way a client does: public GET from the cleophis-dist bucket, each artifact
# sha256-verified against the catalog's pinned hash. No auth (app is GET-only).
#
# Usage:  ./fetch-artifacts.sh [base_model]     # default: Qwen3-4B
#   base_model ∈ {Llama-3.2-1B, Qwen3-1.7B, Qwen3-4B, Qwen3-8B}
# Output: ~/cleophis-artifacts/<file>.gguf  (base + every adapter for that model)
#
# The 4B is the fullest real composed stack today (base + behavioral + contract;
# there is no voice adapter in the catalog yet). The 1B is the Llama floor
# (base + behavioral only).
set -euo pipefail

# Compiled-in ARTIFACT_BASE_URL from src-tauri/src/catalog_dist.rs.
BASE="https://cleophis-dist.s3.us-east-005.backblazeb2.com"
MODEL="${1:-Qwen3-4B}"
DEST="${DEST:-$HOME/cleophis-artifacts}"
mkdir -p "$DEST"

echo "== fetching dist catalog =="
CATALOG="$(curl -fsSL "$BASE/catalog.json")"

# Emit "path sha256" lines for the base + adapters of the chosen base_model.
mapfile -t ROWS < <(printf '%s' "$CATALOG" | python3 -c "
import sys, json
d = json.load(sys.stdin)
for a in d['artifacts']:
    if a.get('base_model') == '$MODEL':
        print(a['path'], a['sha256'])
")
[ "${#ROWS[@]}" -gt 0 ] || { echo "no artifacts for base_model=$MODEL in catalog"; exit 1; }

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
