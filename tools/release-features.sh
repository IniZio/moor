#!/usr/bin/env bash
# Fails when a release build of moor enables the `test-upstream` feature, which lets a Box Proxy
# trust a local test upstream. Only test builds get it, through moor's dev-dependency on itself.
set -euo pipefail
. "$(dirname "$0")/lib/repo-root.sh"
cd_repo_root
bad=0
# The features a release build resolves for moor: no dev-dependencies, the features release builds
# pass, and whatever another workspace crate asks of moor. `cargo tree` cannot take `moor/<feature>`
# beside moor's dev-dependency on itself, so the two are asked apart.
own=$(cargo tree --locked -q -p moor -e normal,build,features --target all -i moor --features "${TEST_UPSTREAM_FEATURES:-pty-surface}")
others=$(cargo tree --locked -q --workspace -e normal,build,features --target all -i moor)
if grep -q 'feature "test-upstream"' <<<"$own$others"; then
  echo "release-features: a release build of moor enables the feature"
  bad=1
fi
files=(Dockerfile Makefile .github/workflows/*.yaml deploy)
if grep -rn 'test-upstream' "${files[@]}"; then
  echo "release-features: the release build, image or workflows above name the feature"
  bad=1
fi
[ "$bad" -eq 0 ] || exit 1
echo "release-features: no release build enables it"
