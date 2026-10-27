#!/bin/sh
# Records which moor-herdr the plugin runs: $MOOR_HERDR_BIN, else the one on PATH.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
bin=${MOOR_HERDR_BIN:-$(command -v moor-herdr || true)}
[ -n "$bin" ] || { echo "moor herdr plugin: put moor-herdr on PATH or set MOOR_HERDR_BIN" >&2; exit 1; }
printf '%s\n' "$bin" > "$root/moor-herdr.path"
printf 'moor herdr plugin: using %s\n' "$bin"
