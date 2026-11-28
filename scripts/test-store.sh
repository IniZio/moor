#!/bin/sh
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
if [ -z "${MOOR_TEST_POSTGRES_DSN:-}" ]; then
	exec sh "$root/scripts/with-postgres.sh" sh "$0"
fi
log=$(mktemp)
trap 'rm -f "$log"' EXIT

cd "$root"
status=0
go test -race -count=1 -v ./internal/adapter/store/... ./cmd/portal/... >"$log" 2>&1 || status=$?
grep -E '^(=== RUN|--- (FAIL|SKIP)|FAIL|ok|panic)|_test.go:[0-9]+:' "$log" | grep -v '^=== RUN' | tail -n 40 || true
[ "$status" -eq 0 ] || exit "$status"
if grep -q -- '--- SKIP' "$log"; then
	echo "test-store: a store test skipped, so it did not run against Postgres" >&2
	exit 1
fi
echo "test-store: $(grep -c -- '^--- PASS\|^    --- PASS' "$log") passed, 0 skipped"
