#!/usr/bin/env bash
# Proves the git selftests leave the caller's repo alone when run with GIT_DIR exported, as under `git rebase -x`.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
unset GIT_WORK_TREE GIT_INDEX_FILE GIT_COMMON_DIR GIT_OBJECT_DIRECTORY
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null

git init -q -b main "$tmp/victim"
before="$(cat "$tmp/victim/.git/config")"
s=stack-overlap-selftest.sh
GIT_DIR="$tmp/victim/.git" "$root/tools/$s" >/dev/null 2>&1 || { echo "git-env-selftest: $s failed" >&2; exit 1; }
[ "$(cat "$tmp/victim/.git/config")" = "$before" ] || { echo "git-env-selftest: $s rewrote the caller's repo config" >&2; exit 1; }
echo "git-env-selftest: ok"
