#!/bin/sh
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
log=$(mktemp)
trap 'rm -f "$log"' EXIT
cd "$root"
status=0
MOOR_TEST_DOCKER=1 go test -race -count=1 -v -timeout 20m ./internal/adapter/docker/... >"$log" 2>&1 || status=$?
grep -E '^(=== RUN|--- (FAIL|SKIP)|FAIL|ok|panic)|_test.go:[0-9]+:' "$log" | grep -v '^=== RUN' | tail -n 40 || true
[ "$status" -eq 0 ] || exit "$status"
if grep -q -- '--- SKIP' "$log"; then
	echo "test-docker: a Docker test skipped, so it did not run against Docker" >&2
	exit 1
fi
echo "test-docker: $(grep -c -- '^--- PASS\|^    --- PASS' "$log") passed, 0 skipped"
