#!/usr/bin/env bash
set -euo pipefail

dir=$(cd "$(dirname "$0")/contract-fixtures" && pwd)
fail() { echo "contract-fixtures-selftest: $*" >&2; exit 1; }

n=0
for ev in "$dir"/selftest/*/; do
  ev=${ev%/}
  case=$(basename "$ev")
  fixture=${case%-*}
  want=${case##*-}
  rc=0
  out=$("$dir/run" --check "$ev" "$fixture") || rc=$?
  case $want in
    pass) [ "$rc" -eq 0 ] || fail "$case should pass: $out" ;;
    fail) if [ "$rc" -ne 1 ] || ! grep -q 'failed: ' <<<"$out"; then fail "$case should fail with a named assertion (exit $rc): $out"; fi ;;
    *) fail "$case: name it <fixture>-pass or <fixture>-fail" ;;
  esac
  n=$((n + 1))
done
[ "$n" -ge 2 ] || fail "found $n recorded runs, want a passing and a failing one"
for f in "$dir"/fixtures/*.toml; do
  rc=0
  "$dir/run" --check "$dir/selftest" "$(basename "$f" .toml)" >/dev/null || rc=$?
  [ "$rc" -ne 2 ] || fail "$f does not load"
done
echo "contract-fixtures-selftest: $n recorded runs ok"
