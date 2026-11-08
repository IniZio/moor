#!/bin/sh
set -eu

GO_VERSION=go1.27.3
GO_SHA256=65efbc80fdc1fa2cd2631cad5b481777a68e8725f34e89f50fc6ac714e379b40
CRABBOX_COMMIT=ee5e5205c9d95f54f7420bed587bc88b91efeeb4
CRABBOX_VERSION=0.79.1-moor.17
ADAPTER_VERSION=v0.2.1

NESTED=${NESTED:-$HOME/.cache/moor-nested}
BIN=$NESTED/bin
STAMP=$NESTED/stamp
mkdir -p "$BIN" "$STAMP" "$NESTED/dl"

as_root() {
	if [ "$(id -u)" = 0 ]; then "$@"; else sudo -n "$@"; fi
}

fetch() { # url sha256 dest
	[ -s "$3" ] && [ "$(sha256sum "$3" | cut -d' ' -f1)" = "$2" ] && return 0
	timeout 600 curl --proto '=https' --tlsv1.2 -fsSL --retry 3 -o "$3.part" "$1"
	[ "$(sha256sum "$3.part" | cut -d' ' -f1)" = "$2" ] || {
		echo "checksum mismatch for $1" >&2
		rm -f "$3.part"
		return 1
	}
	mv "$3.part" "$3"
}

fresh() { # name version
	[ "$(cat "$STAMP/$1" 2>/dev/null)" = "$2" ]
}

done_step() {
	printf '%s' "$2" >"$STAMP/$1"
}

link_bin() {
	as_root ln -sf "$BIN/$1" "/usr/local/bin/$1"
}

# Packages the adapter and the inner VM need.
need=""
for pair in qemu-img:qemu-utils resize2fs:e2fsprogs rsync:rsync ssh:openssh-client git:git curl:curl; do
	command -v "${pair%%:*}" >/dev/null || need="$need ${pair#*:}"
done
if [ -n "$need" ]; then
	as_root apt-get update -qq
	# shellcheck disable=SC2086
	as_root env DEBIAN_FRONTEND=noninteractive apt-get install -y -qq $need
fi

if [ -e /dev/kvm ] && [ "$(id -u)" != 0 ]; then
	user=$(id -un)
	id -nG "$user" | tr ' ' '\n' | grep -qx kvm || as_root usermod -aG kvm "$user"
fi

# git_rev dir url rev: a checkout of exactly rev, without history.
git_rev() {
	rm -rf "$1"
	mkdir -p "$1"
	git -C "$1" init -q
	timeout 300 git -C "$1" fetch -q --depth 1 "$2" "$3"
	git -C "$1" checkout -q FETCH_HEAD
}

if ! fresh crabbox "$CRABBOX_COMMIT-$CRABBOX_VERSION"; then
	fetch "https://go.dev/dl/$GO_VERSION.linux-amd64.tar.gz" "$GO_SHA256" "$NESTED/dl/$GO_VERSION.linux-amd64.tar.gz"
	gosrc=$(mktemp -d)
	tar -xzf "$NESTED/dl/$GO_VERSION.linux-amd64.tar.gz" -C "$gosrc"
	src=$(mktemp -d)
	git_rev "$src" https://github.com/IniZio/crabbox "$CRABBOX_COMMIT"
	(cd "$src" && PATH="$gosrc/go/bin:$PATH" GOTOOLCHAIN=local timeout 900 go build -trimpath -ldflags "-X github.com/openclaw/crabbox/internal/cli.version=$CRABBOX_VERSION" -o "$BIN/crabbox" ./cmd/crabbox)
	rm -rf "$src" "$gosrc"
	done_step crabbox "$CRABBOX_COMMIT-$CRABBOX_VERSION"
fi
link_bin crabbox

if ! fresh crabbox-cloudhypervisor "$ADAPTER_VERSION"; then
	timeout 600 curl --proto '=https' --tlsv1.2 -fsSL --retry 3 -o "$NESTED/dl/adapter-install.sh" \
		"https://github.com/IniZio/crabbox-cloudhypervisor/releases/download/$ADAPTER_VERSION/install.sh"
	CRABBOX_CLOUDHYPERVISOR_INSTALL_DIR=$BIN CRABBOX_CLOUDHYPERVISOR_VERSION=$ADAPTER_VERSION \
		sh "$NESTED/dl/adapter-install.sh"
	XDG_DATA_HOME=$NESTED/data CRABBOX_CONFIG=$NESTED/crabbox-install.yaml \
		"$BIN/crabbox-cloudhypervisor" install
	done_step crabbox-cloudhypervisor "$ADAPTER_VERSION"
fi
for b in cloud-hypervisor passt; do ln -sf "$NESTED/data/crabbox-cloudhypervisor/bin/$b" "$BIN/$b"; done
link_bin cloud-hypervisor
link_bin passt
link_bin crabbox-cloudhypervisor
