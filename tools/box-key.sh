#!/usr/bin/env bash
# Fails when production Rust builds a Box-derived identifier outside the BoxKey seam.
# It only catches literal patterns such as format!("moor-{..."); an identifier built
# another way, for example by concat! or from a constant prefix, passes unseen.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
pattern='"(moor-\{|\.moor-cred-\{|\.moor-tmp-\{|moor/\{)'
scan() {
  grep -rnE "$pattern" \
    --include='*.rs' \
    --exclude='tests.rs' --exclude='box_key.rs' --exclude-dir=tests --exclude-dir=target \
    "$@" || true
}
if [ "${1:-}" = "--selftest" ]; then
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT
  echo 'let s = format!("moor-{}-{}", name, nonce);' >"$tmp/planted.rs"
  if [ -z "$(scan "$tmp")" ]; then
    echo "box-key selftest: planted violation not detected"
    exit 1
  fi
  echo "box-key selftest: ok"
  exit 0
fi
hits=$(scan crates/)
if [ -n "$hits" ]; then
  echo "box-key: identifiers built outside the BoxKey seam"
  echo "$hits"
  echo "fix: build the name with a moor_core::box_key::BoxKey method, adding one there if none fits"
  exit 1
fi
echo "box-key: ok"
