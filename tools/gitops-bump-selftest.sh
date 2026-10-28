#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
bump=$root/.github/scripts/gitops-bump.sh
hostkey=$root/.github/scripts/gitops-hostkey.sh
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

fail() { echo "FAIL: $*" >&2; exit 1; }

seed=$tmp/seed
mkdir -p "$seed/deploy/environments/nightly" "$seed/deploy/environments/stable"
if [ -n "${GITOPS_FIXTURE_DIR:-}" ]; then
  for c in nightly stable; do cp "$GITOPS_FIXTURE_DIR/$c/values.yaml" "$seed/deploy/environments/$c/"; done
else
  for c in nightly stable; do
    cat > "$seed/deploy/environments/$c/values.yaml" <<'YAML'
# Hand-filled values.
sidecar:
  tag: "keep-me"
image:
  repository: ghcr.io/example/moor
  tag: ""   # pinned by CD
host: ""
other:
  tag: "keep-me-too"
YAML
  done
fi
git init -q --bare -b main "$tmp/gitops.git"
git -C "$seed" init -q -b main
git -C "$seed" -c user.name=t -c user.email=t@example.com add -A
git -C "$seed" -c user.name=t -c user.email=t@example.com commit -q -m seed
git -C "$seed" push -q "$tmp/gitops.git" main

run() { TAG=$1 GITOPS_DEPLOY_KEY=k GITOPS_REPO=example/gitops GITOPS_URL=$tmp/gitops.git "$bump"; }
show() { git --git-dir="$tmp/gitops.git" show "$1:deploy/environments/$2/values.yaml"; }
tagline() { show "$1" "$2" | awk '/^image:/ { i = 1; next } /^[^[:space:]]/ { i = 0 } i && /tag:/'; }

# Unset secret or variable: one notice, success, nothing pushed.
out=$(TAG=v1.2.3 GITOPS_DEPLOY_KEY='' GITOPS_REPO=example/gitops "$bump")
grep -q '^::notice::' <<<"$out" || fail "no notice for an unset secret"
[ "$(wc -l <<<"$out")" -eq 1 ] || fail "more than one line for an unset secret"
out=$(TAG=v1.2.3 GITOPS_DEPLOY_KEY=k GITOPS_REPO='' "$bump")
grep -q '^::notice::' <<<"$out" || fail "no notice for an unset variable"

# Nightly: one commit on main, only the tag line differs.
before=$(git --git-dir="$tmp/gitops.git" rev-parse main)
run v1.2.3-nightly.4 >/dev/null
[ "$(git --git-dir="$tmp/gitops.git" rev-list --count "$before..main")" -eq 1 ] || fail "nightly is not one commit"
[ "$(git --git-dir="$tmp/gitops.git" log -1 --format=%s main)" = "chore: deploy 1.2.3-nightly.4 to nightly" ] || fail "nightly subject"
tagline main nightly | grep -q 'tag: "1.2.3-nightly.4"' || fail "nightly tag not set"
changed=$(diff <(show "$before" nightly) <(show main nightly) | grep -c '^[<>]' || true)
[ "$changed" -eq 2 ] || fail "nightly changed $changed lines, want 2"
[ "$(show "$before" stable)" = "$(show main stable)" ] || fail "nightly touched stable"
if [ -z "${GITOPS_FIXTURE_DIR:-}" ]; then
  show main nightly | grep -q 'tag: "keep-me"' || fail "sidecar tag changed"
  show main nightly | grep -q 'tag: "keep-me-too"' || fail "other tag changed"
fi

# Running the same version again changes nothing.
head=$(git --git-dir="$tmp/gitops.git" rev-parse main)
run v1.2.3-nightly.4 >/dev/null
[ "$(git --git-dir="$tmp/gitops.git" rev-parse main)" = "$head" ] || fail "repeat run pushed"

# Stable: branch bump/<version>, main untouched.
run v1.2.3 >/dev/null
[ "$(git --git-dir="$tmp/gitops.git" rev-parse main)" = "$head" ] || fail "stable moved main"
tagline bump/1.2.3 stable | grep -q 'tag: "1.2.3"' || fail "stable tag not set"
[ "$(git --git-dir="$tmp/gitops.git" log -1 --format=%s bump/1.2.3)" = "chore: deploy 1.2.3 to stable" ] || fail "stable subject"
[ "$(show main stable)" = "$(show "$before" stable)" ] || fail "stable file on main changed"
run v1.2.3 >/dev/null || fail "stable re-run failed"

if run vnext >/dev/null 2>&1; then fail "accepted a bad tag"; fi

printf 'github.com ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl\n' > "$tmp/good"
"$hostkey" "$tmp/good" "$tmp/known" || fail "rejected GitHub's key"
ssh-keygen -q -t ed25519 -N '' -f "$tmp/other"
printf 'github.com %s\n' "$(cut -d' ' -f1,2 "$tmp/other.pub")" > "$tmp/bad"
if "$hostkey" "$tmp/bad" "$tmp/known2" >/dev/null 2>&1; then fail "accepted a foreign host key"; fi
echo "gitops-bump selftest ok"
