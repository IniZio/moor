#!/bin/sh
# herdr runs this on a GitHub install, not on `plugin link`. It fetches the moor-herdr release
# matching this checkout, unless MOOR_HERDR_BIN names one, and leaves a default shell and the
# config snippet in the plugin's config dir, whose path survives plugin updates.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
conf=${XDG_CONFIG_HOME:-$HOME/.config}/herdr/plugins/config/moor
fail() { echo "moor herdr plugin: $*" >&2; exit 1; }

fetch() {
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
  dl="$root/.download"
  rm -rf "$dl" && mkdir -p "$dl"
  curl -fsSL -o "$dl/$asset" "$url/$asset" || fail "could not download $url/$asset"
  curl -fsSL -o "$dl/$asset.sha256" "$url/$asset.sha256" || fail "could not download $url/$asset.sha256"
  if command -v sha256sum >/dev/null; then
    (cd "$dl" && sha256sum -c "$asset.sha256" >/dev/null) || fail "$asset does not match its checksum"
  else
    (cd "$dl" && shasum -a 256 -c "$asset.sha256" >/dev/null) || fail "$asset does not match its checksum"
  fi
  tar -xzf "$dl/$asset" -C "$dl"
  mv "$dl/moor-herdr" "$root/moor-herdr"
  rm -rf "$dl"
  bin=$root/moor-herdr
}

if [ -n "${MOOR_HERDR_BIN:-}" ]; then
  bin=$MOOR_HERDR_BIN
else
  fetch
fi
[ -x "$bin" ] || fail "$bin is not an executable"
printf '%s\n' "$bin" > "$root/moor-herdr.path"

mkdir -p "$conf"
cat > "$conf/shell.sh" <<EOF
#!/bin/sh
MOOR_HERDR_ROOT='$root'
[ -f "\$MOOR_HERDR_ROOT/bin/shell.sh" ] || exec /bin/sh
. "\$MOOR_HERDR_ROOT/bin/shell.sh"
EOF
chmod 755 "$conf/shell.sh"
"$bin" herdr-setup --shell "$conf/shell.sh" > "$conf/herdr-setup.toml"
printf 'moor herdr plugin: using %s; config snippet in %s\n' "$bin" "$conf/herdr-setup.toml"
