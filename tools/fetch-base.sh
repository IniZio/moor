#!/bin/sh
# Makes sure the migration-number check's base ref exists. A Box clone may hold
# only the branch it started on; fetch origin/<branch> when it is missing, and
# say plainly what to do when that is not possible.
# Usage: tools/fetch-base.sh [origin/develop]
set -eu

base=${1:-origin/develop}
git rev-parse --verify --quiet "$base^{commit}" >/dev/null && exit 0

case "$base" in
origin/*)
	branch=${base#origin/}
	echo "fetch-base: $base is missing; fetching it" >&2
	# Only a shallow clone gets a one-commit fetch; a full clone must not gain a shallow boundary.
	depth=
	[ "$(git rev-parse --is-shallow-repository)" != true ] || depth=--depth=1
	if git fetch --no-tags $depth origin "+refs/heads/$branch:refs/remotes/$base"; then exit 0; fi
	echo "fetch-base: could not fetch $branch from origin; fetch it, or set BASE to a ref that exists" >&2
	;;
*)
	echo "fetch-base: $base does not exist; set BASE to a ref that exists" >&2
	;;
esac
exit 1
