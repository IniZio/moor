#!/bin/sh
# herdr runs this on a GitHub install, not on `plugin link`, and in a checkout it moves
# afterwards. So the matching moor-herdr release and a copy of the default shell go to the
# plugin's state dir, and the config snippet naming that shell to its config dir.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
conf=${XDG_CONFIG_HOME:-$HOME/.config}/herdr/plugins/config/moor
state=${XDG_STATE_HOME:-$HOME/.local/state}/herdr/plugins/moor
fail() { echo "moor herdr plugin: $*" >&2; exit 1; }

version=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$root/../Cargo.toml" | head -n 1)
[ -n "$version" ] || fail "no version in $root/../Cargo.toml"
case $(uname -s)-$(uname -m) in
  Linux-x86_64) target=x86_64-unknown-linux-musl ;;
  Linux-aarch64 | Linux-arm64) target=aarch64-unknown-linux-musl ;;
  Darwin-arm64) target=aarch64-apple-darwin ;;
  Darwin-x86_64) target=x86_64-apple-darwin ;;
  *) fail "no moor-herdr release for $(uname -s) $(uname -m); build it and set MOOR_HERDR_BIN" ;;
esac
asset=moor-herdr-$target.tar.gz
url=${MOOR_HERDR_RELEASES:-https://github.com/IniZio/moor/releases/download}/moor-herdr-v$version
dl=$(mktemp -d)
trap 'rm -rf "$dl"' EXIT
curl -fsSL -o "$dl/$asset" "$url/$asset" || fail "could not download $url/$asset"
curl -fsSL -o "$dl/$asset.sha256" "$url/$asset.sha256" || fail "could not download $url/$asset.sha256"
if command -v sha256sum >/dev/null; then
  (cd "$dl" && sha256sum -c "$asset.sha256" >/dev/null) || fail "$asset does not match its checksum"
else
  (cd "$dl" && shasum -a 256 -c "$asset.sha256" >/dev/null) || fail "$asset does not match its checksum"
fi
tar -xzf "$dl/$asset" -C "$dl"

mkdir -p "$state/bin" "$conf"
mv "$dl/moor-herdr" "$state/bin/moor-herdr"
cp "$root/bin/shell.sh" "$state/shell.sh.new" && mv "$state/shell.sh.new" "$state/shell.sh"
"$state/bin/moor-herdr" herdr-setup --shell "$state/shell.sh" > "$conf/herdr-setup.toml"
printf 'moor herdr plugin: moor-herdr %s in %s; config snippet in %s\n' "$version" "$state/bin" "$conf/herdr-setup.toml"
