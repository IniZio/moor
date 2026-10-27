#!/bin/sh
# Opens moor's picker as a popup over the current workspace.
set -eu
exec "${HERDR_BIN_PATH:-herdr}" plugin pane open --plugin moor --entrypoint new \
  --placement popup --width 70 --height 18 --env "MOOR_HERDR_WORKSPACE=$HERDR_WORKSPACE_ID"
