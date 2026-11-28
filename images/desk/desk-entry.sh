#!/bin/bash.real
set -e
mkdir -p /desk/log
hc server start >/desk/log/hc-server.log 2>&1
exec sleep infinity
