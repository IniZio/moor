#!/bin/sh
# A plain tab in the current workspace until moor's own picker takes this key.
set -eu
exec "${HERDR_BIN_PATH:-herdr}" tab create --workspace "$HERDR_WORKSPACE_ID" --focus
