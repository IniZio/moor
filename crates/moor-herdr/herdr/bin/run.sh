#!/bin/sh
# Runs one moor-herdr command; MOOR_HERDR_SOCKET may come from $HERDR_PLUGIN_CONFIG_DIR/env.
set -eu
root=${HERDR_PLUGIN_ROOT:-$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)}
bin=$(cat "$root/moor-herdr.path" 2>/dev/null || echo moor-herdr)
if [ -f "${HERDR_PLUGIN_CONFIG_DIR:-/nonexistent}/env" ]; then
  set -a; . "$HERDR_PLUGIN_CONFIG_DIR/env"; set +a
fi
exec "$bin" "$@"
