#!/usr/bin/env bash
# Fails when a removed identifier reappears in production Rust or shell sources.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
names=(
  proxy_gate MOOR_PROXY_UPSTREAM MOOR_GH_API MOOR_GH_REPO MOOR_PUSH_URL
  PROXY_SERVED gh_shim model_route ModelRoute ANTHROPIC_BASE_URL insteadOf
  RouteRow RouteTable
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
echo "removed-names: ok"
