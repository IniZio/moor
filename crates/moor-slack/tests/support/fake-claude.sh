#!/bin/sh
dir=$(dirname "$0")
if [ "$1" = "--version" ]; then
  if [ -f "$dir/version" ]; then cat "$dir/version"; else echo "2.1.304 (Claude Code)"; fi
  exit 0
fi
n=0
while [ -e "$dir/run-$n" ]; do n=$((n + 1)); done
out="$dir/run-$n"
mkdir "$out"
echo $$ > "$out/pid"
for arg in "$@"; do printf '%s\n' "$arg"; done > "$out/argv"
tr '\0' '\n' < /proc/$$/environ > "$out/env"
pwd > "$out/cwd"
ls -l /proc/$$/fd > "$out/fds" 2>&1
cat > "$out/stdin"
cat <&3 > "$out/fd3" 2>/dev/null
token=$(cat "$out/fd3")
init() {
  echo "{\"type\":\"system\",\"subtype\":\"init\",\"tools\":[$1],\"mcp_servers\":[],\"skills\":[],\"slash_commands\":[],\"plugins\":[{\"name\":\"cc-plugin-telemetry\"}]}"
}
case "$token" in
  *-good-*)
    init
    echo '{"type":"result","subtype":"success","is_error":false,"result":"OK","total_cost_usd":0.0125}'
    ;;
  *-tool-*)
    init '"Bash"'
    sleep 30
    echo '{"type":"result","subtype":"success","is_error":false,"result":"ran a tool"}'
    ;;
  *-hang-*)
    init
    : > "$out/waiting"
    sleep 60
    ;;
  *-channel-*)
    init
    echo '{"type":"result","subtype":"success","is_error":false,"result":"<!channel> deploy & <@U9> done"}'
    ;;
  *-bare-*)
    echo '{"type":"result","subtype":"success","is_error":false,"result":"no init"}'
    ;;
  "")
    echo '{"type":"result","subtype":"success","is_error":true,"result":"Not logged in · Please run /login"}'
    ;;
  *-crash-*)
    echo "crashed reading $token" >&2
    ;;
  *-echo-*)
    echo "{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":true,\"result\":\"Bad token $token\"}"
    ;;
  *)
    init
    echo '{"type":"result","subtype":"success","is_error":true,"api_error_status":401,"result":"Failed to authenticate. API Error: 401 OAuth access token is invalid."}'
    ;;
esac
