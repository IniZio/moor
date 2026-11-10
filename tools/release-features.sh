#!/usr/bin/env bash
set -euo pipefail
. "$(dirname "$0")/lib/repo-root.sh"
cd_repo_root
bad=0
own=$(cargo tree --locked -q -p moor -e normal,build,features --target all -i moor)
others=$(cargo tree --locked -q --workspace -e normal,build,features --target all -i moor)
files=(Dockerfile Makefile .github/workflows deploy)
for feature in test-upstream failpoints; do
  if grep -q "feature \"$feature\"" <<<"$own$others"; then
    echo "release-features: a release build of moor enables $feature"
    bad=1
  fi
  if grep -rn "$feature" "${files[@]}"; then
    echo "release-features: the release build, image or workflows above name $feature"
    bad=1
  fi
done
[ "$bad" -eq 0 ] || exit 1
echo "release-features: no release build enables test-upstream or failpoints"
