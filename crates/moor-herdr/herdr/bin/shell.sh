#!/bin/sh
# herdr's default_shell. herdr runs it with no plugin env, so it loads the env file itself,
# and it must always end in a shell: a missing moor-herdr falls back to the user's own.
conf=${HERDR_PLUGIN_CONFIG_DIR:-${XDG_CONFIG_HOME:-$HOME/.config}/herdr/plugins/config/moor}
if [ -f "$conf/env" ]; then
  set -a; . "$conf/env"; set +a
fi
case $0 in
  -*) login=--login ;;
  *) login= ;;
esac
case $0 in
  */*) root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd) ;;
  *) root=/nonexistent ;;
esac
bin=${MOOR_HERDR_BIN:-$(cat "$root/moor-herdr.path" 2>/dev/null || command -v moor-herdr || echo "$HOME/.cargo/bin/moor-herdr")}
if [ -x "$bin" ]; then
  exec "$bin" shell $login
fi
case ${SHELL:-} in
  */shell.sh | "") SHELL=/bin/sh ;;
esac
exec "${MOOR_HERDR_HOST_SHELL:-$SHELL}"
