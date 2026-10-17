#!/usr/bin/env bash
# Runs stack-overlap.sh against a throwaway repo with a stub gh on PATH.
set -euo pipefail

# A caller such as `git rebase -x` exports GIT_DIR and friends; git init below would then rewrite the caller's repo.
# shellcheck disable=SC2046
unset $(git rev-parse --local-env-vars) GIT_NAMESPACE GIT_CEILING_DIRECTORIES

root="$(cd "$(dirname "$0")/.." && pwd)"
script="$root/tools/stack-overlap.sh"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
export GIT_CONFIG_GLOBAL=/dev/null GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@t GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@t

mkdir "$tmp/bin" "$tmp/nogh"
cat > "$tmp/bin/gh" <<'SH'
#!/bin/sh
if [ "$1" = auth ]; then [ -z "${STUB_UNAUTH:-}" ]; exit; fi
echo "$*" >> "$GH_LOG"
cat "$PRS_TSV"
SH
chmod +x "$tmp/bin/gh"
for t in git sort comm cat dirname rm wc sed; do ln -s "$(command -v "$t")" "$tmp/nogh/$t"; done
export PATH="$tmp/bin:$PATH" PRS_TSV="$tmp/prs.tsv" GH_LOG="$tmp/gh.log"

git init -q --bare "$tmp/origin.git"
git init -q -b main "$tmp/w"
cd "$tmp/w"
git remote add origin "$tmp/origin.git"
commit() { mkdir -p "$(dirname "$1")"; echo "$2" > "$1"; git add "$1"; git commit -qm "$1"; }
commit a.txt 1; commit b.txt 1; commit c.txt 1
git push -q origin main main:refs/heads/base0
pr() { git push -q origin "$1:refs/pull/$2/head"; }

git checkout -q -b prA main; commit a.txt 2; pr prA 1
git checkout -q -b prB prA; commit b.txt 2; pr prB 2; git push -q origin prA:refs/heads/prA
git checkout -q -b prC main; commit c.txt 2; pr prC 3
git checkout -q -b merged main; commit d.txt 1; pr merged 4
git checkout -q main; git merge -q --ff-only merged; git push -q origin main
git checkout -q -b prE main; commit a.txt 9; commit e.txt 9; pr prE 5
git checkout -q -b prF main; commit "x y.txt" 1; pr prF 6
git checkout -q -b prG main; commit sub/deep/y.txt 1; pr prG 7
git checkout -q -b stacked prA; commit e.txt 1
git checkout -q --orphan orph; git rm -rqf .; commit o.txt 1; pr orph 8
git checkout -q -b spwork main; commit "x y.txt" 2
git checkout -q -b work main; commit a.txt 3; commit b.txt 3
git fetch -q origin

row() { printf '%s\t%s\t%s\t%s\tu/%s\n' "$1" "PR $1" "$2" "$3" "$1"; }
{
  row 5 prE main; row 1 prA main; row 2 prB prA; row 3 prC main
  row 4 merged base0; row 6 prF main; row 7 prG main
} > "$PRS_TSV"

out="" rc=0
fail() { echo "stack-overlap-selftest: $name FAILED"; echo "$out"; exit 1; }
run() { out="$("$script" "$@" 2>&1)" && rc=0 || rc=$?; }
ok() {
  run "${@:2}"
  if [ "$rc" -ne 0 ] || [ "$out" != "$1" ]; then fail; fi
}
bash=$(command -v bash)
bash_run() { out="$("$bash" "$script" "$@" 2>&1)" && rc=0 || rc=$?; }
code() { run "${@:2}"; [ "$rc" -eq "$1" ] || fail; }

name=overlap; ok $'prE\ta.txt\nprA\ta.txt\nprB\tb.txt' main work
name=stacked; ok $'prE\te.txt' main stacked
name=pretty; ok $'#2 PR 2 u/2\n  b.txt' --pretty main work --files 'b.*'
name=skip-in-base; ok '' main prC --files 'd.*'
name=glob-added; ok $'prE\te.txt' main --files 'e.*'
name=glob-nested; ok $'prG\tsub/deep/y.txt' main --files 'sub/*'
name=none; ok '' main HEAD --files 'zzz*'
name=spaces; ok $'prF\tx y.txt' main spwork
name=bad-head; code 2 main nope
name=glob-no-flag; code 2 main HEAD 'a.*'
name=no-args; code 2
name=files-empty; code 2 main --files
name=no-gh; PATH="$tmp/nogh" bash_run main; [ "$rc" -eq 4 ] || fail
name=unauth; STUB_UNAUTH=1 run main; [ "$rc" -eq 4 ] || fail
name=bad-fetch; cp "$PRS_TSV" "$tmp/good.tsv"; row 99 ghost main >> "$PRS_TSV"; code 5 main; cp "$tmp/good.tsv" "$PRS_TSV"
name=parallel
git worktree add -q --detach "$tmp/w2" work
want=$'prE\ta.txt\nprA\ta.txt\nprB\tb.txt'
for _ in 1 2 3; do
  (cd "$tmp/w" && "$script" main work > "$tmp/p1.out") & p1=$!
  (cd "$tmp/w2" && "$script" main work > "$tmp/p2.out") & p2=$!
  wait "$p1" || fail
  wait "$p2" || fail
  out="$(cat "$tmp/p1.out" "$tmp/p2.out")"
  [ "$out" = "$want"$'\n'"$want" ] || fail
done
name=no-merge-base; cp "$PRS_TSV" "$tmp/good.tsv"; row 8 orph main >> "$PRS_TSV"
run main work; cp "$tmp/good.tsv" "$PRS_TSV"
if [ "$rc" -ne 5 ] || ! grep -q 'no merge base' <<<"$out"; then fail; fi
name=cleanup; [ -z "$(git for-each-ref refs/stack-overlap)" ] || fail

name=repo-arg
remote_case() {
  : > "$GH_LOG"
  git config remote.origin.url "$1"
  git config "url.$tmp/origin.git.insteadOf" "$1"
  run main
  git config --unset "url.$tmp/origin.git.insteadOf"
  [ "$rc" -eq 0 ] || fail
  if [ -n "$2" ]; then grep -q -- "--repo $2 " "$GH_LOG" || fail; else ! grep -q -- --repo "$GH_LOG" || fail; fi
}
for c in https://github.com/o/r.git=o/r https://github.com/o/r=o/r git@github.com:o/r.git=o/r \
  ssh://git@github.com/o/r.git=o/r https://tok@github.com/o/r.git=o/r git@github-work:o/r.git= \
  ssh://git@ssh.github.com:443/o/r.git= https://ghe.corp.com/o/r.git= https://notgithub.com/o/r= \
  /some/local/path.git=; do
  remote_case "${c%=*}" "${c##*=}"
done
git config remote.origin.url "$tmp/origin.git"

ln -s "$root/tools" tools
mkdir -p sub/deep; echo x > sub/deep/y.txt
name='make'
out="$(make -s -f "$root/Makefile" overlap BASE=main FILES='sub/*' 2>&1)" || fail
[ "$out" = $'#7 PR 7 u/7\n  sub/deep/y.txt' ] || fail
echo "stack-overlap-selftest: ok"
