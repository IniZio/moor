#!/bin/sh
set -eu
out=../internal/web/static
mkdir -p "$out/fonts"
cp node_modules/htmx.org/dist/htmx.min.js "$out/htmx.min.js"
cp node_modules/htmx-ext-sse/dist/sse.min.js "$out/htmx-ext-sse.min.js"
for f in figtree bricolage-grotesque jetbrains-mono; do
	cp "node_modules/@fontsource-variable/$f/files/$f-latin-wght-normal.woff2" "$out/fonts/"
done
