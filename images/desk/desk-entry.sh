#!/bin/bash.real
set -e
mkdir -p /desk/log
hc server start >/desk/log/hc-server.log 2>&1
hc run --bg --name task --no-parent --keep --cwd /workspace bash.real -c 'BOX_SSH='"$HARNESS_BOX_SSH"' harness; exec bash.real' >/desk/log/hc-run.log 2>&1
exec sleep infinity
