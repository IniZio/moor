#!/bin/sh
# Installs the pinned non-cargo tools (helm, herdr-core) into one bin directory.
# Box setup (`make setup`) and CI both call this, so the pins live only here and
# in the Makefile. Usage: tools/install-tools.sh [helm] [herdr-core]; no
# arguments installs both. MOOR_TOOLS_BIN overrides the directory, which
# defaults to ~/.cargo/bin, the directory the Box keeps in its cargo-bin volume.
set -eu

ROOT=$(cd "$(dirname "$0")/.." && pwd)
BIN=${MOOR_TOOLS_BIN:-$HOME/.cargo/bin}
HERDR_CORE_VERSION=0.1.16

case "$(uname -m)" in
x86_64 | amd64) HELM_ARCH=amd64 HERDR_ARCH=x86_64 ;;
aarch64 | arm64) HELM_ARCH=arm64 HERDR_ARCH=aarch64 ;;
*)
	echo "install-tools: unsupported machine $(uname -m)" >&2
	exit 1
	;;
esac
[ "$(uname -s)" = Linux ] || {
	echo "install-tools: only Linux is supported; install the pins by hand" >&2
	exit 1
}

# sha256 of each pinned archive, by version and arch.
helm_sha() {
	case "$1/$HELM_ARCH" in
	3.21.3/amd64) echo 15e041a93a590dce8100f39385cd98c84a765c9e36aeeb9e2dc6ff9e4769e2e0 ;;
	3.21.3/arm64) echo 67f58155079ff9ffab98ba5c88daff0ed9b542f3a4732f5dd426dde7dd0f5244 ;;
	esac
}
herdr_sha() {
	case "$1/$HERDR_ARCH" in
	0.1.16/x86_64) echo 58d6861176dc6fe905b58880d5bff07a9959be613a98e5d66b4583b04b6cfa45 ;;
	0.1.16/aarch64) echo b6573137d70baff24fb346c957a9879af6bdc4c641188cdf9e47a7b2a677bc92 ;;
	esac
}

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$BIN"

# fetch_extract url sha256 member name: download, verify, install one member as $BIN/name.
fetch_extract() {
	curl --proto '=https' --tlsv1.2 -fsSL --retry 3 --max-time 600 -o "$WORK/a.tar.gz" "$1"
	echo "$2  $WORK/a.tar.gz" | sha256sum -c - >/dev/null || {
		echo "install-tools: checksum mismatch for $1" >&2
		exit 1
	}
	rm -rf "$WORK/x" && mkdir "$WORK/x"
	tar -xzf "$WORK/a.tar.gz" -C "$WORK/x"
	install -m 0755 "$WORK/x/$3" "$BIN/$4.part"
	mv "$BIN/$4.part" "$BIN/$4"
}

install_helm() {
	v=$(make -s -C "$ROOT" print-helm)
	sha=$(helm_sha "$v")
	[ -n "$sha" ] || {
		echo "install-tools: no checksum for helm $v on $HELM_ARCH; add it to tools/install-tools.sh" >&2
		exit 1
	}
	if "$BIN/helm" version --short 2>/dev/null | grep -q "^v$v+"; then return 0; fi
	fetch_extract "https://get.helm.sh/helm-v$v-linux-$HELM_ARCH.tar.gz" "$sha" "linux-$HELM_ARCH/helm" helm
	echo "install-tools: helm $v in $BIN"
}

install_herdr_core() {
	v=$HERDR_CORE_VERSION
	sha=$(herdr_sha "$v")
	[ -n "$sha" ] || {
		echo "install-tools: no checksum for herdr-core $v on $HERDR_ARCH" >&2
		exit 1
	}
	if [ -x "$BIN/herdr-core" ] && [ -f "$BIN/.herdr-core-$v" ]; then return 0; fi
	fetch_extract "https://releases.herdr.dev/core/$v/herdr-core-linux-$HERDR_ARCH.tar.gz" "$sha" herdr-core herdr-core
	rm -f "$BIN"/.herdr-core-*
	: >"$BIN/.herdr-core-$v"
	echo "install-tools: herdr-core $v in $BIN"
}

[ $# -gt 0 ] || set -- helm herdr-core
for t in "$@"; do
	case "$t" in
	helm) install_helm ;;
	herdr-core) install_herdr_core ;;
	*)
		echo "install-tools: unknown tool $t" >&2
		exit 1
		;;
	esac
done
