#!/usr/bin/env bash
# Fails when a removed identifier reappears in production Rust or shell sources,
# or when the word "task" appears anywhere outside tools/removed-names.allow.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
. "$here/lib/repo-root.sh"
# MOOR_SCAN_ROOT points the scan at another tree (the lint selftest); default is the repo root.
if [ -n "${MOOR_SCAN_ROOT:-}" ]; then cd "$MOOR_SCAN_ROOT"; else cd_repo_root; fi
names=(
  proxy_gate MOOR_PROXY_UPSTREAM MOOR_GH_API MOOR_GH_REPO MOOR_PUSH_URL
  PROXY_SERVED gh_shim model_route ModelRoute ANTHROPIC_BASE_URL insteadOf
  RouteRow RouteTable WebConfig web_feed start_web
)
args=()
for n in "${names[@]}"; do args+=(-e "$n"); done
hits=$(grep -rnF "${args[@]}" \
  --include='*.rs' --include='*.sh' \
  --exclude='tests.rs' --exclude-dir=tests --exclude-dir=target \
  crates/ || true)
if [ -n "$hits" ]; then
  echo "removed-names: removed names found"
  echo "$hits"
  exit 1
fi

# The removed Task concept must not return in any file, test or not.
allow="$(mktemp)"
trap 'rm -f "$allow"' EXIT
grep -v -e '^#' -e '^[[:space:]]*$' "$here/removed-names.allow" > "$allow" || true
targets=()
for t in crates tools README.md .github; do [ -e "$t" ] && targets+=("$t"); done
task_hits=$(grep -rniI --exclude-dir=target --exclude=Cargo.lock -e task "${targets[@]}" \
  | grep -v -e '^tools/removed-names\.sh:' -e '^tools/lint-selftest\.sh:' -e '^tools/removed-names\.allow:' \
  | grep -vFf "$allow" || true)
if [ -n "$task_hits" ]; then
  echo "removed-names: the removed Task concept found"
  echo "$task_hits"
  exit 1
fi
echo "removed-names: ok"
