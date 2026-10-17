#!/usr/bin/env bash
# Fails when Seatbelt rule text or the nono seatbelt profile key appears outside the Mac profile builder.
set -euo pipefail
# Scans $1 (or $SEATBELT_TEXT_ROOT) when given, so the checks can be self-tested; defaults to the repo's crates/.
if [ -n "${1:-${SEATBELT_TEXT_ROOT:-}}" ]; then
  root="${1:-$SEATBELT_TEXT_ROOT}"
else
  cd "$(git rev-parse --show-toplevel)"
  root=crates
fi
sandbox="$root/moor/src/adapters/box_agent/sandbox"
files=$(find "$root" -name '*.rs' -not -path "$root/target/*" -not -path "$root/*/target/*" \
  -not -path "$sandbox/mac.rs" -not -path "$sandbox/mac/*" | sort)
[ -n "$files" ] || { echo "seatbelt-text: ok"; exit 0; }
# Comments count: rule text in a comment is still rule text.
# shellcheck disable=SC2086
hits=$(grep -nF -e '(allow ' -e 'unsafe_macos_seatbelt_rules' $files /dev/null \
  | sed 's/$/  <- Seatbelt rule text outside the Mac profile builder/' || true)
if [ -n "$hits" ]; then
  echo "seatbelt-text: Seatbelt rule text outside the Mac profile builder"
  echo "$hits"
  exit 1
fi
echo "seatbelt-text: ok"
