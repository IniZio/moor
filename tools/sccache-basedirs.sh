#!/usr/bin/env bash
set -euo pipefail

command -v sccache >/dev/null || exit 0
version=$(sccache --version | awk '{print $2}')
[ "$(printf '%s\n' 0.20.0 "$version" | sort -V | head -1)" = 0.20.0 ] || exit 0

conf=${SCCACHE_CONF:-${XDG_CONFIG_HOME:-$HOME/.config}/sccache/config}
want="basedirs = [$(git worktree list --porcelain | awk '/^worktree /{printf "%s\"%s\"", sep, substr($0, 10); sep=", "}')]"
mkdir -p "$(dirname "$conf")"
touch "$conf"
grep -qxF "$want" "$conf" && exit 0

grep -v '^basedirs = ' "$conf" > "$conf.tmp" || true
printf '%s\n' "$want" | cat - "$conf.tmp" > "$conf"
rm -f "$conf.tmp"
sccache --stop-server >/dev/null 2>&1 || true
echo "sccache-basedirs: listed $(git worktree list | wc -l) worktrees in $conf"
