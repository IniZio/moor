#!/bin/sh
# Opens moor's Boxes panel as a split beside the current pane.
set -eu
exec "${HERDR_BIN_PATH:-herdr}" plugin pane open --plugin moor --entrypoint panel \
  --placement split --env "MOOR_HERDR_WORKSPACE=$HERDR_WORKSPACE_ID"
