#!/bin/sh
# Startup hooks must exit, so the follower runs in the background, once per herdr server.
set -eu
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
state=${HERDR_PLUGIN_STATE_DIR:-${TMPDIR:-/tmp}}
mkdir -p "$state"
if [ -f "$state/sync.pid" ] && kill -0 "$(cat "$state/sync.pid")" 2>/dev/null; then
  exit 0
fi
nohup sh "$here/run.sh" sync --every 5 >>"$state/sync.log" 2>&1 &
echo $! > "$state/sync.pid"
