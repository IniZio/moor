#!/usr/bin/env bash
# Starts a Box from the agent and work images on this Docker host and checks what moor relies on.
# Every Docker object it makes is named boxsmoke-<pid>-*, and it removes each by exact name.
#
#   tools/box-smoke.sh                 builds both images from this checkout
#   BOX_SMOKE_AGENT_IMAGE=... BOX_SMOKE_WORK_IMAGE=... tools/box-smoke.sh   uses built ones
set -uo pipefail
. "$(dirname "$0")/lib/repo-root.sh"
cd_repo_root

p=boxsmoke-$$
net=$p-net
agent=$p-agent
work=$p-work
vols=("$p-tree" "$p-run" "$p-agent-home" "$p-work-home" "$p-docker")
built=()
builder=
fails=0

agent_image=${BOX_SMOKE_AGENT_IMAGE:-}
work_image=${BOX_SMOKE_WORK_IMAGE:-}

ok() { printf 'ok    %s\n' "$1"; }
bad() { printf 'FAIL  %s\n' "$1"; [ $# -lt 2 ] || printf '%s\n' "$2" | sed 's/^/      /'; fails=$((fails + 1)); }
check() {
  local name=$1 want=$2 got=$3
  if [ "$got" = "$want" ]; then ok "$name"; else bad "$name" "want: $want"$'\n'"got:  $got"; fi
}
contains() {
  case $3 in *"$2"*) ok "$1" ;; *) bad "$1" "want text: $2"$'\n'"got: $3" ;; esac
}

cleanup() {
  local v
  docker rm -f "$work" >/dev/null 2>&1
  docker rm -f "$agent" >/dev/null 2>&1
  for v in "${vols[@]}"; do docker volume rm "$v" >/dev/null 2>&1; done
  [ -z "$builder" ] || docker buildx rm "$builder" >/dev/null 2>&1
  docker network rm "$net" >/dev/null 2>&1
  for v in ${built[@]+"${built[@]}"}; do docker rmi "$v" >/dev/null 2>&1; done
}
trap cleanup EXIT

wait_for() {
  local secs=$1 i
  shift
  for ((i = 0; i < secs * 2; i++)); do
    "$@" >/dev/null 2>&1 && return 0
    sleep 0.5
  done
  return 1
}

in_agent() { docker exec "$agent" "$@"; }
in_work() { docker exec "$work" "$@"; }
shell() { in_agent box-shell -c "$1"; }
agent_ready() { in_agent test -e /run/moor/agent.ready; }
work_ready() { in_agent test -e /run/moor/work.ready; }
shell_works() { shell true; }

route_mtu() {
  local dev
  dev=$(ip route get "$1" 2>/dev/null | sed -n 's/.* dev \([^ ]*\).*/\1/p' | head -n1)
  [ -n "$dev" ] && cat "/sys/class/net/$dev/mtu" 2>/dev/null
}
probe_mtu() {
  local m mtu=1500 h
  for h in 140.82.112.6 1.1.1.1 8.8.8.8; do
    m=$(route_mtu "$h") || continue
    [ -n "$m" ] && [ "$m" -lt "$mtu" ] && mtu=$m
  done
  echo "$mtu"
}

start_agent() {
  docker run -d --name "$agent" --hostname "$p" --network "$net" --restart no \
    --memory 1g -v "$p-tree:/home/agent/moor" -v "$p-run:/run/moor" -v "$p-agent-home:/home/agent" \
    "$agent_image" >/dev/null
}
start_work() {
  docker run -d --name "$work" --network "container:$agent" --privileged --restart no \
    --memory 768m --memory-swap 768m \
    -v "$p-tree:/home/agent/moor" -v "$p-run:/run/moor" -v "$p-work-home:/home/agent" \
    -v "$p-docker:/var/lib/docker" "$work_image" >/dev/null
}
work_state() { docker inspect -f '{{.State.Status}} {{.State.ExitCode}} {{.RestartCount}}' "$work"; }

docker info >/dev/null 2>&1 || { echo "box-smoke: no Docker daemon is reachable" >&2; exit 2; }

mtu=$(probe_mtu)
echo "network MTU $mtu"
docker network create --opt "com.docker.network.driver.mtu=$mtu" "$net" >/dev/null || exit 2
build() {
  if [ -n "$builder" ]; then docker buildx build --builder "$builder" --load -q "$@" >/dev/null; else docker build -q "$@" >/dev/null; fi
}
if [ -z "$agent_image" ] || [ -z "$work_image" ]; then
  if [ "$mtu" -lt 1500 ]; then
    builder=$p-builder
    docker buildx create --name "$builder" --driver docker-container --driver-opt "network=$net" >/dev/null || exit 2
  fi
fi
if [ -z "$agent_image" ]; then
  agent_image=$p-agent:smoke
  built+=("$agent_image")
  echo "building $agent_image"
  build -f images/agent/Dockerfile -t "$agent_image" . || { echo "box-smoke: the agent image did not build" >&2; exit 2; }
fi
if [ -z "$work_image" ]; then
  work_image=$p-work:smoke
  built+=("$work_image")
  echo "building $work_image"
  build -t "$work_image" images/work || { echo "box-smoke: the work image did not build" >&2; exit 2; }
fi
for v in "${vols[@]}"; do docker volume create "$v" >/dev/null || exit 2; done

start_agent || exit 2
wait_for 30 agent_ready || { bad "the agent place becomes ready"; exit 1; }
start_work || exit 2
wait_for 90 work_ready || { bad "the work place becomes ready" "$(docker logs --tail 20 "$work" 2>&1)"; exit 1; }
ok "both places start"

wait_for 90 shell docker info || bad "box-shell reaches Docker inside the work place" "$(shell 'docker info' 2>&1 | tail -5)"
out=$(shell 'docker info --format "{{.ServerVersion}}"' 2>&1)
case $out in [0-9]*) ok "box-shell -c 'docker info' works from the agent place" ;; *) bad "box-shell -c 'docker info' works from the agent place" "$out" ;; esac
check "the agent place has no Docker of its own" "no" "$(in_agent sh -c 'command -v docker >/dev/null && echo yes || echo no')"
check "the inner dockerd carries the network MTU" "$mtu" "$(shell 'jq -r .mtu /etc/docker/daemon.json' 2>&1)"

in_agent sh -c 'echo shared > /home/agent/moor/from-agent'
check "the work place sees the agent place's files at the same path" "shared" "$(shell 'cat /home/agent/moor/from-agent' 2>&1)"
shell 'echo made > /home/agent/moor/from-work'
check "the agent place sees the work place's files, owned by uid 1000" "made 1000" "$(in_agent sh -c 'echo "$(cat /home/agent/moor/from-work) $(stat -c %u /home/agent/moor/from-work)"')"
check "a line runs in the caller's directory" "/home/agent/moor" "$(docker exec -w /home/agent/moor "$agent" box-shell -c pwd 2>&1)"

in_agent sh -c "echo 'export BOXSMOKE_ENV=through-profile' > /run/moor/box.env"
check "box.env reaches the work place's login shell" "through-profile" "$(shell 'echo $BOXSMOKE_ENV' 2>&1)"

in_agent sh -c "echo 'moor: smoke note one' >> /run/moor/notes"
first=$(shell true 2>&1)
second=$(shell true 2>&1)
check "an appended note prints in the next box-shell output" "moor: smoke note one" "$first"
check "the note does not print again" "" "$second"

oom_before=$(in_agent cat /run/moor/oom)
hog=$(shell 'timeout 120 tail /dev/zero; echo "tail exited $?"' 2>&1)
sleep 2
oom_after=$(in_agent cat /run/moor/oom)
if [ "$oom_after" -gt "$oom_before" ]; then ok "an in-Box memory hog bumps the OOM marker ($oom_before to $oom_after)"; else bad "an in-Box memory hog bumps the OOM marker" "$oom_before to $oom_after"; fi
contains "box-shell says the Box ran out of memory" "ran out of memory" "$hog"
check "the work place stays up after the hog" "running 0 0" "$(work_state)"
shell_works && ok "box-shell still works after the hog" || bad "box-shell still works after the hog"

in_agent sh -c 'rm -f /tmp/bs.pid'
docker exec -d "$agent" sh -c 'echo $$ > /tmp/bs.pid; exec box-shell -c "sleep 601"'
wait_for 20 in_work pgrep -f 'sleep 601' || bad "the remote sleep 601 starts"
in_agent sh -c 'kill -9 "$(cat /tmp/bs.pid)"'
start_ts=$(date +%s.%N)
gone=no
for _ in $(seq 1 30); do
  if ! in_work pgrep -f 'sleep 601' >/dev/null 2>&1; then gone=yes; break; fi
  sleep 0.25
done
took=$(awk -v a="$start_ts" -v b="$(date +%s.%N)" 'BEGIN { printf "%.1f", b - a }')
if [ "$gone" = yes ] && awk -v t="$took" 'BEGIN { exit !(t <= 7) }'; then ok "killing the box-shell client ends the remote sleep 600 in ${took}s (limit 7s)"; else bad "killing the box-shell client ends the remote sleep 600 within 7s" "gone=$gone after ${took}s"; fi

msg=$(shell 'box-rebuild "box-smoke asks"' 2>&1)
contains "box-rebuild says it asked for a fresh Box" "asked for a fresh Box" "$msg"
wait_for 20 sh -c "[ \"\$(docker inspect -f '{{.State.Status}}' '$work')\" = exited ]" || bad "the work place exits after box-rebuild"
check "box-rebuild makes the work place exit 75" "exited 75" "$(work_state | cut -d' ' -f1,2)"
check "box-rebuild leaves the reason on the run volume" "box-smoke asks" "$(in_agent sed -n 2p /run/moor/rebuild.asked)"
shell_works >/dev/null 2>&1
check "box-shell exits 75 while the work place is down" "75" "$(shell_works >/dev/null 2>&1; echo $?)"
in_agent rm -f /run/moor/rebuild.asked
docker start "$work" >/dev/null
wait_for 90 work_ready || bad "the work place restarts after box-rebuild"
wait_for 90 shell docker info || bad "box-shell works again after the work place restarts"
ok "the work place comes back after box-rebuild"

docker restart -t 5 "$agent" >/dev/null
wait_for 30 agent_ready || bad "the agent place restarts"
check "the work place keeps running after the agent place restarts" "running" "$(work_state | cut -d' ' -f1)"
sleep 3
check "the restarted agent place cannot reach the work place" "75" "$(shell_works >/dev/null 2>&1; echo $?)"
docker restart -t 10 "$work" >/dev/null
wait_for 90 work_ready || bad "the work place restarts after the agent place"
if wait_for 90 shell docker info; then ok "restarting the work place after the agent place makes it reachable again"; else bad "restarting the work place after the agent place makes it reachable again"; fi

echo
if [ "$fails" -eq 0 ]; then echo "box-smoke: all checks passed"; else echo "box-smoke: $fails check(s) failed"; exit 1; fi
