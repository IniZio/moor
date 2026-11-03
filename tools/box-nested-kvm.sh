#!/bin/sh
set -eu

CH_VERSION=v52.0-moor.1
CH_SHA256=f0052b204ea9e22b9428845ff6f22b6f7738d1b49e29e00b7e3a2856ab1f098c
PASST_REV=a9c61ffaf15347b8dfcc2347c5440e4b0e82333b
PASST_SHA256=cf4746812d3e2461f7582b7b8089591ce4e8b9d1f45beb28d9c177c365346907
GO_VERSION=go1.27.3
GO_SHA256=65efbc80fdc1fa2cd2631cad5b481777a68e8725f34e89f50fc6ac714e379b40
CRABBOX_COMMIT=8fd0d749e674b626482b19aebfa78e1cf788da43
CRABBOX_VERSION=0.79.1-moor.15
ADAPTER_REV=8eba85b61787e77c9da025c1d38ab8436022dccf

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
for pair in qemu-img:qemu-utils resize2fs:e2fsprogs rsync:rsync ssh:openssh-client git:git curl:curl make:make cc:build-essential; do
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

if ! fresh cloud-hypervisor "$CH_VERSION"; then
	fetch "https://github.com/IniZio/cloud-hypervisor/releases/download/$CH_VERSION/cloud-hypervisor-static" \
		"$CH_SHA256" "$NESTED/dl/cloud-hypervisor-$CH_VERSION"
	install -m 0755 "$NESTED/dl/cloud-hypervisor-$CH_VERSION" "$BIN/cloud-hypervisor"
	done_step cloud-hypervisor "$CH_VERSION"
fi
link_bin cloud-hypervisor

# Ubuntu's passt predates --vhost-user, so build the pinned release.
if ! fresh passt "$PASST_REV"; then
	fetch "https://passt.top/passt/snapshot/passt-$PASST_REV.tar.gz" "$PASST_SHA256" "$NESTED/dl/passt-$PASST_REV.tar.gz"
	src=$(mktemp -d)
	tar -xzf "$NESTED/dl/passt-$PASST_REV.tar.gz" -C "$src" --strip-components=1
	make -C "$src" -j"$(nproc)" passt >/dev/null
	install -m 0755 "$src/passt" "$BIN/passt"
	rm -rf "$src"
	done_step passt "$PASST_REV"
fi
link_bin passt

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

if ! fresh crabbox-cloudhypervisor "$ADAPTER_REV"; then
	toolchain=$(rustup show active-toolchain | cut -d' ' -f1)
	src=$(mktemp -d)
	git_rev "$src" https://github.com/IniZio/crabbox-cloudhypervisor "$ADAPTER_REV"
	(cd "$src" && RUSTUP_TOOLCHAIN=$toolchain timeout 1800 cargo install --path . --locked --root "$NESTED/cargo" --force)
	install -m 0755 "$NESTED/cargo/bin/crabbox-cloudhypervisor" "$BIN/crabbox-cloudhypervisor"
	rm -rf "$src"
	done_step crabbox-cloudhypervisor "$ADAPTER_REV"
fi
link_bin crabbox-cloudhypervisor

# The pty surface (`--features moor/pty-surface`) builds with Zig; the Makefile pins it.
read -r ZIG_VERSION ZIG_SHA256 <<EOF
$(make -s -C "$(dirname "$0")/.." print-zig)
EOF
if ! fresh zig "$ZIG_VERSION"; then
	fetch "https://ziglang.org/download/$ZIG_VERSION/zig-x86_64-linux-$ZIG_VERSION.tar.xz" \
		"$ZIG_SHA256" "$NESTED/dl/zig-$ZIG_VERSION.tar.xz"
	rm -rf "$NESTED/zig"
	mkdir -p "$NESTED/zig"
	tar -xJf "$NESTED/dl/zig-$ZIG_VERSION.tar.xz" -C "$NESTED/zig" --strip-components=1
	ln -sf "$NESTED/zig/zig" "$BIN/zig"
	done_step zig "$ZIG_VERSION"
fi
link_bin zig
