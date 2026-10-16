#!/usr/bin/env bash
# Lists open PRs that touch the work's files and are not yet in BASE or HEAD.
# usage: stack-overlap.sh [--pretty] BASE [HEAD] [--files glob...]
# stdout: "<branch>\t<file>" per overlapping file; --pretty: "#N title url" and paths.
set -euo pipefail

die() { echo "stack-overlap: $2" >&2; exit "$1"; }

pretty=0 files=0 pos=() globs=()
while [ "$#" -gt 0 ]; do
  case "$1" in
    --pretty) pretty=1 ;;
    --files) files=1 ;;
    -*) die 2 "unknown option $1" ;;
    *) if [ "$files" -eq 1 ]; then globs+=("$1"); else pos+=("$1"); fi ;;
  esac
  shift
done
if [ "${#pos[@]}" -lt 1 ] || [ "${#pos[@]}" -gt 2 ]; then
  die 2 "usage: $0 [--pretty] BASE [HEAD] [--files glob...] (globs go after --files)"
fi
[ "$files" -eq 0 ] || [ "${#globs[@]}" -gt 0 ] || die 2 "--files needs at least one glob"
base=${pos[0]}
head=${pos[1]:-HEAD}

cd "$(git rev-parse --show-toplevel)"
for r in "$base" "$head"; do
  git rev-parse --verify -q "$r^{commit}" >/dev/null || die 2 "unknown ref $r"
done
command -v gh >/dev/null || die 4 "gh not found on PATH"
gh auth status >/dev/null 2>&1 || die 4 "gh is not authenticated; run gh auth login"
ns=refs/stack-overlap/$$
trap 'git for-each-ref --format="delete %(refname)" "$ns" | git update-ref --stdin' EXIT

origin_url=$(git config --get remote.origin.url || true)
ghargs=()
if [[ $origin_url =~ ^(https?://([^@/]+@)?|ssh://[^@/]+@|git@)github\.com[:/](.+)$ ]]; then
  repo=${BASH_REMATCH[3]%.git}
  if [[ $repo =~ ^[^/]+/[^/]+$ ]]; then ghargs=(--repo "$repo"); fi
fi

limit=1000
prs=$(gh pr list "${ghargs[@]}" --state open --limit "$limit" \
  --json number,title,headRefName,baseRefName,url \
  --jq '.[] | [.number,.title,.headRefName,.baseRefName,.url] | @tsv') \
  || die 4 "gh pr list failed"
rows=()
refspecs=()
while IFS=$'\t' read -r n title branch pbase url; do
  [ -n "$n" ] || continue
  rows+=("$n"$'\t'"$title"$'\t'"$branch"$'\t'"$pbase"$'\t'"$url")
  refspecs+=("+refs/pull/$n/head:$ns/pr/$n" "+refs/heads/$pbase:$ns/base/$pbase")
done <<<"$prs"
if [ "${#rows[@]}" -ge "$limit" ]; then
  echo "stack-overlap: $limit PRs listed; older open PRs may be missing" >&2
fi
if [ "${#rows[@]}" -gt 0 ]; then
  git fetch -q origin "${refspecs[@]}" || die 5 "cannot fetch PR heads from origin"
fi

anchor=$base
best=$(git rev-list --count "$base..$head")
open=()
for row in ${rows[@]+"${rows[@]}"}; do
  IFS=$'\t' read -r n _ <<<"$row"
  tip=$ns/pr/$n
  if git merge-base --is-ancestor "$tip" "$base"; then continue; fi
  if git merge-base --is-ancestor "$tip" "$head"; then
    c=$(git rev-list --count "$tip..$head")
    if [ "$c" -lt "$best" ]; then best=$c; anchor=$tip; fi
    continue
  fi
  open+=("$row")
done

mine=""
if [ "$files" -eq 0 ]; then mine=$(git diff --name-only "$anchor...$head"); fi

for row in ${open[@]+"${open[@]}"}; do
  IFS=$'\t' read -r n title branch pbase url <<<"$row"
  tip=$ns/pr/$n
  mb=$(git merge-base "$ns/base/$pbase" "$tip") || die 5 "no merge base for PR #$n"
  changed=$(git diff --name-only "$mb..$tip")
  if [ "$files" -eq 1 ]; then
    hits=""
    while IFS= read -r f; do
      for g in "${globs[@]}"; do
        # shellcheck disable=SC2254
        case "$f" in $g) hits+="$f"$'\n'; break ;; esac
      done
    done <<<"$changed"
    hits=${hits%$'\n'}
  else
    hits=$(comm -12 <(sort -u <<<"$mine") <(sort -u <<<"$changed"))
  fi
  [ -n "$hits" ] || continue
  if [ "$pretty" -eq 1 ]; then
    echo "#$n $title $url"
    while IFS= read -r f; do printf '  %s\n' "$f"; done <<<"$hits"
  else
    while IFS= read -r f; do printf '%s\t%s\n' "$branch" "$f"; done <<<"$hits"
  fi
done
