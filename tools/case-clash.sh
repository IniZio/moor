#!/usr/bin/env bash
set -euo pipefail
. "$(dirname "$0")/lib/repo-root.sh"

# clashes: reads paths on stdin and prints each group that shares a name once case is ignored.
clashes() {
  awk '
    { low = tolower($0); print low "\t" $0 }
    /\.(tsx?|jsx?|mjs)$/ { s = $0; sub(/\.[^.\/]*$/, "", s); print "stem:" tolower(s) "\t" $0 }
  ' | sort | awk -F'\t' '
    $1 == prev { if (!shown) print "case-clash: " first; print "case-clash: " $2; shown = 1; next }
    { prev = $1; first = $2; shown = 0 }
  '
}

if [ "${1:-}" = "--selftest" ]; then
  out=$(printf '%s\n' a/Markdown.tsx a/markdown.ts a/Other.rs | clashes)
  [ "$(printf '%s\n' "$out" | wc -l)" -eq 2 ] || { echo "case-clash selftest: stem clash missed" >&2; exit 1; }
  out=$(printf '%s\n' a/Readme.md a/README.md | clashes)
  [ -n "$out" ] || { echo "case-clash selftest: case-only clash missed" >&2; exit 1; }
  out=$(printf '%s\n' a/Markdown.tsx b/markdown.ts a/plan.ts a/plan.css | clashes)
  [ -z "$out" ] || { echo "case-clash selftest: false positive" >&2; exit 1; }
  echo "case-clash selftest: ok"
  exit 0
fi

cd_repo_root
found=$(git ls-files -co --exclude-standard | clashes)
if [ -n "$found" ]; then
  printf '%s\n' "$found" >&2
  echo "case-clash: rename one of each group; macOS cannot hold both" >&2
  exit 1
fi
echo "case-clash: ok"
