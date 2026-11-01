#!/bin/sh
set -eu
src=$1
log=$2
line=$(grep -m1 -E 'build\.zig\.zon:[0-9]+:[0-9]+: error' "$log" | sed 's/^ *//') || exit 1
zon=$src/${line%%:*}
at=$(echo "$line" | cut -d: -f2)
field() { sed -n "$1p" "$zon" | sed -E 's/.*"(.*)".*/\1/'; }
case $line in
*"hash mismatch"*)
	hash=$(echo "$line" | sed -E 's/.*declares ([^ ]+) but.*/\1/')
	url=$(field $((at - 1)))
	;;
*)
	url=$(field "$at")
	hash=$(sed -n "$((at + 1)),$((at + 3))p" "$zon" | grep -m1 '\.hash' | sed -E 's/.*"(.*)".*/\1/')
	;;
esac
[ -n "$hash" ] && [ -n "$url" ] || exit 1
echo "zig-seed: $hash <- $url"
dest=$src/zig-pkg/$hash
rm -rf "$dest"
mkdir -p "$dest"
case $url in
git+*)
	repo=${url#git+}
	git -C "$dest" init -q
	timeout 300 git -C "$dest" fetch -q --depth 1 "${repo%%#*}" "${repo##*#}"
	git -C "$dest" checkout -q FETCH_HEAD
	rm -rf "$dest/.git"
	;;
*)
	archive=$(mktemp)
	timeout 300 curl --proto '=https' --tlsv1.2 -fsSL --retry 3 -o "$archive" "$url"
	tar -xf "$archive" -C "$dest" --strip-components=1
	rm -f "$archive"
	;;
esac
