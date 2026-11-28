#!/bin/sh
# Each fixture is a module named like the product, so the product's own config judges it at the same paths.
set -u
root=$(cd "$(dirname "$0")/.." && pwd)
total=0
rejected=0
for dir in "$root"/lint/fixtures/*/; do
	name=$(basename "$dir")
	total=$((total + 1))
	IFS='	' read -r linter pattern <"$dir/want"
	out=$(cd "$dir" && golangci-lint run --config "$root/.golangci.yml" --output.text.print-issued-lines=false --show-stats=false ./... 2>&1)
	code=$?
	issues=$(printf '%s\n' "$out" | grep -E '^[^ ]+:[0-9]+(:[0-9]+)?: ')
	others=$(printf '%s\n' "$issues" | grep -v " ($linter)$" | grep -v '^$')
	if [ "$code" -eq 1 ] && [ -z "$others" ] && printf '%s\n' "$issues" | grep " ($linter)$" | grep -qE "$pattern"; then
		rejected=$((rejected + 1))
		echo "ok   $name: rejected by $linter"
	else
		echo "FAIL $name: want only $linter matching /$pattern/, golangci-lint exit $code"
		printf '%s\n' "$out" | sed 's/^/     /'
	fi
done
echo "lint-selftest: $rejected of $total fixtures rejected"
[ "$total" -gt 0 ] && [ "$rejected" -eq "$total" ]
