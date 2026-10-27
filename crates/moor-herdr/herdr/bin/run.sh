#!/bin/sh
set -eu
root=${HERDR_PLUGIN_ROOT:-$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)}
if [ -f "${HERDR_PLUGIN_CONFIG_DIR:-/nonexistent}/env" ]; then
  set -a; . "$HERDR_PLUGIN_CONFIG_DIR/env"; set +a
fi
bin=${MOOR_HERDR_BIN:-$(cat "$root/moor-herdr.path" 2>/dev/null || command -v moor-herdr || echo "$HOME/.cargo/bin/moor-herdr")}
[ -x "$bin" ] || {
  echo "moor: no moor-herdr at $bin; run cargo install --path crates/moor-herdr, or set MOOR_HERDR_BIN in the plugin's env file" >&2
  exit 127
}
exec "$bin" "$@"
