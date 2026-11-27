#!/usr/bin/env bash
set -eu
V=${MVL_BIN:-$HOME/.claude/skills/verify-moor-live/bin}
. "$V/env.sh"
name=$(mvl_slug "${2:?usage: instance.sh up NAME WORKTREE | down NAME}")
mvl_paths "$name"
. "$V/portal-lib.sh"
portal_paths "$name"
case $1 in
  up)
    wt=$(cd "${3:?no worktree}" && pwd)
    MVL_BUILD_EXTRA_PKGS=moor-gate "$V/build" "$wt"
    portal_ports "$name"
    ctl=$(mktemp)
    usr=$(mktemp)
    trap 'rm -f "$ctl" "$usr"' EXIT
    printf 'fronts:\n  authgear:\n    step_reports: true\n' >"$ctl"
    printf 'moor_service:\n  upstream: 127.0.0.1:%s\n' "$BOX_PORT" >"$usr"
    MVL_CTL_EXTRA=$ctl MVL_USER_EXTRA=$usr MVL_PREBUILT=1 "$V/up" "$wt" "$name"
    mkdir -p "$PD"
    portal_write_json
    portal_dex_up || mvl_die "Dex did not come up on 127.0.0.1:$DEX_PORT"
    portal_gate_up "$wt/target/debug/moor-gate" 1 || mvl_die "gate did not come up; see $PD/gate.log"
    echo "instance $name: controller $(mvl_ctl_pid), gate $(portal_gate_pid) on $PORTAL_URL"
    ;;
  down) exec "$V/portal-down" "$name" ;;
  *) mvl_die "usage: instance.sh up NAME WORKTREE | down NAME" ;;
esac
