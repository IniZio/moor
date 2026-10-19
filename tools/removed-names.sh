#!/usr/bin/env bash
# Fails when a removed identifier reappears in production Rust or shell sources.
set -euo pipefail
. "$(dirname "$0")/lib/repo-root.sh"
cd_repo_root
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
echo "removed-names: ok"
