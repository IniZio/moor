#!/bin/sh
# Install moor binaries from a GitHub release. Needs curl, tar and sha256sum or shasum.
#   sh install.sh [--nightly] [binary...]     default: every binary in bins below
#   sh install.sh [--nightly] --server URL [--token TOKEN] [--name NAME]
#     installs moor-machine-agent, then runs moor-machine-agent up with those flags
# MOOR_VERSION pins a tag, MOOR_CHANNEL=nightly takes the newest prerelease,
# MOOR_INSTALL_DIR sets the target directory (default ~/.local/bin).
set -eu

repo=IniZio/moor
api=https://api.github.com/repos/$repo
dir=${MOOR_INSTALL_DIR:-$HOME/.local/bin}
channel=${MOOR_CHANNEL:-stable}
version=${MOOR_VERSION:-}

fail() {
  echo "install.sh: $*" >&2
  exit 1
}

bins=
server=
token=
name=
while [ $# -gt 0 ]; do
  case $1 in
    --nightly) channel=nightly ;;
    --server | --token | --name)
      [ $# -ge 2 ] || fail "$1 needs a value"
      case $1 in
        --server) server=$2 ;;
        --token) token=$2 ;;
        --name) name=$2 ;;
      esac
      shift
      ;;
    --server=*) server=${1#*=} ;;
    --token=*) token=${1#*=} ;;
    --name=*) name=${1#*=} ;;
    -*) fail "unknown option $1" ;;
    *) bins="$bins $1" ;;
  esac
  shift
done
[ -n "$server" ] || [ -z "$token$name" ] || fail "--token and --name need --server"
if [ -z "$bins" ]; then
  if [ -n "$server" ]; then
    bins=" moor-machine-agent"
  else
    bins=" moor moor-gate moor-egress-forward moor-machine-agent moor-machine-runner"
  fi
fi

command -v curl >/dev/null 2>&1 || fail "curl is required"
command -v tar >/dev/null 2>&1 || fail "tar is required"
if command -v sha256sum >/dev/null 2>&1; then
  sha() { sha256sum "$1" | cut -d ' ' -f 1; }
elif command -v shasum >/dev/null 2>&1; then
  sha() { shasum -a 256 "$1" | cut -d ' ' -f 1; }
else
  fail "sha256sum or shasum is required"
fi

case $(uname -s) in
  Linux) os=unknown-linux-musl ;;
  Darwin) os=apple-darwin ;;
  *) fail "unsupported OS $(uname -s); releases cover Linux and macOS" ;;
esac
case $(uname -m) in
  x86_64 | amd64) arch=x86_64 ;;
  aarch64 | arm64) arch=aarch64 ;;
  *) fail "unsupported CPU $(uname -m); releases cover x86_64 and aarch64" ;;
esac
[ "$os-$arch" != apple-darwin-x86_64 ] || fail "moor has no Intel Mac build; use an Apple Silicon Mac or Linux"
target=$arch-$os

if [ -z "$version" ]; then
  if [ "$channel" = nightly ]; then
    url="$api/releases?per_page=1"
  else
    url=$api/releases/latest
  fi
  body=$(curl -fsSL "$url") || fail "could not look up the release at $url (no stable release yet? try --nightly or MOOR_VERSION=<tag>)"
  version=$(printf '%s\n' "$body" | sed -n 's/^ *"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1)
  [ -n "$version" ] || fail "no release tag found at $url"
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT HUP INT TERM
mkdir -p "$dir"

for bin in $bins; do
  asset=$bin-$target.tar.gz
  base=https://github.com/$repo/releases/download/$version
  curl -fsSL -o "$tmp/$asset" "$base/$asset" || fail "download failed: $base/$asset"
  curl -fsSL -o "$tmp/$asset.sha256" "$base/$asset.sha256" || fail "download failed: $base/$asset.sha256"
  want=$(cut -d ' ' -f 1 "$tmp/$asset.sha256")
  got=$(sha "$tmp/$asset")
  [ "$want" = "$got" ] || fail "checksum mismatch for $asset: expected $want, got $got"
  echo "sha256 ok  $got  $asset"
  mkdir "$tmp/x-$bin"
  tar -xzf "$tmp/$asset" -C "$tmp/x-$bin" "$bin" || fail "$asset does not contain $bin"
  chmod 755 "$tmp/x-$bin/$bin"
  mv -f "$tmp/x-$bin/$bin" "$dir/$bin"
  echo "installed $dir/$bin ($version)"
done

if [ -n "$server" ]; then
  agent=$dir/moor-machine-agent
  [ -x "$agent" ] || fail "$agent is not installed; leave out the binary list, or name moor-machine-agent in it"
  set -- up --server "$server"
  [ -z "$name" ] || set -- "$@" --name "$name"
  if [ -n "$token" ]; then
    MOOR_PAIR_TOKEN=$token
    export MOOR_PAIR_TOKEN
  fi
  rm -rf "$tmp"
  trap - EXIT HUP INT TERM
  echo "running: $agent $*"
  # Under curl | sh this script is stdin; up asks its one sudo question on the terminal.
  if [ ! -t 0 ] && (: </dev/tty) 2>/dev/null; then
    exec "$agent" "$@" </dev/tty
  fi
  exec "$agent" "$@"
fi

first=${bins# }
first=${first%% *}
case ":$PATH:" in
  *":$dir:"*) echo "next: $first --help" ;;
  *) echo "next: add $dir to PATH, then run $first --help" ;;
esac
