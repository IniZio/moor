#!/bin/bash
set -euo pipefail

REPO_DIR=$(git rev-parse --show-toplevel)
D=${NESTED_E2E_DIR:-$REPO_DIR/.moor/dev/nested-e2e}
NAME=${NESTED_E2E_NAME:-nested-e2e-$(date +%s)}
GH_REPO_NAME=${NESTED_E2E_REPO:-IniZio/moor}
NESTED=${NESTED:-$HOME/.cache/moor-nested}
# The inner Box runs the repo's whole setup, nested-KVM tools included, before its agent starts.
START_TIMEOUT=${NESTED_E2E_START_TIMEOUT:-3600}
PROMPT="Create the file NESTED.md with the text ok, commit, push your branch, and open a draft PR titled 'test: nested Box check (close me)'"
BRANCH=moor/$NAME
STATE=$D/state
CTL=$D/ctl
OUT=$D/out
export MOOR_CONFIG=$D/config.yaml
export MOOR_STATE_DIR=$STATE
export XDG_CONFIG_HOME=$STATE/xdg
export ZIG=${ZIG:-$NESTED/bin/zig}
export PATH=$REPO_DIR/target/debug:$PATH

log() { printf 'nested-e2e: %s\n' "$*" | tee -a "$OUT/run.log" >&2; }

# Each run starts clean: a Box left in state from an earlier run would hold the node.
rm -rf "$STATE" "$D/ch-state" "$D/run"
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
    stateDir: $D/ch-state
    runtimeDir: $D/run
    cgroup: "off"
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

build() {
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
}

STARTED=
EVENTS_PID=
CTL_PGID=
cleanup() {
	local rc=$?
	trap - EXIT INT TERM
	set +e
	if [ -n "$STARTED" ]; then
		moor events --box "$NAME" >"$OUT/events.jsonl" 2>/dev/null
		log "closing Box $NAME"
		timeout 600 moor close "$NAME" --force >>"$OUT/run.log" 2>&1
	fi
	local pr
	pr=$(gh pr list -R "$GH_REPO_NAME" --head "$BRANCH" --state open --json url -q '.[].url' 2>/dev/null)
	for url in $pr; do
		log "closing $url"
		gh pr close "$url" -R "$GH_REPO_NAME" --delete-branch >>"$OUT/run.log" 2>&1
	done
	if gh api "repos/$GH_REPO_NAME/git/ref/heads/$BRANCH" >/dev/null 2>&1; then
		log "deleting branch $BRANCH"
		gh api -X DELETE "repos/$GH_REPO_NAME/git/refs/heads/$BRANCH" >>"$OUT/run.log" 2>&1
	fi
	[ -n "$EVENTS_PID" ] && kill "$EVENTS_PID" 2>/dev/null
	if [ -n "$CTL_PGID" ]; then
		kill -TERM -- "-$CTL_PGID" 2>/dev/null
		for _ in $(seq 1 30); do
			pgrep -g "$CTL_PGID" >/dev/null || break
			sleep 1
		done
		if pgrep -g "$CTL_PGID" >/dev/null; then
			log "killing controller group $CTL_PGID: $(pgrep -g "$CTL_PGID" | tr '\n' ' ')"
			kill -KILL -- "-$CTL_PGID" 2>/dev/null
			sleep 1
		fi
	fi
	local left
	left=$(pgrep -f "cloud-hypervisor.*$D/run|passt.*$D/run|crabbox-cloudhypervisor.*$D/ch-state")
	if [ -n "$left" ]; then
		log "killing leftover VM processes: $(echo $left)"
		kill $left
		[ "$rc" = 0 ] && rc=1
		sleep 2
	fi
	# Lease disks are per run; base images stay in $NESTED.
	rm -rf "$D/ch-state/leases" "$D/run"
	log "left running: $(pgrep -af "^([^ ]*/)?(cloud-hypervisor|passt|moor|crabbox-cloudhypervisor) " | tr '\n' ';')"
	log "exit $rc"
	exit "$rc"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

log "building with the pty surface"
build
sg kvm -c "exec setsid moor controller serve --config $CTL/controller.yaml" >"$OUT/controller.log" 2>&1 </dev/null &
for _ in $(seq 1 30); do
	[ -S "$CTL/controller.sock" ] && break
	sleep 1
done
ctl_pid=$(pgrep -f "controller serve --config $CTL/controller.yaml" | head -1)
CTL_PGID=$(ps -o pgid= -p "$ctl_pid" | tr -d ' ')
moor user enroll >/dev/null
for secret in github claude; do
	moor user secret ls | grep -q "\"$secret\"" || printf '%s' "$secret" | moor user secret set "$secret" >/dev/null
done
moor events --follow >"$OUT/events-follow.jsonl" 2>&1 &
EVENTS_PID=$!

log "starting Box $NAME on $GH_REPO_NAME"
STARTED=1
timeout "$START_TIMEOUT" moor start "$NAME" --repo "$GH_REPO_NAME" --prompt "$PROMPT" | tee "$OUT/start.txt"
timeout 120 moor run "$NAME" -- sh -c "systemd-detect-virt; grep -c -E 'vmx|svm' /proc/cpuinfo" | tee "$OUT/virt.txt"
log "waiting for the agent"
timeout 2000 moor wait "$NAME" --for done --timeout 1800
moor show "$NAME" --agent --events 100 --pane >"$OUT/show.txt"
url=$(gh pr list -R "$GH_REPO_NAME" --head "$BRANCH" --state open --json url,isDraft,title \
	-q '.[] | select(.isDraft and .title == "test: nested Box check (close me)") | .url')
if [ -z "$url" ]; then
	log "no draft PR from $BRANCH"
	exit 1
fi
echo "$url" >"$OUT/pr.txt"
log "draft PR $url"
