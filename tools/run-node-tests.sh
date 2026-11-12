#!/usr/bin/env bash
# Run `node --test` over an explicit, sorted list of test files found under the
# given directories. Node 20 searches a directory argument for test files, but
# Node 21 through 24 treat it as a glob that matches the directory itself and
# fail with "Cannot find module". Passing explicit file paths behaves the same
# on every Node version. Discovery follows Node 20's documented rule.
# Usage: tools/run-node-tests.sh <dir> [<dir>...]
set -euo pipefail

if [ "$#" -eq 0 ]; then
  echo "usage: $0 <dir> [<dir>...]" >&2
  exit 2
fi

files=()
while IFS= read -r f; do
  files+=("$f")
done < <(
  find "$@" -type d -name node_modules -prune -o -type f \
    \( -name '*.js' -o -name '*.cjs' -o -name '*.mjs' \) \
    \( -name 'test.*' -o -name 'test-*' -o -name '*.test.*' \
       -o -name '*-test.*' -o -name '*_test.*' -o -path '*/test/*' \) \
    -print | LC_ALL=C sort
)

if [ "${#files[@]}" -eq 0 ]; then
  echo "run-node-tests: no test files found under: $*" >&2
  exit 1
fi

exec node --test "${files[@]}"
