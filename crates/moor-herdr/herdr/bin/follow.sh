#!/bin/sh
# Startup hooks must exit, so the follower runs in the background, once per herdr server.
# --if-down leaves a running follower alone: herdr runs startup hooks only at server start.
set -eu
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
state=${HERDR_PLUGIN_STATE_DIR:-${TMPDIR:-/tmp}}
mkdir -p "$state"
old=$(cat "$state/sync.pid" 2>/dev/null || true)
if [ -n "$old" ] && ps -o args= -p "$old" 2>/dev/null | grep -q 'moor-herdr.* sync --every'; then
  [ "${1:-}" = --if-down ] && exit 0
  kill "$old"
fi
nohup sh "$here/run.sh" sync --every 5 >>"$state/sync.log" 2>&1 &
echo $! > "$state/sync.pid"
