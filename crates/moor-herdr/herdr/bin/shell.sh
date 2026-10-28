#!/bin/sh
# herdr's default_shell. herdr runs it with no plugin env, so it loads the env file itself,
# and it must always end in a shell: a missing moor-herdr falls back to the user's own.
conf=${HERDR_PLUGIN_CONFIG_DIR:-${XDG_CONFIG_HOME:-$HOME/.config}/herdr/plugins/config/moor}
state=${HERDR_PLUGIN_STATE_DIR:-${XDG_STATE_HOME:-$HOME/.local/state}/herdr/plugins/moor}
if [ -f "$conf/env" ]; then
  set -a; . "$conf/env"; set +a
fi
case $0 in
  -*) login=--login ;;
  *) login= ;;
esac
[ -x "$state/bin/moor-herdr" ] && installed=$state/bin/moor-herdr || installed=
bin=${MOOR_HERDR_BIN:-${installed:-$(command -v moor-herdr || echo "$HOME/.cargo/bin/moor-herdr")}}
if [ -x "$bin" ]; then
  exec "$bin" shell $login
fi
case ${SHELL:-} in
  */shell.sh | "") SHELL=/bin/sh ;;
esac
exec "${MOOR_HERDR_HOST_SHELL:-$SHELL}"
