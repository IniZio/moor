#!/bin/sh
set -eu
state=${HERDR_PLUGIN_STATE_DIR:-${XDG_STATE_HOME:-$HOME/.local/state}/herdr/plugins/moor}
if [ -f "${HERDR_PLUGIN_CONFIG_DIR:-/nonexistent}/env" ]; then
  set -a; . "$HERDR_PLUGIN_CONFIG_DIR/env"; set +a
fi
[ -x "$state/bin/moor-herdr" ] && installed=$state/bin/moor-herdr || installed=
bin=${MOOR_HERDR_BIN:-${installed:-$(command -v moor-herdr || echo "$HOME/.cargo/bin/moor-herdr")}}
[ -x "$bin" ] || {
  echo "moor: no moor-herdr at $bin; install the plugin with herdr plugin install, or set MOOR_HERDR_BIN in the plugin's env file" >&2
  exit 127
}
case "$*" in
  "sync --every"*) ;;
  *) sh "$(dirname -- "$0")/follow.sh" --if-down >/dev/null 2>&1 || true ;;
esac
exec "$bin" "$@"
