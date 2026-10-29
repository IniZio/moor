#!/bin/sh
set -eu
state=${HERDR_PLUGIN_STATE_DIR:-${XDG_STATE_HOME:-$HOME/.local/state}/herdr/plugins/moor}
if [ -f "${HERDR_PLUGIN_CONFIG_DIR:-/nonexistent}/env" ]; then
  set -a; . "$HERDR_PLUGIN_CONFIG_DIR/env"; set +a
fi
[ -x "$state/bin/moor-herdr" ] && installed=$state/bin/moor-herdr || installed=
bin=${MOOR_HERDR_BIN:-${installed:-$(command -v moor-herdr || echo "$HOME/.cargo/bin/moor-herdr")}}

# herdr closes a pane the moment its command exits, so a pane that fails at startup would
# vanish before anyone reads why.
hold() {
  printf '\nmoor: %s failed (exit %s)\n' "$1" "$2"
  tail -n 15 "$3"
  printf 'hint: check MOOR_HERDR_BIN and MOOR_HERDR_SERVER in %s/env\npress any key to close\n' \
    "${HERDR_PLUGIN_CONFIG_DIR:-the plugin config dir}"
  tty=$(stty -g 2>/dev/null) && stty -icanon -echo min 1 2>/dev/null || true
  dd bs=1 count=1 >/dev/null 2>&1 || true
  [ -z "${tty:-}" ] || stty "$tty" 2>/dev/null || true
}

case ${1:-} in
  panel | new | connect) pane=$1 ;;
  *) pane= ;;
esac

missing="moor: no moor-herdr at $bin; install the plugin with herdr plugin install, or set MOOR_HERDR_BIN in the plugin's env file"
if [ -z "$pane" ]; then
  [ -x "$bin" ] || { echo "$missing" >&2; exit 127; }
  case "$*" in
    "sync --every"*) ;;
    *) sh "$(dirname -- "$0")/follow.sh" --if-down >/dev/null 2>&1 || true ;;
  esac
  exec "$bin" "$@"
fi

err=$(mktemp)
trap 'rm -f "$err"' EXIT
code=0
if [ -x "$bin" ]; then
  sh "$(dirname -- "$0")/follow.sh" --if-down >/dev/null 2>&1 || true
  "$bin" "$@" 2>"$err" || code=$?
else
  echo "$missing" >"$err"
  code=127
fi
[ "$code" -eq 0 ] || hold "$pane" "$code" "$err"
exit "$code"
