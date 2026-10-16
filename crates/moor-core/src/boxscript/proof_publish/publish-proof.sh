#!/bin/sh
# Publish .moor/proof/* to the orphan-style branch moor-proof/<task> and print Markdown for it.
# Never touches HEAD, the current branch, the working tree or the real index.
set -eu

die() {
  echo "publish-proof: $*" >&2
  exit 1
}

task=${1:-}
[ -n "$task" ] || die "usage: sh .moor/publish-proof.sh <task>"
branch="moor-proof/$task"
git check-ref-format "refs/heads/$branch" || die "bad task name: $task"

url=$(git config --get remote.moor.url) || die "no moor remote"
url=${url%/}
url=${url%.git}
case "$url" in
  https://github.com/*/*) path=${url#https://github.com/} ;;
  git@github.com:*/*) path=${url#git@github.com:} ;;
  *) die "cannot read owner/repo from $url" ;;
esac
owner=${path%%/*}
repo=${path#*/}
case "$owner$repo" in
  '' | */* | *[!A-Za-z0-9._-]*) die "cannot read owner/repo from $url" ;;
esac
[ -n "$owner" ] && [ -n "$repo" ] || die "cannot read owner/repo from $url"

tmp=$(mktemp -d "${TMPDIR:-/tmp}/moor-proof.XXXXXX") || die "mktemp failed"
trap 'rm -rf "$tmp"' EXIT
list="$tmp/files"
: >"$list"
for f in .moor/proof/*; do
  if [ -f "$f" ] && [ ! -L "$f" ]; then
    printf '%s\n' "${f##*/}" >>"$list"
  fi
done
[ -s "$list" ] || die "no regular files in .moor/proof"

index="$tmp/index"
while IFS= read -r name; do
  blob=$(git hash-object -w -- ".moor/proof/$name")
  GIT_INDEX_FILE="$index" git update-index --add --cacheinfo "100644,$blob,$name"
done <"$list"
tree=$(GIT_INDEX_FILE="$index" git write-tree)

parent=
if git fetch --quiet moor "refs/heads/$branch" 2>/dev/null; then
  parent=$(git rev-parse --verify --quiet "FETCH_HEAD^{commit}") || parent=
fi

if [ -z "$(git config user.name || true)" ]; then
  GIT_AUTHOR_NAME=moor GIT_COMMITTER_NAME=moor
  export GIT_AUTHOR_NAME GIT_COMMITTER_NAME
fi
if [ -z "$(git config user.email || true)" ]; then
  GIT_AUTHOR_EMAIL=moor@localhost GIT_COMMITTER_EMAIL=moor@localhost
  export GIT_AUTHOR_EMAIL GIT_COMMITTER_EMAIL
fi

if [ -n "$parent" ] && [ "$(git rev-parse "$parent^{tree}")" = "$tree" ]; then
  sha=$parent
elif [ -n "$parent" ]; then
  sha=$(git commit-tree "$tree" -p "$parent" -m "proof for $task")
else
  sha=$(git commit-tree "$tree" -m "proof for $task")
fi

git push --quiet moor "$sha:refs/heads/$branch"

base="https://github.com/$owner/$repo/blob/$sha"
while IFS= read -r name; do
  enc=$(printf '%s' "$name" | sed -e 's/%/%25/g' -e 's/ /%20/g' -e 's/#/%23/g' \
    -e 's/?/%3F/g' -e 's/(/%28/g' -e 's/)/%29/g' -e 's/\[/%5B/g' -e 's/\]/%5D/g')
  case "$name" in
    *.*) ext=$(printf '%s' "${name##*.}" | tr 'A-Z' 'a-z') ;;
    *) ext= ;;
  esac
  case "$ext" in
    png | jpg | jpeg | gif | webp | svg) printf '![%s](%s/%s?raw=true)\n' "$name" "$base" "$enc" ;;
    *) printf '[%s](%s/%s)\n' "$name" "$base" "$enc" ;;
  esac
done <"$list"
