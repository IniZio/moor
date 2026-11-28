#!/usr/bin/env bash
set -euo pipefail

pinned='SHA256:+DiY3wvvV6TuJJhbpZisF/zLDA0zPMSvHdkr4UvCOqU'

scan=${1:?keyscan output file}
out=${2:?known_hosts file to write}

found=$(ssh-keygen -lf "$scan" -E sha256 | awk '$NF == "(ED25519)" { print $2 }')
if [ "$found" != "$pinned" ]; then
  echo "::error::github.com host key ${found:-missing} does not match the pinned $pinned"
  exit 1
fi
grep -E '^github\.com ssh-ed25519 ' "$scan" > "$out"
