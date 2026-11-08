#!/bin/bash
set -euo pipefail

REPO_DIR=$(git rev-parse --show-toplevel)
D=${NESTED_E2E_DIR:-$REPO_DIR/.moor/dev/nested-e2e}
NAME=${NESTED_E2E_NAME:-nested-e2e-$(date +%s)}
# A repo without .moor.yaml, so the inner Box has no setup to run.
GH_REPO_NAME=${NESTED_E2E_REPO:-IniZio/moor-nested-check}
NESTED=${NESTED:-$HOME/.cache/moor-nested}
START_TIMEOUT=${NESTED_E2E_START_TIMEOUT:-600}
PROMPT="Create the file NESTED.md with the text ok, commit, and push your branch"
BRANCH=
STATE=$D/state
CTL=$D/ctl
OUT=$D/out
export MOOR_CONFIG=$D/config.yaml
export MOOR_STATE_DIR=$STATE
export XDG_CONFIG_HOME=$STATE/xdg
export ZIG=${ZIG:-$NESTED/bin/zig}
export PATH=$REPO_DIR/target/debug:$PATH

log() { printf 'nested-e2e: %s\n' "$*" | tee -a "$OUT/run.log" >&2; }

ours() {
	local pid
	for pid in $(pgrep -f "^([^ ]*/)?(cloud-hypervisor|passt|moor|crabbox-cloudhypervisor) "); do
		if grep -qF "$D/" "/proc/$pid/cmdline" 2>/dev/null ||
			tr '\0' '\n' <"/proc/$pid/environ" 2>/dev/null | grep -qxF "MOOR_STATE_DIR=$STATE"; then
			echo "$pid"
		fi
	done
}

# stop_ours: TERM, then KILL whatever is still up after 15s.
stop_ours() {
	local pids
	pids=$(ours)
	[ -z "$pids" ] && return 0
	# shellcheck disable=SC2086
	kill $pids 2>/dev/null || true
	for _ in $(seq 1 15); do
		[ -z "$(ours)" ] && return 0
		sleep 1
	done
	pids=$(ours)
	log "killing $(echo $pids)"
	# shellcheck disable=SC2086
	kill -KILL $pids 2>/dev/null || true
	sleep 1
}

mkdir -p "$OUT"
# A run killed outright (SIGKILL, reboot) leaves its controller and VM behind.
stop_ours
# Each run starts clean: a Box left in state from an earlier run would hold the node.
rm -rf "$STATE" "$D/ch-state" "$D/run" "$OUT"
mkdir -p "$STATE" "$CTL" "$OUT" "$D/ch-state" "$D/run" "$XDG_CONFIG_HOME/crabbox"
: >"$OUT/run.log"
chmod 700 "$STATE" "$CTL" "$XDG_CONFIG_HOME"
dir=$XDG_CONFIG_HOME/crabbox
while [ "$dir" != "$REPO_DIR" ] && [ "$dir" != / ]; do
	chmod go-w "$dir"
	dir=$(dirname "$dir")
done

cat >"$XDG_CONFIG_HOME/crabbox/config.yaml" <<EOF
provider: external
target: linux
external:
  command: $(command -v crabbox-cloudhypervisor)
  capabilities:
    idempotentLeaseId: true
    checkpoints: true
  connection:
    ssh:
      trustProviderOutput: true
  config:
    cpus: 2
    memoryMiB: 4096
    classes:
      standard: {cpus: 2, memoryMiB: 4096}
      fast: {cpus: 2, memoryMiB: 4096}
      large: {cpus: 2, memoryMiB: 4096}
      beast: {cpus: 2, memoryMiB: 4096}
    cacheDir: $NESTED
    cloudHypervisor: $NESTED/bin/cloud-hypervisor
    passt: $NESTED/bin/passt
    stateDir: $D/ch-state
    runtimeDir: $D/run
    cgroup: "off"
    bootTemplate: true
EOF
chmod 600 "$XDG_CONFIG_HOME/crabbox/config.yaml"


cat >"$MOOR_CONFIG" <<EOF
nodes:
  local:
    runtime: external
    max_boxes: 1
surface:
  kind: pty
controller:
  socket: $CTL/controller.sock
EOF
cat >"$CTL/controller.yaml" <<EOF
socket: $CTL/controller.sock
state_dir: $STATE
EOF

# built: target/debug/moor is newer than every tracked file.
built() {
	local bin=$REPO_DIR/target/debug/moor
	[ -x "$bin" ] || return 1
	[ -z "$(cd "$REPO_DIR" && git ls-files -z |
		xargs -0 sh -c 'bin=$1; shift; find -L "$@" -maxdepth 0 -newer "$bin" -print -quit' sh "$bin" 2>/dev/null)" ]
}

build() {
	if built; then
		log "build skipped: target/debug/moor is newer than every tracked file"
		return 0
	fi
	log "building with the pty surface"
	local tries=0
	until timeout 2400 cargo build --workspace --locked --features moor/pty-surface >"$OUT/build.log" 2>&1; do
		tries=$((tries + 1))
		local src
		src=$(ls -td "$REPO_DIR"/target/debug/build/ghostty-vt-sys-*/out/src 2>/dev/null | head -1)
		if [ "$tries" -gt 30 ] || [ -z "$src" ] || ! "$REPO_DIR/tools/zig-seed.sh" "$src" "$OUT/build.log"; then
			log "build failed, see $OUT/build.log"
			return 1
		fi
	done
	# Cargo leaves the binary alone when no Rust changed, so mark it current.
	touch "$REPO_DIR/target/debug/moor"
}

open_prs() {
	local prs
	prs=$(timeout 60 gh api "repos/$GH_REPO_NAME/pulls?state=open&head=${GH_REPO_NAME%%/*}:$BRANCH" --jq '.[].number' 2>/dev/null) && printf '%s\n' "$prs"
}

remote_branch() {
	timeout 60 git ls-remote "https://github.com/$GH_REPO_NAME.git" "refs/heads/$BRANCH" 2>/dev/null
}

# moor names the branch moor/<id>-<name>; ask it rather than guess the id.
box_branch() {
	timeout 60 moor show "$NAME" --quiet 2>/dev/null | sed -n 's/^branch: *\(moor\/.*\)$/\1/p'
}

STARTED=
cleanup() {
	local rc=$?
	# Finish cleaning up even when a second Ctrl-C arrives.
	trap '' INT TERM
	trap - EXIT
	set +e
	if [ -n "$STARTED" ]; then
		[ -n "$BRANCH" ] || BRANCH=$(box_branch)
		timeout 60 moor events --box "$NAME" >"$OUT/events.jsonl" 2>/dev/null
		log "closing Box $NAME"
		# A Box still starting refuses to close; stop_ours then takes its VM down.
		timeout 120 moor close "$NAME" --force >>"$OUT/run.log" 2>&1
	fi
	local n
	[ -n "$BRANCH" ] && for n in $(open_prs); do
		log "closing PR #$n"
		timeout 60 gh api -X PATCH "repos/$GH_REPO_NAME/pulls/$n" -f state=closed 2>>"$OUT/run.log" >/dev/null
	done
	if [ -n "$BRANCH" ] && [ -n "$(remote_branch)" ]; then
		log "deleting branch $BRANCH"
		timeout 120 git push -q "https://github.com/$GH_REPO_NAME.git" --delete "$BRANCH" >>"$OUT/run.log" 2>&1
	fi
	if [ -n "$BRANCH" ] && { [ -n "$(open_prs)" ] || [ -n "$(remote_branch)" ]; }; then
		log "PR or branch $BRANCH is still on $GH_REPO_NAME"
		[ "$rc" = 0 ] && rc=1
	fi
	stop_ours
	# Lease disks are per run; base images stay in $NESTED.
	rm -rf "$D/ch-state/leases" "$D/run"
	log "left running: $(ours | tr '\n' ' ')"
	log "exit $rc"
	exit "$rc"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

build
sg kvm -c "exec setsid moor controller serve --config $CTL/controller.yaml" >"$OUT/controller.log" 2>&1 </dev/null &
for _ in $(seq 1 30); do
	[ -S "$CTL/controller.sock" ] && break
	sleep 1
done
moor user enroll >/dev/null
for secret in github claude; do
	moor user secret ls | grep -q "\"$secret\"" || printf '%s' "$secret" | moor user secret set "$secret" >/dev/null
done
moor events --follow >"$OUT/events-follow.jsonl" 2>&1 &

log "starting Box $NAME on $GH_REPO_NAME"
STARTED=1
timeout --foreground "$START_TIMEOUT" moor start "$NAME" --repo "$GH_REPO_NAME" --prompt "$PROMPT" | tee "$OUT/start.txt"
timeout --foreground 120 moor run "$NAME" -- sh -c "systemd-detect-virt; grep -c -E 'vmx|svm' /proc/cpuinfo" | tee "$OUT/virt.txt"
log "waiting for the agent"
timeout --foreground 2000 moor wait "$NAME" --for done --timeout 1800
moor show "$NAME" --agent --events 100 --pane >"$OUT/show.txt"
BRANCH=$(box_branch)
if [ -z "$BRANCH" ]; then
	log "moor show gave no branch for $NAME"
	exit 1
fi
rm -rf "$D/check"
git init -q "$D/check"
nested=$(timeout 60 git -C "$D/check" fetch -q --depth=1 "https://github.com/$GH_REPO_NAME.git" "refs/heads/$BRANCH" 2>>"$OUT/run.log" &&
	git -C "$D/check" show FETCH_HEAD:NESTED.md 2>>"$OUT/run.log") || nested=
if [ "$nested" != ok ]; then
	log "NESTED.md on $BRANCH is not ok: ${nested:-missing}"
	exit 1
fi
log "pushed $BRANCH with NESTED.md ok"
