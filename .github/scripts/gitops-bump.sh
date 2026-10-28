#!/usr/bin/env bash
# Records a release in the private deploy repository.
#   nightly tag -> commit on the default branch
#   stable tag  -> branch bump/<version>, which that repository turns into a PR
# Env: TAG, GITOPS_DEPLOY_KEY, GITOPS_REPO; GITOPS_URL replaces the SSH remote.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
tag=${TAG:?TAG is required}

if [ -z "${GITOPS_DEPLOY_KEY:-}" ] || [ -z "${GITOPS_REPO:-}" ]; then
  echo "::notice::GITOPS_DEPLOY_KEY or GITOPS_REPO is not set; skipping the deploy repository bump."
  exit 0
fi

version=${tag#v}
if ! [[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+(-nightly\.[0-9]+)?$ ]]; then
  echo "::error::$tag is not a release tag"
  exit 1
fi
if [[ $version == *-nightly.* ]]; then channel=nightly; else channel=stable; fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

url=${GITOPS_URL:-}
if [ -z "$url" ]; then
  url="git@github.com:${GITOPS_REPO}.git"
  (umask 077 && printf '%s\n' "$GITOPS_DEPLOY_KEY" > "$work/key")
  ssh-keyscan -t ed25519 github.com > "$work/scan" 2>/dev/null
  "$here/gitops-hostkey.sh" "$work/scan" "$work/known_hosts"
  export GIT_SSH_COMMAND="ssh -i $work/key -o IdentitiesOnly=yes -o StrictHostKeyChecking=yes -o UserKnownHostsFile=$work/known_hosts"
fi

git clone -q "$url" "$work/repo"
cd "$work/repo"
git config user.name 'github-actions[bot]'
git config user.email '41898282+github-actions[bot]@users.noreply.github.com'

file=deploy/environments/$channel/values.yaml
# Only the tag key inside the top-level image block changes, so comments and quoting elsewhere survive.
awk -v v="$version" '
  /^[^[:space:]#]/ { in_image = ($0 ~ /^image:[[:space:]]*(#.*)?$/) }
  in_image && !done && /^[[:space:]]+tag:/ {
    match($0, /^[[:space:]]+/)
    print substr($0, 1, RLENGTH) "tag: \"" v "\""
    done = 1
    next
  }
  { print }
  END { exit done ? 0 : 1 }
' "$file" > "$work/values.yaml" || { echo "::error::no image.tag in $file"; exit 1; }
cat "$work/values.yaml" > "$file"

if git diff --quiet; then
  echo "$file already names $version"
  exit 0
fi

if [ "$channel" = nightly ]; then
  git commit -q -am "chore: deploy $version to nightly"
  for attempt in 1 2 3; do
    git push -q origin HEAD && break
    [ "$attempt" = 3 ] && { echo "::error::could not push to the deploy repository"; exit 1; }
    git pull -q --rebase
  done
else
  git switch -q -c "bump/$version"
  git commit -q -am "chore: deploy $version to stable"
  # CD owns this branch, so a re-run of the same release replaces its earlier push.
  git push -q --force origin "bump/$version"
fi
echo "recorded $version for $channel"
