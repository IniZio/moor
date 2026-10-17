#!/usr/bin/env bash
set -euo pipefail
. "$(dirname "$0")/lib/repo-root.sh"
cd_repo_root
prod=${FILE_CAP_PROD:-400}
test_cap=${FILE_CAP_TEST:-600}
bad=0
n=0
list=$(git ls-files -co --exclude-standard '*.rs')
while IFS= read -r f; do
  case "$f" in target/*|*/target/*) continue ;; esac
  [ -f "$f" ] || continue
  case "$f" in tests/*|*/tests/*|tests.rs|*/tests.rs) cap=$test_cap ;; *) cap=$prod ;; esac
  lines=$(wc -l <"$f")
  n=$((n + 1))
  if [ "$lines" -gt "$cap" ]; then
    echo "file-cap: $f $lines > $cap"
    bad=$((bad + 1))
  fi
done <<<"$list"
[ "$n" -gt 0 ] || { echo "file-cap: no .rs files found; nothing was checked" >&2; exit 2; }
[ "$bad" -eq 0 ] || exit 1
echo "file-cap: $n files ok"
