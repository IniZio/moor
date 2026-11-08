#!/usr/bin/env bash
set -euo pipefail
. "$(dirname "$0")/lib/repo-root.sh"
cd_repo_root
bad=0
own=$(cargo tree --locked -q -p moor -e normal,build,features --target all -i moor)
others=$(cargo tree --locked -q --workspace -e normal,build,features --target all -i moor)
if grep -q 'feature "test-upstream"' <<<"$own$others"; then
  echo "release-features: a release build of moor enables the feature"
  bad=1
fi
files=(Dockerfile Makefile .github/workflows deploy)
if grep -rn 'test-upstream' "${files[@]}"; then
  echo "release-features: the release build, image or workflows above name the feature"
  bad=1
fi
[ "$bad" -eq 0 ] || exit 1
echo "release-features: no release build enables it"
